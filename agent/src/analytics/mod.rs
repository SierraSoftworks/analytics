//! The SQL query layer. Statistics are computed by DuckDB over the events
//! table, filtered by a compiled [`filter`] expression, and bounded to a
//! half-open `[from, to)` time range.
//!
//! Each endpoint is a handful of aggregation queries sharing one pooled
//! connection; DuckDB's buffer manager (bounded by `storage.memory_limit_mb`)
//! serves concurrent requests from shared cached blocks, and zone maps prune
//! the time range since events are appended in arrival order. Window bounds
//! and limits are trusted integers interpolated into the SQL; everything
//! user-influenced travels as a bound parameter.
//!
//! Queries are CPU-bound and synchronous, so handlers run them via
//! `web::block`.

pub mod filter;

use std::collections::HashMap;

#[cfg(test)]
use chrono::Utc;

use analytics_api::{
    BreakdownRow, Breakdowns, CountRow, Dashboard, EventBreakdowns, EventDetail, EventVariant,
    ExceptionBreakdowns, ExceptionGroup, ExceptionGroupDetail, ExceptionStatus, ExceptionVariant,
    MetricSummary, SessionTrace, TREND_BUCKETS, TimeSeriesPoint, TraceEvent, TraceEventKind,
    TraceSummary, VersionRow, pixel_source, source_label, summary_line,
};
use duckdb::types::Value;
use duckdb::{Connection, Row, params_from_iter};

use crate::errors::{Result, ResultExt};
use crate::store::Store;

use filter::CompiledFilter;

const ADVICE: &[&str] = &["This is an internal analytics error; please report it with the logs."];

const BREAKDOWN_LIMIT: usize = 25;
/// How many recent session traces the dashboard payload samples.
const TRACE_SAMPLE: usize = 10;
/// `[100ms, 5s]` is treated as a bounce (per the medama methodology).
const BOUNCE_MIN_MS: i64 = 100;
const BOUNCE_MAX_MS: i64 = 5_000;
const MIN_BOUNCE_SAMPLES: i64 = 5;
/// Exception listings are capped to the most recently seen groups.
const EXCEPTION_GROUP_LIMIT: usize = 500;

/// A composable `WHERE` fragment: trusted integers (window bounds, limits) are
/// interpolated, user-influenced values ride as bound parameters in text order.
#[derive(Clone)]
struct Where {
    sql: String,
    params: Vec<Value>,
}

impl Where {
    /// The half-open `[from, to)` window plus the compiled `q`, if any.
    fn window(from_ms: i64, to_ms: i64, filter: Option<&CompiledFilter>) -> Where {
        let mut this = Where {
            sql: format!("received_ms >= {from_ms} AND received_ms < {to_ms}"),
            params: Vec::new(),
        };
        if let Some(filter) = filter {
            this.sql = format!("{} AND ({})", this.sql, filter.sql);
            this.params.extend(filter.params.iter().cloned());
        }
        this
    }

    /// Conjoin an extra condition (with its bound parameters).
    fn and(mut self, sql: &str, params: Vec<Value>) -> Where {
        self.sql = format!("{} AND ({sql})", self.sql);
        self.params.extend(params);
        self
    }
}

/// Run a query, mapping every row.
fn rows<T>(
    conn: &Connection,
    sql: &str,
    where_: &Where,
    map: impl FnMut(&Row<'_>) -> duckdb::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql).or_system_err(ADVICE)?;
    let mapped = stmt
        .query_map(params_from_iter(where_.params.iter().cloned()), map)
        .or_system_err(ADVICE)?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row.or_system_err(ADVICE)?);
    }
    Ok(out)
}

/// Run an aggregate query that returns exactly one row.
fn one<T>(
    conn: &Connection,
    sql: &str,
    where_: &Where,
    map: impl FnOnce(&Row<'_>) -> duckdb::Result<T>,
) -> Result<T> {
    conn.query_row(sql, params_from_iter(where_.params.iter().cloned()), map)
        .or_system_err(ADVICE)
}

/// The full dashboard payload: headline metrics with a previous-window baseline,
/// the (index-aligned) time series pair, every dimension breakdown, and the
/// project/source rollups.
///
/// `filter` is the compiled `q` expression (see [`filter::compile_query`]);
/// `None` means unfiltered.
pub fn dashboard(
    store: &Store,
    filter: Option<&CompiledFilter>,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Dashboard> {
    let len = (to_ms - from_ms).max(1);
    let prev_from = from_ms - len;
    let bucket_ms = bucket_ms.max(1);

    // With a path filter active, `is_unique_user` (which rides only on the first
    // page load of a visitor's day) would undercount non-landing pages to ~zero;
    // daily-unique *page* views are the honest visitor count there.
    let flag = if filter.is_some_and(|f| f.references("path")) {
        "is_unique_page"
    } else {
        "is_unique_user"
    };

    // One span covers both the current window and the comparison baseline.
    let span = Where::window(prev_from, to_ms, filter);
    let current = Where::window(from_ms, to_ms, filter);

    store.with_conn(|conn| {
        // Headline metrics for both windows in one scan: each aggregate is
        // computed twice, split on the window boundary.
        let windows = [
            format!("received_ms >= {from_ms}"),
            format!("received_ms < {from_ms}"),
        ];
        let metrics: Vec<String> = windows
            .iter()
            .map(|in_window| {
                format!(
                    "count(*) FILTER (kind = 'page_load' AND {in_window}),
                     count(*) FILTER (kind = 'page_load' AND {flag} AND {in_window}),
                     count(*) FILTER (kind IN ('pixel', 'custom') AND {in_window}),
                     count(duration_ms) FILTER ({in_window}),
                     count(*) FILTER (duration_ms BETWEEN {BOUNCE_MIN_MS} AND {BOUNCE_MAX_MS} AND {in_window}),
                     median(duration_ms) FILTER ({in_window})"
                )
            })
            .collect();
        let (summary, previous_summary) = one(
            conn,
            &format!(
                "SELECT {} FROM events WHERE {}",
                metrics.join(", "),
                span.sql
            ),
            &span,
            |row| Ok((metric_summary(row, 0)?, metric_summary(row, 6)?)),
        )?;

        // Both time series in one scan. The previous series is computed on the
        // *current* window's bucket grid by shifting events forward one window
        // length, guaranteeing index alignment; timestamps are shifted back to
        // the previous window's own instants after zero-filling.
        let mut current_buckets: HashMap<i64, BucketCounts> = HashMap::new();
        let mut previous_buckets: HashMap<i64, BucketCounts> = HashMap::new();
        let series = rows(
            conn,
            &format!(
                "SELECT received_ms >= {from_ms},
                        CASE WHEN received_ms >= {from_ms}
                             THEN received_ms - received_ms % {bucket_ms}
                             ELSE (received_ms + {len}) - (received_ms + {len}) % {bucket_ms}
                        END AS bucket,
                        count(*) FILTER (kind = 'page_load'),
                        count(*) FILTER (kind = 'page_load' AND {flag}),
                        count(*) FILTER (kind IN ('pixel', 'custom')),
                        count(*) FILTER (kind = 'exception')
                 FROM events
                 WHERE {} AND kind IN ('page_load', 'pixel', 'custom', 'exception')
                 GROUP BY 1, 2",
                span.sql
            ),
            &span,
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, i64>(1)?,
                    (row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?),
                ))
            },
        )?;
        for (in_current, bucket, counts) in series {
            let target = if in_current {
                &mut current_buckets
            } else {
                &mut previous_buckets
            };
            target.insert(bucket, counts);
        }
        let timeseries = fill_series(&current_buckets, from_ms, to_ms, bucket_ms);
        let mut previous_timeseries = fill_series(&previous_buckets, from_ms, to_ms, bucket_ms);
        for point in &mut previous_timeseries {
            point.timestamp_ms -= len;
        }

        // Dimension breakdowns over the current window's page loads. The pages
        // panel always counts daily-unique *page* views; the rest follow the
        // headline's uniqueness flag.
        let breakdown = |column: &str, flag: &str| -> Result<Vec<BreakdownRow>> {
            rows(
                conn,
                &format!(
                    "SELECT coalesce({column}, '') AS key,
                            count(*),
                            count(*) FILTER ({flag})
                     FROM events WHERE {} AND kind = 'page_load'
                     GROUP BY key ORDER BY 2 DESC, key LIMIT {BREAKDOWN_LIMIT}",
                    current.sql
                ),
                &current,
                |row| {
                    Ok(BreakdownRow {
                        key: row.get(0)?,
                        pageviews: row.get(1)?,
                        visitors: row.get(2)?,
                        events: 0,
                    })
                },
            )
        };

        // The client-versions breakdown, keyed by the (application, version)
        // pair — a version number is only meaningful within its application.
        let versions = rows(
            conn,
            &format!(
                "SELECT coalesce(ua_browser, ''), coalesce(ua_version, ''),
                        count(*), count(*) FILTER ({flag})
                 FROM events WHERE {} AND kind = 'page_load'
                 GROUP BY 1, 2 ORDER BY 3 DESC, 1, 2 LIMIT {BREAKDOWN_LIMIT}",
                current.sql
            ),
            &current,
            |row| {
                Ok(VersionRow {
                    app: row.get(0)?,
                    version: row.get(1)?,
                    pageviews: row.get(2)?,
                    visitors: row.get(3)?,
                    events: 0,
                })
            },
        )?;

        // The custom/pixel events breakdown, keyed by event name (unnamed
        // events aggregate under the empty sentinel). Only the `events` count
        // is meaningful — these rows have no page views, and visitor
        // uniqueness rides on page loads.
        let event_names = rows(
            conn,
            &format!(
                "SELECT coalesce(event_name, '') AS key, count(*)
                 FROM events WHERE {} AND kind IN ('pixel', 'custom')
                 GROUP BY key ORDER BY 2 DESC, key LIMIT {BREAKDOWN_LIMIT}",
                current.sql
            ),
            &current,
            |row| {
                Ok(BreakdownRow {
                    key: row.get(0)?,
                    visitors: 0,
                    pageviews: 0,
                    events: row.get(1)?,
                })
            },
        )?;

        // Per-source totals. Page loads count as `pageviews`; pixel hits and
        // custom events count as `events` so pixel-only and application
        // sources still surface; `visitors` uses the same daily-unique flag as
        // every other aggregation in the response, so the panels agree with
        // the headline.
        let per_source = rows(
            conn,
            &format!(
                "SELECT source,
                        count(*) FILTER (kind = 'page_load'),
                        count(*) FILTER ({flag}),
                        count(*) FILTER (kind IN ('pixel', 'custom'))
                 FROM events WHERE {} AND kind IN ('page_load', 'pixel', 'custom')
                 GROUP BY source
                 ORDER BY count(*) FILTER (kind = 'page_load')
                          + count(*) FILTER (kind IN ('pixel', 'custom')) DESC, source",
                current.sql
            ),
            &current,
            |row| {
                Ok(BreakdownRow {
                    key: row.get(0)?,
                    pageviews: row.get(1)?,
                    visitors: row.get(2)?,
                    events: row.get(3)?,
                })
            },
        )?;
        let (projects, sources, unassigned) = project_rollup(store, per_source)?;

        // Sample the most recently started sessions in the filtered window,
        // then summarize just those — scoped by `q` exactly the way every
        // other panel is.
        let sids: Vec<String> = rows(
            conn,
            &format!(
                "SELECT sid FROM events WHERE {} AND sid IS NOT NULL AND sid <> ''
                 GROUP BY sid ORDER BY min(received_ms) DESC, sid LIMIT {TRACE_SAMPLE}",
                current.sql
            ),
            &current,
            |row| row.get(0),
        )?;
        let traces = traces_of_sessions(conn, &sids, &current)?;

        Ok(Dashboard {
            summary,
            previous_summary,
            timeseries,
            previous_timeseries,
            breakdowns: Breakdowns {
                pages: breakdown("pathname", "is_unique_page")?,
                referrers: breakdown("referrer_host", flag)?,
                countries: breakdown("country", flag)?,
                languages: breakdown("language", flag)?,
                browsers: breakdown("ua_browser", flag)?,
                versions,
                operating_systems: breakdown("ua_os", flag)?,
                devices: breakdown("ua_device", flag)?,
                utm_sources: breakdown("utm_source", flag)?,
                utm_mediums: breakdown("utm_medium", flag)?,
                utm_campaigns: breakdown("utm_campaign", flag)?,
                event_names,
                projects,
                sources,
            },
            unassigned,
            traces,
        })
    })
}

