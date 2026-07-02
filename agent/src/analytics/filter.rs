//! Compiles [`filt-rs`] filter expressions into polars predicates.
//!
//! The dashboard's `q` parameter is a filter expression over event dimensions
//! (`browser == "Chrome" && (country == "DE" || country == "AT")`). This module
//! walks the parsed AST with an [`ExprVisitor`] and folds it into a polars
//! [`Expr`], mirroring the filter language's semantics: string comparisons are
//! case-insensitive (the `_cs` operator variants are exact), `like` is a glob,
//! `matches` a regular expression, and comparing a dimension to the empty
//! string matches events where it is absent.
//!
//! [`filt-rs`]: https://github.com/SierraSoftworks/filters

use std::collections::HashSet;

use filt_rs::{
    BinaryOperator, CompiledRegex, Expr as FilterNode, ExprVisitor, Filter, FilterValue, Function,
    Glob, LogicalOperator, UnaryOperator,
};
use polars::prelude::*;

use crate::store::Store;

/// Which endpoint's field vocabulary a query compiles against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldSet {
    /// Page-event dimensions (`GET /api/v1/stats`).
    Dashboard,
    /// Exception-event dimensions (`GET /api/v1/exceptions`).
    Exceptions,
}

impl FieldSet {
    /// `property name → events column`, or `None` for unknown properties.
    fn column(self, property: &str) -> Option<Field> {
        let string = |column: &'static str| {
            Some(Field {
                column,
                boolean: false,
            })
        };
        match (self, property) {
            (_, "source") => string("source"),
            (_, "browser") => string("ua_browser"),
            (_, "os") => string("ua_os"),
            (_, "device") => string("ua_device"),
            (FieldSet::Dashboard, "path") => string("pathname"),
            (FieldSet::Dashboard, "referrer") => string("referrer_host"),
            (FieldSet::Dashboard, "country") => string("country"),
            (FieldSet::Dashboard, "language") => string("language"),
            (FieldSet::Dashboard, "utm_source") => string("utm_source"),
            (FieldSet::Dashboard, "utm_medium") => string("utm_medium"),
            (FieldSet::Dashboard, "utm_campaign") => string("utm_campaign"),
            // The application *is* the source (exceptions attribute to the
            // reporting hostname), so `app` is an alias for `source`.
            (FieldSet::Exceptions, "app") => string("source"),
            (FieldSet::Exceptions, "app_version") => string("app_version"),
            (FieldSet::Exceptions, "type") => string("exc_type"),
            (FieldSet::Exceptions, "message") => string("exc_message"),
            (FieldSet::Exceptions, "handled") => Some(Field {
                column: "exc_handled",
                boolean: true,
            }),
            _ => None,
        }
    }

    /// The property names this field set accepts (for error messages).
    fn known(self) -> &'static str {
        match self {
            FieldSet::Dashboard => {
                "project, source, path, referrer, country, language, browser, os, device, \
                 utm_source, utm_medium, utm_campaign"
            }
            FieldSet::Exceptions => {
                "project, source, browser, os, device, app, app_version, type, message, handled"
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Field {
    column: &'static str,
    boolean: bool,
}

/// A query compiled to a polars predicate, plus the properties it referenced
/// (the caller switches visitor-count semantics when `path` is filtered).
pub struct CompiledFilter {
    pub predicate: Expr,
    referenced: HashSet<String>,
}

impl CompiledFilter {
    pub fn references(&self, property: &str) -> bool {
        self.referenced.contains(property)
    }
}

/// Parse and compile a `q` expression. `Err` carries a human-readable message
/// suitable for a 400 response (and for display under the UI's query bar).
pub fn compile_query(
    q: &str,
    fields: FieldSet,
    store: &Store,
) -> std::result::Result<Option<CompiledFilter>, String> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(None);
    }
    let filter = Filter::new(q).map_err(|err| err.to_string())?;
    let mut compiler = Compiler {
        fields,
        store,
        referenced: HashSet::new(),
    };
    let node = filter.visit(&mut compiler);
    let predicate = node?.into_predicate()?;
    Ok(Some(CompiledFilter {
        predicate,
        referenced: compiler.referenced,
    }))
}

