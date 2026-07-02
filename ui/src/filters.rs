//! The dashboard's shared filter state: a time range plus a [filt-rs] filter
//! expression, living **in the URL query string** (`?q=browser == "Chrome"`)
//! so every view is shareable and the back button walks filter history.
//!
//! The expression is decomposed into structured **terms** — top-level
//! `field == "value"` conjuncts, which render as removable chips and are what
//! click-to-filter edits — plus an **advanced** remainder (anything more
//! complex: `||`, `contains`, `like`, …) that round-trips verbatim and is
//! edited through the header query bar.
//!
//! State flows one way: components parse the location into a [`FilterSet`]
//! ([`use_filters`]) and mutate it only inside user event handlers by pushing a
//! new URL ([`use_apply_filters`]) — never from effects, which would loop.
//!
//! [filt-rs]: https://github.com/SierraSoftworks/filters

use filt_rs::{
    BinaryOperator, CompiledRegex, Expr, ExprVisitor, Filter, FilterValue, Glob, LogicalOperator,
    UnaryOperator,
};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::Route;

const DAY_MS: i64 = 86_400_000;

/// A chip-able event dimension. `Project` and `Source` scope which sources are
/// queried; the rest match enriched event columns. A source *is* an
/// application, so exception app attribution rides on `Source` + `AppVersion`.
/// (The query language also accepts `app` — an alias for `source` — plus
/// `type`, `message`, and `handled` on exceptions; those stay in the advanced
/// expression rather than becoming chips.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dim {
    Project,
    Source,
    Path,
    Referrer,
    Country,
    Language,
    Browser,
    Os,
    Device,
    UtmSource,
    UtmMedium,
    UtmCampaign,
    AppVersion,
}

impl Dim {
    pub const ALL: [Dim; 13] = [
        Dim::Project,
        Dim::Source,
        Dim::Path,
        Dim::Referrer,
        Dim::Country,
        Dim::Language,
        Dim::Browser,
        Dim::Os,
        Dim::Device,
        Dim::UtmSource,
        Dim::UtmMedium,
        Dim::UtmCampaign,
        Dim::AppVersion,
    ];

    /// The field name used in filter expressions.
    pub fn param(self) -> &'static str {
        match self {
            Dim::Project => "project",
            Dim::Source => "source",
            Dim::Path => "path",
            Dim::Referrer => "referrer",
            Dim::Country => "country",
            Dim::Language => "language",
            Dim::Browser => "browser",
            Dim::Os => "os",
            Dim::Device => "device",
            Dim::UtmSource => "utm_source",
            Dim::UtmMedium => "utm_medium",
            Dim::UtmCampaign => "utm_campaign",
            Dim::AppVersion => "app_version",
        }
    }

    pub fn from_param(param: &str) -> Option<Dim> {
        Dim::ALL.into_iter().find(|d| d.param() == param)
    }

    /// The human label used on filter chips and add-filter menus.
    pub fn label(self) -> &'static str {
        match self {
            Dim::Project => "Project",
            Dim::Source => "Source",
            Dim::Path => "Page",
            Dim::Referrer => "Referrer",
            Dim::Country => "Country",
            Dim::Language => "Language",
            Dim::Browser => "Browser",
            Dim::Os => "OS",
            Dim::Device => "Device",
            Dim::UtmSource => "UTM source",
            Dim::UtmMedium => "UTM medium",
            Dim::UtmCampaign => "UTM campaign",
            Dim::AppVersion => "App version",
        }
    }

    /// The label shown for the sentinel (absent) value of this dimension.
    pub fn absent_label(self) -> &'static str {
        match self {
            Dim::Referrer => "Direct / none",
            Dim::UtmSource | Dim::UtmMedium | Dim::UtmCampaign => "None",
            _ => "Unknown",
        }
    }

    /// Whether the exceptions endpoint can honour this dimension (exception
    /// events carry source/UA/version columns only).
    pub fn applies_to_exceptions(self) -> bool {
        matches!(
            self,
            Dim::Project | Dim::Source | Dim::Browser | Dim::Os | Dim::Device | Dim::AppVersion
        )
    }

    /// Whether the dashboard (page-event) endpoint can honour this dimension.
    pub fn applies_to_dashboard(self) -> bool {
        !matches!(self, Dim::AppVersion)
    }
}

