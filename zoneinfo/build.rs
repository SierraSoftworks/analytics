//! Build script: parse the bundled IANA tz database (git submodule at `tz/`)
//! with `parse-zoneinfo` and generate exhaustive `match` statements mapping
//! every timezone (canonical zones plus legacy link aliases) to its ISO 3166-1
//! country code, and every country code to its English name.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use parse_zoneinfo::line::Line;
use parse_zoneinfo::table::TableBuilder;

/// The tz database source files that define zones, rules, and link aliases.
const FILES: &[&str] = &[
    "africa",
    "antarctica",
    "asia",
    "australasia",
    "backward",
    "etcetera",
    "europe",
    "northamerica",
    "southamerica",
];

/// Drop everything from the first `#` onward; tz data never uses `#` in values.
fn strip_comments(mut line: String) -> String {
    if let Some(pos) = line.find('#') {
        line.truncate(pos);
    }
    line
}

/// Read a `.tab` file into tab-separated columns, skipping comments and blanks.
fn read_tab_rows(path: &Path) -> Vec<Vec<String>> {
    let file = File::open(path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
    BufReader::new(file)
        .lines()
        .map(|l| l.expect("read tab line"))
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let tz = manifest.join("tz");

    println!("cargo:rerun-if-changed=build.rs");
    for f in FILES {
        println!("cargo:rerun-if-changed=tz/{f}");
    }
    println!("cargo:rerun-if-changed=tz/zone.tab");
    println!("cargo:rerun-if-changed=tz/zone1970.tab");
    println!("cargo:rerun-if-changed=tz/iso3166.tab");

    if !tz.join("zone1970.tab").exists() {
        panic!(
            "IANA tz submodule not found at {}.\nRun: git submodule update --init zoneinfo/tz",
            tz.display()
        );
    }

    // 1. Parse the zone definition files into a table of zones + link aliases.
    let mut builder = TableBuilder::new();
    for f in FILES {
        let path = tz.join(f);
        let file =
            File::open(&path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
        for line in BufReader::new(file).lines() {
            let line = strip_comments(line.expect("read tz line"));
            let parsed = Line::new(&line)
                .unwrap_or_else(|e| panic!("cannot parse tz line in {f}: {line:?}: {e:?}"));
            builder
                .add_line(parsed)
                .unwrap_or_else(|e| panic!("invalid tz line in {f}: {line:?}: {e:?}"));
        }
    }
    let table = builder.build();

    // 2. Timezone -> ISO country code. `zone.tab` is the deprecated but more
    //    granular table: exactly one country per row, and it keeps dedicated
    //    rows for zones that modern data merges into a shared representative
    //    zone (e.g. Africa/Accra stays GH rather than folding into the
    //    Africa/Abidjan row, whose primary is CI). This matches the zone name a
    //    browser actually reports. Any zones missing there are filled from the
    //    canonical `zone1970.tab`, whose column 0 lists overlapping countries
    //    with the primary (most-populous) one first.
    let mut zone_country: HashMap<String, String> = HashMap::new();
    for row in read_tab_rows(&tz.join("zone.tab")) {
        if row.len() < 3 {
            continue;
        }
        let code = row[0].trim();
        if !code.is_empty() {
            zone_country.insert(row[2].clone(), code.to_string());
        }
    }
    for row in read_tab_rows(&tz.join("zone1970.tab")) {
        if row.len() < 3 {
            continue;
        }
        let primary = row[0].split(',').next().unwrap_or("").trim();
        if !primary.is_empty() {
            zone_country
                .entry(row[2].clone())
                .or_insert_with(|| primary.to_string());
        }
    }

    // 3. iso3166.tab: ISO country code -> English country name.
    let mut country_name: BTreeMap<String, String> = BTreeMap::new();
    for row in read_tab_rows(&tz.join("iso3166.tab")) {
        if row.len() < 2 {
            continue;
        }
        country_name.insert(row[0].clone(), row[1].clone());
    }

    // 4. Full timezone -> country map: canonical zones plus every link alias,
    //    following link chains back to a canonical zone with a known country.
    let mut tz_country: BTreeMap<String, String> = BTreeMap::new();
    for (zone, code) in &zone_country {
        tz_country.insert(zone.clone(), code.clone());
    }
    for alias in table.links.keys() {
        let mut current = alias.clone();
        let mut hops = 0;
        let resolved = loop {
            if let Some(code) = zone_country.get(&current) {
                break Some(code.clone());
            }
            match table.links.get(&current) {
                Some(target) if hops < 32 => {
                    current = target.clone();
                    hops += 1;
                }
                _ => break None,
            }
        };
        if let Some(code) = resolved {
            tz_country.insert(alias.clone(), code);
        }
    }

    // 5. Emit the generated lookup functions.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let dest = out.join("generated.rs");
    let mut f = File::create(&dest).unwrap_or_else(|e| panic!("create {}: {e}", dest.display()));

    writeln!(
        f,
        "/// Maps an IANA timezone identifier to its ISO 3166-1 alpha-2 country code."
    )
    .unwrap();
    writeln!(
        f,
        "pub fn country_code_from_timezone(timezone: &str) -> Option<&'static str> {{"
    )
    .unwrap();
    writeln!(f, "    let code = match timezone {{").unwrap();
    for (zone, code) in &tz_country {
        writeln!(f, "        {zone:?} => {code:?},").unwrap();
    }
    writeln!(f, "        _ => return None,").unwrap();
    writeln!(f, "    }};").unwrap();
    writeln!(f, "    Some(code)").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(
        f,
        "/// Maps an ISO 3166-1 alpha-2 country code to its English name."
    )
    .unwrap();
    writeln!(
        f,
        "pub fn country_name_from_code(code: &str) -> Option<&'static str> {{"
    )
    .unwrap();
    writeln!(f, "    let name = match code {{").unwrap();
    for (code, name) in &country_name {
        writeln!(f, "        {code:?} => {name:?},").unwrap();
    }
    writeln!(f, "        _ => return None,").unwrap();
    writeln!(f, "    }};").unwrap();
    writeln!(f, "    Some(name)").unwrap();
    writeln!(f, "}}").unwrap();
}
