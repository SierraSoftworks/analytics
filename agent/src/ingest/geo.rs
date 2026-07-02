//! Map an IANA timezone (reported by the browser) to an ISO 3166-1 alpha-2 country
//! code. This is the only geo signal used — no IP geolocation. The map is a curated
//! subset of common zones; unknown zones resolve to `None` ("Unknown"), which is
//! privacy-safe. Extend as needed.

/// Resolve a country code from an IANA timezone identifier.
pub fn country_from_timezone(tz: &str) -> Option<&'static str> {
    let code = match tz {
        // North America
        "America/New_York"
        | "America/Detroit"
        | "America/Chicago"
        | "America/Denver"
        | "America/Phoenix"
        | "America/Los_Angeles"
        | "America/Anchorage"
        | "America/Adak"
        | "Pacific/Honolulu"
        | "America/Boise"
        | "America/Indiana/Indianapolis"
        | "America/Kentucky/Louisville" => "US",
        "America/Toronto" | "America/Vancouver" | "America/Edmonton" | "America/Winnipeg"
        | "America/Halifax" | "America/St_Johns" | "America/Regina" => "CA",
        "America/Mexico_City" | "America/Tijuana" | "America/Monterrey" | "America/Cancun" => "MX",
        // Central & South America
        "America/Guatemala" => "GT",
        "America/Costa_Rica" => "CR",
        "America/Panama" => "PA",
        "America/Bogota" => "CO",
        "America/Lima" => "PE",
        "America/Caracas" => "VE",
        "America/Santiago" => "CL",
        "America/Argentina/Buenos_Aires" => "AR",
        "America/Montevideo" => "UY",
        "America/Asuncion" => "PY",
        "America/La_Paz" => "BO",
        "America/Sao_Paulo" | "America/Bahia" | "America/Fortaleza" | "America/Manaus" => "BR",
        // Europe
        "Europe/London" | "Europe/Belfast" => "GB",
        "Europe/Dublin" => "IE",
        "Europe/Lisbon" => "PT",
        "Europe/Madrid" => "ES",
        "Europe/Paris" => "FR",
        "Europe/Brussels" => "BE",
        "Europe/Amsterdam" => "NL",
        "Europe/Luxembourg" => "LU",
        "Europe/Berlin" | "Europe/Busingen" => "DE",
        "Europe/Zurich" => "CH",
        "Europe/Vienna" => "AT",
        "Europe/Rome" => "IT",
        "Europe/Malta" => "MT",
        "Europe/Copenhagen" => "DK",
        "Europe/Oslo" => "NO",
        "Europe/Stockholm" => "SE",
        "Europe/Helsinki" => "FI",
        "Europe/Tallinn" => "EE",
        "Europe/Riga" => "LV",
        "Europe/Vilnius" => "LT",
        "Europe/Warsaw" => "PL",
        "Europe/Prague" => "CZ",
        "Europe/Bratislava" => "SK",
        "Europe/Budapest" => "HU",
        "Europe/Ljubljana" => "SI",
        "Europe/Zagreb" => "HR",
        "Europe/Belgrade" => "RS",
        "Europe/Sarajevo" => "BA",
        "Europe/Bucharest" => "RO",
        "Europe/Sofia" => "BG",
        "Europe/Athens" => "GR",
        "Europe/Istanbul" => "TR",
        "Europe/Kyiv" | "Europe/Kiev" => "UA",
        "Europe/Minsk" => "BY",
        "Europe/Moscow" | "Europe/Samara" | "Asia/Yekaterinburg" | "Asia/Novosibirsk"
        | "Asia/Krasnoyarsk" | "Asia/Irkutsk" | "Asia/Vladivostok" => "RU",
        "Europe/Reykjavik" => "IS",
        // Africa
        "Africa/Casablanca" => "MA",
        "Africa/Algiers" => "DZ",
        "Africa/Tunis" => "TN",
        "Africa/Cairo" => "EG",
        "Africa/Lagos" => "NG",
        "Africa/Accra" => "GH",
        "Africa/Nairobi" => "KE",
        "Africa/Addis_Ababa" => "ET",
        "Africa/Dar_es_Salaam" => "TZ",
        "Africa/Kampala" => "UG",
        "Africa/Johannesburg" => "ZA",
        "Africa/Harare" => "ZW",
        "Africa/Lusaka" => "ZM",
        "Africa/Kinshasa" => "CD",
        // Middle East
        "Asia/Jerusalem" | "Asia/Tel_Aviv" => "IL",
        "Asia/Beirut" => "LB",
        "Asia/Amman" => "JO",
        "Asia/Riyadh" => "SA",
        "Asia/Qatar" => "QA",
        "Asia/Dubai" => "AE",
        "Asia/Kuwait" => "KW",
        "Asia/Baghdad" => "IQ",
        "Asia/Tehran" => "IR",
        // South & Central Asia
        "Asia/Karachi" => "PK",
        "Asia/Kolkata" | "Asia/Calcutta" => "IN",
        "Asia/Colombo" => "LK",
        "Asia/Dhaka" => "BD",
        "Asia/Kathmandu" => "NP",
        "Asia/Tashkent" => "UZ",
        "Asia/Almaty" => "KZ",
        // East & Southeast Asia
        "Asia/Bangkok" => "TH",
        "Asia/Ho_Chi_Minh" | "Asia/Saigon" => "VN",
        "Asia/Jakarta" => "ID",
        "Asia/Kuala_Lumpur" => "MY",
        "Asia/Singapore" => "SG",
        "Asia/Manila" => "PH",
        "Asia/Hong_Kong" => "HK",
        "Asia/Taipei" => "TW",
        "Asia/Shanghai" | "Asia/Urumqi" => "CN",
        "Asia/Tokyo" => "JP",
        "Asia/Seoul" => "KR",
        // Oceania
        "Australia/Sydney"
        | "Australia/Melbourne"
        | "Australia/Brisbane"
        | "Australia/Perth"
        | "Australia/Adelaide"
        | "Australia/Hobart"
        | "Australia/Darwin" => "AU",
        "Pacific/Auckland" => "NZ",
        "Pacific/Fiji" => "FJ",
        _ => return None,
    };
    Some(code)
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