/// Every field name the *exceptions* endpoint accepts (chips plus the
/// advanced-only fields), for deciding whether an advanced expression can be
/// sent along.
const EXCEPTION_FIELDS: &[&str] = &[
    "project", "source", "browser", "os", "device", "app", "app_version", "type", "message",
    "handled",
];
const DASHBOARD_FIELDS: &[&str] = &[
    "project", "source", "path", "referrer", "country", "language", "browser", "os", "device",
    "utm_source", "utm_medium", "utm_campaign",
];

/// A relative lookback preset. Presets stay relative in the URL (`range=7d`), so
/// a shared "last 7 days" link is still the last 7 days when opened tomorrow;
/// explicit `from`/`to` parameters freeze an absolute window instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RangePreset {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl RangePreset {
    pub const ALL: [RangePreset; 5] = [
        RangePreset::Day,
        RangePreset::Week,
        RangePreset::Month,
        RangePreset::Quarter,
        RangePreset::Year,
    ];

    pub fn token(self) -> &'static str {
        match self {
            RangePreset::Day => "24h",
            RangePreset::Week => "7d",
            RangePreset::Month => "30d",
            RangePreset::Quarter => "90d",
            RangePreset::Year => "12m",
        }
    }

    pub fn from_token(token: &str) -> Option<RangePreset> {
        Self::ALL.into_iter().find(|p| p.token() == token)
    }

    pub fn label(self) -> &'static str {
        match self {
            RangePreset::Day => "Last 24 hours",
            RangePreset::Week => "Last 7 days",
            RangePreset::Month => "Last 30 days",
            RangePreset::Quarter => "Last 90 days",
            RangePreset::Year => "Last 12 months",
        }
    }

    pub fn days(self) -> i64 {
        match self {
            RangePreset::Day => 1,
            RangePreset::Week => 7,
            RangePreset::Month => 30,
            RangePreset::Quarter => 90,
            RangePreset::Year => 365,
        }
    }

    /// The bucket size paired with this window (coarser windows, coarser buckets).
    fn interval(self) -> &'static str {
        match self {
            RangePreset::Day => "hour",
            RangePreset::Week => "6h",
            RangePreset::Month | RangePreset::Quarter => "day",
            RangePreset::Year => "week",
        }
    }
}

/// The queried time window: a relative preset or a frozen absolute range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimeRange {
    Preset(RangePreset),
    Custom { from: i64, to: i64 },
}

impl Default for TimeRange {
    fn default() -> Self {
        TimeRange::Preset(RangePreset::Week)
    }
}

impl TimeRange {
    /// Resolve to `(from, to, interval)`, anchoring presets to `now`.
    pub fn resolve(self, now_ms: i64) -> (i64, i64, &'static str) {
        match self {
            TimeRange::Preset(preset) => {
                (now_ms - preset.days() * DAY_MS, now_ms, preset.interval())
            }
            TimeRange::Custom { from, to } => {
                let span = (to - from).max(1);
                let interval = if span <= 2 * DAY_MS {
                    "hour"
                } else if span <= 14 * DAY_MS {
                    "6h"
                } else if span <= 120 * DAY_MS {
                    "day"
                } else {
                    "week"
                };
                (from, to, interval)
            }
        }
    }
}

/// The structured view of the `q` expression: chip-able equality terms plus an
/// opaque advanced remainder, recombined with `&&`.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Query {
    /// Top-level `field == "value"` conjuncts, in chip order. An empty value
    /// means the "absent" sentinel. One value per dimension (click-to-filter
    /// replaces).
    pub terms: Vec<(Dim, String)>,
    /// The rest of the expression (disjunctions, `contains`, `like`, …), or a
    /// raw string that failed to parse (the server then reports the error).
    pub advanced: Option<String>,
}