/// An intermediate value produced while folding the AST: either a finished
/// boolean predicate or an operand (column reference / literal) awaiting its
/// enclosing comparison.
enum Node {
    Predicate(Expr),
    Column(Field),
    /// The `project` pseudo-field: comparisons resolve to source membership.
    Project,
    Str(String),
    Bool(bool),
    List(Vec<String>),
    Null,
}

impl Node {
    fn into_predicate(self) -> std::result::Result<Expr, String> {
        match self {
            Node::Predicate(expr) => Ok(expr),
            // A bare string field is truthy when present and non-empty.
            Node::Column(field) if !field.boolean => Ok(col(field.column)
                .is_not_null()
                .and(col(field.column).neq(lit("")))),
            Node::Column(field) => Ok(col(field.column).fill_null(lit(false))),
            Node::Bool(value) => Ok(lit(value)),
            Node::Project => Err("`project` must be compared, e.g. project == \"<id>\"".into()),
            Node::Str(_) | Node::List(_) | Node::Null => {
                Err("this value cannot be used as a condition on its own".into())
            }
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Node::Predicate(_) => "a condition",
            Node::Column(_) => "a field",
            Node::Project => "project",
            Node::Str(_) => "a string",
            Node::Bool(_) => "a boolean",
            Node::List(_) => "a list",
            Node::Null => "null",
        }
    }
}

type Fold = std::result::Result<Node, String>;

struct Compiler<'s> {
    fields: FieldSet,
    store: &'s Store,
    referenced: HashSet<String>,
}

impl Compiler<'_> {
    /// The source URIs belonging to a project (empty for unknown ids, which
    /// then matches nothing — never everything).
    fn project_sources(&self, project_id: &str) -> Vec<String> {
        super::project_source_uris(self.store, project_id).unwrap_or_default()
    }

    fn source_membership(&self, project_ids: &[String]) -> Expr {
        let uris: Vec<String> = project_ids
            .iter()
            .flat_map(|id| self.project_sources(id))
            .collect();
        col("source").is_in(lit(Series::new("sources".into(), uris)), false)
    }

    /// `column == value` with the language's semantics: case-insensitive, and
    /// the empty string matching absent values.
    fn string_eq(&self, field: Field, value: &str, case_sensitive: bool) -> Expr {
        if field.boolean {
            return col(field.column)
                .fill_null(lit(false))
                .eq(lit(value.eq_ignore_ascii_case("true")));
        }
        if value.is_empty() {
            // The sentinel means "absent on a page view". Pixel/custom events
            // have *every* dimension null, so on the dashboard field set they
            // must not ride through an empty-valued comparison (the events
            // metric would ignore the filter entirely). Exception queries run
            // on a kind-scoped frame where "absent" genuinely means unknown.
            let absent = col(field.column)
                .is_null()
                .or(col(field.column).eq(lit("")));
            return match self.fields {
                FieldSet::Dashboard => absent.and(
                    col("kind")
                        .eq(lit("pixel"))
                        .or(col("kind").eq(lit("custom")))
                        .not(),
                ),
                FieldSet::Exceptions => absent,
            };
        }
        // Sources are stored as canonical URIs (`https://…`, `app://…`,
        // `pixel://…`) but read as bare hostnames everywhere in the UI, so a
        // scheme-less comparison matches any canonical form of that name.
        if field.column == "source" {
            return source_in(std::slice::from_ref(&value.to_string()));
        }
        if case_sensitive {
            col(field.column).eq(lit(value.to_string()))
        } else {
            col(field.column)
                .str()
                .to_lowercase()
                .eq(lit(value.to_lowercase()))
        }
    }
}

