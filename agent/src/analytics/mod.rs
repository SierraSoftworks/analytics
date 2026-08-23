//! The fold-based query layer. Statistics are computed over the union of the
//! redb hot store and the cold Parquet partitions, filtered by a compiled
//! [`filter`] expression, and bounded to a half-open `[from, to)` time range.
//!
//! The union is never materialized: [`scan`] streams it one partition frame at
//! a time and each query folds the rows into small aggregate state (count maps,
//! time buckets, per-session accumulators), so a query's peak memory tracks its
//! *results* — a few thousand aggregate entries — rather than the size of the
//! window it spans. polars is used only to decode each partition and to apply
//! the compiled `q` predicate per frame. Queries are CPU-bound and synchronous,
//! so handlers run them via `web::block`.

pub mod filter;
mod scan;

use std::collections::{HashMap, HashSet};

#[cfg(test)]
use chrono::{TimeZone, Utc};

use analytics_api::{
    BreakdownRow, Breakdowns, CountRow, Dashboard, EventBreakdowns, EventDetail, EventVariant,
    ExceptionBreakdowns, ExceptionGroup, ExceptionGroupDetail, ExceptionStatus, ExceptionVariant,
    MetricSummary, SessionTrace, TREND_BUCKETS, TimeSeriesPoint, TraceEvent, TraceEventKind,
    TraceSummary, VersionRow, pixel_source, source_label, summary_line,
};

use crate::errors::Result;
use crate::store::Store;

use filter::CompiledFilter;
use scan::{Rows, scan_events};

const BREAKDOWN_LIMIT: usize = 25;
/// How many recent session traces the dashboard payload samples.
const TRACE_SAMPLE: usize = 10;
/// `[100ms, 5s]` is treated as a bounce (per the medama methodology).
const BOUNCE_MIN_MS: i64 = 100;
const BOUNCE_MAX_MS: i64 = 5_000;
const MIN_BOUNCE_SAMPLES: i64 = 5;
/// Exception listings are capped to the most recently seen groups.
const EXCEPTION_GROUP_LIMIT: usize = 500;