impl Query {
    /// Decompose a filter expression into terms + advanced remainder. An
    /// unparseable expression is preserved whole as `advanced` so the query
    /// bar still shows it (and the server's 400 explains what's wrong).
    pub fn parse(expression: &str) -> Query {
        let expression = expression.trim();
        if expression.is_empty() {
            return Query::default();
        }
        let Ok(filter) = Filter::new(expression) else {
            return Query { terms: Vec::new(), advanced: Some(expression.to_string()) };
        };
        let pieces = filter.visit(&mut Decomposer);
        let mut query = Query::default();
        let mut advanced: Vec<String> = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Term(dim, value) => {
                    // Last occurrence of a duplicated dimension wins.
                    query.terms.retain(|(d, _)| *d != dim);
                    query.terms.push((dim, value));
                }
                Piece::Advanced(text) => advanced.push(text),
            }
        }
        if !advanced.is_empty() {
            query.advanced = Some(advanced.join(" && "));
        }
        query
    }

    /// The full filter expression (the inverse of [`Query::parse`]).
    pub fn to_expression(&self) -> String {
        let mut parts: Vec<String> = self
            .terms
            .iter()
            .map(|(dim, value)| format!("{} == {}", dim.param(), quote(value)))
            .collect();
        if let Some(advanced) = &self.advanced
            && !advanced.trim().is_empty()
        {
            if parts.is_empty() {
                parts.push(advanced.clone());
            } else {
                parts.push(format!("({advanced})"));
            }
        }
        parts.join(" && ")
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.advanced.is_none()
    }

    /// The field names the advanced remainder references (empty when there is
    /// no advanced part or it doesn't parse).
    pub fn advanced_fields(&self) -> Vec<String> {
        let Some(advanced) = &self.advanced else {
            return Vec::new();
        };
        let Ok(filter) = Filter::new(advanced.as_str()) else {
            return Vec::new();
        };
        filter.visit(&mut FieldCollector::default())
    }
}

/// The complete filter state of a view.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FilterSet {
    pub range: TimeRange,
    pub query: Query,
}

impl FilterSet {
    /// Parse a location query string (with or without the leading `?`).
    pub fn parse(query_str: &str) -> FilterSet {
        let mut set = FilterSet::default();
        let (mut from, mut to) = (None, None);
        for (key, value) in pairs_of(query_str) {
            match key.as_str() {
                "range" => {
                    if let Some(preset) = RangePreset::from_token(&value) {
                        set.range = TimeRange::Preset(preset);
                    }
                }
                "from" => from = value.parse::<i64>().ok(),
                "to" => to = value.parse::<i64>().ok(),
                "q" => set.query = Query::parse(&value),
                _ => {}
            }
        }
        if let (Some(from), Some(to)) = (from, to)
            && from < to
        {
            set.range = TimeRange::Custom { from, to };
        }
        set
    }

