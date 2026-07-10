//! Noise-normalization pipeline used by exception fingerprinting.
//!
//! Exception grouping keys off a *normalized* form of the error type, the top
//! stack frames, and — when there is no stack — the message summary. Rather than
//! one monolithic cleanup function, normalization is expressed as a small pipeline
//! of independently-testable [`Processor`]s, each removing one class of
//! high-cardinality noise (URLs, hex addresses, line/column numbers, …) so that
//! semantically-equal failures collapse into a single group.
//!
//! Adding a new rule is a matter of writing one `Processor` and slotting it into
//! [`FRAME_PIPELINE`] and/or [`MESSAGE_PIPELINE`]; each processor can be unit
//! tested in isolation without reasoning about the rest of the chain.

use std::sync::LazyLock;

/// A single, order-dependent text-normalization step. Implementations are kept
/// tiny so each rule can be exercised on its own.
trait Processor: Send + Sync {
    fn process(&self, input: &str) -> String;
}

/// An ordered chain of [`Processor`]s. `run` threads the text through each stage
/// in turn, feeding one stage's output into the next.
struct Pipeline {
    stages: Vec<Box<dyn Processor>>,
}

impl Pipeline {
    fn new(stages: Vec<Box<dyn Processor>>) -> Self {
        Self { stages }
    }

    fn run(&self, input: &str) -> String {
        self.stages
            .iter()
            .fold(input.to_string(), |text, stage| stage.process(&text))
    }
}

/// Frame lines carry query strings, so they get the extra [`DropQuery`] stage
/// ahead of the shared noise-stripping steps. Structural stripping ([`StripUrls`],
/// [`NormalizePaths`]) runs first so directory/host noise is gone before the token
/// normalizers ([`NormalizeUuids`], [`NormalizeHexAddresses`], [`NormalizeHashes`])
/// look at what remains; those run after [`Lowercase`] (so hex matching only needs
/// lower-case `a-f`) but before [`StripDigits`] (which would otherwise shred the
/// very tokens they need to recognise).
static FRAME_PIPELINE: LazyLock<Pipeline> = LazyLock::new(|| {
    Pipeline::new(vec![
        Box::new(DropQuery),
        Box::new(StripUrls),
        Box::new(NormalizePaths),
        Box::new(Lowercase),
        Box::new(NormalizeUuids),
        Box::new(NormalizeHexAddresses),
        Box::new(NormalizeHashes),
        Box::new(StripDigits),
        Box::new(CollapseWhitespace),
    ])
});

/// The message fallback has no frame syntax, so it skips [`DropQuery`] — and
/// [`NormalizePaths`], since a bare `/foo/bar` in prose is more often a route or
/// idiom (`and/or`) than a filesystem path — but shares every other rule with the
/// frame pipeline.
static MESSAGE_PIPELINE: LazyLock<Pipeline> = LazyLock::new(|| {
    Pipeline::new(vec![
        Box::new(StripUrls),
        Box::new(Lowercase),
        Box::new(NormalizeUuids),
        Box::new(NormalizeHexAddresses),
        Box::new(NormalizeHashes),
        Box::new(StripDigits),
        Box::new(CollapseWhitespace),
    ])
});

/// Normalize the non-empty lines of a stack trace, one frame per entry, with
/// order preserved so a caller can key on the top few frames.
///
/// Consecutive identical frames are collapsed (Sentry-style "de-recursion"), so a
/// runaway recursion or stack overflow groups the same regardless of how deep it
/// got — otherwise the top-N window would be filled with a variable number of the
/// repeated frame and push the distinguishing frames out of view.
pub fn frames(stack: &str) -> Vec<String> {
    let mut frames: Vec<String> = stack
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| FRAME_PIPELINE.run(line))
        .filter(|line| !line.is_empty())
        .collect();
    frames.dedup();
    frames
}

/// Normalize a message (the stack-less fallback signal).
pub fn message(text: &str) -> String {
    MESSAGE_PIPELINE.run(text)
}

// --------------------------------------------------------------------------
// Processors
// --------------------------------------------------------------------------

/// Keep only the text before the first `?`, dropping query strings such as the
/// cache-busting suffixes on bundled asset paths (`app.js?v=2` → `app.js`).
struct DropQuery;

impl Processor for DropQuery {
    fn process(&self, input: &str) -> String {
        input.split('?').next().unwrap_or(input).to_string()
    }
}

