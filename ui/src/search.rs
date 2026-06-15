//! The unified search filter shared by the app bar and the routed pages.
//!
//! A query is a space-separated list of terms. A term may be scoped to a property
//! with a `field:value` prefix (for example `project:blog` or `status:resolved`);
//! an unscoped term matches against every searchable property of a row. All terms
//! must match (logical AND); matching is case-insensitive and substring-based.

use std::rc::Rc;

use yew::prelude::*;

/// The canonical `field:` prefixes offered as autocomplete suggestions, paired
/// with a short human description. The order here is the order presented.
pub const FIELD_PREFIXES: &[(&str, &str)] = &[
    ("project:", "Match a project by name"),
    ("page:", "Match a page path"),
    ("source:", "Match a reporting source"),
    ("status:", "Match an exception status"),
];

/// A property a search term can be scoped to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Project,
    Page,
    Source,
    Status,
}

impl Field {
    fn parse(prefix: &str) -> Option<Field> {
        match prefix.to_ascii_lowercase().as_str() {
            "project" | "proj" | "p" => Some(Field::Project),
            "page" | "path" => Some(Field::Page),
            "source" | "src" | "host" => Some(Field::Source),
            "status" | "state" => Some(Field::Status),
            _ => None,
        }
    }
}

/// A single parsed search term.
#[derive(Clone, PartialEq)]
struct Term {
    /// The property the term is scoped to, or `None` for free text.
    field: Option<Field>,
    /// The lowercased needle to look for.
    needle: String,
}

/// A parsed query: a conjunction of [`Term`]s.
#[derive(Clone, PartialEq, Default)]
pub struct SearchFilter {
    terms: Vec<Term>,
}

/// The searchable properties of a single row, supplied when evaluating a filter.
/// A field a page doesn't have is left empty; `text` is a pre-lowercased blob of
/// everything searchable, used for free-text terms.
#[derive(Default)]
pub struct MatchContext<'a> {
    pub project: &'a str,
    pub page: &'a str,
    pub source: &'a str,
    pub status: &'a str,
    pub text: &'a str,
}

impl SearchFilter {
    /// Parse a raw query string. A `field:value` token is only scoped when `field`
    /// is a known property; otherwise the whole token is a free-text term.
    pub fn parse(query: &str) -> Self {
        let terms = query
            .split_whitespace()
            .filter_map(|token| {
                if let Some((prefix, value)) = token.split_once(':')
                    && let Some(field) = Field::parse(prefix)
                {
                    if value.is_empty() {
                        return None;
                    }
                    return Some(Term {
                        field: Some(field),
                        needle: value.to_lowercase(),
                    });
                }
                Some(Term {
                    field: None,
                    needle: token.to_lowercase(),
                })
            })
            .collect();
        Self { terms }
    }

    /// Evaluate the filter against a single row.
    pub fn matches(&self, ctx: &MatchContext) -> bool {
        self.terms.iter().all(|term| {
            let haystack = match term.field {
                Some(Field::Project) => ctx.project,
                Some(Field::Page) => ctx.page,
                Some(Field::Source) => ctx.source,
                Some(Field::Status) => ctx.status,
                None => ctx.text,
            };
            haystack.to_lowercase().contains(&term.needle)
        })
    }
}

/// Shared search state: the app bar owns the input, pages consume the parsed filter.
#[derive(Clone, PartialEq)]
pub struct SearchContext {
    pub query: AttrValue,
    pub filter: Rc<SearchFilter>,
    pub set: Callback<String>,
}

/// Concrete values offered for context-aware completion of scoped terms. Project
/// names come from the shell's project list; the page-specific lists (pages,
/// sources, statuses) are published by the routed page that owns that data.
#[derive(Clone, PartialEq, Default)]
pub struct SearchVocabulary {
    pub pages: Vec<AttrValue>,
    pub sources: Vec<AttrValue>,
    pub statuses: Vec<AttrValue>,
}

impl SearchVocabulary {
    /// Candidate values for a page-published field, or `None` for `project:`
    /// (which the app bar completes from the shell's project list instead).
    pub fn values_for(&self, field: &str) -> Option<&[AttrValue]> {
        match Field::parse(field)? {
            Field::Page => Some(&self.pages),
            Field::Source => Some(&self.sources),
            Field::Status => Some(&self.statuses),
            Field::Project => None,
        }
    }
}

/// The shared completion vocabulary, provided above both the app bar and the page.
#[derive(Clone, PartialEq)]
pub struct VocabularyContext {
    pub vocabulary: Rc<SearchVocabulary>,
    pub set: Callback<SearchVocabulary>,
}
