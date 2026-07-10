//! Debug-only demo data generator.
//!
//! Seeds the store with randomly-generated but *representative* projects,
//! sources, pixels, and events so the dashboard and every drill-down view can be
//! exercised locally without a live tracker. Compiled only under
//! `#[cfg(debug_assertions)]`; it is never present in release builds.
//!
//! The generator writes fully-enriched [`StoredEvent`]s straight into the hot
//! store (the same rows the ingest pipeline would produce), spread across the last
//! [`DAYS`] days with a mild upward trend and weekend dips so time series, session
//! traces, exception groups, and custom-event breakdowns all look plausible.

use std::collections::BTreeMap;

use analytics_api::{Pixel, Project, Source, SourceKind};
use chrono::{Duration, Utc};
use ulid::Ulid;

use crate::errors::Result;
use crate::store::{EventKind, Store, StoredEvent};

/// How many days of history to synthesize.
const DAYS: i64 = 90;
const DAY_MS: i64 = 86_400_000;
/// Events are appended in transactions of this size to bound peak memory.
const FLUSH_EVERY: usize = 2_000;

// ---------------------------------------------------------------- distributions

/// A browser/OS/device profile sampled for a whole visit.
struct Ua {
    browser: &'static str,
    versions: &'static [&'static str],
    os: &'static str,
    device: &'static str,
}

const UA_PROFILES: &[(Ua, u32)] = &[
    (
        Ua {
            browser: "Chrome",
            versions: &["120.0", "121.0", "122.0", "123.0", "124.0"],
            os: "Windows",
            device: "Desktop",
        },
        30,
    ),
    (
        Ua {
            browser: "Chrome",
            versions: &["120.0", "121.0", "122.0", "123.0"],
            os: "Android",
            device: "Mobile",
        },
        18,
    ),
    (
        Ua {
            browser: "Safari",
            versions: &["17.2", "17.3", "17.4"],
            os: "macOS",
            device: "Desktop",
        },
        12,
    ),
    (
        Ua {
            browser: "Safari",
            versions: &["17.2", "17.3", "17.4"],
            os: "iOS",
            device: "Mobile",
        },
        14,
    ),
    (
        Ua {
            browser: "Firefox",
            versions: &["121.0", "122.0", "123.0"],
            os: "Windows",
            device: "Desktop",
        },
        8,
    ),
    (
        Ua {
            browser: "Firefox",
            versions: &["121.0", "122.0"],
            os: "Linux",
            device: "Desktop",
        },
        5,
    ),
    (
        Ua {
            browser: "Edge",
            versions: &["120.0", "121.0", "122.0"],
            os: "Windows",
            device: "Desktop",
        },
        9,
    ),
    (
        Ua {
            browser: "Chrome",
            versions: &["121.0", "122.0"],
            os: "Linux",
            device: "Desktop",
        },
        4,
    ),
];

/// ISO-3166 country codes (as the geo enrichment would derive from a timezone).
const COUNTRIES: &[(&str, u32)] = &[
    ("US", 30),
    ("GB", 12),
    ("DE", 10),
    ("FR", 8),
    ("CA", 6),
    ("AU", 5),
    ("NL", 4),
    ("IN", 7),
    ("BR", 5),
    ("JP", 4),
    ("SE", 3),
    ("ES", 3),
    ("IT", 3),
];

const LANGUAGES: &[(&str, u32)] = &[
    ("en", 55),
    ("de", 10),
    ("fr", 8),
    ("es", 7),
    ("pt", 5),
    ("ja", 4),
    ("nl", 3),
    ("sv", 3),
    ("it", 2),
];

/// Entry referrers as `(host, group)`, matching what the referrer classifier
/// would emit. `(None, None)` is direct traffic.
const REFERRERS: &[((Option<&str>, Option<&str>), u32)] = &[
    ((None, None), 45),
    ((Some("google.com"), Some("Search")), 22),
    ((Some("bing.com"), Some("Search")), 5),
    ((Some("duckduckgo.com"), Some("Search")), 4),
    ((Some("news.ycombinator.com"), Some("Social")), 6),
    ((Some("reddit.com"), Some("Social")), 5),
    ((Some("twitter.com"), Some("Social")), 4),
    ((Some("github.com"), None), 6),
    ((Some("lobste.rs"), Some("Social")), 3),
];

