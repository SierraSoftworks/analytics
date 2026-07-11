//! Timezone/country lookup tables generated at build time from the bundled IANA
//! tz database (`tz/` git submodule) via `parse-zoneinfo`. See `build.rs`.
//!
//! - [`country_code_from_timezone`]: IANA timezone -> ISO 3166-1 alpha-2 code.
//! - [`country_name_from_code`]: ISO 3166-1 alpha-2 code -> English name.
//!
//! Both cover every entry in the IANA database, including deprecated link
//! aliases (e.g. `Asia/Calcutta`). Unknown inputs resolve to `None`.

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_canonical_zones_to_countries() {
        assert_eq!(country_code_from_timezone("America/New_York"), Some("US"));
        assert_eq!(country_code_from_timezone("Europe/Berlin"), Some("DE"));
        assert_eq!(country_code_from_timezone("Asia/Tokyo"), Some("JP"));
        assert_eq!(country_code_from_timezone("Australia/Sydney"), Some("AU"));
    }

    #[test]
    fn resolves_link_aliases_to_countries() {
        // Legacy aliases defined as `Link` lines in the tz `backward` file.
        assert_eq!(country_code_from_timezone("Asia/Calcutta"), Some("IN"));
        assert_eq!(country_code_from_timezone("US/Eastern"), Some("US"));
        assert_eq!(country_code_from_timezone("America/Buenos_Aires"), Some("AR"));
    }

    #[test]
    fn merged_zones_keep_their_home_country() {
        // These zones are Links to a shared representative zone in modern tz
        // data (e.g. Africa/Abidjan, primary CI), but browsers still report the
        // per-country name, which must resolve to that country, not the primary.
        assert_eq!(country_code_from_timezone("Africa/Accra"), Some("GH"));
        assert_eq!(country_code_from_timezone("Africa/Addis_Ababa"), Some("ET"));
        assert_eq!(country_code_from_timezone("Africa/Dar_es_Salaam"), Some("TZ"));
    }

    #[test]
    fn unknown_timezone_is_none() {
        assert_eq!(country_code_from_timezone("Mars/Olympus_Mons"), None);
        assert_eq!(country_code_from_timezone(""), None);
    }

    #[test]
    fn maps_country_codes_to_names() {
        assert_eq!(country_name_from_code("US"), Some("United States"));
        assert_eq!(country_name_from_code("DE"), Some("Germany"));
        assert_eq!(country_name_from_code("JP"), Some("Japan"));
    }

    #[test]
    fn unknown_country_code_is_none() {
        assert_eq!(country_name_from_code("ZZ"), None);
        assert_eq!(country_name_from_code(""), None);
    }
}