impl<'a> ExprVisitor<'a, Fold> for Compiler<'_> {
    fn visit_literal(&mut self, value: &'a FilterValue<'a>) -> Fold {
        match value {
            FilterValue::Null => Ok(Node::Null),
            FilterValue::Bool(b) => Ok(Node::Bool(*b)),
            FilterValue::String(s) => Ok(Node::Str(s.to_string())),
            FilterValue::Tuple(values) => {
                let mut items = Vec::with_capacity(values.len());
                for value in values {
                    match value {
                        FilterValue::String(s) => items.push(s.to_string()),
                        other => {
                            return Err(format!(
                                "lists may only contain strings here (found {other})"
                            ));
                        }
                    }
                }
                Ok(Node::List(items))
            }
            other => Err(format!("unsupported literal {other}")),
        }
    }

    fn visit_property(&mut self, name: &'a str) -> Fold {
        let name = name.to_ascii_lowercase();
        self.referenced.insert(name.clone());
        if name == "project" {
            return Ok(Node::Project);
        }
        match self.fields.column(&name) {
            Some(field) => Ok(Node::Column(field)),
            None => Err(format!(
                "unknown field `{name}` — expected one of: {}",
                self.fields.known()
            )),
        }
    }

    fn visit_function_call(
        &mut self,
        function: &'a dyn Function,
        _args: &'a [FilterNode<'a>],
    ) -> Fold {
        Err(format!(
            "functions are not supported in analytics queries (`{}(…)`)",
            function.name()
        ))
    }

    fn visit_binary(
        &mut self,
        left: &'a FilterNode<'a>,
        operator: BinaryOperator,
        right: &'a FilterNode<'a>,
    ) -> Fold {
        let left = self.visit_expr(left)?;
        let right = self.visit_expr(right)?;
        // Normalise `"value" == field` to `field == "value"`.
        let (subject, object) = match (&left, &right) {
            (Node::Str(_) | Node::List(_) | Node::Null | Node::Bool(_), _) => (right, left),
            _ => (left, right),
        };

        use BinaryOperator::*;
        let case_sensitive = matches!(operator, ContainsCs | InCs | StartsWithCs | EndsWithCs);

        match (&subject, operator, &object) {
            // ---- project membership --------------------------------------
            (Node::Project, Equals, Node::Str(id)) => Ok(Node::Predicate(
                self.source_membership(std::slice::from_ref(id)),
            )),
            (Node::Project, NotEquals, Node::Str(id)) => Ok(Node::Predicate(
                self.source_membership(std::slice::from_ref(id)).not(),
            )),
            (Node::Project, In | InCs, Node::List(ids)) => {
                Ok(Node::Predicate(self.source_membership(ids)))
            }

            // ---- equality --------------------------------------------------
            (Node::Column(field), Equals, Node::Str(value)) => {
                Ok(Node::Predicate(self.string_eq(*field, value, false)))
            }
            (Node::Column(field), NotEquals, Node::Str(value)) if value.is_empty() => {
                // `field != ""` means "the dimension is present".
                Ok(Node::Predicate(
                    col(field.column)
                        .is_not_null()
                        .and(col(field.column).neq(lit(""))),
                ))
            }
            (Node::Column(field), NotEquals, Node::Str(value)) => {
                // "not X" includes rows where the dimension is absent.
                let eq = self.string_eq(*field, value, false);
                Ok(Node::Predicate(eq.not().or(col(field.column).is_null())))
            }
            (Node::Column(field), Equals, Node::Null) => {
                Ok(Node::Predicate(col(field.column).is_null()))
            }
            (Node::Column(field), NotEquals, Node::Null) => {
                Ok(Node::Predicate(col(field.column).is_not_null()))
            }
            (Node::Column(field), Equals, Node::Bool(value)) if field.boolean => Ok(
                Node::Predicate(col(field.column).fill_null(lit(false)).eq(lit(*value))),
            ),
            (Node::Column(field), NotEquals, Node::Bool(value)) if field.boolean => Ok(
                Node::Predicate(col(field.column).fill_null(lit(false)).neq(lit(*value))),
            ),

            // ---- membership ------------------------------------------------
            // Source membership gets the same scheme tolerance as source
            // equality: `source in ["a.com", "b.com"]` matches each name's
            // canonical URI forms.
            (Node::Column(field), In | InCs, Node::List(values)) if field.column == "source" => {
                Ok(Node::Predicate(source_in(values)))
            }
            (Node::Column(field), In | InCs, Node::List(values)) => {
                let values: Vec<String> = if case_sensitive {
                    values.clone()
                } else {
                    values.iter().map(|v| v.to_lowercase()).collect()
                };
                let column = if case_sensitive {
                    col(field.column)
                } else {
                    col(field.column).str().to_lowercase()
                };
                Ok(Node::Predicate(
                    column.is_in(lit(Series::new("values".into(), values)), false),
                ))
            }

            // ---- substrings / affixes -------------------------------------
            (Node::Column(field), Contains | ContainsCs, Node::Str(value)) => {
                let (column, needle) = ci(*field, value, case_sensitive);
                Ok(Node::Predicate(column.str().contains_literal(lit(needle))))
            }
            (Node::Column(field), StartsWith | StartsWithCs, Node::Str(value)) => {
                let (column, needle) = ci(*field, value, case_sensitive);
                Ok(Node::Predicate(column.str().starts_with(lit(needle))))
            }
            (Node::Column(field), EndsWith | EndsWithCs, Node::Str(value)) => {
                let (column, needle) = ci(*field, value, case_sensitive);
                Ok(Node::Predicate(column.str().ends_with(lit(needle))))
            }

            // ---- everything else ------------------------------------------
            (subject, GreaterThan | GreaterEqual | SmallerThan | SmallerEqual, _) => Err(format!(
                "ordering comparisons are not supported for {}",
                subject.describe()
            )),
            (subject, Plus | Minus, _) => Err(format!(
                "arithmetic is not supported (near {})",
                subject.describe()
            )),
            (subject, operator, object) => Err(format!(
                "cannot apply `{operator}` to {} and {}",
                subject.describe(),
                object.describe()
            )),
        }
    }

    fn visit_logical(
        &mut self,
        left: &'a FilterNode<'a>,
        operator: LogicalOperator,
        right: &'a FilterNode<'a>,
    ) -> Fold {
        let left = self.visit_expr(left)?.into_predicate()?;
        let right = self.visit_expr(right)?.into_predicate()?;
        Ok(Node::Predicate(match operator {
            LogicalOperator::And => left.and(right),
            LogicalOperator::Or => left.or(right),
        }))
    }

    fn visit_unary(&mut self, operator: UnaryOperator, right: &'a FilterNode<'a>) -> Fold {
        let operand = self.visit_expr(right)?.into_predicate()?;
        Ok(Node::Predicate(match operator {
            UnaryOperator::Not => operand.not(),
        }))
    }

    fn visit_like(&mut self, left: &'a FilterNode<'a>, glob: &'a Glob) -> Fold {
        let subject = self.visit_expr(left)?;
        let Node::Column(field) = subject else {
            return Err(format!(
                "`like` expects a field on the left, found {}",
                subject.describe()
            ));
        };
        let pattern = glob_regex(glob.pattern(), glob.is_case_sensitive());
        Ok(Node::Predicate(
            col(field.column).str().contains(lit(pattern), true),
        ))
    }

    fn visit_matches(&mut self, left: &'a FilterNode<'a>, regex: &'a CompiledRegex) -> Fold {
        let subject = self.visit_expr(left)?;
        let Node::Column(field) = subject else {
            return Err(format!(
                "`matches` expects a field on the left, found {}",
                subject.describe()
            ));
        };
        Ok(Node::Predicate(
            col(field.column)
                .str()
                .contains(lit(regex.pattern().to_string()), true),
        ))
    }
}