    /// The URL query pairs for this state (the inverse of [`FilterSet::parse`]).
    /// The default range serializes to nothing, keeping the home URL clean.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        match self.range {
            TimeRange::Preset(RangePreset::Week) => {}
            TimeRange::Preset(preset) => pairs.push(("range".into(), preset.token().into())),
            TimeRange::Custom { from, to } => {
                pairs.push(("from".into(), from.to_string()));
                pairs.push(("to".into(), to.to_string()));
            }
        }
        let expression = self.query.to_expression();
        if !expression.is_empty() {
            pairs.push(("q".into(), expression));
        }
        pairs
    }

    /// A canonical string form, used as an effect dependency key.
    pub fn canonical(&self) -> String {
        self.to_pairs()
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    }

    pub fn get(&self, dim: Dim) -> Option<&str> {
        self.query.terms.iter().find(|(d, _)| *d == dim).map(|(_, v)| v.as_str())
    }

    /// Set (or replace — one value per dimension) a filter term.
    pub fn with(&self, dim: Dim, value: String) -> FilterSet {
        let mut next = self.clone();
        next.query.terms.retain(|(d, _)| *d != dim);
        next.query.terms.push((dim, value));
        next
    }

    pub fn without(&self, dim: Dim) -> FilterSet {
        let mut next = self.clone();
        next.query.terms.retain(|(d, _)| *d != dim);
        next
    }

    pub fn without_advanced(&self) -> FilterSet {
        let mut next = self.clone();
        next.query.advanced = None;
        next
    }

    /// Replace the whole query (the header query bar's apply).
    pub fn with_query(&self, query: Query) -> FilterSet {
        let mut next = self.clone();
        next.query = query;
        next
    }

    pub fn with_range(&self, range: TimeRange) -> FilterSet {
        let mut next = self.clone();
        next.range = range;
        next
    }

    /// The `GET /api/v1/stats` query string, with the range resolved against `now`.
    /// Terms the dashboard can't honour (app/app_version) are stripped — their
    /// chips render inert on that page.
    pub fn stats_query(&self, now_ms: i64) -> String {
        let (from, to, interval) = self.range.resolve(now_ms);
        let mut pairs = vec![
            ("from".to_string(), from.to_string()),
            ("to".to_string(), to.to_string()),
            ("interval".to_string(), interval.to_string()),
        ];
        let query = Query {
            terms: self
                .query
                .terms
                .iter()
                .filter(|(d, _)| d.applies_to_dashboard())
                .cloned()
                .collect(),
            advanced: self
                .query
                .advanced
                .clone()
                .filter(|_| fields_apply(&self.query.advanced_fields(), DASHBOARD_FIELDS)),
        };
        let expression = query.to_expression();
        if !expression.is_empty() {
            pairs.push(("q".to_string(), expression));
        }
        encode_pairs(&pairs)
    }

    /// Just the resolved `from=…&to=…` pair (for the exception detail fetch,
    /// which must cover the same window as the inbox that linked to it).
    pub fn range_query(&self, now_ms: i64) -> String {
        let (from, to, _) = self.range.resolve(now_ms);
        format!("from={from}&to={to}")
    }

    /// A short human label for the queried window ("Last 7 days", "Jun 30 – Jul 2").
    pub fn range_label(&self) -> String {
        match self.range {
            TimeRange::Preset(preset) => preset.label().to_string(),
            TimeRange::Custom { from, to } => {
                format!("{} – {}", crate::format::short_date(from), crate::format::short_date(to))
            }
        }
    }

    /// The `GET /api/v1/exceptions` query string: the resolved range plus only
    /// the parts of the query exception events can honour.
    pub fn exceptions_query(&self, now_ms: i64) -> String {
        let (from, to, _) = self.range.resolve(now_ms);
        let mut pairs = vec![
            ("from".to_string(), from.to_string()),
            ("to".to_string(), to.to_string()),
        ];
        let query = Query {
            terms: self
                .query
                .terms
                .iter()
                .filter(|(d, _)| d.applies_to_exceptions())
                .cloned()
                .collect(),
            advanced: self
                .query
                .advanced
                .clone()
                .filter(|_| fields_apply(&self.query.advanced_fields(), EXCEPTION_FIELDS)),
        };
        let expression = query.to_expression();
        if !expression.is_empty() {
            pairs.push(("q".to_string(), expression));
        }
        encode_pairs(&pairs)
    }

    /// Whether the advanced expression applies on the exceptions page (an
    /// inapplicable one renders as an inert chip there).
    pub fn advanced_applies_to_exceptions(&self) -> bool {
        fields_apply(&self.query.advanced_fields(), EXCEPTION_FIELDS)
    }

    /// Whether the advanced expression applies on the dashboard.
    pub fn advanced_applies_to_dashboard(&self) -> bool {
        fields_apply(&self.query.advanced_fields(), DASHBOARD_FIELDS)
    }
}

/// True when every referenced field is known to the target endpoint (an
/// unparseable advanced expression reports no fields and is sent as-is so the
/// server can explain the syntax error).
fn fields_apply(referenced: &[String], known: &[&str]) -> bool {
    referenced.iter().all(|f| known.contains(&f.as_str()))
}

/// Escape and quote a string literal for a filter expression.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// --------------------------------------------------------------- decomposition

/// One top-level conjunct of a parsed expression.
enum Piece {
    Term(Dim, String),
    Advanced(String),
}

/// Splits an expression's top-level `&&` chain into chip-able terms and
/// printed advanced remainders.
struct Decomposer;

impl<'a> ExprVisitor<'a, Vec<Piece>> for Decomposer {
    fn visit_logical(
        &mut self,
        left: &'a Expr<'a>,
        operator: LogicalOperator,
        right: &'a Expr<'a>,
    ) -> Vec<Piece> {
        match operator {
            LogicalOperator::And => {
                let mut pieces = self.visit_expr(left);
                pieces.extend(self.visit_expr(right));
                pieces
            }
            LogicalOperator::Or => {
                let text = format!("{} || {}", print(left), print(right));
                vec![Piece::Advanced(text)]
            }
        }
    }

