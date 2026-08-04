mod rules;

use hbb_common::log;
use maxminddb::{geoip2, Mmap, Reader};
use once_cell::sync::Lazy;
use std::{
    env,
    net::IpAddr,
    path::Path,
    sync::{atomic::AtomicUsize, RwLock},
};

use rules::{DbRequirements, RuleSet};

const DEFAULT_COUNTRY_DB_PATH: &str = "/root/geoip/GeoLite2-Country.mmdb";
const DEFAULT_CITY_DB_PATH: &str = "/root/geoip/GeoLite2-City.mmdb";
const DEFAULT_ASN_DB_PATH: &str = "/root/geoip/GeoLite2-ASN.mmdb";

static STATE: Lazy<RwLock<GeoState>> = Lazy::new(|| RwLock::new(GeoState::disabled()));
static ROTATION: AtomicUsize = AtomicUsize::new(0);

struct GeoState {
    enabled: bool,
    readers: GeoReaders,
    rules: RuleSet,
    warnings: Vec<String>,
}

impl GeoState {
    fn disabled() -> Self {
        Self {
            enabled: false,
            readers: GeoReaders::default(),
            rules: RuleSet::empty(),
            warnings: Vec::new(),
        }
    }

    fn from_env() -> Result<Self, String> {
        if !parse_bool_env("GEO_RELAY_ENABLED", true) {
            return Ok(Self::disabled());
        }

        let raw_rules = env::var("GEO_RELAY_RULES").unwrap_or_default();
        let rules = RuleSet::parse(&raw_rules)?;
        let readers = GeoReaders::from_env()?;
        let warnings = readers.missing_requirements(rules.requirements());

        Ok(Self {
            enabled: true,
            readers,
            rules,
            warnings,
        })
    }
}

#[derive(Default)]
struct GeoReaders {
    country: Option<Reader<Mmap>>,
    city: Option<Reader<Mmap>>,
    asn: Option<Reader<Mmap>>,
    loaded: Vec<String>,
}

impl GeoReaders {
    fn from_env() -> Result<Self, String> {
        let country_path = env_path_with_legacy(
            "GEOIP_COUNTRY_DB_PATH",
            "GEOIP_DB_PATH",
            DEFAULT_COUNTRY_DB_PATH,
        );
        let city_path = env_path("GEOIP_CITY_DB_PATH", DEFAULT_CITY_DB_PATH);
        let asn_path = env_path("GEOIP_ASN_DB_PATH", DEFAULT_ASN_DB_PATH);

        let country = open_optional_reader("Country", &country_path)?;
        let city = open_optional_reader("City", &city_path)?;
        let asn = open_optional_reader("ASN", &asn_path)?;
        let mut loaded = Vec::new();
        if country.is_some() {
            loaded.push(format!("Country={country_path}"));
        }
        if city.is_some() {
            loaded.push(format!("City={city_path}"));
        }
        if asn.is_some() {
            loaded.push(format!("ASN={asn_path}"));
        }

        Ok(Self {
            country,
            city,
            asn,
            loaded,
        })
    }

    fn lookup(&self, ip: IpAddr) -> GeoFacts {
        let mut facts = GeoFacts::default();
        if let Some(reader) = self.city.as_ref() {
            lookup_city(reader, ip, &mut facts);
        }
        if let Some(reader) = self.country.as_ref() {
            lookup_country(reader, ip, &mut facts);
        }
        if let Some(reader) = self.asn.as_ref() {
            lookup_asn(reader, ip, &mut facts);
        }
        facts
    }