/// Campaign tags occasionally attached to an entry, as `(source, medium, campaign)`.
const UTMS: &[(&str, &str, &str)] = &[
    ("newsletter", "email", "weekly-digest"),
    ("twitter", "social", "launch"),
    ("google", "cpc", "brand"),
    ("hackernews", "referral", "show-hn"),
];

/// An exception template. `group` is a stable fingerprint so repeated occurrences
/// collapse into one group in the inbox.
struct Exc {
    group: &'static str,
    exc_type: &'static str,
    message: &'static str,
    stack: &'static str,
    handled: bool,
}

const WEB_EXCEPTIONS: &[(Exc, u32)] = &[
    (
        Exc {
            group: "a1b2c3d4e5f60718",
            exc_type: "TypeError",
            message: "Cannot read properties of undefined (reading 'id')",
            stack: "at renderProfile (app.js:842:19)\nat onLoad (app.js:120:7)\nat dispatch (runtime.js:55:3)",
            handled: false,
        },
        40,
    ),
    (
        Exc {
            group: "112233445566778a",
            exc_type: "NetworkError",
            message: "Failed to fetch /api/v1/dashboard (503)",
            stack: "at fetchJson (api.js:31:11)\nat loadDashboard (dashboard.js:88:22)",
            handled: true,
        },
        28,
    ),
    (
        Exc {
            group: "9f8e7d6c5b4a3928",
            exc_type: "ReferenceError",
            message: "analytics is not defined",
            stack: "at track (tracker.js:14:5)\nat HTMLDocument.<anonymous> (index.html:60:9)",
            handled: false,
        },
        16,
    ),
    (
        Exc {
            group: "0011223344556677",
            exc_type: "RangeError",
            message: "Maximum call stack size exceeded",
            stack: "at walk (tree.js:210:14)\nat walk (tree.js:214:9)\nat walk (tree.js:214:9)",
            handled: false,
        },
        8,
    ),
];

const APP_EXCEPTIONS: &[(Exc, u32)] = &[
    (
        Exc {
            group: "deadbeefcafe0001",
            exc_type: "panic",
            message: "called `Result::unwrap()` on an `Err` value: ConnectionRefused",
            stack: "at bender::db::connect (db.rs:88)\nat bender::main (main.rs:24)",
            handled: false,
        },
        30,
    ),
    (
        Exc {
            group: "deadbeefcafe0002",
            exc_type: "io::Error",
            message: "No such file or directory (os error 2)",
            stack: "at bender::config::load (config.rs:41)\nat bender::run (main.rs:57)",
            handled: true,
        },
        18,
    ),
];

/// The reported releases for the application source (drives the version breakdown).
const APP_VERSIONS: &[(&str, u32)] = &[("1.4.2", 40), ("1.4.1", 25), ("1.3.0", 15), ("1.5.0-rc1", 8)];

// --------------------------------------------------------------------- sites

/// A source to synthesize traffic for.
struct Site {
    uri: &'static str,
    display_name: &'static str,
    project_id: Option<&'static str>,
    kind: SourceKind,
    /// Weighted paths (or app screens) users land on / navigate to.
    paths: &'static [(&'static str, u32)],
    /// Average distinct visitors per day (before trend/weekend/jitter factors).
    daily_base: f64,
    /// Weighted custom event names emitted during some sessions (empty = none).
    events: &'static [(&'static str, u32)],
    /// Fraction of sessions that also report an exception.
    exception_rate: f64,
    /// Application sources report a release, have no referrer, and a device of "App".
    is_app: bool,
}

const PROJECTS: &[(&str, &str, &str)] = &[
    ("proj-sierra", "Sierra Softworks", "sierra-softworks"),
    ("proj-bender", "Bender", "bender"),
    ("proj-blog", "DevBlog", "devblog"),
];

