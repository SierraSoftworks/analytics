//! Display formatting shared across the dashboard: numbers, durations,
//! timestamps, deltas, and country/language names.

/// Group an integer with thousands separators, e.g. `12,345`.
pub fn group_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A compact count for dense panels: `982`, `12.3k`, `4.1M`.
pub fn compact(n: i64) -> String {
    let abs = n.abs() as f64;
    if abs >= 1_000_000.0 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if abs >= 10_000.0 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_duration(ms: i64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

/// A compact "time ago" for last-seen columns.
pub fn ago(ms: i64) -> String {
    let now = js_sys::Date::now() as i64;
    let secs = ((now - ms) / 1000).max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// The percentage change from `previous` to `current`, if computable.
pub fn delta_percent(current: f64, previous: f64) -> Option<f64> {
    (previous.abs() > f64::EPSILON).then(|| (current - previous) / previous * 100.0)
}

/// Format an epoch-millis instant for an axis label, sized to the bucket width:
/// time-of-day for hourly (and finer) buckets, a date otherwise — a 6-hour or
/// daily series labelled by time-of-day would repeat the same few times.
pub fn axis_label(ms: i64, bucket_ms: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    if bucket_ms <= 3_600_000 {
        format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
    } else {
        format!("{} {}", month_short(date.get_month()), date.get_date())
    }
}

/// Format an epoch-millis instant for the hover tooltip: full date, plus the
/// time of day when buckets are sub-day.
pub fn tooltip_label(ms: i64, bucket_ms: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let base = format!(
        "{} {} {}",
        month_short(date.get_month()),
        date.get_date(),
        date.get_full_year()
    );
    if bucket_ms < 86_400_000 {
        format!("{base}, {:02}:{:02}", date.get_hours(), date.get_minutes())
    } else {
        base
    }
}

/// Format an epoch-millis instant as a short date (for custom-range chips).
pub fn short_date(ms: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!("{} {}", month_short(date.get_month()), date.get_date())
}

fn month_short(month: u32) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(month as usize)
    .copied()
    .unwrap_or("?")
}

/// The flag emoji for an ISO 3166-1 alpha-2 country code.
pub fn country_flag(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    code.chars()
        .map(|c| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32)))
        .collect::<Option<String>>()
}

/// A display name for an ISO 3166-1 alpha-2 country code (falls back to the code).
pub fn country_name(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();
    COUNTRIES
        .iter()
        .find(|(c, _)| *c == upper)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or(upper)
}

/// A display name for a primary language subtag (falls back to the code).
pub fn language_name(code: &str) -> String {
    let lower = code.trim().to_ascii_lowercase();
    let primary = lower.split('-').next().unwrap_or(&lower);
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == primary)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| code.to_string())
}

const COUNTRIES: &[(&str, &str)] = &[
    ("AE", "United Arab Emirates"),
    ("AR", "Argentina"),
    ("AT", "Austria"),
    ("AU", "Australia"),
    ("BE", "Belgium"),
    ("BG", "Bulgaria"),
    ("BR", "Brazil"),
    ("CA", "Canada"),
    ("CH", "Switzerland"),
    ("CL", "Chile"),
    ("CN", "China"),
    ("CO", "Colombia"),
    ("CZ", "Czechia"),
    ("DE", "Germany"),
    ("DK", "Denmark"),
    ("EE", "Estonia"),
    ("EG", "Egypt"),
    ("ES", "Spain"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("GB", "United Kingdom"),
    ("GR", "Greece"),
    ("HK", "Hong Kong"),
    ("HR", "Croatia"),
    ("HU", "Hungary"),
    ("ID", "Indonesia"),
    ("IE", "Ireland"),
    ("IL", "Israel"),
    ("IN", "India"),
    ("IS", "Iceland"),
    ("IT", "Italy"),
    ("JP", "Japan"),
    ("KE", "Kenya"),
    ("KR", "South Korea"),
    ("LT", "Lithuania"),
    ("LU", "Luxembourg"),
    ("LV", "Latvia"),
    ("MX", "Mexico"),
    ("MY", "Malaysia"),
    ("NG", "Nigeria"),
    ("NL", "Netherlands"),
    ("NO", "Norway"),
    ("NZ", "New Zealand"),
    ("PH", "Philippines"),
    ("PK", "Pakistan"),
    ("PL", "Poland"),
    ("PT", "Portugal"),
    ("RO", "Romania"),
    ("RS", "Serbia"),
    ("RU", "Russia"),
    ("SA", "Saudi Arabia"),
    ("SE", "Sweden"),
    ("SG", "Singapore"),
    ("SI", "Slovenia"),
    ("SK", "Slovakia"),
    ("TH", "Thailand"),
    ("TR", "Türkiye"),
    ("TW", "Taiwan"),
    ("UA", "Ukraine"),
    ("US", "United States"),
    ("VN", "Vietnam"),
    ("ZA", "South Africa"),
];

const LANGUAGES: &[(&str, &str)] = &[
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("nb", "Norwegian"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sr", "Serbian"),
    ("sv", "Swedish"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];