    fn missing_requirements(&self, requirements: DbRequirements) -> Vec<String> {
        let mut warnings = Vec::new();
        if requirements.city && self.city.is_none() {
            warnings.push("rules use City fields but the City database is unavailable".to_owned());
        }
        if requirements.asn && self.asn.is_none() {
            warnings.push("rules use ASN fields but the ASN database is unavailable".to_owned());
        }
        if requirements.country && self.country.is_none() && self.city.is_none() {
            warnings.push(
                "rules use Country/Continent fields but neither Country nor City database is available"
                    .to_owned(),
            );
        }
        warnings
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct GeoFacts {
    pub(super) continent: Option<String>,
    pub(super) country: Option<String>,
    pub(super) subdivision_codes: Vec<String>,
    pub(super) subdivision_names: Vec<String>,
    pub(super) city_names: Vec<String>,
    pub(super) city_geoname_id: Option<u32>,
    pub(super) asn: Option<u32>,
    pub(super) asn_org: Option<String>,
}

pub fn reload() -> String {
    match GeoState::from_env() {
        Ok(new_state) => {
            let enabled = new_state.enabled;
            let rule_count = new_state.rules.len();
            let syntax = new_state.rules.syntax_name();
            let databases = if new_state.readers.loaded.is_empty() {
                "none".to_owned()
            } else {
                new_state.readers.loaded.join(", ")
            };
            let warnings = new_state.warnings.clone();
            match STATE.write() {
                Ok(mut state) => {
                    *state = new_state;
                    if !enabled {
                        return "Geo relay disabled by GEO_RELAY_ENABLED".to_owned();
                    }
                    let mut message = format!(
                        "Geo relay loaded: {rule_count} ordered rules ({syntax}), databases={databases}"
                    );
                    if !warnings.is_empty() {
                        message.push_str(&format!("; warnings: {}", warnings.join("; ")));
                    }
                    message
                }
                Err(err) => format!("Geo relay state lock failed: {err}"),
            }
        }
        Err(err) => {
            let keeping_previous = STATE.read().map(|state| state.enabled).unwrap_or(false);
            if keeping_previous {
                format!("Geo relay reload failed; keeping previous data: {err}")
            } else {
                format!("Geo relay unavailable; using upstream round-robin: {err}")
            }
        }
    }
}

pub fn select_relay(pa: IpAddr, pb: IpAddr, online_relays: &[String]) -> Option<String> {
    let state = STATE.read().ok()?;
    if !state.enabled || online_relays.is_empty() {
        return None;
    }

    let facts_a = state.readers.lookup(pa);
    let facts_b = state.readers.lookup(pb);
    let selection = state
        .rules
        .select(&facts_a, &facts_b, online_relays, &ROTATION)?;
    log::debug!(
        "Geo relay selected {} for {pa}/{pb} by rule '{}' (a={facts_a:?}, b={facts_b:?})",
        selection.relay,
        selection.rule_name
    );
    Some(selection.relay)
}

fn open_optional_reader(label: &str, path: &str) -> Result<Option<Reader<Mmap>>, String> {
    if path.trim().is_empty() || !Path::new(path).is_file() {
        return Ok(None);
    }

    // SAFETY: the bundled updater always writes a new temporary file and atomically renames it.
    // It never modifies or truncates an inode while an existing Reader still maps that inode.
    let reader = unsafe { Reader::open_mmap(path) }
        .map_err(|err| format!("cannot open {label} MMDB {path}: {err}"))?;
    Ok(Some(reader))
}

fn lookup_city(reader: &Reader<Mmap>, ip: IpAddr, facts: &mut GeoFacts) {
    let Ok(result) = reader.lookup(ip) else {
        return;
    };
    let Ok(Some(record)) = result.decode::<geoip2::City>() else {
        return;
    };

    set_if_empty(&mut facts.continent, record.continent.code);
    set_if_empty(&mut facts.country, record.country.iso_code);
    facts.city_geoname_id = record.city.geoname_id;
    append_names(&mut facts.city_names, &record.city.names);
    for subdivision in record.subdivisions {
        if let Some(code) = subdivision.iso_code {
            push_unique(&mut facts.subdivision_codes, code);
        }
        append_names(&mut facts.subdivision_names, &subdivision.names);
    }
}

fn lookup_country(reader: &Reader<Mmap>, ip: IpAddr, facts: &mut GeoFacts) {
    let Ok(result) = reader.lookup(ip) else {
        return;
    };
    let Ok(Some(record)) = result.decode::<geoip2::Country>() else {
        return;
    };
    set_if_empty(&mut facts.continent, record.continent.code);
    set_if_empty(&mut facts.country, record.country.iso_code);
}

fn lookup_asn(reader: &Reader<Mmap>, ip: IpAddr, facts: &mut GeoFacts) {
    let Ok(result) = reader.lookup(ip) else {
        return;
    };
    let Ok(Some(record)) = result.decode::<geoip2::Asn>() else {
        return;
    };
    facts.asn = record.autonomous_system_number;
    facts.asn_org = record
        .autonomous_system_organization
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
}

fn append_names(target: &mut Vec<String>, names: &geoip2::Names<'_>) {
    for name in [
        names.english,
        names.simplified_chinese,
        names.japanese,
        names.german,
        names.spanish,
        names.french,
        names.brazilian_portuguese,
        names.russian,
    ]
    .into_iter()
    .flatten()
    {
        push_unique(target, name);
    }
}

fn push_unique(target: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !target.iter().any(|old| old.eq_ignore_ascii_case(value)) {
        target.push(value.to_owned());
    }
}

fn set_if_empty(target: &mut Option<String>, value: Option<&str>) {
    if target.is_none() {
        *target = value
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty());
    }
}

fn env_path(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn env_path_with_legacy(name: &str, legacy_name: &str, default: &str) -> String {
    env::var(name)
        .or_else(|_| env::var(legacy_name))
        .unwrap_or_else(|_| default.to_owned())
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => true,
            "0" | "false" | "no" | "n" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}