const SITES: &[Site] = &[
    Site {
        uri: "https://sierrasoftworks.com",
        display_name: "Sierra Softworks",
        project_id: Some("proj-sierra"),
        kind: SourceKind::Website,
        paths: &[
            ("/", 40),
            ("/projects", 18),
            ("/about", 10),
            ("/blog", 14),
            ("/blog/introducing-analytics", 9),
            ("/contact", 5),
        ],
        daily_base: 45.0,
        events: &[("signup", 5), ("download", 8), ("newsletter_subscribe", 4)],
        exception_rate: 0.05,
        is_app: false,
    },
    Site {
        uri: "https://docs.sierrasoftworks.com",
        display_name: "Documentation",
        project_id: Some("proj-sierra"),
        kind: SourceKind::Website,
        paths: &[
            ("/", 20),
            ("/getting-started", 22),
            ("/guides/tracking", 16),
            ("/guides/privacy", 12),
            ("/api/reference", 18),
            ("/faq", 8),
        ],
        daily_base: 18.0,
        events: &[("search", 10), ("copy_snippet", 6)],
        exception_rate: 0.03,
        is_app: false,
    },
    Site {
        uri: "https://bender.sierrasoftworks.com",
        display_name: "Bender Web",
        project_id: Some("proj-bender"),
        kind: SourceKind::Website,
        paths: &[
            ("/", 30),
            ("/dashboard", 24),
            ("/settings", 10),
            ("/reports", 16),
            ("/login", 12),
        ],
        daily_base: 15.0,
        events: &[("report_export", 7), ("filter_apply", 12)],
        exception_rate: 0.08,
        is_app: false,
    },
    Site {
        uri: "app://bender-cli",
        display_name: "Bender CLI",
        project_id: Some("proj-bender"),
        kind: SourceKind::Application,
        paths: &[
            ("/sync", 30),
            ("/status", 20),
            ("/push", 18),
            ("/pull", 18),
            ("/login", 8),
        ],
        daily_base: 12.0,
        events: &[("command_run", 20), ("update_check", 6)],
        exception_rate: 0.18,
        is_app: true,
    },
    Site {
        uri: "https://blog.pannell.dev",
        display_name: "DevBlog",
        project_id: Some("proj-blog"),
        kind: SourceKind::Website,
        paths: &[
            ("/", 24),
            ("/posts/rust-async", 20),
            ("/posts/observability", 16),
            ("/posts/redb-notes", 14),
            ("/tags/rust", 10),
            ("/about", 6),
        ],
        daily_base: 22.0,
        events: &[("share_click", 9)],
        exception_rate: 0.02,
        is_app: false,
    },
    Site {
        // Intentionally unassigned so the "unassigned sources" panel is populated.
        uri: "https://staging.example.com",
        display_name: "Staging",
        project_id: None,
        kind: SourceKind::Website,
        paths: &[("/", 30), ("/preview", 20), ("/health", 10)],
        daily_base: 4.0,
        events: &[],
        exception_rate: 0.12,
        is_app: false,
    },
];

const PIXEL_ID: &str = "px-newsletter";

// ----------------------------------------------------------------------- rng

/// A tiny, dependency-free SplitMix64 PRNG. Adequate for demo data; not for
/// anything security-sensitive.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x2545_F491_4F6C_DD1D)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `0.0..1.0`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A uniform integer in `0..n` (returns 0 when `n == 0`).
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// A uniform `i64` in `lo..=hi`.
    fn between(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo + 1) as u64) as i64
    }

    /// `true` with probability `p`.
    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    /// Weighted choice over `(value, weight)` pairs.
    fn weighted<'a, T>(&mut self, items: &'a [(T, u32)]) -> &'a T {
        let total: u32 = items.iter().map(|(_, w)| *w).sum();
        let mut roll = self.below(total.max(1) as u64) as u32;
        for (value, weight) in items {
            if roll < *weight {
                return value;
            }
            roll -= *weight;
        }
        &items[0].0
    }
}

// --------------------------------------------------------------------- seeding