/// Membership over the `source` column, tolerant of scheme-less names: each
/// value expands to every canonical URI form it could be stored as (sources
/// are stored as `https://…`/`app://…`/`pixel://…` but read as bare hostnames
/// throughout the UI). Values that already carry a scheme match as-is, always
/// case-insensitively (hostnames are lowercased at ingest).
fn source_in(values: &[String]) -> Expr {
    let forms: Vec<String> = values
        .iter()
        .flat_map(|value| {
            let value = value.to_lowercase();
            if value.contains("://") {
                vec![value]
            } else {
                ["https://", "app://", "pixel://", ""]
                    .iter()
                    .map(|scheme| format!("{scheme}{value}"))
                    .collect()
            }
        })
        .collect();
    col("source")
        .str()
        .to_lowercase()
        .is_in(lit(Series::new("sources".into(), forms)), false)
}

/// Case-fold a column/needle pair for the case-insensitive operator variants.
fn ci(field: Field, value: &str, case_sensitive: bool) -> (Expr, String) {
    if case_sensitive {
        (col(field.column), value.to_string())
    } else {
        (col(field.column).str().to_lowercase(), value.to_lowercase())
    }
}

/// Translate a filter-language glob (`*`/`?` wildcards) into an anchored regex.
fn glob_regex(pattern: &str, case_sensitive: bool) -> String {
    let mut regex = String::with_capacity(pattern.len() + 8);
    if !case_sensitive {
        regex.push_str("(?i)");
    }
    regex.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c if regex_syntax_meta(c) => {
                regex.push('\\');
                regex.push(c);
            }
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex
}

