//! Reporter-supplied metadata, classified for display: short entries render as
//! inline **tag** pills, longer ones as a **context** definition list (shown
//! above stack traces). Shared by the trace timeline and the exception detail
//! exemplars so metadata reads the same everywhere.

use std::collections::BTreeMap;

use yew::prelude::*;

/// A key/value entry is a tag when both halves are short enough to scan inline;
/// anything longer (ids, paths, component stacks) reads better as context.
const TAG_MAX_KEY: usize = 24;
const TAG_MAX_VALUE: usize = 32;

/// Metadata split into inline tags and longer context entries. `raw` carries
/// the original text when it wasn't a JSON object (rendered verbatim).
#[derive(Default)]
pub struct Metadata {
    pub tags: Vec<(String, String)>,
    pub context: Vec<(String, String)>,
    pub raw: Option<String>,
}

impl Metadata {
    /// Parse and classify a raw metadata JSON object string.
    pub fn parse(metadata: Option<&str>) -> Metadata {
        let Some(raw) = metadata.filter(|m| !m.trim().is_empty()) else {
            return Metadata::default();
        };
        let Ok(fields) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(raw) else {
            return Metadata {
                raw: Some(raw.to_string()),
                ..Default::default()
            };
        };

        let mut split = Metadata::default();
        for (key, value) in fields {
            let value = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            if key.chars().count() <= TAG_MAX_KEY && value.chars().count() <= TAG_MAX_VALUE {
                split.tags.push((key, value));
            } else {
                split.context.push((key, value));
            }
        }
        split
    }

    /// The tag entries as inline `key: value` pills.
    pub fn tag_pills(&self) -> Html {
        let pills = self.tags.iter().map(|(key, value)| {
            let text = format!("{key}: {value}");
            html! {
                <code class="meta-tag" key={key.clone()} title={text.clone()}>{ text }</code>
            }
        });
        html! { <>{ for pills }</> }
    }

    /// The context entries as a definition list (plus the raw fallback), or
    /// nothing when there is no long-form metadata.
    pub fn context_list(&self) -> Html {
        if self.context.is_empty() && self.raw.is_none() {
            return html! {};
        }
        if let Some(raw) = &self.raw {
            return html! {
                <div class="meta-context">
                    <span class="meta-context__title">{ "Context" }</span>
                    <pre class="stack">{ raw.clone() }</pre>
                </div>
            };
        }
        let rows = self.context.iter().map(|(key, value)| {
            html! {
                <div class="meta-context__row" key={key.clone()}>
                    <dt>{ key.clone() }</dt>
                    <dd title={value.clone()}>{ value.clone() }</dd>
                </div>
            }
        });
        html! {
            <div class="meta-context">
                <span class="meta-context__title">{ "Context" }</span>
                <dl class="meta-context__list">{ for rows }</dl>
            </div>
        }
    }
}