/// Generate representative data and inject it into `store`. Returns the number of
/// events written. Registers projects, sources, and a pixel, then synthesizes
/// visits, custom events, exceptions, and pixel hits across the last [`DAYS`] days.
pub fn seed(store: &Store) -> Result<usize> {
    let now = Utc::now();
    let now_ms = now.timestamp_millis();
    let window_start_ms = now_ms - DAYS * DAY_MS;
    let mut rng = Rng::new(now_ms as u64);

    register_entities(store, now)?;

    let mut batch: Vec<StoredEvent> = Vec::with_capacity(FLUSH_EVERY);
    let mut total = 0usize;

    for site in SITES {
        for day in 0..DAYS {
            let day_start = window_start_ms + day * DAY_MS;
            let weekend = matches!((day_start / DAY_MS) % 7, 5 | 6);
            let weekend_factor = if weekend { 0.6 } else { 1.0 };
            // A gentle upward trend across the window.
            let trend = 0.55 + 0.45 * (day as f64 / DAYS as f64);
            let jitter = 0.7 + rng.unit() * 0.6;
            let visitors = (site.daily_base * weekend_factor * trend * jitter).round() as i64;

            for _ in 0..visitors.max(0) {
                let start = (day_start + rng.below(DAY_MS as u64) as i64).min(now_ms - 1_000);
                simulate_visit(&mut rng, site, start, &mut batch);
                if batch.len() >= FLUSH_EVERY {
                    total += flush(store, &mut batch)?;
                }
            }
        }
    }

    // Pixel hits (newsletter opens) attributed to the Sierra Softworks project.
    for day in 0..DAYS {
        let day_start = window_start_ms + day * DAY_MS;
        let trend = 0.55 + 0.45 * (day as f64 / DAYS as f64);
        let hits = (14.0 * trend * (0.7 + rng.unit() * 0.6)).round() as i64;
        for _ in 0..hits.max(0) {
            let t = (day_start + rng.below(DAY_MS as u64) as i64).min(now_ms - 1_000);
            batch.push(pixel_hit(&mut rng, t));
            if batch.len() >= FLUSH_EVERY {
                total += flush(store, &mut batch)?;
            }
        }
    }

    total += flush(store, &mut batch)?;
    Ok(total)
}

/// Register the demo projects, sources, and pixel.
fn register_entities(store: &Store, now: chrono::DateTime<Utc>) -> Result<()> {
    let created = now - Duration::days(DAYS + 30);
    let first_seen = now - Duration::days(DAYS);

    for (id, name, slug) in PROJECTS {
        store.put_project(&Project {
            id: (*id).to_string(),
            name: (*name).to_string(),
            slug: (*slug).to_string(),
            created_at: created,
        })?;
    }

    for site in SITES {
        store.put_source(&Source {
            uri: site.uri.to_string(),
            project_id: site.project_id.map(str::to_string),
            kind: site.kind,
            display_name: Some(site.display_name.to_string()),
            created_at: created,
            first_seen: Some(first_seen),
            last_seen: Some(now),
        })?;
    }

    let mut metadata = BTreeMap::new();
    metadata.insert("campaign".to_string(), "weekly-digest".to_string());
    store.put_pixel(&Pixel {
        id: PIXEL_ID.to_string(),
        project_id: "proj-sierra".to_string(),
        name: "Newsletter Open".to_string(),
        event_name: "newsletter_open".to_string(),
        metadata,
        created_at: created,
        last_hit: Some(now),
    })?;

    Ok(())
}

