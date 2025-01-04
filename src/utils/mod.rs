pub fn normalize_page_uri(mut uri: &str) -> String {
    if let Some((_scheme, rest)) = uri.split_once("://") {
        uri = rest;
    }

    let mut normalized = uri.trim_matches('/').to_lowercase();
    normalized.insert(0, '/');
    normalized
}