/// The six headline aggregates of one window, starting at column `offset`.
fn metric_summary(row: &Row<'_>, offset: usize) -> duckdb::Result<MetricSummary> {
    let pageviews: i64 = row.get(offset)?;
    let visitors: i64 = row.get(offset + 1)?;
    let events: i64 = row.get(offset + 2)?;
    let samples: i64 = row.get(offset + 3)?;
    let bounces: i64 = row.get(offset + 4)?;
    let median: Option<f64> = row.get(offset + 5)?;
    Ok(MetricSummary {
        visitors,
        pageviews,
        events,
        bounce_rate: (samples >= MIN_BOUNCE_SAMPLES).then(|| bounces as f64 / samples as f64),
        median_duration_ms: median.map(|m| m.round() as i64),
    })
}

/// The source URIs belonging to the project whose **name** matches `name`
/// (case-insensitively, matching the filter language's string semantics; names
/// are unique). Values that name no project fall back to an id lookup, so
/// pre-rename links that filtered by project id keep working. An unknown value
/// resolves to no sources, so the filter matches nothing — never everything.
pub fn project_source_uris_by_name(store: &Store, name: &str) -> Result<Vec<String>> {
    let projects = store.list_projects()?;
    let needle = name.to_lowercase();
    let project = projects
        .iter()
        .find(|p| p.name.to_lowercase() == needle)
        .or_else(|| projects.iter().find(|p| p.id == name));
    match project {
        Some(project) => project_source_uris(store, &project.id),
        None => Ok(Vec::new()),
    }
}

/// The source URIs belonging to a project: its assigned sources plus its pixels
/// (as `pixel://<id>` URIs).
pub fn project_source_uris(store: &Store, project_id: &str) -> Result<Vec<String>> {
    let mut uris: Vec<String> = store
        .list_sources()?
        .into_iter()
        .filter(|s| s.project_id.as_deref() == Some(project_id))
        .map(|s| s.uri)
        .collect();
    for pixel in store.list_pixels()? {
        if pixel.project_id == project_id {
            uris.push(pixel_source(&pixel.id));
        }
    }
    Ok(uris)
}

/// Exception groups matching the compiled filter, grouped by
/// `(fingerprint, source)` with a [`TREND_BUCKETS`]-bucket occurrence trend
/// each. A fingerprint is computed from the error alone, so the same
/// `exc_group` legitimately occurs on multiple sources/projects; keeping the
/// source in the key keeps those occurrences separate. The caller folds
/// per-source rows up to per-project rows (summing trends element-wise) for
/// the global Exceptions inbox.
pub fn exception_groups_by_source(
    store: &Store,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
) -> Result<Vec<(ExceptionGroup, String)>> {
    let occurrences = Where::window(from_ms, to_ms, filter)
        .and("kind = 'exception' AND exc_group IS NOT NULL", Vec::new());

    store.with_conn(|conn| {
        let mut out: Vec<(ExceptionGroup, String)> = rows(
            conn,
            &format!(
                "SELECT exc_group, source, count(*),
                        min(received_ms), max(received_ms),
                        arg_max(exc_type, received_ms) FILTER (exc_type IS NOT NULL),
                        arg_max(exc_message, received_ms) FILTER (exc_message IS NOT NULL)
                 FROM events WHERE {}
                 GROUP BY exc_group, source
                 ORDER BY max(received_ms) DESC, exc_group, source
                 LIMIT {EXCEPTION_GROUP_LIMIT}",
                occurrences.sql
            ),
            &occurrences,
            |row| {
                Ok((
                    ExceptionGroup {
                        group_id: row.get(0)?,
                        exc_type: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        sample_message: summary_line(
                            row.get::<_, Option<String>>(6)?.as_deref().unwrap_or(""),
                        )
                        .to_string(),
                        count: row.get(2)?,
                        first_seen_ms: row.get(3)?,
                        last_seen_ms: row.get(4)?,
                        status: ExceptionStatus::Unresolved,
                        resolved: false,
                        muted: false,
                        note: None,
                        trend: vec![0; TREND_BUCKETS],
                    },
                    row.get(1)?,
                ))
            },
        )?;

        // Trends per (group, source), merged onto the capped listing.
        let index: HashMap<(String, String), usize> = out
            .iter()
            .enumerate()
            .map(|(i, (group, source))| ((group.group_id.clone(), source.clone()), i))
            .collect();
        let buckets = trend_rows(conn, &occurrences, "exc_group, source", from_ms, to_ms)?;
        for (keys, bucket, count) in buckets {
            let key = (keys[0].clone(), keys[1].clone());
            if let Some(&i) = index.get(&key) {
                out[i].0.trend[bucket] += count;
            }
        }
        Ok(out)
    })
}