/// Simulate a single visit (a session with one or more page views), appending its
/// events to `batch`.
fn simulate_visit(rng: &mut Rng, site: &Site, start_ms: i64, batch: &mut Vec<StoredEvent>) {
    let ua = rng.weighted(UA_PROFILES);
    let (browser, version, os, device) = if site.is_app {
        (
            "bender",
            *rng.weighted(APP_VERSIONS),
            *rng.pick(&["Linux", "macOS", "Windows"]),
            "App",
        )
    } else {
        (ua.browser, *rng.pick(ua.versions), ua.os, ua.device)
    };
    let app_version = site.is_app.then(|| version.to_string());
    let country = country_of(rng);
    let language = Some((*rng.weighted(LANGUAGES)).to_string());
    let sid = Ulid::new().to_string();

    // Pageview count skews toward short visits (bounces).
    let pageviews = *rng.weighted(&[(1u32, 42), (2, 24), (3, 15), (4, 10), (5, 6), (6, 3)]);

    // Applications and self-referred navigations have no external referrer.
    let (ref_host, ref_group) = if site.is_app {
        (None, None)
    } else {
        let (host, group) = rng.weighted(REFERRERS);
        (host.map(str::to_string), group.map(str::to_string))
    };
    let utm = (!site.is_app && rng.chance(0.15)).then(|| *rng.pick(UTMS));

    let mut t = start_ms;
    let mut seen_paths: Vec<&str> = Vec::new();

    for index in 0..pageviews {
        let path = rng.weighted(site.paths);
        let is_first_page = !seen_paths.contains(path);
        if is_first_page {
            seen_paths.push(path);
        }

        let (utm_source, utm_medium, utm_campaign) = match (index, utm) {
            (0, Some((s, m, c))) => (
                Some(s.to_string()),
                Some(m.to_string()),
                Some(c.to_string()),
            ),
            _ => (None, None, None),
        };

        let load = StoredEvent {
            created_ms: t,
            received_ms: t,
            bid: Ulid::new().to_string(),
            sid: Some(sid.clone()),
            kind: EventKind::PageLoad,
            source: site.uri.to_string(),
            pathname: Some(path.to_string()),
            is_unique_user: index == 0,
            is_unique_page: is_first_page,
            referrer_host: if index == 0 { ref_host.clone() } else { None },
            referrer_group: if index == 0 { ref_group.clone() } else { None },
            country: country.clone(),
            language: language.clone(),
            ua_browser: Some(browser.to_string()),
            ua_version: Some(version.to_string()),
            ua_os: Some(os.to_string()),
            ua_device: Some(device.to_string()),
            utm_source,
            utm_medium,
            utm_campaign,
            app_version: app_version.clone(),
            ..Default::default()
        };
        let load_bid = load.bid.clone();
        batch.push(load);

        // Most page views end with an unload beacon carrying the dwell time.
        let dwell = dwell_ms(rng);
        if rng.chance(0.8) {
            batch.push(StoredEvent {
                created_ms: t + dwell,
                received_ms: t + dwell,
                bid: load_bid.clone(),
                sid: Some(sid.clone()),
                kind: EventKind::PageUnload,
                source: site.uri.to_string(),
                pathname: Some(path.to_string()),
                country: country.clone(),
                language: language.clone(),
                ua_browser: Some(browser.to_string()),
                ua_version: Some(version.to_string()),
                ua_os: Some(os.to_string()),
                ua_device: Some(device.to_string()),
                duration_ms: Some(dwell),
                app_version: app_version.clone(),
                ..Default::default()
            });
        }

        // A custom event sometimes fires mid-visit.
        if !site.events.is_empty() && rng.chance(0.18) {
            let name = *rng.weighted(site.events);
            batch.push(StoredEvent {
                created_ms: t + rng.between(200, dwell.max(300)),
                received_ms: t + rng.between(200, dwell.max(300)),
                bid: load_bid.clone(),
                sid: Some(sid.clone()),
                kind: EventKind::Custom,
                source: site.uri.to_string(),
                pathname: Some(path.to_string()),
                country: country.clone(),
                language: language.clone(),
                ua_browser: Some(browser.to_string()),
                ua_version: Some(version.to_string()),
                ua_os: Some(os.to_string()),
                ua_device: Some(device.to_string()),
                event_name: Some(name.to_string()),
                metadata_json: Some(event_metadata(rng, name)),
                app_version: app_version.clone(),
                ..Default::default()
            });
        }

        t += dwell + rng.between(300, 4_000);
    }

    // Some visits report an exception.
    if rng.chance(site.exception_rate) {
        let templates = if site.is_app { APP_EXCEPTIONS } else { WEB_EXCEPTIONS };
        let exc = rng.weighted(templates);
        let et = start_ms + rng.between(500, 60_000);
        batch.push(StoredEvent {
            created_ms: et,
            received_ms: et,
            bid: Ulid::new().to_string(),
            sid: Some(sid.clone()),
            kind: EventKind::Exception,
            source: site.uri.to_string(),
            country: country.clone(),
            language: language.clone(),
            ua_browser: Some(browser.to_string()),
            ua_version: Some(version.to_string()),
            ua_os: Some(os.to_string()),
            ua_device: Some(device.to_string()),
            app_version: app_version.clone(),
            exc_type: Some(exc.exc_type.to_string()),
            exc_message: Some(exc.message.to_string()),
            exc_stack: Some(exc.stack.to_string()),
            exc_group: Some(exc.group.to_string()),
            exc_handled: Some(exc.handled),
            ..Default::default()
        });
    }
}