    fn visit_binary(
        &mut self,
        left: &'a Expr<'a>,
        operator: BinaryOperator,
        right: &'a Expr<'a>,
    ) -> Vec<Piece> {
        if operator == BinaryOperator::Equals
            && let (Expr::Property(name), Expr::Literal(FilterValue::String(value))) =
                (left, right)
            && let Some(dim) = Dim::from_param(&name.to_ascii_lowercase())
        {
            return vec![Piece::Term(dim, value.to_string())];
        }
        vec![Piece::Advanced(format!(
            "{} {} {}",
            print_operand(left),
            operator.symbol(),
            print_operand(right)
        ))]
    }

    fn visit_literal(&mut self, value: &'a FilterValue<'a>) -> Vec<Piece> {
        vec![Piece::Advanced(value.to_string())]
    }

    fn visit_property(&mut self, name: &'a str) -> Vec<Piece> {
        vec![Piece::Advanced(name.to_string())]
    }

    fn visit_function_call(&mut self, name: &'a str, args: &'a [Expr<'a>]) -> Vec<Piece> {
        vec![Piece::Advanced(print_call(name, args))]
    }

    fn visit_unary(&mut self, operator: UnaryOperator, right: &'a Expr<'a>) -> Vec<Piece> {
        vec![Piece::Advanced(format!("{}{}", operator.symbol(), print_operand(right)))]
    }

    fn visit_like(&mut self, left: &'a Expr<'a>, glob: &'a Glob) -> Vec<Piece> {
        vec![Piece::Advanced(print_like(left, glob))]
    }

    fn visit_matches(&mut self, left: &'a Expr<'a>, regex: &'a CompiledRegex) -> Vec<Piece> {
        vec![Piece::Advanced(print_matches(left, regex))]
    }
}

// -------------------------------------------------------------------- printing

/// Prints an expression back into filter syntax (the crate's own `Display` is
/// an s-expression debug form, so we render infix ourselves).
struct Printer;

impl<'a> ExprVisitor<'a, String> for Printer {
    fn visit_literal(&mut self, value: &'a FilterValue<'a>) -> String {
        value.to_string()
    }

    fn visit_property(&mut self, name: &'a str) -> String {
        name.to_string()
    }