/// Occurrence counts per trend bucket, grouped by `group_columns` (comma
/// separated; pass an empty string for a single global trend). Returns
/// `(group key values, bucket index, count)` rows.
fn trend_rows(
    conn: &Connection,
    occurrences: &Where,
    group_columns: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(Vec<String>, usize, i64)>> {
    let span = (to_ms - from_ms).max(1);
    let buckets = TREND_BUCKETS as i64;
    let bucket = format!(
        "least(greatest((received_ms - {from_ms}) * {buckets} / {span}, 0), {})",
        buckets - 1
    );
    let (select, group_by, key_count) = if group_columns.is_empty() {
        (format!("{bucket}, count(*)"), "1".to_string(), 0)
    } else {
        (
            format!("{group_columns}, {bucket}, count(*)"),
            format!("{group_columns}, {}", group_columns.split(',').count() + 1),
            group_columns.split(',').count(),
        )
    };
    rows(
        conn,
        &format!(
            "SELECT {select} FROM events WHERE {} GROUP BY {group_by}",
            occurrences.sql
        ),
        occurrences,
        |row| {
            let mut keys = Vec::with_capacity(key_count);
            for i in 0..key_count {
                keys.push(row.get::<_, Option<String>>(i)?.unwrap_or_default());
            }
            let bucket: i64 = row.get(key_count)?;
            let count: i64 = row.get(key_count + 1)?;
            Ok((keys, bucket.clamp(0, buckets - 1) as usize, count))
        },
    )
}

/// A single trend over all matching occurrences.
fn trend_of(conn: &Connection, occurrences: &Where, from_ms: i64, to_ms: i64) -> Result<Vec<i64>> {
    let mut trend = vec![0i64; TREND_BUCKETS];
    for (_, bucket, count) in trend_rows(conn, occurrences, "", from_ms, to_ms)? {
        trend[bucket] += count;
    }
    Ok(trend)
}

/// Occurrence counts per value of `column` (nulls under the empty-string
/// sentinel), largest first.
fn count_by(conn: &Connection, occurrences: &Where, column: &str) -> Result<Vec<CountRow>> {
    rows(
        conn,
        &format!(
            "SELECT coalesce({column}, '') AS key, count(*)
             FROM events WHERE {}
             GROUP BY key ORDER BY 2 DESC, key LIMIT {BREAKDOWN_LIMIT}",
            occurrences.sql
        ),
        occurrences,
        |row| {
            Ok(CountRow {
                key: row.get(0)?,
                count: row.get(1)?,
            })
        },
    )
}

/// A single exception group in forensic detail: the aggregate (with trend),
/// how its occurrences distribute across key dimensions, and its **distinct
/// variants** — occurrences collapsed by (message, stack, handledness) so an
/// operator scrubs through genuinely different examples rather than paging
/// hundreds of identical ones. Looked up by id directly (no top-N cap), so a
/// linked or bookmarked group opens regardless of how many fingerprints a
/// project has. Returns `None` if the group has no occurrences in
/// `[from_ms, to_ms)`.
pub fn exception_detail(
    store: &Store,
    sources: &[String],
    group_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<ExceptionGroupDetail>> {
    let mut params = vec![Value::Text(group_id.to_string())];
    params.extend(sources.iter().map(|s| Value::Text(s.clone())));
    let source_list = if sources.is_empty() {
        "FALSE".to_string()
    } else {
        format!("source IN ({})", vec!["?"; sources.len()].join(", "))
    };
    let occurrences = Where::window(from_ms, to_ms, None).and(
        &format!("kind = 'exception' AND exc_group = ? AND {source_list}"),
        params,
    );

    store.with_conn(|conn| {
        let (count, first, last, exc_type, message) = one(
            conn,
            &format!(
                "SELECT count(*), min(received_ms), max(received_ms),
                        arg_max(exc_type, received_ms) FILTER (exc_type IS NOT NULL),
                        arg_max(exc_message, received_ms) FILTER (exc_message IS NOT NULL)
                 FROM events WHERE {}",
                occurrences.sql
            ),
            &occurrences,
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )?;
        if count == 0 {
            return Ok(None);
        }

        let group = ExceptionGroup {
            group_id: group_id.to_string(),
            exc_type: exc_type.unwrap_or_default(),
            sample_message: summary_line(message.as_deref().unwrap_or("")).to_string(),
            count,
            first_seen_ms: first.unwrap_or(0),
            last_seen_ms: last.unwrap_or(0),
            status: ExceptionStatus::Unresolved,
            resolved: false,
            muted: false,
            note: None,
            trend: trend_of(conn, &occurrences, from_ms, to_ms)?,
        };

        // Occurrence counts per reported release, folded through the label
        // qualification rules (see [`app_version_rows`]).
        let totals: HashMap<(String, String), i64> = rows(
            conn,
            &format!(
                "SELECT coalesce(source, ''), coalesce(app_version, ''), count(*)
                 FROM events WHERE {} GROUP BY 1, 2",
                occurrences.sql
            ),
            &occurrences,
            |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)),
        )?
        .into_iter()
        .collect();

        let breakdowns = ExceptionBreakdowns {
            app_versions: app_version_rows(totals),
            browsers: count_by(conn, &occurrences, "ua_browser")?,
            operating_systems: count_by(conn, &occurrences, "ua_os")?,
            devices: count_by(conn, &occurrences, "ua_device")?,
        };

        // Distinct variants keyed by (message, stack, handledness): one
        // representative each, counted, most frequent first. The
        // representative context comes from the variant's latest occurrence
        // that carries each value.
        let variants = rows(
            conn,
            &format!(
                "SELECT exc_message, exc_stack, exc_handled,
                        count(*), min(received_ms), max(received_ms),
                        arg_max(ua_browser, received_ms) FILTER (ua_browser IS NOT NULL),
                        arg_max(ua_os, received_ms) FILTER (ua_os IS NOT NULL),
                        arg_max(source, received_ms) FILTER (source IS NOT NULL),
                        arg_max(app_version, received_ms) FILTER (app_version IS NOT NULL),
                        arg_max(metadata_json, received_ms) FILTER (metadata_json IS NOT NULL),
                        arg_max(sid, received_ms) FILTER (sid IS NOT NULL)
                 FROM events WHERE {}
                 GROUP BY 1, 2, 3
                 ORDER BY 4 DESC, 6 DESC
                 LIMIT {limit}",
                occurrences.sql
            ),
            &occurrences,
            |row| {
                Ok(ExceptionVariant {
                    message: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    stack: row.get(1)?,
                    handled: row.get::<_, Option<bool>>(2)?.unwrap_or(false),
                    count: row.get(3)?,
                    first_seen_ms: row.get(4)?,
                    last_seen_ms: row.get(5)?,
                    ua_browser: row.get(6)?,
                    ua_os: row.get(7)?,
                    source: row.get(8)?,
                    app_version: row.get(9)?,
                    metadata: row.get(10)?,
                    session_id: row.get(11)?,
                })
            },
        )?;

        let traces = occurrence_traces(conn, &occurrences, from_ms, to_ms)?;
        Ok(Some(ExceptionGroupDetail {
            group,
            breakdowns,
            variants,
            traces,
        }))
    })
}