/// The full dashboard payload: headline metrics with a previous-window baseline,
/// the (index-aligned) time series pair, every dimension breakdown, and the
/// project/source rollups — all folded from one streamed pass over the event
/// store.
///
/// `filter` is the compiled `q` expression (see [`filter::compile_query`]);
/// `None` means unfiltered.
///
/// One pass spanning `[from - len, to)` feeds every panel: events before `from`
/// land only in the previous-window baseline, events after it in every
/// current-window aggregate.
pub fn dashboard(
    store: &Store,
    parquet_dir: &str,
    filter: Option<&CompiledFilter>,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Dashboard> {
    let _archive = crate::store::archive_read();
    let len = (to_ms - from_ms).max(1);
    let prev_from = from_ms - len;
    let bucket_ms = bucket_ms.max(1);

    // With a path filter active, `is_unique_user` (which rides only on the first
    // page load of a visitor's day) would undercount non-landing pages to ~zero;
    // daily-unique *page* views are the honest visitor count there.
    let unique_by_page = filter.is_some_and(|f| f.references("path"));

    let mut current = SummaryFold::default();
    let mut previous = SummaryFold::default();
    let mut current_buckets: HashMap<i64, BucketCounts> = HashMap::new();
    let mut previous_buckets: HashMap<i64, BucketCounts> = HashMap::new();
    let mut pages = BreakdownFold::default();
    let mut referrers = BreakdownFold::default();
    let mut countries = BreakdownFold::default();
    let mut languages = BreakdownFold::default();
    let mut browsers = BreakdownFold::default();
    let mut operating_systems = BreakdownFold::default();
    let mut devices = BreakdownFold::default();
    let mut utm_sources = BreakdownFold::default();
    let mut utm_mediums = BreakdownFold::default();
    let mut utm_campaigns = BreakdownFold::default();
    let mut versions: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut event_names: HashMap<String, i64> = HashMap::new();
    let mut per_source: HashMap<String, (i64, i64, i64)> = HashMap::new();
    // Sessions are folded in a second, sid-scoped pass (see below); this pass
    // only records when each one started, which is all the sampling needs.
    let mut session_starts: HashMap<String, i64> = HashMap::new();

    scan_events(
        store,
        parquet_dir,
        prev_from,
        to_ms,
        filter.map(|f| f.predicate.clone()),
        &mut |rows| {
            for i in 0..rows.height {
                let t = rows.received_ms.get(i).unwrap_or(0);
                let kind = rows.kind.get(i).unwrap_or("");
                let is_load = kind == "page_load";
                let is_event = kind == "pixel" || kind == "custom";
                let is_exception = kind == "exception";
                let unique = if unique_by_page {
                    rows.is_unique_page.get(i)
                } else {
                    rows.is_unique_user.get(i)
                }
                .unwrap_or(false);

                // Events before `from` feed the baseline only. Its time series is
                // bucketed on the *current* window's grid by shifting timestamps
                // forward one window length, guaranteeing index alignment; the
                // emitted points are shifted back below.
                let in_current = t >= from_ms;
                let (summary, buckets, bucket_t) = if in_current {
                    (&mut current, &mut current_buckets, t)
                } else {
                    (&mut previous, &mut previous_buckets, t + len)
                };
                summary.observe(is_load, is_event, unique, rows.duration_ms.get(i));
                if is_load || is_event || is_exception {
                    let counts = buckets.entry(bucket_t - bucket_t % bucket_ms).or_default();
                    counts.0 += is_load as i64;
                    counts.1 += (is_load && unique) as i64;
                    counts.2 += is_event as i64;
                    counts.3 += is_exception as i64;
                }
                if !in_current {
                    continue;
                }

                if is_load {
                    pages.hit(
                        rows.pathname.get(i),
                        rows.is_unique_page.get(i).unwrap_or(false),
                    );
                    referrers.hit(rows.referrer_host.get(i), unique);
                    countries.hit(rows.country.get(i), unique);
                    languages.hit(rows.language.get(i), unique);
                    browsers.hit(rows.ua_browser.get(i), unique);
                    operating_systems.hit(rows.ua_os.get(i), unique);
                    devices.hit(rows.ua_device.get(i), unique);
                    utm_sources.hit(rows.utm_source.get(i), unique);
                    utm_mediums.hit(rows.utm_medium.get(i), unique);
                    utm_campaigns.hit(rows.utm_campaign.get(i), unique);
                    let version = versions
                        .entry((
                            rows.ua_browser.get(i).unwrap_or("").to_string(),
                            rows.ua_version.get(i).unwrap_or("").to_string(),
                        ))
                        .or_default();
                    version.0 += 1;
                    version.1 += unique as i64;
                }
                if is_event {
                    *counter(&mut event_names, rows.event_name.get(i).unwrap_or("")) += 1;
                }
                if is_load || is_event {
                    let source = counter(&mut per_source, rows.source.get(i).unwrap_or(""));
                    source.0 += is_load as i64;
                    source.1 += unique as i64;
                    source.2 += is_event as i64;
                }
                if let Some(sid) = rows.sid.get(i)
                    && !sid.is_empty()
                {
                    match session_starts.get_mut(sid) {
                        Some(started) => *started = (*started).min(t),
                        None => {
                            session_starts.insert(sid.to_string(), t);
                        }
                    }
                }
            }
            Ok(())
        },
    )?;

    // Sample the most recently started sessions, then summarize just those in
    // a second sid-scoped pass — filtered like every other panel, so the list
    // is scoped exactly the way the operator's `q` scopes the rest.
    let traces = traces_of_sessions(store, parquet_dir, session_starts, filter, from_ms, to_ms)?;

    let timeseries = fill_series(&current_buckets, from_ms, to_ms, bucket_ms);
    let mut previous_timeseries = fill_series(&previous_buckets, from_ms, to_ms, bucket_ms);
    for point in &mut previous_timeseries {
        point.timestamp_ms -= len;
    }

    let mut version_rows: Vec<VersionRow> = versions
        .into_iter()
        .map(|((app, version), (pageviews, visitors))| VersionRow {
            app,
            version,
            pageviews,
            visitors,
            events: 0,
        })
        .collect();
    version_rows.sort_by(|a, b| {
        b.pageviews
            .cmp(&a.pageviews)
            .then_with(|| (&a.app, &a.version).cmp(&(&b.app, &b.version)))
    });
    version_rows.truncate(BREAKDOWN_LIMIT);

    let mut event_name_rows: Vec<BreakdownRow> = event_names
        .into_iter()
        .map(|(key, events)| BreakdownRow {
            key,
            visitors: 0,
            pageviews: 0,
            events,
        })
        .collect();
    event_name_rows.sort_by(|a, b| b.events.cmp(&a.events).then_with(|| a.key.cmp(&b.key)));
    event_name_rows.truncate(BREAKDOWN_LIMIT);

    // Per-source totals. Page loads count as `pageviews`; pixel hits and custom
    // events count as `events` so pixel-only and application sources still
    // surface; `visitors` uses the same daily-unique flag as every other
    // aggregation in the response, so the panels agree with the headline.
    let mut source_rows: Vec<BreakdownRow> = per_source
        .into_iter()
        .map(|(key, (pageviews, visitors, events))| BreakdownRow {
            key,
            pageviews,
            visitors,
            events,
        })
        .collect();
    source_rows.sort_by(|a, b| {
        (b.pageviews + b.events)
            .cmp(&(a.pageviews + a.events))
            .then_with(|| a.key.cmp(&b.key))
    });
    let (projects, sources, unassigned) = project_rollup(store, source_rows)?;

    Ok(Dashboard {
        summary: current.finish(),
        previous_summary: previous.finish(),
        timeseries,
        previous_timeseries,
        breakdowns: Breakdowns {
            pages: pages.finish(),
            referrers: referrers.finish(),
            countries: countries.finish(),
            languages: languages.finish(),
            browsers: browsers.finish(),
            versions: version_rows,
            operating_systems: operating_systems.finish(),
            devices: devices.finish(),
            utm_sources: utm_sources.finish(),
            utm_mediums: utm_mediums.finish(),
            utm_campaigns: utm_campaigns.finish(),
            event_names: event_name_rows,
            projects,
            sources,
        },
        unassigned,
        traces,
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
    parquet_dir: &str,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
) -> Result<Vec<(ExceptionGroup, String)>> {
    let _archive = crate::store::archive_read();

    struct GroupAcc {
        count: i64,
        first: i64,
        last: i64,
        exc_type: Latest,
        message: Latest,
        trend: Vec<i64>,
    }
    let mut groups: HashMap<(String, String), GroupAcc> = HashMap::new();

    scan_events(
        store,
        parquet_dir,
        from_ms,
        to_ms,
        filter.map(|f| f.predicate.clone()),
        &mut |rows| {
            for i in 0..rows.height {
                if rows.kind.get(i) != Some("exception") {
                    continue;
                }
                let Some(group) = rows.exc_group.get(i) else {
                    continue;
                };
                let t = rows.received_ms.get(i).unwrap_or(0);
                let source = rows.source.get(i).unwrap_or("");
                let acc = groups
                    .entry((group.to_string(), source.to_string()))
                    .or_insert_with(|| GroupAcc {
                        count: 0,
                        first: t,
                        last: t,
                        exc_type: Latest::default(),
                        message: Latest::default(),
                        trend: vec![0; TREND_BUCKETS],
                    });
                acc.count += 1;
                acc.first = acc.first.min(t);
                acc.last = acc.last.max(t);
                acc.exc_type.observe(t, rows.exc_type.get(i));
                acc.message.observe(t, rows.exc_message.get(i));
                acc.trend[trend_index(t, from_ms, to_ms)] += 1;
            }
            Ok(())
        },
    )?;

    let mut out: Vec<(ExceptionGroup, String)> = groups
        .into_iter()
        .map(|((group_id, source), acc)| {
            (
                ExceptionGroup {
                    group_id,
                    exc_type: acc.exc_type.value.unwrap_or_default(),
                    sample_message: summary_line(acc.message.value.as_deref().unwrap_or(""))
                        .to_string(),
                    count: acc.count,
                    first_seen_ms: acc.first,
                    last_seen_ms: acc.last,
                    status: ExceptionStatus::Unresolved,
                    resolved: false,
                    muted: false,
                    note: None,
                    trend: acc.trend,
                },
                source,
            )
        })
        .collect();
    out.sort_by(|a, b| {
        b.0.last_seen_ms
            .cmp(&a.0.last_seen_ms)
            .then_with(|| a.0.group_id.cmp(&b.0.group_id))
    });
    out.truncate(EXCEPTION_GROUP_LIMIT);
    Ok(out)
}

/// A single exception group in forensic detail: the aggregate (with trend),
/// how its occurrences distribute across key dimensions, and its **distinct
/// variants** — occurrences collapsed by (message, stack, handledness) so an
/// operator scrubs through genuinely different examples rather than paging
/// hundreds of identical ones. Folded from one streamed pass filtered to the
/// group; looked up by id directly (no top-N cap), so a linked or bookmarked
/// group opens regardless of how many fingerprints a project has. Returns
/// `None` if the group has no occurrences in `[from_ms, to_ms)`.
pub fn exception_detail(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    group_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<ExceptionGroupDetail>> {
    let _archive = crate::store::archive_read();
    let sources: HashSet<&str> = sources.iter().map(String::as_str).collect();

    #[derive(Default)]
    struct VariantAcc {
        count: i64,
        first: i64,
        last: i64,
        // The representative context comes from the variant's latest occurrence
        // (even if that occurrence's value is null)…
        ctx_t: i64,
        ua_browser: Option<String>,
        ua_os: Option<String>,
        source: Option<String>,
        app_version: Option<String>,
        // …except metadata and the session link, which come from the latest
        // occurrence that actually carries one.
        metadata: Latest,
        sid: Latest,
    }

    let mut count = 0i64;
    let mut first = i64::MAX;
    let mut last = 0i64;
    let mut latest_type = Latest::default();
    let mut latest_message = Latest::default();
    let mut trend = vec![0i64; TREND_BUCKETS];
    let mut app_versions: HashMap<(String, String), i64> = HashMap::new();
    let mut browsers = CountFold::default();
    let mut operating_systems = CountFold::default();
    let mut devices = CountFold::default();
    // Variants collapse occurrences by (message, stack, handledness).
    type VariantKey = (Option<String>, Option<String>, Option<bool>);
    let mut variants: HashMap<VariantKey, VariantAcc> = HashMap::new();
    let mut occurrence_sids: HashMap<String, i64> = HashMap::new();

    scan_events(store, parquet_dir, from_ms, to_ms, None, &mut |rows| {
        for i in 0..rows.height {
            if rows.kind.get(i) != Some("exception")
                || rows.exc_group.get(i) != Some(group_id)
                || !rows.source.get(i).is_some_and(|s| sources.contains(s))
            {
                continue;
            }
            let t = rows.received_ms.get(i).unwrap_or(0);
            count += 1;
            first = first.min(t);
            last = last.max(t);
            latest_type.observe(t, rows.exc_type.get(i));
            latest_message.observe(t, rows.exc_message.get(i));
            trend[trend_index(t, from_ms, to_ms)] += 1;

            *app_versions
                .entry((
                    rows.source.get(i).unwrap_or("").to_string(),
                    rows.app_version.get(i).unwrap_or("").to_string(),
                ))
                .or_default() += 1;
            browsers.hit(rows.ua_browser.get(i));
            operating_systems.hit(rows.ua_os.get(i));
            devices.hit(rows.ua_device.get(i));

            let variant = variants
                .entry((
                    rows.exc_message.get(i).map(str::to_string),
                    rows.exc_stack.get(i).map(str::to_string),
                    rows.exc_handled.get(i),
                ))
                .or_insert_with(|| VariantAcc {
                    first: t,
                    last: t,
                    ..Default::default()
                });
            variant.count += 1;
            variant.first = variant.first.min(t);
            variant.last = variant.last.max(t);
            if t >= variant.ctx_t {
                variant.ctx_t = t;
                variant.ua_browser = rows.ua_browser.get(i).map(str::to_string);
                variant.ua_os = rows.ua_os.get(i).map(str::to_string);
                variant.source = rows.source.get(i).map(str::to_string);
                variant.app_version = rows.app_version.get(i).map(str::to_string);
            }
            variant.metadata.observe(t, rows.metadata_json.get(i));
            variant.sid.observe(t, rows.sid.get(i));

            if let Some(sid) = rows.sid.get(i)
                && !sid.is_empty()
            {
                let seen = occurrence_sids.entry(sid.to_string()).or_insert(t);
                *seen = (*seen).max(t);
            }
        }
        Ok(())
    })?;

    if count == 0 {
        return Ok(None);
    }

    let group = ExceptionGroup {
        group_id: group_id.to_string(),
        exc_type: latest_type.value.unwrap_or_default(),
        sample_message: summary_line(latest_message.value.as_deref().unwrap_or("")).to_string(),
        count,
        first_seen_ms: first,
        last_seen_ms: last,
        status: ExceptionStatus::Unresolved,
        resolved: false,
        muted: false,
        note: None,
        trend,
    };

    let mut variant_rows: Vec<(VariantKey, VariantAcc)> = variants.into_iter().collect();
    variant_rows.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then_with(|| b.1.last.cmp(&a.1.last))
    });
    variant_rows.truncate(limit);
    let variants = variant_rows
        .into_iter()
        .map(|((message, stack, handled), acc)| ExceptionVariant {
            message: message.unwrap_or_default(),
            stack,
            handled: handled.unwrap_or(false),
            count: acc.count,
            first_seen_ms: acc.first,
            last_seen_ms: acc.last,
            ua_browser: acc.ua_browser,
            ua_os: acc.ua_os,
            source: acc.source,
            app_version: acc.app_version,
            metadata: acc.metadata.value,
            session_id: acc.sid.value,
        })
        .collect();

    let breakdowns = ExceptionBreakdowns {
        app_versions: app_version_rows(app_versions),
        browsers: browsers.finish(),
        operating_systems: operating_systems.finish(),
        devices: devices.finish(),
    };
    let traces = traces_of_sessions(store, parquet_dir, occurrence_sids, None, from_ms, to_ms)?;

    Ok(Some(ExceptionGroupDetail {
        group,
        breakdowns,
        variants,
        traces,
    }))
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
    parquet_dir: &str,
    name: &str,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
    limit: usize,
) -> Result<Option<EventDetail>> {
    let _archive = crate::store::archive_read();

    #[derive(Default)]
    struct VariantAcc {
        count: i64,
        first: i64,
        last: i64,
        // Context of the latest occurrence; the session link is the latest
        // occurrence that has one.
        ctx_t: i64,
        ua_browser: Option<String>,
        ua_os: Option<String>,
        source: Option<String>,
        pathname: Option<String>,
        sid: Latest,
    }

    let mut count = 0i64;
    let mut first = i64::MAX;
    let mut last = 0i64;
    let mut trend = vec![0i64; TREND_BUCKETS];
    let mut sources = CountFold::default();
    let mut pages = CountFold::default();
    let mut browsers = CountFold::default();
    let mut operating_systems = CountFold::default();
    let mut devices = CountFold::default();
    let mut countries = CountFold::default();
    let mut languages = CountFold::default();
    let mut variants: HashMap<Option<String>, VariantAcc> = HashMap::new();
    let mut occurrence_sids: HashMap<String, i64> = HashMap::new();

    scan_events(
        store,
        parquet_dir,
        from_ms,
        to_ms,
        filter.map(|f| f.predicate.clone()),
        &mut |rows| {
            for i in 0..rows.height {
                let kind = rows.kind.get(i).unwrap_or("");
                if (kind != "pixel" && kind != "custom") || rows.event_name.get(i) != Some(name) {
                    continue;
                }
                let t = rows.received_ms.get(i).unwrap_or(0);
                count += 1;
                first = first.min(t);
                last = last.max(t);
                trend[trend_index(t, from_ms, to_ms)] += 1;

                sources.hit(rows.source.get(i));
                pages.hit(rows.pathname.get(i));
                browsers.hit(rows.ua_browser.get(i));
                operating_systems.hit(rows.ua_os.get(i));
                devices.hit(rows.ua_device.get(i));
                countries.hit(rows.country.get(i));
                languages.hit(rows.language.get(i));

                let variant = variants
                    .entry(rows.metadata_json.get(i).map(str::to_string))
                    .or_insert_with(|| VariantAcc {
                        first: t,
                        last: t,
                        ..Default::default()
                    });
                variant.count += 1;
                variant.first = variant.first.min(t);
                variant.last = variant.last.max(t);
                if t >= variant.ctx_t {
                    variant.ctx_t = t;
                    variant.ua_browser = rows.ua_browser.get(i).map(str::to_string);
                    variant.ua_os = rows.ua_os.get(i).map(str::to_string);
                    variant.source = rows.source.get(i).map(str::to_string);
                    variant.pathname = rows.pathname.get(i).map(str::to_string);
                }
                variant.sid.observe(t, rows.sid.get(i));

                if let Some(sid) = rows.sid.get(i)
                    && !sid.is_empty()
                {
                    let seen = occurrence_sids.entry(sid.to_string()).or_insert(t);
                    *seen = (*seen).max(t);
                }
            }
            Ok(())
        },
    )?;

    if count == 0 {
        return Ok(None);
    }

    let mut variant_rows: Vec<(Option<String>, VariantAcc)> = variants.into_iter().collect();
    variant_rows.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then_with(|| b.1.last.cmp(&a.1.last))
    });
    variant_rows.truncate(limit);
    let variants = variant_rows
        .into_iter()
        .map(|(metadata, acc)| EventVariant {
            metadata,
            count: acc.count,
            first_seen_ms: acc.first,
            last_seen_ms: acc.last,
            ua_browser: acc.ua_browser,
            ua_os: acc.ua_os,
            source: acc.source,
            pathname: acc.pathname,
            session_id: acc.sid.value,
        })
        .collect();

    let breakdowns = EventBreakdowns {
        sources: sources.finish(),
        pages: pages.finish(),
        browsers: browsers.finish(),
        operating_systems: operating_systems.finish(),
        devices: devices.finish(),
        countries: countries.finish(),
        languages: languages.finish(),
    };
    let traces = traces_of_sessions(store, parquet_dir, occurrence_sids, None, from_ms, to_ms)?;

    Ok(Some(EventDetail {
        name: name.to_string(),
        count,
        first_seen_ms: first,
        last_seen_ms: last,
        trend,
        breakdowns,
        variants,
        traces,
    }))
}