fn regex_syntax_meta(c: char) -> bool {
    matches!(
        c,
        '\\' | '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (Store, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "analytics-filter-{}-{}.redb",
            std::process::id(),
            n
        ));
        (Store::open(&path).unwrap(), path)
    }

    fn compiles(q: &str) -> bool {
        let (store, path) = temp_store();
        let result = compile_query(q, FieldSet::Dashboard, &store);
        drop(store);
        let _ = std::fs::remove_file(&path);
        result.is_ok()
    }

    #[test]
    fn accepts_the_supported_grammar() {
        for q in [
            r#"browser == "Chrome""#,
            r#"browser != "Chrome" && (country == "DE" || country == "AT")"#,
            r#"country in ["DE", "AT", "CH"]"#,
            r#"path startswith "/docs""#,
            r#"referrer contains "google""#,
            r#"path like "/docs/*""#,
            r#"!(browser == "Safari")"#,
            r#"referrer == """#,
            r#"project == "some-project-id""#,
            "browser", // truthy: browser is present
        ] {
            assert!(compiles(q), "expected `{q}` to compile");
        }
    }

    #[test]
    fn rejects_unknown_fields_and_unsupported_operations() {
        for q in [
            r#"flavour == "vanilla""#,
            r#"browser > 4"#,
            r#"now() == browser"#,
            r#"browser == "a" + "b""#,
        ] {
            assert!(!compiles(q), "expected `{q}` to be rejected");
        }
    }

    #[test]
    fn records_referenced_properties() {
        let (store, path) = temp_store();
        let compiled = compile_query(
            r#"path startswith "/docs" && browser == "Chrome""#,
            FieldSet::Dashboard,
            &store,
        )
        .unwrap()
        .unwrap();
        assert!(compiled.references("path"));
        assert!(compiled.references("browser"));
        assert!(!compiled.references("country"));
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn exception_fields_differ_from_dashboard_fields() {
        let (store, path) = temp_store();
        assert!(compile_query(r#"app_version == "1.2.3""#, FieldSet::Exceptions, &store).is_ok());
        assert!(compile_query(r#"app_version == "1.2.3""#, FieldSet::Dashboard, &store).is_err());
        assert!(compile_query(r#"path == "/x""#, FieldSet::Exceptions, &store).is_err());
        assert!(compile_query("handled == false", FieldSet::Exceptions, &store).is_ok());
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn source_equality_and_membership_tolerate_missing_schemes() {
        // Covered end-to-end in analytics::tests; here just assert the forms compile.
        assert!(compiles(r#"source == "docs.example.com""#));
        assert!(compiles(r#"source == "https://docs.example.com""#));
        assert!(compiles(
            r#"source in ["docs.example.com", "https://shop.example.com"]"#
        ));
        assert!(compiles(r#"source in_cs ["docs.example.com"]"#));
    }

    #[test]
    fn glob_translation_escapes_regex_metacharacters() {
        assert_eq!(glob_regex("/docs/*", false), "(?i)^/docs/.*$");
        assert_eq!(glob_regex("v1.2?", true), "^v1\\.2.$");
    }
}