/// One named custom/pixel event in forensic detail: the aggregate (with
/// trend), how its occurrences distribute across key dimensions, its
/// **distinct metadata variants** (one representative per unique reporter
/// payload), and the sessions it occurred in. `filter` is the dashboard's
/// compiled `q` expression, so the numbers cover the same slice the operator
/// was looking at. Returns `None` if the event has no occurrences in
/// `[from_ms, to_ms)`.
pub fn event_detail(
    store: &Store,
    name: &str,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
    limit: usize,
) -> Result<Option<EventDetail>> {
    let occurrences = Where::window(from_ms, to_ms, filter).and(
        "kind IN ('pixel', 'custom') AND event_name = ?",
        vec![Value::Text(name.to_string())],
    );

    store.with_conn(|conn| {
        let (count, first, last) = one(
            conn,
            &format!(
                "SELECT count(*), min(received_ms), max(received_ms) FROM events WHERE {}",
                occurrences.sql
            ),
            &occurrences,
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )?;
        if count == 0 {
            return Ok(None);
        }

        let breakdowns = EventBreakdowns {
            sources: count_by(conn, &occurrences, "source")?,
            pages: count_by(conn, &occurrences, "pathname")?,
            browsers: count_by(conn, &occurrences, "ua_browser")?,
            operating_systems: count_by(conn, &occurrences, "ua_os")?,
            devices: count_by(conn, &occurrences, "ua_device")?,
            countries: count_by(conn, &occurrences, "country")?,
            languages: count_by(conn, &occurrences, "language")?,
        };

        // Distinct variants keyed by their reporter metadata: one
        // representative each, counted, most frequent first, with the context
        // of each variant's latest occurrence.
        let variants = rows(
            conn,
            &format!(
                "SELECT metadata_json, count(*), min(received_ms), max(received_ms),
                        arg_max(ua_browser, received_ms) FILTER (ua_browser IS NOT NULL),
                        arg_max(ua_os, received_ms) FILTER (ua_os IS NOT NULL),
                        arg_max(source, received_ms) FILTER (source IS NOT NULL),
                        arg_max(pathname, received_ms) FILTER (pathname IS NOT NULL),
                        arg_max(sid, received_ms) FILTER (sid IS NOT NULL)
                 FROM events WHERE {}
                 GROUP BY 1 ORDER BY 2 DESC, 4 DESC LIMIT {limit}",
                occurrences.sql
            ),
            &occurrences,
            |row| {
                Ok(EventVariant {
                    metadata: row.get(0)?,
                    count: row.get(1)?,
                    first_seen_ms: row.get(2)?,
                    last_seen_ms: row.get(3)?,
                    ua_browser: row.get(4)?,
                    ua_os: row.get(5)?,
                    source: row.get(6)?,
                    pathname: row.get(7)?,
                    session_id: row.get(8)?,
                })
            },
        )?;

        let traces = occurrence_traces(conn, &occurrences, from_ms, to_ms)?;
        Ok(Some(EventDetail {
            name: name.to_string(),
            count,
            first_seen_ms: first.unwrap_or(0),
            last_seen_ms: last.unwrap_or(0),
            trend: trend_of(conn, &occurrences, from_ms, to_ms)?,
            breakdowns,
            variants,
            traces,
        }))
    })
}

