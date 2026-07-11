//! Map an IANA timezone (reported by the browser) to an ISO 3166-1 alpha-2
//! country code. This is the only geo signal used — no IP geolocation.
//!
//! The mapping is generated at build time from the IANA tz database by the
//! [`zoneinfo`] crate, so it covers every timezone (including deprecated link
//! aliases). Unknown zones resolve to `None` ("Unknown"), which is privacy-safe.

/// Resolve a country code from an IANA timezone identifier.
pub fn country_from_timezone(tz: &str) -> Option<&'static str> {
    zoneinfo::country_code_from_timezone(tz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_zones() {
        assert_eq!(country_from_timezone("America/New_York"), Some("US"));
        assert_eq!(country_from_timezone("Europe/Berlin"), Some("DE"));
        assert_eq!(country_from_timezone("Asia/Tokyo"), Some("JP"));
        assert_eq!(country_from_timezone("Australia/Sydney"), Some("AU"));
    }

    #[test]
    fn unknown_zone_is_none() {
        assert_eq!(country_from_timezone("Mars/Olympus_Mons"), None);
        assert_eq!(country_from_timezone(""), None);
    }
}