    fn visit_function_call(&mut self, name: &'a str, args: &'a [Expr<'a>]) -> String {
        print_call(name, args)
    }

    fn visit_binary(
        &mut self,
        left: &'a Expr<'a>,
        operator: BinaryOperator,
        right: &'a Expr<'a>,
    ) -> String {
        format!("{} {} {}", print_operand(left), operator.symbol(), print_operand(right))
    }

    fn visit_logical(
        &mut self,
        left: &'a Expr<'a>,
        operator: LogicalOperator,
        right: &'a Expr<'a>,
    ) -> String {
        format!("{} {} {}", print_operand(left), operator.symbol(), print_operand(right))
    }

    fn visit_unary(&mut self, operator: UnaryOperator, right: &'a Expr<'a>) -> String {
        format!("{}{}", operator.symbol(), print_operand(right))
    }

    fn visit_like(&mut self, left: &'a Expr<'a>, glob: &'a Glob) -> String {
        print_like(left, glob)
    }

    fn visit_matches(&mut self, left: &'a Expr<'a>, regex: &'a CompiledRegex) -> String {
        print_matches(left, regex)
    }
}

fn print(expr: &Expr<'_>) -> String {
    Printer.visit_expr(expr)
}

/// Print a child expression, parenthesised when composite so the recombined
/// text can never re-parse with different precedence.
fn print_operand(expr: &Expr<'_>) -> String {
    let text = print(expr);
    match expr {
        Expr::Literal(_) | Expr::Property(_) | Expr::FunctionCall(..) => text,
        _ => format!("({text})"),
    }
}

fn print_call(name: &str, args: &[Expr<'_>]) -> String {
    let args: Vec<String> = args.iter().map(print).collect();
    format!("{name}({})", args.join(", "))
}

fn print_like(left: &Expr<'_>, glob: &Glob) -> String {
    let keyword = if glob.is_case_sensitive() { "like_cs" } else { "like" };
    format!("{} {keyword} {}", print_operand(left), quote(glob.pattern()))
}

fn print_matches(left: &Expr<'_>, regex: &CompiledRegex) -> String {
    format!("{} matches {}", print_operand(left), quote(regex.pattern()))
}

/// Collects the property names an expression references.
#[derive(Default)]
struct FieldCollector;

impl<'a> ExprVisitor<'a, Vec<String>> for FieldCollector {
    fn visit_literal(&mut self, _value: &'a FilterValue<'a>) -> Vec<String> {
        Vec::new()
    }

    fn visit_property(&mut self, name: &'a str) -> Vec<String> {
        vec![name.to_ascii_lowercase()]
    }

    fn visit_function_call(&mut self, _name: &'a str, args: &'a [Expr<'a>]) -> Vec<String> {
        args.iter().flat_map(|a| self.visit_expr(a)).collect()
    }

    fn visit_binary(
        &mut self,
        left: &'a Expr<'a>,
        _operator: BinaryOperator,
        right: &'a Expr<'a>,
    ) -> Vec<String> {
        let mut fields = self.visit_expr(left);
        fields.extend(self.visit_expr(right));
        fields
    }

    fn visit_logical(
        &mut self,
        left: &'a Expr<'a>,
        _operator: LogicalOperator,
        right: &'a Expr<'a>,
    ) -> Vec<String> {
        let mut fields = self.visit_expr(left);
        fields.extend(self.visit_expr(right));
        fields
    }

    fn visit_unary(&mut self, _operator: UnaryOperator, right: &'a Expr<'a>) -> Vec<String> {
        self.visit_expr(right)
    }

    fn visit_like(&mut self, left: &'a Expr<'a>, _glob: &'a Glob) -> Vec<String> {
        self.visit_expr(left)
    }

    fn visit_matches(&mut self, left: &'a Expr<'a>, _regex: &'a CompiledRegex) -> Vec<String> {
        self.visit_expr(left)
    }
}

/// Validate a query-bar expression, returning the error message for display.
pub fn validate_expression(expression: &str) -> Option<String> {
    let expression = expression.trim();
    if expression.is_empty() {
        return None;
    }
    Filter::new(expression).err().map(|e| e.to_string())
}

// ------------------------------------------------------------------ URL codec

/// Decoded `(key, value)` pairs of a query string (leading `?` tolerated).
fn pairs_of(query_str: &str) -> Vec<(String, String)> {
    query_str
        .trim_start_matches('?')
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(key), decode(value))
        })
        .collect()
}

fn decode(value: &str) -> String {
    js_sys::decode_uri_component(&value.replace('+', " "))
        .map(String::from)
        .unwrap_or_else(|_| value.to_string())
}

fn encode_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", String::from(js_sys::encode_uri_component(v))))
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------- hooks

/// The current location's [`FilterSet`]. Re-parses whenever navigation occurs.
#[hook]
pub fn use_filters() -> FilterSet {
    let location = use_location();
    let query = location.as_ref().map(|l| l.query_str().to_string()).unwrap_or_default();
    FilterSet::parse(&query)
}

/// A callback that applies a new [`FilterSet`] by pushing a URL, staying on the
/// current page (detail pages fall back to the dashboard). This is the *only*
/// sanctioned way to mutate filter state.
#[hook]
pub fn use_apply_filters() -> Callback<FilterSet> {
    let navigator = use_navigator();
    let route = use_route::<Route>();
    Callback::from(move |filters: FilterSet| {
        let Some(navigator) = &navigator else { return };
        let target = match &route {
            Some(Route::Exceptions) => Route::Exceptions,
            _ => Route::Overview,
        };
        let _ = navigator.push_with_query(&target, &filters.to_pairs());
    })
}

/// Navigate to `route`, carrying the given filter state along.
#[hook]
pub fn use_navigate_with_filters() -> Callback<(Route, FilterSet)> {
    let navigator = use_navigator();
    Callback::from(move |(route, filters): (Route, FilterSet)| {
        let Some(navigator) = &navigator else { return };
        let _ = navigator.push_with_query(&route, &filters.to_pairs());
    })
}