/// The sessions of a detail view's occurrences: the sessions with the most
/// recent matching occurrences, summarized **in full** (their page views and
/// events, not just the occurrences that matched).
fn occurrence_traces(
    conn: &Connection,
    occurrences: &Where,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<TraceSummary>> {
    let sids: Vec<String> = rows(
        conn,
        &format!(
            "SELECT sid FROM events WHERE {} AND sid IS NOT NULL AND sid <> ''
             GROUP BY sid ORDER BY max(received_ms) DESC, sid LIMIT {TRACE_SAMPLE}",
            occurrences.sql
        ),
        occurrences,
        |row| row.get(0),
    )?;
    traces_of_sessions(conn, &sids, &Where::window(from_ms, to_ms, None))
}

/// Summaries of the given sessions within `where_`'s scope, newest first. The
/// context columns are each session's earliest reported value; events without
/// a session id never form a trace.
fn traces_of_sessions(
    conn: &Connection,
    sids: &[String],
    where_: &Where,
) -> Result<Vec<TraceSummary>> {
    if sids.is_empty() {
        return Ok(Vec::new());
    }
    let sessions = where_.clone().and(
        &format!("sid IN ({})", vec!["?"; sids.len()].join(", ")),
        sids.iter().map(|s| Value::Text(s.clone())).collect(),
    );
    rows(
        conn,
        &format!(
            "SELECT sid, min(received_ms) AS started, max(received_ms),
                    arg_min(source, received_ms) FILTER (source IS NOT NULL),
                    arg_min(pathname, received_ms)
                        FILTER (kind = 'page_load' AND pathname IS NOT NULL),
                    arg_min(country, received_ms) FILTER (country IS NOT NULL),
                    arg_min(ua_browser, received_ms) FILTER (ua_browser IS NOT NULL),
                    arg_min(ua_version, received_ms) FILTER (ua_version IS NOT NULL),
                    arg_min(ua_device, received_ms) FILTER (ua_device IS NOT NULL),
                    arg_min(app_version, received_ms) FILTER (app_version IS NOT NULL),
                    count(*) FILTER (kind = 'page_load'),
                    count(*) FILTER (kind IN ('pixel', 'custom')),
                    count(*) FILTER (kind = 'exception')
             FROM events WHERE {}
             GROUP BY sid ORDER BY started DESC, sid LIMIT {TRACE_SAMPLE}",
            sessions.sql
        ),
        &sessions,
        |row| {
            Ok(TraceSummary {
                session_id: row.get(0)?,
                started_ms: row.get(1)?,
                last_ms: row.get(2)?,
                source: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                entry_path: row.get(4)?,
                country: row.get(5)?,
                ua_browser: row.get(6)?,
                ua_version: row.get(7)?,
                ua_device: row.get(8)?,
                app_version: row.get(9)?,
                pageviews: row.get(10)?,
                events: row.get(11)?,
                exceptions: row.get(12)?,
            })
        },
    )
}

/// One session's full timeline: every event carrying the session id, oldest
/// first, plus the visit's context (source, locale, client, claimed release)
/// drawn from the earliest event that reports each. Looked up by id directly —
/// no recency cap — so a trace linked from an exception exemplar or a bookmark
/// always opens; `limit` bounds the returned timeline. Returns `None` when the
/// session has no events in `[from_ms, to_ms)`.
pub fn session_trace(
    store: &Store,
    session_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<SessionTrace>> {
    struct Collected {
        received_ms: i64,
        kind: String,
        bid: String,
        pathname: Option<String>,
        duration_ms: Option<i64>,
        event_name: Option<String>,
        metadata: Option<String>,
        exc_type: Option<String>,
        exc_message: Option<String>,
        exc_stack: Option<String>,
        exc_group: Option<String>,
        exc_handled: Option<bool>,
        source: Option<String>,
        country: Option<String>,
        language: Option<String>,
        ua_browser: Option<String>,
        ua_version: Option<String>,
        ua_os: Option<String>,
        app_version: Option<String>,
    }

    let timeline = Where::window(from_ms, to_ms, None)
        .and("sid = ?", vec![Value::Text(session_id.to_string())]);
    let collected = store.with_conn(|conn| {
        rows(
            conn,
            &format!(
                "SELECT received_ms, kind, bid, pathname, duration_ms, event_name,
                        metadata_json, exc_type, exc_message, exc_stack, exc_group,
                        exc_handled, source, country, language, ua_browser,
                        ua_version, ua_os, app_version
                 FROM events WHERE {}
                 ORDER BY received_ms, seq LIMIT {limit}",
                timeline.sql
            ),
            &timeline,
            |row| {
                Ok(Collected {
                    received_ms: row.get(0)?,
                    kind: row.get(1)?,
                    bid: row.get(2)?,
                    pathname: row.get(3)?,
                    duration_ms: row.get(4)?,
                    event_name: row.get(5)?,
                    metadata: row.get(6)?,
                    exc_type: row.get(7)?,
                    exc_message: row.get(8)?,
                    exc_stack: row.get(9)?,
                    exc_group: row.get(10)?,
                    exc_handled: row.get(11)?,
                    source: row.get(12)?,
                    country: row.get(13)?,
                    language: row.get(14)?,
                    ua_browser: row.get(15)?,
                    ua_version: row.get(16)?,
                    ua_os: row.get(17)?,
                    app_version: row.get(18)?,
                })
            },
        )
    })?;

    if collected.is_empty() {
        return Ok(None);
    }

    let events: Vec<TraceEvent> = collected
        .iter()
        .filter_map(|row| {
            let kind = match row.kind.as_str() {
                "page_load" => TraceEventKind::PageLoad,
                "page_unload" => TraceEventKind::PageUnload,
                "custom" => TraceEventKind::Custom,
                "exception" => TraceEventKind::Exception,
                // Pixels carry no session; anything else has no place on a trace.
                _ => return None,
            };
            Some(TraceEvent {
                received_ms: row.received_ms,
                kind,
                bid: row.bid.clone(),
                pathname: row.pathname.clone(),
                duration_ms: row.duration_ms,
                event_name: row.event_name.clone(),
                metadata: row.metadata.clone(),
                exc_type: row.exc_type.clone(),
                exc_message: row.exc_message.clone(),
                exc_stack: row.exc_stack.clone(),
                exc_group: row.exc_group.clone(),
                exc_handled: row.exc_handled,
            })
        })
        .collect();

    // The visit's context: the earliest non-null value of each dimension (a
    // session is one client on one source, so any row would do — but events
    // differ in which columns they carry).
    let first_str = |get: fn(&Collected) -> Option<&String>| -> Option<String> {
        collected.iter().find_map(|row| get(row).cloned())
    };

    Ok(Some(SessionTrace {
        session_id: session_id.to_string(),
        started_ms: collected.first().map(|r| r.received_ms).unwrap_or(0),
        ended_ms: collected.last().map(|r| r.received_ms).unwrap_or(0),
        source: first_str(|r| r.source.as_ref()).unwrap_or_default(),
        country: first_str(|r| r.country.as_ref()),
        language: first_str(|r| r.language.as_ref()),
        ua_browser: first_str(|r| r.ua_browser.as_ref()),
        ua_version: first_str(|r| r.ua_version.as_ref()),
        ua_os: first_str(|r| r.ua_os.as_ref()),
        app_version: first_str(|r| r.app_version.as_ref()),
        events,
    }))
}

// ----------------------------------------------------------------- internals

/// Per-bucket `(pageviews, visitors, events, exceptions)` counts.
type BucketCounts = (i64, i64, i64, i64);

/// A continuous time series over `[from_ms, to_ms)` at `bucket_ms` resolution.
/// Buckets with no events are emitted as zeros so the chart shows a gap-free
/// line across the whole window instead of collapsing absent periods.
fn fill_series(
    counts: &HashMap<i64, BucketCounts>,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Vec<TimeSeriesPoint> {
    let point = |timestamp_ms: i64, (pageviews, visitors, events, exceptions): BucketCounts| {
        TimeSeriesPoint {
            timestamp_ms,
            pageviews,
            visitors,
            events,
            exceptions,
        }
    };

    let first = from_ms - from_ms.rem_euclid(bucket_ms);
    let last = (to_ms - 1) - (to_ms - 1).rem_euclid(bucket_ms);
    // Guard against a pathological window/bucket combination producing a huge vec.
    let estimated = ((last - first) / bucket_ms).unsigned_abs() as usize + 1;
    if first > last || estimated > 5_000 {
        // Fall back to the populated buckets only (sorted).
        let mut points: Vec<TimeSeriesPoint> =
            counts.iter().map(|(b, tuple)| point(*b, *tuple)).collect();
        points.sort_by_key(|p| p.timestamp_ms);
        return points;
    }

    let mut points = Vec::with_capacity(estimated);
    let mut b = first;
    while b <= last {
        points.push(point(b, counts.get(&b).copied().unwrap_or((0, 0, 0, 0))));
        b += bucket_ms;
    }
    points
}

/// Occurrence counts per reported release, from totals keyed by
/// `(source, version)`. When the occurrences span several sources, rows are
/// keyed as `app @ version` (the app being the source's label) — a release
/// number is only meaningful within its application. Occurrences scoped to a
/// single source (the per-source detail view) key rows by the bare version
/// number, since the application is given. Occurrences with no reported
/// version aggregate under the empty sentinel, whichever source they came
/// from. Labels are compared (not URIs) since distinct sources can share one
/// (http vs https).
fn app_version_rows(totals: HashMap<(String, String), i64>) -> Vec<CountRow> {
    let mut labels: Vec<&str> = totals
        .keys()
        .filter(|(app, _)| !app.is_empty())
        .map(|(app, _)| source_label(app))
        .collect();
    labels.sort_unstable();
    labels.dedup();
    let qualify = labels.len() > 1;

    let mut folded: HashMap<String, i64> = HashMap::new();
    for ((app, version), count) in &totals {
        let key = match (app.is_empty(), version.is_empty()) {
            (_, true) => String::new(),
            (true, false) => version.clone(),
            (false, false) if qualify => format!("{} @ {version}", source_label(app)),
            (false, false) => version.clone(),
        };
        *folded.entry(key).or_insert(0) += count;
    }
    let mut rows: Vec<CountRow> = folded
        .into_iter()
        .map(|(key, count)| CountRow { key, count })
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    rows.truncate(BREAKDOWN_LIMIT);
    rows
}

/// Fold per-source totals up to per-project rows, and split off the sources that
/// belong to no project (the operator's "assign these" inbox). Also returns the
/// per-source rows themselves, capped like every other breakdown.
fn project_rollup(
    store: &Store,
    per_source: Vec<BreakdownRow>,
) -> Result<(Vec<BreakdownRow>, Vec<BreakdownRow>, Vec<BreakdownRow>)> {
    // Build a source-URI -> project map from assigned sources and pixels.
    let mut uri_project: HashMap<String, String> = HashMap::new();
    for source in store.list_sources()? {
        if let Some(project_id) = source.project_id {
            uri_project.insert(source.uri, project_id);
        }
    }
    for pixel in store.list_pixels()? {
        uri_project.insert(pixel_source(&pixel.id), pixel.project_id);
    }

    let mut totals: HashMap<String, BreakdownRow> = HashMap::new();
    let mut unassigned: Vec<BreakdownRow> = Vec::new();
    for row in &per_source {
        match uri_project.get(&row.key) {
            Some(project_id) => {
                let entry = totals
                    .entry(project_id.clone())
                    .or_insert_with(|| BreakdownRow {
                        key: project_id.clone(),
                        visitors: 0,
                        pageviews: 0,
                        events: 0,
                    });
                entry.visitors += row.visitors;
                entry.pageviews += row.pageviews;
                entry.events += row.events;
            }
            None => unassigned.push(row.clone()),
        }
    }

    // Every project appears, even with zero traffic in the window, so the panel
    // doubles as the project directory.
    let mut projects: Vec<BreakdownRow> = store
        .list_projects()?
        .into_iter()
        .map(|project| {
            totals.remove(&project.id).unwrap_or(BreakdownRow {
                key: project.id,
                visitors: 0,
                pageviews: 0,
                events: 0,
            })
        })
        .collect();
    projects.sort_by_key(|r| std::cmp::Reverse(r.pageviews + r.events));
    unassigned.sort_by_key(|r| std::cmp::Reverse(r.pageviews + r.events));

    let mut sources = per_source;
    sources.truncate(BREAKDOWN_LIMIT);

    Ok((projects, sources, unassigned))
}

/// The timestamp of the earliest stored event, or `None` when nothing has been
/// recorded yet. Used to resolve "all time" queries (`from=0`) to the real
/// start of the data, so the time series isn't padded with decades of empty
/// buckets.
pub fn earliest_event_ms(store: &Store) -> Result<Option<i64>> {
    store.with_conn(|conn| {
        conn.query_row("SELECT min(received_ms) FROM events", [], |row| row.get(0))
            .or_system_err(ADVICE)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventKind, StoredEvent};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_redb() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("analytics-stats-{}-{}.redb", std::process::id(), n))
    }

    fn load(source: &str, received_ms: i64, unique: bool, duration: Option<i64>) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b".into(),
            kind: if duration.is_some() {
                EventKind::PageUnload
            } else {
                EventKind::PageLoad
            },
            source: source.into(),
            pathname: Some("/home".into()),
            is_unique_user: unique,
            is_unique_page: unique,
            ua_browser: Some("Chrome".into()),
            ua_version: Some("120.0".into()),
            duration_ms: duration,
            ..Default::default()
        }
    }

    /// Compile a dashboard `q` expression (panics on error — tests only).
    fn dash_filter(store: &Store, q: &str) -> CompiledFilter {
        filter::compile_query(q, filter::FieldSet::Dashboard, store)
            .unwrap()
            .unwrap()
    }

    fn source_q(source: &str) -> String {
        format!(r#"source == "{source}""#)
    }

    #[test]
    fn earliest_event_ms_tracks_the_oldest_stored_event() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // An empty store has no earliest event ("all time" stays unanchored).
        assert_eq!(earliest_event_ms(&store).unwrap(), None);

        store
            .append_events(&[
                load("https://a.com", 5_000, true, None),
                load("https://a.com", 3_000, true, None),
            ])
            .unwrap();
        assert_eq!(earliest_event_ms(&store).unwrap(), Some(3_000));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn computes_summary_from_hot_store() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
                load("https://a.com", 3_000, true, None),
                load("https://b.com", 4_000, true, None), // different source, excluded
            ])
            .unwrap();

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();

        assert_eq!(dash.summary.pageviews, 3);
        assert_eq!(dash.summary.visitors, 2); // two unique loads for a.com
        assert_eq!(
            dash.breakdowns.pages.first().map(|p| p.key.as_str()),
            Some("/home")
        );
        assert_eq!(dash.breakdowns.pages.first().map(|p| p.pageviews), Some(3));
        assert_eq!(
            dash.breakdowns
                .versions
                .first()
                .map(|v| (v.app.as_str(), v.version.as_str())),
            Some(("Chrome", "120.0"))
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn timeseries_zero_fills_empty_buckets() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let day = 86_400_000i64;
        // Two views on the first day only; the window spans three days.
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
            ])
            .unwrap();

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(&store, Some(&filter), 0, 3 * day, day).unwrap();

        // Buckets at 0, 1d, 2d — empty days filled with zeros, not dropped
        // (the range is half-open, so the bucket at 3d is not included).
        assert_eq!(dash.timeseries.len(), 3);
        assert_eq!(dash.timeseries[0].pageviews, 2);
        assert!(
            dash.timeseries[1..]
                .iter()
                .all(|p| p.pageviews == 0 && p.visitors == 0)
        );

        // The comparison series is index-aligned: identical length, shifted stamps.
        assert_eq!(dash.previous_timeseries.len(), dash.timeseries.len());
        for (prev, cur) in dash.previous_timeseries.iter().zip(&dash.timeseries) {
            assert_eq!(prev.timestamp_ms, cur.timestamp_ms - 3 * day);
        }

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn previous_window_feeds_summary_not_current() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // One view in the previous window, two in the current one. The event at
        // exactly `from` belongs to the current window only (half-open ranges).
        store
            .append_events(&[
                load("https://a.com", 4_000, true, None),
                load("https://a.com", 10_000, true, None),
                load("https://a.com", 12_000, false, None),
            ])
            .unwrap();

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(&store, Some(&filter), 10_000, 20_000, 86_400_000).unwrap();

        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.previous_summary.pageviews, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn query_expressions_scope_everything() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut firefox = load("https://a.com", 2_000, false, None);
        firefox.ua_browser = Some("Firefox".into());
        let mut direct = load("https://a.com", 3_000, false, None);
        direct.ua_browser = None;
        store
            .append_events(&[load("https://a.com", 1_000, true, None), firefox, direct])
            .unwrap();

        // Equality is case-insensitive, mirroring the filter language.
        let filter = dash_filter(&store, r#"browser == "chrome""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 1);

        // Disjunction spans values.
        let filter = dash_filter(&store, r#"browser == "Chrome" || browser == "Firefox""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // Membership lists work too.
        let filter = dash_filter(&store, r#"browser in ["chrome", "firefox"]"#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // An empty value matches events where the dimension is absent.
        let filter = dash_filter(&store, r#"browser == """#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 0);

        // The absent value surfaces as a sentinel row rather than being dropped.
        let dash = dashboard(&store, None, 0, 10_000, 86_400_000).unwrap();
        let sentinel = dash.breakdowns.browsers.iter().find(|r| r.key.is_empty());
        assert_eq!(sentinel.map(|r| r.pageviews), Some(1));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn source_filter_matches_bare_hostnames() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[load("https://a.com", 1_000, true, None)])
            .unwrap();

        let filter = dash_filter(&store, r#"source == "a.com""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn source_membership_selects_multiple_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://b.com", 2_000, true, None),
                load("https://c.com", 3_000, true, None), // excluded
                typed("pixel://p1", 4_000, EventKind::Pixel),
            ])
            .unwrap();

        // Bare hostnames expand to every canonical URI form.
        let filter = dash_filter(&store, r#"source in ["a.com", "b.com", "p1"]"#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.events, 1); // the pixel matched via pixel://p1
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "https://c.com")
        );

        // Mixed bare and fully-qualified names work too.
        let filter = dash_filter(&store, r#"source in ["https://a.com", "b.com"]"#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.events, 0);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn sentinel_filter_excludes_pixel_and_custom_events() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut no_browser = load("https://a.com", 1_000, false, None);
        no_browser.ua_browser = None;
        store
            .append_events(&[
                load("https://a.com", 2_000, true, None), // Chrome
                no_browser,
                typed("pixel://p1", 3_000, EventKind::Pixel),
            ])
            .unwrap();

        // browser == "" (absent) must match the browserless page view but NOT
        // the pixel hit, whose dimensions are null for a different reason.
        let filter = dash_filter(&store, r#"browser == """#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.events, 0);
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "pixel://p1")
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn path_filter_switches_rollup_visitors_too() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // A non-landing page view: not the visitor's first load of the day
        // (is_unique_user=false) but the first view of that page.
        let mut blog = load("https://a.com", 1_000, false, None);
        blog.pathname = Some("/blog".into());
        blog.is_unique_page = true;
        store.append_events(&[blog]).unwrap();

        let filter = dash_filter(&store, r#"path == "/blog""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        // The sources rollup must agree with the headline visitor count.
        assert_eq!(dash.summary.visitors, 1);
        let source = dash.breakdowns.sources.first().expect("source row");
        assert_eq!(source.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn path_filter_counts_unique_page_views_as_visitors() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // A visitor lands on / (daily-unique) then reads /blog: the /blog view is
        // not their first of the day (is_unique_user=false) but *is* the first
        // view of that page (is_unique_page=true).
        let mut landing = load("https://a.com", 1_000, true, None);
        landing.pathname = Some("/".into());
        let mut blog = load("https://a.com", 2_000, false, None);
        blog.pathname = Some("/blog".into());
        blog.is_unique_page = true;
        store.append_events(&[landing, blog]).unwrap();

        let filter = dash_filter(&store, r#"path == "/blog""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        // is_unique_user would report 0 here; is_unique_page reports the truth.
        assert_eq!(dash.summary.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn project_with_no_sources_sees_no_traffic() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[load("https://a.com", 1_000, true, None)])
            .unwrap();

        let filter = dash_filter(&store, r#"project == "empty-project""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 0);
        assert_eq!(dash.summary.visitors, 0);
        assert!(dash.breakdowns.sources.is_empty());

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn project_filter_resolves_names_case_insensitively_with_id_fallback() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .put_project(&analytics_api::Project {
                id: "01ARZAPPS".into(),
                name: "Apps".into(),
                slug: "apps".into(),
                created_at: Utc::now(),
            })
            .unwrap();
        store
            .put_source(&analytics_api::Source {
                uri: "https://a.com".into(),
                project_id: Some("01ARZAPPS".into()),
                kind: analytics_api::default_kind("https://a.com"),
                display_name: None,
                created_at: Utc::now(),
                first_seen: None,
                last_seen: None,
            })
            .unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://b.com", 2_000, true, None), // unassigned, excluded
            ])
            .unwrap();

        // The (unique) name selects the project in any case; the id still
        // resolves so pre-rename links keep working.
        for q in [
            r#"project == "Apps""#,
            r#"project == "APPS""#,
            r#"project in ["Apps"]"#,
            r#"project == "01ARZAPPS""#,
        ] {
            let filter = dash_filter(&store, q);
            let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
            assert_eq!(dash.summary.pageviews, 1, "query `{q}`");
        }

        // Negation excludes the project's traffic but keeps everything else.
        let filter = dash_filter(&store, r#"project != "Apps""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "https://a.com")
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    fn typed(source: &str, received_ms: i64, kind: EventKind) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            source: source.into(),
            kind,
            is_unique_user: false,
            ..Default::default()
        }
    }

    fn exc(group: &str, received_ms: i64) -> StoredEvent {
        exc_on("https://a.com", group, received_ms)
    }

    fn exc_on(source: &str, group: &str, received_ms: i64) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            kind: EventKind::Exception,
            source: source.into(),
            exc_type: Some("TypeError".into()),
            exc_message: Some("boom".into()),
            exc_group: Some(group.into()),
            exc_handled: Some(false),
            ..Default::default()
        }
    }

    #[test]
    fn exception_groups_keep_sources_separate_and_carry_trends() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // Same fingerprint on two different sources (e.g. a shared-library error).
        store
            .append_events(&[
                exc_on("https://a.com", "g1", 1_000),
                exc_on("https://b.com", "g1", 2_000),
                exc_on("https://a.com", "g1", 3_000),
            ])
            .unwrap();

        let rows = exception_groups_by_source(&store, 0, 10_000, None).unwrap();
        // One row per (fingerprint, source) — not collapsed across sources.
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(g, _)| g.group_id == "g1"));
        let a = rows.iter().find(|(_, s)| s == "https://a.com").unwrap();
        assert_eq!(a.0.count, 2);
        assert_eq!(a.0.trend.len(), TREND_BUCKETS);
        assert_eq!(a.0.trend.iter().sum::<i64>(), 2);
        let b = rows.iter().find(|(_, s)| s == "https://b.com").unwrap();
        assert_eq!(b.0.count, 1);
        assert_eq!(b.0.trend.iter().sum::<i64>(), 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn exception_group_lookup_ignores_the_recency_cap() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let events: Vec<_> = (1..=505)
            .map(|i| exc(&format!("g{i}"), i * 1_000))
            .collect();
        store.append_events(&events).unwrap();
        let sources = ["https://a.com".to_string()];

        // g1 is the oldest, so it falls outside the top-500-by-recency listing...
        let listing_filter = filter::compile_query(
            &source_q("https://a.com"),
            filter::FieldSet::Exceptions,
            &store,
        )
        .unwrap()
        .unwrap();
        let listed =
            exception_groups_by_source(&store, 0, 10_000_000, Some(&listing_filter)).unwrap();
        assert_eq!(listed.len(), 500);
        assert!(!listed.iter().any(|(g, _)| g.group_id == "g1"));

        // ...but a direct lookup still resolves it (group + variants in one scan).
        let g1 = exception_detail(&store, &sources, "g1", 0, 10_000_000, 10).unwrap();
        let detail = g1.expect("g1 resolves");
        assert_eq!(detail.group.group_id, "g1");
        assert_eq!(detail.group.count, 1);
        assert_eq!(detail.group.trend.iter().sum::<i64>(), 1);
        assert_eq!(detail.variants.len(), 1);
        // An unknown group resolves to None.
        assert!(
            exception_detail(&store, &sources, "nope", 0, 10_000_000, 10)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn exception_detail_collapses_variants_and_attributes_releases() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // One group, three occurrences: two share a message/stack (one variant
        // of count 2), the third differs. Different app versions throughout,
        // and the latest occurrence of the repeated variant carries metadata.
        let mut a1 = exc("g1", 1_000);
        a1.exc_message = Some("boom at start".into());
        a1.exc_stack = Some("at start (app.js)".into());
        a1.app_version = Some("1.0.0".into());
        let mut a2 = exc("g1", 2_000);
        a2.exc_message = Some("boom at start".into());
        a2.exc_stack = Some("at start (app.js)".into());
        a2.app_version = Some("1.1.0".into());
        a2.metadata_json = Some(r#"{"feature_flag":"checkout-v2"}"#.into());
        a2.sid = Some("sess-1".into());
        let mut b = exc("g1", 3_000);
        b.exc_message = Some("boom at shutdown".into());
        b.exc_stack = Some("at shutdown (app.js)".into());
        b.app_version = Some("1.1.0".into());
        store.append_events(&[a1, a2, b]).unwrap();

        let sources = ["https://a.com".to_string()];
        let detail = exception_detail(&store, &sources, "g1", 0, 10_000, 10)
            .unwrap()
            .expect("g1 resolves");

        // Two distinct variants; the repeated one carries its count and the
        // context (source-as-app, version, metadata) of its latest occurrence.
        assert_eq!(detail.variants.len(), 2);
        let repeated = detail
            .variants
            .iter()
            .find(|v| v.message == "boom at start")
            .unwrap();
        assert_eq!(repeated.count, 2);
        assert_eq!(repeated.source.as_deref(), Some("https://a.com"));
        assert_eq!(repeated.app_version.as_deref(), Some("1.1.0"));
        assert_eq!(
            repeated.metadata.as_deref(),
            Some(r#"{"feature_flag":"checkout-v2"}"#)
        );
        // The exemplar links to the session of its latest session-linked
        // occurrence.
        assert_eq!(repeated.session_id.as_deref(), Some("sess-1"));

        // The group's sessions surface as trace summaries for the picker.
        assert_eq!(detail.traces.len(), 1);
        assert_eq!(detail.traces[0].session_id, "sess-1");
        assert_eq!(detail.traces[0].exceptions, 1);

        // Distributions cover app releases (1.1.0 twice, 1.0.0 once). All the
        // occurrences share one source, so versions are keyed bare — the
        // application is given by the (source-scoped) view.
        let versions = &detail.breakdowns.app_versions;
        assert_eq!(
            versions.first().map(|r| (r.key.as_str(), r.count)),
            Some(("1.1.0", 2))
        );
        assert!(versions.iter().any(|r| r.key == "1.0.0" && r.count == 1));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    /// A custom event belonging to a session.
    fn custom_in(source: &str, sid: &str, received_ms: i64, name: &str) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b".into(),
            sid: Some(sid.into()),
            kind: EventKind::Custom,
            source: source.into(),
            event_name: Some(name.into()),
            ..Default::default()
        }
    }

    #[test]
    fn dashboard_breaks_down_event_names() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut unnamed = custom_in("https://a.com", "s1", 4_000, "ignored");
        unnamed.event_name = None;
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                custom_in("https://a.com", "s1", 2_000, "signup"),
                custom_in("https://a.com", "s1", 3_000, "signup"),
                custom_in("https://a.com", "s2", 3_500, "checkout"),
                unnamed,
            ])
            .unwrap();

        let dash = dashboard(&store, None, 0, 10_000, 86_400_000).unwrap();
        let names: Vec<(&str, i64)> = dash
            .breakdowns
            .event_names
            .iter()
            .map(|r| (r.key.as_str(), r.events))
            .collect();
        // Ranked by count; unnamed events fold under the empty sentinel; page
        // loads contribute nothing.
        assert_eq!(names[0], ("signup", 2));
        assert!(names.contains(&("checkout", 1)));
        assert!(names.contains(&("", 1)));
        assert_eq!(names.len(), 3);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn event_detail_collapses_metadata_variants() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut with_meta = custom_in("https://a.com", "s1", 2_000, "signup");
        with_meta.metadata_json = Some(r#"{"plan":"pro"}"#.into());
        with_meta.pathname = Some("/pricing".into());
        let mut repeat = custom_in("https://a.com", "s2", 3_000, "signup");
        repeat.metadata_json = Some(r#"{"plan":"pro"}"#.into());
        repeat.pathname = Some("/pricing".into());
        repeat.ua_browser = Some("Firefox".into());
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                with_meta,
                repeat,
                custom_in("https://a.com", "s1", 4_000, "signup"),
                custom_in("https://a.com", "s1", 5_000, "checkout"),
            ])
            .unwrap();

        let detail = event_detail(&store, "signup", 0, 10_000, None, 10)
            .unwrap()
            .expect("signup resolves");
        assert_eq!(detail.name, "signup");
        assert_eq!(detail.count, 3);
        assert_eq!(detail.first_seen_ms, 2_000);
        assert_eq!(detail.last_seen_ms, 4_000);
        assert_eq!(detail.trend.iter().sum::<i64>(), 3);

        // Two variants: the shared metadata (count 2, context from its latest
        // occurrence) and the metadata-less one.
        assert_eq!(detail.variants.len(), 2);
        let repeated = detail
            .variants
            .iter()
            .find(|v| v.metadata.is_some())
            .unwrap();
        assert_eq!(repeated.count, 2);
        assert_eq!(repeated.metadata.as_deref(), Some(r#"{"plan":"pro"}"#));
        assert_eq!(repeated.ua_browser.as_deref(), Some("Firefox"));
        assert_eq!(repeated.pathname.as_deref(), Some("/pricing"));
        assert_eq!(repeated.session_id.as_deref(), Some("s2"));

        // Distributions cover the event's occurrences only.
        let pages = &detail.breakdowns.pages;
        assert!(pages.iter().any(|r| r.key == "/pricing" && r.count == 2));

        // Both sessions surface as traces; the other event name resolves
        // separately, and an unknown one not at all.
        assert_eq!(detail.traces.len(), 2);
        assert!(
            event_detail(&store, "checkout", 0, 10_000, None, 10)
                .unwrap()
                .is_some()
        );
        assert!(
            event_detail(&store, "nope", 0, 10_000, None, 10)
                .unwrap()
                .is_none()
        );

        // The dashboard filter scopes the detail like every other panel.
        let filter = dash_filter(&store, r#"source == "https://other.com""#);
        assert!(
            event_detail(&store, "signup", 0, 10_000, Some(&filter), 10)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn exception_versions_qualify_only_across_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut a = exc_on("https://a.com", "g1", 1_000);
        a.app_version = Some("1.0.0".into());
        let mut b = exc_on("https://b.com", "g1", 2_000);
        b.app_version = Some("1.0.0".into());
        store.append_events(&[a, b]).unwrap();

        // Across two sources the bare number would be ambiguous, so rows stay
        // qualified as `app @ version`.
        let sources = ["https://a.com".to_string(), "https://b.com".to_string()];
        let detail = exception_detail(&store, &sources, "g1", 0, 10_000, 10)
            .unwrap()
            .expect("g1 resolves");
        let keys: Vec<&str> = detail
            .breakdowns
            .app_versions
            .iter()
            .map(|r| r.key.as_str())
            .collect();
        assert!(keys.contains(&"a.com @ 1.0.0"));
        assert!(keys.contains(&"b.com @ 1.0.0"));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn dashboard_samples_recent_session_traces() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // Session s1: lands on /home, fires a custom event, then crashes.
        let mut s1_load = load("https://a.com", 1_000, true, None);
        s1_load.sid = Some("s1".into());
        let mut s1_exc = exc("g1", 3_000);
        s1_exc.sid = Some("s1".into());
        s1_exc.app_version = Some("1.2.0".into());
        // Session s2 starts later, on a different page.
        let mut s2_load = load("https://a.com", 5_000, false, None);
        s2_load.sid = Some("s2".into());
        s2_load.pathname = Some("/pricing".into());
        s2_load.ua_device = Some("Desktop".into());
        // A sessionless page view (pre-session tracker) forms no trace.
        let plain = load("https://a.com", 6_000, false, None);
        store
            .append_events(&[
                s1_load,
                custom_in("https://a.com", "s1", 2_000, "signup"),
                s1_exc,
                s2_load,
                plain,
            ])
            .unwrap();

        let dash = dashboard(&store, None, 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.traces.len(), 2);

        // Newest session first.
        assert_eq!(dash.traces[0].session_id, "s2");
        assert_eq!(dash.traces[0].entry_path.as_deref(), Some("/pricing"));
        assert_eq!(dash.traces[0].ua_device.as_deref(), Some("Desktop"));

        let s1 = &dash.traces[1];
        assert_eq!(s1.session_id, "s1");
        assert_eq!(s1.started_ms, 1_000);
        assert_eq!(s1.last_ms, 3_000);
        assert_eq!(s1.source, "https://a.com");
        assert_eq!(s1.entry_path.as_deref(), Some("/home"));
        assert_eq!(s1.ua_browser.as_deref(), Some("Chrome"));
        assert_eq!(s1.ua_version.as_deref(), Some("120.0"));
        assert_eq!(s1.app_version.as_deref(), Some("1.2.0"));
        assert_eq!(s1.pageviews, 1);
        assert_eq!(s1.events, 1);
        assert_eq!(s1.exceptions, 1);

        // The dashboard filter scopes traces like every other panel.
        let filter = dash_filter(&store, r#"path == "/pricing""#);
        let dash = dashboard(&store, Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.traces.len(), 1);
        assert_eq!(dash.traces[0].session_id, "s2");

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn session_trace_returns_the_ordered_timeline() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut s1_load = load("https://a.com", 1_000, true, None);
        s1_load.sid = Some("s1".into());
        let mut s1_exc = exc("g1", 3_000);
        s1_exc.sid = Some("s1".into());
        let mut other = load("https://a.com", 1_500, false, None);
        other.sid = Some("other".into());
        // Appended out of order: the timeline must come back sorted by time.
        store
            .append_events(&[
                s1_exc,
                s1_load,
                custom_in("https://a.com", "s1", 2_000, "signup"),
                other,
            ])
            .unwrap();

        let trace = session_trace(&store, "s1", 0, 10_000, 1_000)
            .unwrap()
            .expect("s1 resolves");
        assert_eq!(trace.session_id, "s1");
        assert_eq!(trace.source, "https://a.com");
        assert_eq!(trace.started_ms, 1_000);
        assert_eq!(trace.ended_ms, 3_000);
        assert_eq!(trace.ua_browser.as_deref(), Some("Chrome"));

        let kinds: Vec<TraceEventKind> = trace.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TraceEventKind::PageLoad,
                TraceEventKind::Custom,
                TraceEventKind::Exception,
            ]
        );
        assert_eq!(trace.events[0].pathname.as_deref(), Some("/home"));
        assert_eq!(trace.events[1].event_name.as_deref(), Some("signup"));
        assert_eq!(trace.events[2].exc_type.as_deref(), Some("TypeError"));
        assert_eq!(trace.events[2].exc_group.as_deref(), Some("g1"));

        // An unknown session resolves to None.
        assert!(
            session_trace(&store, "nope", 0, 10_000, 1_000)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn timeseries_counts_exceptions_alongside_traffic() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                exc_on("https://a.com", "g1", 1_500),
                exc_on("https://a.com", "g1", 1_600),
            ])
            .unwrap();

        let dash = dashboard(&store, None, 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.timeseries.len(), 1);
        assert_eq!(dash.timeseries[0].pageviews, 1);
        assert_eq!(dash.timeseries[0].exceptions, 2);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn dashboard_surfaces_pixel_and_custom_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                typed("pixel://p1", 2_000, EventKind::Pixel),
                typed("app://svc", 3_000, EventKind::Custom),
            ])
            .unwrap();

        let dash = dashboard(&store, None, 0, 10_000, 86_400_000).unwrap();
        let uris: Vec<&str> = dash.unassigned.iter().map(|u| u.key.as_str()).collect();
        assert!(uris.contains(&"https://a.com"));
        assert!(uris.contains(&"pixel://p1")); // previously invisible
        assert!(uris.contains(&"app://svc")); // previously invisible

        // The website keeps its visitor count; pixel/custom count as events, not
        // pageviews, in both the rollup and the headline summary.
        let site = dash
            .unassigned
            .iter()
            .find(|u| u.key == "https://a.com")
            .unwrap();
        assert_eq!(site.visitors, 1);
        assert_eq!(site.pageviews, 1);
        assert_eq!(site.events, 0);
        let pixel = dash
            .unassigned
            .iter()
            .find(|u| u.key == "pixel://p1")
            .unwrap();
        assert_eq!(pixel.events, 1);
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.events, 2);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }
}