/// Summaries of a fold's most interesting sessions, newest first, so the
/// operator can pick which trace to open. `sids` maps each candidate session
/// id to its sampling weight — the dashboard passes session start times, the
/// detail views the latest matching occurrence — and only the top
/// [`TRACE_SAMPLE`] are summarized, by a second sid-scoped streamed pass.
/// The detail views pass no `filter` so their sessions are summarized in full
/// (their page views and events, not just the occurrences that matched); the
/// dashboard passes its `q` so the list is scoped like every other panel.
fn traces_of_sessions(
    store: &Store,
    parquet_dir: &str,
    sids: HashMap<String, i64>,
    filter: Option<&CompiledFilter>,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<TraceSummary>> {
    if sids.is_empty() {
        return Ok(Vec::new());
    }
    let mut recent: Vec<(String, i64)> = sids.into_iter().collect();
    recent.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    recent.truncate(TRACE_SAMPLE);
    let wanted: HashSet<String> = recent.into_iter().map(|(sid, _)| sid).collect();

    let mut traces = TracesFold::default();
    scan_events(
        store,
        parquet_dir,
        from_ms,
        to_ms,
        filter.map(|f| f.predicate.clone()),
        &mut |rows| {
            for i in 0..rows.height {
                if rows.sid.get(i).is_some_and(|sid| wanted.contains(sid)) {
                    let t = rows.received_ms.get(i).unwrap_or(0);
                    traces.observe(rows, i, t, rows.kind.get(i).unwrap_or(""));
                }
            }
            Ok(())
        },
    )?;
    Ok(traces.finish(TRACE_SAMPLE))
}

/// One session's full timeline: every event carrying the session id, oldest
/// first, plus the visit's context (source, locale, client, claimed release)
/// drawn from the earliest event that reports each. Looked up by id directly —
/// no recency cap — so a trace linked from an exception exemplar or a bookmark
/// always opens; `limit` bounds the returned timeline. Returns `None` when the
/// session has no events in `[from_ms, to_ms)`.
pub fn session_trace(
    store: &Store,
    parquet_dir: &str,
    session_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<SessionTrace>> {
    let _archive = crate::store::archive_read();

    struct Collected {
        received_ms: i64,
        seq: u64,
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

    let mut collected: Vec<Collected> = Vec::new();
    scan_events(store, parquet_dir, from_ms, to_ms, None, &mut |rows| {
        for i in 0..rows.height {
            if rows.sid.get(i) != Some(session_id) {
                continue;
            }
            collected.push(Collected {
                received_ms: rows.received_ms.get(i).unwrap_or(0),
                seq: rows.seq.get(i).unwrap_or(0),
                kind: rows.kind.get(i).unwrap_or("").to_string(),
                bid: rows.bid.get(i).unwrap_or("").to_string(),
                pathname: rows.pathname.get(i).map(str::to_string),
                duration_ms: rows.duration_ms.get(i),
                event_name: rows.event_name.get(i).map(str::to_string),
                metadata: rows.metadata_json.get(i).map(str::to_string),
                exc_type: rows.exc_type.get(i).map(str::to_string),
                exc_message: rows.exc_message.get(i).map(str::to_string),
                exc_stack: rows.exc_stack.get(i).map(str::to_string),
                exc_group: rows.exc_group.get(i).map(str::to_string),
                exc_handled: rows.exc_handled.get(i),
                source: rows.source.get(i).map(str::to_string),
                country: rows.country.get(i).map(str::to_string),
                language: rows.language.get(i).map(str::to_string),
                ua_browser: rows.ua_browser.get(i).map(str::to_string),
                ua_version: rows.ua_version.get(i).map(str::to_string),
                ua_os: rows.ua_os.get(i).map(str::to_string),
                app_version: rows.app_version.get(i).map(str::to_string),
            });
        }
        Ok(())
    })?;

    if collected.is_empty() {
        return Ok(None);
    }
    // `seq` breaks same-millisecond ties in arrival order.
    collected.sort_by_key(|row| (row.received_ms, row.seq));
    collected.truncate(limit);

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

/// Headline-metric accumulator for one window (current or previous).
#[derive(Default)]
struct SummaryFold {
    pageviews: i64,
    visitors: i64,
    events: i64,
    bounces: i64,
    durations: Vec<i64>,
}

impl SummaryFold {
    fn observe(&mut self, is_load: bool, is_event: bool, unique: bool, duration: Option<i64>) {
        if is_load {
            self.pageviews += 1;
            if unique {
                self.visitors += 1;
            }
        }
        if is_event {
            self.events += 1;
        }
        if let Some(duration) = duration {
            self.durations.push(duration);
            if (BOUNCE_MIN_MS..=BOUNCE_MAX_MS).contains(&duration) {
                self.bounces += 1;
            }
        }
    }

    fn finish(mut self) -> MetricSummary {
        let samples = self.durations.len() as i64;
        self.durations.sort_unstable();
        let median = match self.durations.len() {
            0 => None,
            n if n % 2 == 1 => Some(self.durations[n / 2] as f64),
            n => Some((self.durations[n / 2 - 1] as f64 + self.durations[n / 2] as f64) / 2.0),
        };
        MetricSummary {
            visitors: self.visitors,
            pageviews: self.pageviews,
            events: self.events,
            bounce_rate: (samples >= MIN_BOUNCE_SAMPLES)
                .then(|| self.bounces as f64 / samples as f64),
            median_duration_ms: median.map(|m| m.round() as i64),
        }
    }
}

/// A dimension breakdown accumulator over the page-load rows. Null (and empty)
/// dimension values aggregate under the sentinel empty-string key rather than
/// being dropped, so direct traffic and unknown values stay visible and
/// filterable and share percentages stay honest.
#[derive(Default)]
struct BreakdownFold(HashMap<String, (i64, i64)>);

impl BreakdownFold {
    fn hit(&mut self, key: Option<&str>, unique: bool) {
        let counts = counter(&mut self.0, key.unwrap_or(""));
        counts.0 += 1;
        counts.1 += unique as i64;
    }

    fn finish(self) -> Vec<BreakdownRow> {
        let mut rows: Vec<BreakdownRow> = self
            .0
            .into_iter()
            .map(|(key, (pageviews, visitors))| BreakdownRow {
                key,
                pageviews,
                visitors,
                events: 0,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.pageviews
                .cmp(&a.pageviews)
                .then_with(|| a.key.cmp(&b.key))
        });
        rows.truncate(BREAKDOWN_LIMIT);
        rows
    }
}

/// Occurrence counts per value (nulls under the empty-string sentinel),
/// finished largest first.
#[derive(Default)]
struct CountFold(HashMap<String, i64>);

impl CountFold {
    fn hit(&mut self, key: Option<&str>) {
        *counter(&mut self.0, key.unwrap_or("")) += 1;
    }

    fn finish(self) -> Vec<CountRow> {
        let mut rows: Vec<CountRow> = self
            .0
            .into_iter()
            .map(|(key, count)| CountRow { key, count })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        rows.truncate(BREAKDOWN_LIMIT);
        rows
    }
}

/// A string-keyed accumulator slot, probing by `&str` first so the hot fold
/// paths only allocate an owned key the first time each distinct value is
/// seen — not once per row.
fn counter<'m, V: Default>(map: &'m mut HashMap<String, V>, key: &str) -> &'m mut V {
    if !map.contains_key(key) {
        map.insert(key.to_string(), V::default());
    }
    map.get_mut(key).expect("just inserted")
}

/// The latest (by timestamp) non-null value seen so far.
#[derive(Default)]
struct Latest {
    t: Option<i64>,
    value: Option<String>,
}

impl Latest {
    fn observe(&mut self, t: i64, value: Option<&str>) {
        if let Some(value) = value
            && self.t.is_none_or(|seen| t >= seen)
        {
            self.t = Some(t);
            self.value = Some(value.to_string());
        }
    }
}

/// The earliest (by timestamp) non-null value seen so far.
fn keep_earliest(slot: &mut Option<(i64, String)>, t: i64, value: Option<&str>) {
    if let Some(value) = value
        && !slot.as_ref().is_some_and(|(seen, _)| *seen <= t)
    {
        *slot = Some((t, value.to_string()));
    }
}

/// Folds session summaries out of the streamed rows: one accumulator per
/// session id (events without one — pixel hits, pre-session trackers — never
/// form a trace), finished as the most recently started `limit` sessions. The
/// summary spans the rows the caller feeds it, so a dimension filter scopes
/// this list exactly the way it scopes every other panel.
#[derive(Default)]
struct TracesFold {
    sessions: HashMap<String, TraceAcc>,
}

#[derive(Default)]
struct TraceAcc {
    started: i64,
    last: i64,
    source: Option<(i64, String)>,
    entry_path: Option<(i64, String)>,
    country: Option<(i64, String)>,
    ua_browser: Option<(i64, String)>,
    ua_version: Option<(i64, String)>,
    ua_device: Option<(i64, String)>,
    app_version: Option<(i64, String)>,
    pageviews: i64,
    events: i64,
    exceptions: i64,
}

impl TracesFold {
    fn observe(&mut self, rows: &Rows, i: usize, t: i64, kind: &str) {
        let Some(sid) = rows.sid.get(i) else {
            return;
        };
        if sid.is_empty() {
            return;
        }
        let acc = self
            .sessions
            .entry(sid.to_string())
            .or_insert_with(|| TraceAcc {
                started: t,
                last: t,
                ..Default::default()
            });
        acc.started = acc.started.min(t);
        acc.last = acc.last.max(t);
        keep_earliest(&mut acc.source, t, rows.source.get(i));
        keep_earliest(&mut acc.country, t, rows.country.get(i));
        keep_earliest(&mut acc.ua_browser, t, rows.ua_browser.get(i));
        keep_earliest(&mut acc.ua_version, t, rows.ua_version.get(i));
        keep_earliest(&mut acc.ua_device, t, rows.ua_device.get(i));
        keep_earliest(&mut acc.app_version, t, rows.app_version.get(i));
        match kind {
            "page_load" => {
                acc.pageviews += 1;
                // The first page viewed in the session.
                keep_earliest(&mut acc.entry_path, t, rows.pathname.get(i));
            }
            "pixel" | "custom" => acc.events += 1,
            "exception" => acc.exceptions += 1,
            _ => {}
        }
    }

    fn finish(self, limit: usize) -> Vec<TraceSummary> {
        let mut sessions: Vec<(String, TraceAcc)> = self.sessions.into_iter().collect();
        sessions.sort_by(|a, b| b.1.started.cmp(&a.1.started).then_with(|| a.0.cmp(&b.0)));
        sessions.truncate(limit);
        sessions
            .into_iter()
            .map(|(session_id, acc)| TraceSummary {
                session_id,
                started_ms: acc.started,
                last_ms: acc.last,
                source: acc.source.map(|(_, v)| v).unwrap_or_default(),
                entry_path: acc.entry_path.map(|(_, v)| v),
                country: acc.country.map(|(_, v)| v),
                ua_browser: acc.ua_browser.map(|(_, v)| v),
                ua_version: acc.ua_version.map(|(_, v)| v),
                ua_device: acc.ua_device.map(|(_, v)| v),
                app_version: acc.app_version.map(|(_, v)| v),
                pageviews: acc.pageviews,
                events: acc.events,
                exceptions: acc.exceptions,
            })
            .collect()
    }
}

/// The [`TREND_BUCKETS`] bucket index for an occurrence at `t` within
/// `[from_ms, to_ms)`.
fn trend_index(t: i64, from_ms: i64, to_ms: i64) -> usize {
    let span = (to_ms - from_ms).max(1) as i128;
    ((t - from_ms) as i128 * TREND_BUCKETS as i128 / span).clamp(0, TREND_BUCKETS as i128 - 1)
        as usize
}

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
/// buckets. The cold archive is date-partitioned, so its earliest partition
/// directory answers without scanning any data; only a store with no cold
/// partitions yet (first hours of a deployment) reads the hot store.
pub fn earliest_event_ms(store: &Store, parquet_dir: &str) -> Result<Option<i64>> {
    if let Some(ms) = scan::earliest_partition_ms(std::path::Path::new(parquet_dir)) {
        return Ok(Some(ms));
    }
    // The hot event log is keyed by `(received_ms, seq)`, so the first entry is
    // the earliest.
    Ok(store.all_events()?.first().map(|event| event.received_ms))
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
    fn earliest_event_ms_prefers_the_cold_archive_and_falls_back_to_hot() {
        let parquet_dir = std::env::temp_dir().join(format!(
            "analytics-earliest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // With no partitions and an empty hot store there is no earliest event.
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            None
        );

        // Hot-only: the earliest hot event answers.
        store
            .append_events(&[load("https://a.com", 5_000, true, None)])
            .unwrap();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            Some(5_000)
        );

        // A date partition (even an empty directory tree counts — partitions
        // only exist once written) beats the hot store.
        std::fs::create_dir_all(parquet_dir.join("2024/03/07")).unwrap();
        std::fs::create_dir_all(parquet_dir.join("2025/01/01")).unwrap();
        let expected = Utc
            .with_ymd_and_hms(2024, 3, 7, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            Some(expected)
        );
        std::fs::remove_dir_all(&parquet_dir).ok();
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

        // No parquet dir -> hot store only.
        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(
            &store,
            "/nonexistent-parquet",
            Some(&filter),
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 3 * day, day).unwrap();

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
        let dash = dashboard(&store, "/none", Some(&filter), 10_000, 20_000, 86_400_000).unwrap();

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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 1);

        // Disjunction spans values.
        let filter = dash_filter(&store, r#"browser == "Chrome" || browser == "Firefox""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // Membership lists work too.
        let filter = dash_filter(&store, r#"browser in ["chrome", "firefox"]"#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // An empty value matches events where the dimension is absent.
        let filter = dash_filter(&store, r#"browser == """#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 0);

        // The absent value surfaces as a sentinel row rather than being dropped.
        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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
            let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
            assert_eq!(dash.summary.pageviews, 1, "query `{q}`");
        }

        // Negation excludes the project's traffic but keeps everything else.
        let filter = dash_filter(&store, r#"project != "Apps""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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

        let rows = exception_groups_by_source(&store, "/none", 0, 10_000, None).unwrap();
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
    fn union_deduplicates_a_crash_duplicated_window() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
            ])
            .unwrap();

        // Simulate a crash between archive and delete: the same window now lives in
        // both Parquet and the hot store. The archived rows carry the stamped `seq`.
        let archived = store.all_events().unwrap();
        let parquet_dir =
            std::env::temp_dir().join(format!("analytics-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parquet_dir);
        let partition = parquet_dir
            .join("1970")
            .join("01")
            .join("01")
            .join("events-1.parquet");
        crate::store::write_partition(&archived, &partition).unwrap();

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(
            &store,
            parquet_dir.to_str().unwrap(),
            Some(&filter),
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

        // Without dedup this would double to 4 pageviews / 2 visitors.
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet_dir);
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
            exception_groups_by_source(&store, "/none", 0, 10_000_000, Some(&listing_filter))
                .unwrap();
        assert_eq!(listed.len(), 500);
        assert!(!listed.iter().any(|(g, _)| g.group_id == "g1"));

        // ...but a direct lookup still resolves it (group + variants in one scan).
        let g1 = exception_detail(&store, "/none", &sources, "g1", 0, 10_000_000, 10).unwrap();
        let detail = g1.expect("g1 resolves");
        assert_eq!(detail.group.group_id, "g1");
        assert_eq!(detail.group.count, 1);
        assert_eq!(detail.group.trend.iter().sum::<i64>(), 1);
        assert_eq!(detail.variants.len(), 1);
        // An unknown group resolves to None.
        assert!(
            exception_detail(&store, "/none", &sources, "nope", 0, 10_000_000, 10)
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
        let detail = exception_detail(&store, "/none", &sources, "g1", 0, 10_000, 10)
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

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
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

        let detail = event_detail(&store, "/none", "signup", 0, 10_000, None, 10)
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
            event_detail(&store, "/none", "checkout", 0, 10_000, None, 10)
                .unwrap()
                .is_some()
        );
        assert!(
            event_detail(&store, "/none", "nope", 0, 10_000, None, 10)
                .unwrap()
                .is_none()
        );

        // The dashboard filter scopes the detail like every other panel.
        let filter = dash_filter(&store, r#"source == "https://other.com""#);
        assert!(
            event_detail(&store, "/none", "signup", 0, 10_000, Some(&filter), 10)
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
        let detail = exception_detail(&store, "/none", &sources, "g1", 0, 10_000, 10)
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

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
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
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
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

        let trace = session_trace(&store, "/none", "s1", 0, 10_000, 1_000)
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
            session_trace(&store, "/none", "nope", 0, 10_000, 1_000)
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

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
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

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
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
/// directory containing a `parquet/` archive (e.g. `.prod-dump/analytics`) to
/// enable them; they are silently skipped otherwise. Run with `--release` for a
/// representative memory reading:
/// `ANALYTICS_PROD_DUMP=$PWD/.prod-dump/analytics cargo test -p analytics --release prod_dump -- --nocapture`
#[cfg(test)]
mod prod_dump_tests {
    use super::*;
    use crate::store::Store;
    use std::path::Path;

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    fn count_files(dir: &Path) -> usize {
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if entry.path().is_dir() {
                n += count_files(&entry.path());
            } else if entry.path().extension().is_some_and(|e| e == "parquet") {
                n += 1;
            }
        }
        n
    }

    fn rss_stat_mb(stat: &str) -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        status
            .lines()
            .find_map(|l| l.strip_prefix(stat))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    }

    fn peak_rss_mb() -> f64 {
        rss_stat_mb("VmHWM:")
    }

    /// Measure dashboard queries against an existing archive in a fresh
    /// process (no copies or rewrites): set `ANALYTICS_PROD_DUMP_DIR` to a
    /// parquet archive directory. Reports cold/warm timing and peak RSS only.
    #[test]
    fn dashboard_peak_memory() {
        let Ok(parquet_dir) = std::env::var("ANALYTICS_PROD_DUMP_DIR") else {
            return;
        };
        if let Some(mb) = std::env::var("ANALYTICS_CACHE_MB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            crate::store::partition_cache().set_budget(mb * 1024 * 1024);
        }
        let redb = std::env::temp_dir().join(format!("analytics-peak-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&redb);
        let store = Store::open(&redb).unwrap();
        let from = earliest_event_ms(&store, &parquet_dir)
            .unwrap()
            .expect("archive has data");
        let to = from + 400 * 86_400_000;
        println!("baseline peak RSS {:.1} MB", peak_rss_mb());
        let t = std::time::Instant::now();
        let dash = dashboard(&store, &parquet_dir, None, from, to, 86_400_000).unwrap();
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let _ = dashboard(&store, &parquet_dir, None, from, to, 86_400_000).unwrap();
        println!(
            "dashboard ({} pageviews): cold {cold:?}, warm {:?}, peak RSS {:.1} MB, \
             steady RSS {:.1} MB",
            dash.summary.pageviews,
            t.elapsed(),
            peak_rss_mb(),
            rss_stat_mb("VmRSS:")
        );

        // Eight concurrent warm dashboards sharing the cached frames: the
        // point of the partition pool is that these add fold state only.
        let t = std::time::Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    dashboard(&store, &parquet_dir, None, from, to, 86_400_000).unwrap();
                });
            }
        });
        println!(
            "8 concurrent warm dashboards in {:?}: peak RSS {:.1} MB, steady RSS {:.1} MB",
            t.elapsed(),
            peak_rss_mb(),
            rss_stat_mb("VmRSS:")
        );
        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    /// The dashboard folded over the real archive must produce identical
    /// results as the compactor repacks it: fragmented hourly files, daily
    /// consolidation, and monthly sealing.
    #[test]
    fn dashboard_is_stable_across_consolidation_and_sealing() {
        let Ok(dump) = std::env::var("ANALYTICS_PROD_DUMP") else {
            return;
        };
        let work = std::env::temp_dir().join(format!("analytics-prod-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        copy_tree(&Path::new(&dump).join("parquet"), &work.join("parquet"));
        let parquet = work.join("parquet");
        let parquet_dir = parquet.to_str().unwrap();

        let store = Store::open(work.join("smoke.redb")).unwrap();
        let from = earliest_event_ms(&store, parquet_dir)
            .unwrap()
            .expect("dump has data");
        let to = from + 400 * 86_400_000;

        let t = std::time::Instant::now();
        let fragmented = dashboard(&store, parquet_dir, None, from, to, 86_400_000).unwrap();
        let fragmented_time = t.elapsed();
        assert!(
            fragmented.summary.pageviews > 0,
            "dump should contain page views"
        );

        let files_fragmented = count_files(&parquet);
        crate::ingest::consolidate(&parquet, 1).unwrap();
        let files_daily = count_files(&parquet);
        let daily = dashboard(&store, parquet_dir, None, from, to, 86_400_000).unwrap();
        assert_eq!(fragmented.summary, daily.summary);
        assert_eq!(fragmented.timeseries, daily.timeseries);
        assert_eq!(fragmented.breakdowns, daily.breakdowns);
        assert_eq!(fragmented.traces, daily.traces);

        // Seal every month in the dump (the cutoff is later than all of it).
        crate::ingest::seal_months(&parquet, to, 2).unwrap();
        let files_monthly = count_files(&parquet);
        assert!(files_monthly <= files_daily);
        let t = std::time::Instant::now();
        let monthly = dashboard(&store, parquet_dir, None, from, to, 86_400_000).unwrap();
        let monthly_time = t.elapsed();
        assert_eq!(fragmented.summary, monthly.summary);
        assert_eq!(fragmented.timeseries, monthly.timeseries);
        assert_eq!(fragmented.breakdowns, monthly.breakdowns);
        assert_eq!(fragmented.traces, monthly.traces);

        println!(
            "prod dump: {files_fragmented} -> {files_daily} -> {files_monthly} files, \
             {} pageviews / {} visitors, fragmented dashboard in {fragmented_time:?}, \
             monthly in {monthly_time:?}, peak RSS {:.1} MB",
            fragmented.summary.pageviews,
            fragmented.summary.visitors,
            peak_rss_mb()
        );

        drop(store);
        // `ANALYTICS_PROD_DUMP_KEEP=1` keeps the repacked archive around, e.g.
        // to point `ANALYTICS_PROD_DUMP_DIR` at the sealed layout.
        if std::env::var("ANALYTICS_PROD_DUMP_KEEP").is_ok() {
            println!("keeping {}", parquet.display());
        } else {
            let _ = std::fs::remove_dir_all(&work);
        }
    }
}