/// A single newsletter-open pixel hit.
fn pixel_hit(rng: &mut Rng, t: i64) -> StoredEvent {
    let ua = rng.weighted(UA_PROFILES);
    let mut metadata = BTreeMap::new();
    metadata.insert("campaign".to_string(), "weekly-digest".to_string());
    metadata.insert(
        "edition".to_string(),
        format!("2025-{:02}", rng.between(1, 12)),
    );
    StoredEvent {
        created_ms: t,
        received_ms: t,
        bid: Ulid::new().to_string(),
        kind: EventKind::Pixel,
        source: analytics_api::pixel_source(PIXEL_ID),
        country: country_of(rng),
        language: Some((*rng.weighted(LANGUAGES)).to_string()),
        ua_browser: Some(ua.browser.to_string()),
        ua_version: Some((*rng.pick(ua.versions)).to_string()),
        ua_os: Some(ua.os.to_string()),
        ua_device: Some(ua.device.to_string()),
        event_name: Some("newsletter_open".to_string()),
        metadata_json: serde_json::to_string(&metadata).ok(),
        ..Default::default()
    }
}

/// A country code, or `None` (~4% "unknown") to exercise the empty bucket.
fn country_of(rng: &mut Rng) -> Option<String> {
    if rng.chance(0.04) {
        None
    } else {
        Some((*rng.weighted(COUNTRIES)).to_string())
    }
}

/// Dwell time: ~55% short (potential bounces), the rest engaged.
fn dwell_ms(rng: &mut Rng) -> i64 {
    if rng.chance(0.55) {
        rng.between(800, 4_800)
    } else {
        rng.between(6_000, 420_000)
    }
}

/// A small, varied metadata payload for a custom event.
fn event_metadata(rng: &mut Rng, name: &str) -> String {
    let mut meta = BTreeMap::new();
    match name {
        "signup" | "newsletter_subscribe" => {
            meta.insert("plan".to_string(), (*rng.pick(&["free", "pro", "team"])).to_string());
        }
        "download" => {
            meta.insert(
                "asset".to_string(),
                (*rng.pick(&["installer.exe", "app.dmg", "app.AppImage"])).to_string(),
            );
        }
        "search" => {
            meta.insert(
                "query".to_string(),
                (*rng.pick(&["tracking", "privacy", "api key", "self-host"])).to_string(),
            );
        }
        "command_run" => {
            meta.insert("cmd".to_string(), (*rng.pick(&["sync", "push", "pull", "status"])).to_string());
        }
        _ => {
            meta.insert("value".to_string(), rng.between(1, 100).to_string());
        }
    }
    serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string())
}

/// Persist and clear the current batch, returning how many events were written.
fn flush(store: &Store, batch: &mut Vec<StoredEvent>) -> Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }
    let events = std::mem::take(batch);
    let count = events.len();
    store.append_events(&events)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempStore {
        store: Store,
        path: std::path::PathBuf,
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn temp_store() -> TempStore {
        let path = std::env::temp_dir().join(format!(
            "analytics-demo-test-{}-{:?}.redb",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("open store");
        TempStore { store, path }
    }

    #[test]
    fn seed_populates_the_store() {
        let temp = temp_store();
        let count = seed(&temp.store).expect("seed");

        // A representative dataset should be substantial.
        assert!(count > 1_000, "expected many events, got {count}");
        assert_eq!(temp.store.event_count().unwrap() as usize, count);

        // Entities are registered so the dashboard can roll events up to projects.
        assert_eq!(temp.store.list_projects().unwrap().len(), PROJECTS.len());
        assert_eq!(temp.store.list_sources().unwrap().len(), SITES.len());
        assert_eq!(temp.store.list_pixels().unwrap().len(), 1);
    }

    #[test]
    fn every_event_kind_is_represented() {
        let temp = temp_store();
        seed(&temp.store).expect("seed");
        let events = temp.store.all_events().unwrap();

        let has = |kind: EventKind| events.iter().any(|e| e.kind == kind);
        assert!(has(EventKind::PageLoad));
        assert!(has(EventKind::PageUnload));
        assert!(has(EventKind::Custom));
        assert!(has(EventKind::Pixel));
        assert!(has(EventKind::Exception));

        // Exceptions carry a grouping fingerprint and application sources report a version.
        assert!(
            events
                .iter()
                .any(|e| e.kind == EventKind::Exception && e.exc_group.is_some())
        );
        assert!(
            events
                .iter()
                .any(|e| e.source == "app://bender-cli" && e.app_version.is_some())
        );
    }
}