/// Remove every `scheme://…` URL, leaving surrounding punctuation intact (so
/// `url (https://host/x)` becomes `url ()`).
///
/// This is the rule that keeps semantically-equal failures together: a Rust
/// `human_errors`/`tracing_batteries` "caused by" chain reported as the stack
/// leads with lines like `error sending request for url (https://host/…/tags/…)`,
/// and browser frames embed hashed bundle URLs (`https://cdn/app.abc123.js:10:5`).
/// Dropping the URL leaves the stable signal — the surrounding error shape or the
/// function name — so grouping keys off the error type and frames rather than the
/// volatile request target.
struct StripUrls;

impl Processor for StripUrls {
    fn process(&self, input: &str) -> String {
        strip_urls(input)
    }
}

/// Reduce filesystem paths in a frame to their basename, so the same failure
/// groups regardless of where the code was built or installed.
///
/// A frame's directory prefix is an accident of the build/run environment — the
/// developer's home directory, a CI checkout path, a container mount, a Windows
/// drive — and reporting `/home/alice/app/src/db.rs` vs `/build/app/src/db.rs`
/// for the same crash would otherwise fragment the group. The filename plus the
/// surrounding function name is the stable failure signal, so only the leading
/// path is dropped (`…/src/db.rs:88` → `db.rs:88`). Prose is left alone: a run is
/// only treated as a path when it has two or more separators or a real file
/// extension, so idioms like `and/or` and routes like `/health` survive.
struct NormalizePaths;

impl Processor for NormalizePaths {
    fn process(&self, input: &str) -> String {
        normalize_paths(input)
    }
}

/// Lower-case everything so casing differences never fragment a group.
struct Lowercase;

impl Processor for Lowercase {
    fn process(&self, input: &str) -> String {
        input.to_lowercase()
    }
}

/// Replace UUIDs (e.g. request/entity/trace ids embedded in a message or frame)
/// with a single `<uuid>` token so two occurrences that differ only by their id
/// group together.
struct NormalizeUuids;

impl Processor for NormalizeUuids {
    fn process(&self, input: &str) -> String {
        map_tokens(input, |token| is_uuid(token).then_some("<uuid>"))
    }
}

/// Replace `0x…` memory addresses / pointers with a single `<addr>` token. Digit
/// stripping alone would leave the (still-varying) hex letters behind, so
/// `0xdeadbeef` and `0xcafebabe` would otherwise stay apart.
struct NormalizeHexAddresses;

impl Processor for NormalizeHexAddresses {
    fn process(&self, input: &str) -> String {
        map_tokens(input, |token| is_hex_address(token).then_some("<addr>"))
    }
}

/// Replace long hex runs (SHAs, content hashes, hex trace ids) with a single
/// `<hash>` token. Requiring both a minimum length and at least one digit keeps
/// ordinary hex-shaped words (`deadbeef`, `cafe`) intact while still collapsing
/// genuine hashes.
struct NormalizeHashes;

impl Processor for NormalizeHashes {
    fn process(&self, input: &str) -> String {
        map_tokens(input, |token| is_hex_hash(token).then_some("<hash>"))
    }
}

/// Remove ASCII digits (line/column numbers, ports, ids, version components).
struct StripDigits;

impl Processor for StripDigits {
    fn process(&self, input: &str) -> String {
        input.chars().filter(|ch| !ch.is_ascii_digit()).collect()
    }
}

/// Collapse runs of whitespace to a single space and trim the ends, so the
/// gaps left by the stripping stages don't themselves fragment a group.
struct CollapseWhitespace;