/// Smoke tests against a production data dump. Set `ANALYTICS_PROD_DUMP` to a
/// directory containing the legacy `parquet/` archive (e.g.
/// `.prod-dump/analytics`) to enable them; they are silently skipped
/// otherwise. Run with `--release` for a representative reading:
/// `ANALYTICS_PROD_DUMP=$PWD/.prod-dump/analytics cargo test -p analytics --release prod_dump -- --nocapture`
#[cfg(test)]
mod prod_dump_tests {
    use super::*;
    use crate::config::StorageConfig;
    use crate::store::Store;

    fn rss_stat_mb(stat: &str) -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        status
            .lines()
            .find_map(|l| l.strip_prefix(stat))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    }

    /// Migrating the legacy production archive must produce a database that
    /// serves the dashboard with the exact numbers the legacy engine computed
    /// (recorded before the DuckDB migration), and the migration must be
    /// idempotent across reopens.
    #[test]
    fn migrates_the_production_dump_and_matches_legacy_numbers() {
        let Ok(dump) = std::env::var("ANALYTICS_PROD_DUMP") else {
            return;
        };
        let work = std::env::temp_dir().join(format!("analytics-prod-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let storage = StorageConfig {
            database_path: work.join("analytics.duckdb").to_string_lossy().into_owned(),
            parquet_dir: format!("{dump}/parquet"),
            redb_path: work.join("missing.redb").to_string_lossy().into_owned(),
            ..Default::default()
        };

        let t = std::time::Instant::now();
        let store = Store::open_with_migration(&storage).unwrap();
        let migrate_time = t.elapsed();

        let from = earliest_event_ms(&store).unwrap().expect("dump has data");
        let to = from + 400 * 86_400_000;
        let t = std::time::Instant::now();
        let dash = dashboard(&store, None, from, to, 86_400_000).unwrap();
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let _ = dashboard(&store, None, from, to, 86_400_000).unwrap();
        let warm = t.elapsed();

        // Ground truth computed by the legacy polars fold over this dump.
        assert_eq!(dash.summary.pageviews, 55_193);
        assert_eq!(dash.summary.visitors, 3_345);
        assert_eq!(dash.summary.events, 78_357);
        assert_eq!(dash.traces.len(), 10);

        // Reopening performs no second import.
        let events = store.event_count().unwrap();
        drop(store);
        let reopened = Store::open_with_migration(&storage).unwrap();
        assert_eq!(reopened.event_count().unwrap(), events);

        // Eight concurrent warm dashboards sharing the buffer manager.
        let t = std::time::Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    dashboard(&reopened, None, from, to, 86_400_000).unwrap();
                });
            }
        });
        let concurrent = t.elapsed();

        let db_size = std::fs::metadata(&storage.database_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        println!(
            "prod dump: migrated {events} events in {migrate_time:?} ({db_size:.1}MB db), \
             dashboard cold {cold:?} / warm {warm:?}, 8 concurrent in {concurrent:?}, \
             peak RSS {:.1} MB, steady RSS {:.1} MB",
            rss_stat_mb("VmHWM:"),
            rss_stat_mb("VmRSS:")
        );

        drop(reopened);
        let _ = std::fs::remove_dir_all(&work);
    }
}