impl Processor for CollapseWhitespace {
    fn process(&self, input: &str) -> String {
        input.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Remove every `scheme://…` URL from `input`. A URL runs from its scheme up to
/// the first whitespace or closing delimiter, which keeps the enclosing message
/// structure intact while discarding the volatile target.
fn strip_urls(input: &str) -> String {
    if !input.contains("://") {
        return input.to_string();
    }
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        match url_end(&chars, i) {
            Some(end) => i = end,
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// If a URL starts at `start` (a run of scheme letters followed by `://`), return
/// the index just past its end; otherwise `None`.
fn url_end(chars: &[char], start: usize) -> Option<usize> {
    const TERMINATORS: &[char] =
        &[' ', '\t', '\n', '\r', ')', ']', '}', '>', '"', '\'', '`', ',', ';'];

    let mut scheme_end = start;
    while scheme_end < chars.len() && chars[scheme_end].is_ascii_alphabetic() {
        scheme_end += 1;
    }
    // Need a non-empty scheme immediately followed by "://".
    if scheme_end == start || chars.len() <= scheme_end + 2 {
        return None;
    }
    if chars[scheme_end] != ':' || chars[scheme_end + 1] != '/' || chars[scheme_end + 2] != '/' {
        return None;
    }
    let mut end = scheme_end + 3;
    while end < chars.len() && !TERMINATORS.contains(&chars[end]) {
        end += 1;
    }
    Some(end)
}

/// Reduce every path-like run in `input` to its basename. A run is a maximal
/// sequence of path-segment characters and `/`/`\` separators; it is only rewritten
/// when it actually looks like a path (see [`looks_like_path`]), so ordinary text
/// is left untouched.
fn normalize_paths(input: &str) -> String {
    if !input.contains('/') && !input.contains('\\') {
        return input.to_string();
    }
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if is_path_char(chars[i]) || is_separator(chars[i]) {
            let start = i;
            while i < chars.len() && (is_path_char(chars[i]) || is_separator(chars[i])) {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if looks_like_path(&run) {
                out.push_str(basename(&run));
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Characters that can appear in a filename segment (excludes `:` so a Windows
/// drive letter or a trailing `:line:col` bounds the run).
fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~' | '@' | '+' | '$')
}

/// Whether a separator-bearing run is worth reducing to a basename: a genuine
/// directory path (two or more separators) or a single-separator run whose final
/// segment carries a file extension. This spares idioms (`and/or`), simple routes
/// (`/health`), and ratios while still catching `src/main.rs` and `/a/b/c.js`.
fn looks_like_path(run: &str) -> bool {
    let separators = run.chars().filter(|&c| is_separator(c)).count();
    separators >= 2 || (separators == 1 && basename(run).contains('.'))
}

/// The last non-empty `/`- or `\`-separated segment of `path`.
fn basename(path: &str) -> &str {
    path.rsplit(is_separator)
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Replace whole tokens for which `classify` returns a placeholder, leaving every
/// delimiter (spaces, parens, `::`, `.`, `/`, …) untouched so the surrounding
/// structure survives. A token is a maximal run of `[A-Za-z0-9_-]`, which keeps
/// UUIDs (dash-separated) and `0x…` addresses intact as single units.
fn map_tokens(input: &str, classify: impl Fn(&str) -> Option<&'static str>) -> String {
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    let mut out = String::with_capacity(input.len());
    let mut token_start: Option<usize> = None;
    for (i, c) in input.char_indices() {
        if is_token_char(c) {
            token_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = token_start.take() {
            emit_token(&mut out, &input[start..i], &classify);
        }
        out.push(c);
    }
    if let Some(start) = token_start {
        emit_token(&mut out, &input[start..], &classify);
    }
    out
}

/// Append `token` to `out`, substituting the placeholder if `classify` matches.
fn emit_token(out: &mut String, token: &str, classify: &impl Fn(&str) -> Option<&'static str>) {
    match classify(token) {
        Some(placeholder) => out.push_str(placeholder),
        None => out.push_str(token),
    }
}

/// A canonical `8-4-4-4-12` UUID (case-insensitive hex).
fn is_uuid(token: &str) -> bool {
    if token.len() != 36 {
        return false;
    }
    token.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_hexdigit(),
    })
}

/// A `0x`-prefixed run of hex digits (a memory address or pointer).
fn is_hex_address(token: &str) -> bool {
    token
        .strip_prefix("0x")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A long, all-hex run containing at least one digit — a SHA, content hash, or hex
/// trace id. The digit requirement spares ordinary hex-shaped words.
fn is_hex_hash(token: &str) -> bool {
    token.len() >= 10
        && token.bytes().all(|b| b.is_ascii_hexdigit())
        && token.bytes().any(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- individual processors (each rule verified in isolation) ----

    #[test]
    fn drop_query_keeps_only_the_prefix() {
        assert_eq!(DropQuery.process("app.js?v=2:10:5"), "app.js");
        assert_eq!(DropQuery.process("no query here"), "no query here");
    }

    #[test]
    fn strip_urls_leaves_surrounding_text_intact() {
        assert_eq!(
            StripUrls.process("error for url (https://host/a/b?c=1) now"),
            "error for url () now"
        );
        assert_eq!(StripUrls.process("no url here"), "no url here");
    }

    #[test]
    fn lowercase_folds_case() {
        assert_eq!(Lowercase.process("SendRequest"), "sendrequest");
    }

    #[test]
    fn normalize_uuids_collapses_identifiers() {
        let a = NormalizeUuids.process("entity 550e8400-e29b-41d4-a716-446655440000 missing");
        let b = NormalizeUuids.process("entity 111e8400-e29b-41d4-a716-4466554400ff missing");
        assert_eq!(a, "entity <uuid> missing");
        assert_eq!(a, b);
        // A dash-separated word that isn't a UUID is left alone.
        assert_eq!(NormalizeUuids.process("content-type"), "content-type");
    }

    #[test]
    fn normalize_hex_addresses_collapses_pointers() {
        assert_eq!(NormalizeHexAddresses.process("at frame (0xdeadbeef)"), "at frame (<addr>)");
        let a = NormalizeHexAddresses.process("segfault at 0x7ffee1");
        let b = NormalizeHexAddresses.process("segfault at 0x00abcd");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_hashes_collapses_long_hex_runs() {
        let a = NormalizeHashes.process("trace 4bf92f3577b34da6a3ce929d0e0e4736 lost");
        let b = NormalizeHashes.process("trace 0af1113577b34da6a3ce929d0e0e4736 lost");
        assert_eq!(a, "trace <hash> lost");
        assert_eq!(a, b);
        // Short or digit-free hex-shaped words are preserved.
        assert_eq!(NormalizeHashes.process("deadbeef"), "deadbeef");
        assert_eq!(NormalizeHashes.process("cafe"), "cafe");
    }

    #[test]
    fn strip_digits_removes_every_digit() {
        assert_eq!(StripDigits.process("app.js:42:10"), "app.js::");
        assert_eq!(StripDigits.process("v1.2.3"), "v..");
    }

    #[test]
    fn normalize_paths_reduces_to_basename() {
        // Absolute paths on different machines collapse to the same frame.
        let unix = NormalizePaths.process("at query (/home/alice/app/src/db.rs:88:9)");
        let ci = NormalizePaths.process("at query (/build/ci/checkout/app/src/db.rs:88:9)");
        assert_eq!(unix, "at query (db.rs:88:9)");
        assert_eq!(unix, ci);
        // Windows drive paths and relative paths reduce too.
        assert_eq!(
            NormalizePaths.process("at open (C:\\Users\\bob\\proj\\io.rs:12)"),
            "at open (C:io.rs:12)"
        );
        assert_eq!(NormalizePaths.process("thrown at src/main.rs:24"), "thrown at main.rs:24");
    }

    #[test]
    fn normalize_paths_spares_prose_and_routes() {
        // Idioms, ratios, and bare routes are not filesystem paths.
        assert_eq!(NormalizePaths.process("retry read/write then fail"), "retry read/write then fail");
        assert_eq!(NormalizePaths.process("this and/or that"), "this and/or that");
        assert_eq!(NormalizePaths.process("404 on /health"), "404 on /health");
        // Rust module paths (`::`) contain no separators and are untouched.
        assert_eq!(
            NormalizePaths.process("github_backup::pairing::on_error"),
            "github_backup::pairing::on_error"
        );
    }

    #[test]
    fn collapse_whitespace_trims_and_dedupes() {
        assert_eq!(CollapseWhitespace.process("  a   b \n c "), "a b c");
    }

    // ---- pipelines (rules composed in order) ----

    #[test]
    fn frame_pipeline_ignores_line_numbers() {
        let a = FRAME_PIPELINE.run("at handler (app.js:42:10)");
        let b = FRAME_PIPELINE.run("at handler (app.js:99:3)");
        assert_eq!(a, b);
    }

    #[test]
    fn frame_pipeline_ignores_hashed_bundle_urls() {
        let a = FRAME_PIPELINE.run("at render (https://cdn.example.com/app.abc123.js:10:5)");
        let b = FRAME_PIPELINE.run("at render (https://cdn.example.com/app.def456.js:88:3)");
        assert_eq!(a, b);
    }

    #[test]
    fn frame_pipeline_ignores_build_directory() {
        let a = FRAME_PIPELINE.run("at load (/home/alice/app/src/config.rs:41:9)");
        let b = FRAME_PIPELINE.run("at load (/opt/build/app/src/config.rs:41:9)");
        assert_eq!(a, b);
    }

    #[test]
    fn message_pipeline_strips_urls() {
        let a = message("error sending request for url (https://a.example.com/x/v1.1.0)");
        let b = message("error sending request for url (https://b.other.net/y/v2.0.0)");
        assert_eq!(a, b);
    }

    #[test]
    fn frames_splits_and_drops_empty_lines() {
        let normalized = frames("at a (x.js:1:2)\n\n   \nat b (y.js:3:4)");
        assert_eq!(normalized, vec!["at a (x.js::)", "at b (y.js::)"]);
    }

    #[test]
    fn frames_collapse_consecutive_recursion() {
        // The same recursive failure at different depths must produce identical
        // frame lists so it groups together.
        let shallow = frames("at walk (tree.js:1:1)\nat walk (tree.js:2:2)\nat main (m.js:9:9)");
        let deep = frames(
            "at walk (tree.js:5:5)\nat walk (tree.js:4:4)\nat walk (tree.js:3:3)\n\
             at walk (tree.js:2:2)\nat main (m.js:9:9)",
        );
        assert_eq!(shallow, deep);
        assert_eq!(shallow, vec!["at walk (tree.js::)", "at main (m.js::)"]);
    }
}
