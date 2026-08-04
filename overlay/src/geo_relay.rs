use hbb_common::log;
use maxminddb::{path, Reader};
use once_cell::sync::Lazy;
use std::{
    collections::HashMap,
    env,
    net::IpAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        RwLock,
    },
};

const DEFAULT_DB_PATH: &str = "/root/geoip/GeoLite2-Country.mmdb";

static STATE: Lazy<RwLock<GeoState>> = Lazy::new(|| RwLock::new(GeoState::disabled()));
static ROTATION: AtomicUsize = AtomicUsize::new(0);

struct GeoState {
    enabled: bool,
    reader: Option<Reader<Vec<u8>>>,
    rules: RelayRules,
    db_path: String,
}

impl GeoState {
    fn disabled() -> Self {
        Self {
            enabled: false,
            reader: None,
            rules: RelayRules::default(),
            db_path: String::new(),
        }
    }

    fn from_env() -> Result<Self, String> {
        let enabled = parse_bool_env("GEO_RELAY_ENABLED", true);
        if !enabled {
            return Ok(Self::disabled());
        }

        let raw_rules = env::var("GEO_RELAY_RULES").unwrap_or_default();
        let rules = RelayRules::parse(&raw_rules)?;
        let db_path = env::var("GEOIP_DB_PATH").unwrap_or_else(|_| DEFAULT_DB_PATH.to_owned());
        let reader = Reader::open_readfile(&db_path)
            .map_err(|err| format!("cannot open GeoIP database {db_path}: {err}"))?;

        Ok(Self {
            enabled: true,
            reader: Some(reader),
            rules,
            db_path,
        })
    }
}

#[derive(Default)]
struct RelayRules {
    by_pair: HashMap<String, Vec<Vec<String>>>,
    fallback: Vec<Vec<String>>,
}

impl RelayRules {
    fn parse(raw: &str) -> Result<Self, String> {
        let mut parsed = Self::default();
        if raw.trim().is_empty() {
            return Err("GEO_RELAY_RULES is empty".to_owned());
        }

        for item in raw.split(';').map(str::trim).filter(|item| !item.is_empty()) {
            let (raw_key, raw_value) = item
                .split_once('=')
                .ok_or_else(|| format!("invalid Geo relay rule without '=': {item}"))?;
            let key = normalize_rule_key(raw_key)?;
            let tiers = parse_tiers(raw_value)?;

            if key == "DEFAULT" {
                if !parsed.fallback.is_empty() {
                    return Err("duplicate DEFAULT Geo relay rule".to_owned());
                }
                parsed.fallback = tiers;
            } else if parsed.by_pair.insert(key.clone(), tiers).is_some() {
                return Err(format!("duplicate Geo relay rule: {key}"));
            }
        }

        if parsed.by_pair.is_empty() && parsed.fallback.is_empty() {
            return Err("GEO_RELAY_RULES has no usable rules".to_owned());
        }

        Ok(parsed)
    }
}

pub fn reload() -> String {
    match GeoState::from_env() {
        Ok(new_state) => {
            let enabled = new_state.enabled;
            let rule_count = new_state.rules.by_pair.len();
            let db_path = new_state.db_path.clone();
            match STATE.write() {
                Ok(mut state) => {
                    *state = new_state;
                    if enabled {
                        format!(
                            "Geo relay loaded: {rule_count} country-pair rules, database={db_path}"
                        )
                    } else {
                        "Geo relay disabled by GEO_RELAY_ENABLED".to_owned()
                    }
                }
                Err(err) => format!("Geo relay state lock failed: {err}"),
            }
        }
        Err(err) => {
            // Preserve a previously loaded database/rule set when a periodic update is bad.
            let keeping_previous = STATE
                .read()
                .map(|state| state.enabled && state.reader.is_some())
                .unwrap_or(false);
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

    let reader = state.reader.as_ref()?;
    let ca = lookup_country(reader, pa);
    let cb = lookup_country(reader, pb);
    let pair = match (ca.as_deref(), cb.as_deref()) {
        (Some(a), Some(b)) => Some(country_pair_key(a, b)),
        _ => None,
    };

    if let Some(pair) = pair.as_deref() {
        if let Some(tiers) = state.rules.by_pair.get(pair) {
            if let Some(relay) = select_from_tiers(tiers, online_relays) {
                log::debug!("Geo relay selected {relay} for {pa}/{pb} ({pair})");
                return Some(relay);
            }
        }
    }

    let relay = select_from_tiers(&state.rules.fallback, online_relays);
    if let Some(relay) = relay.as_ref() {
        log::debug!(
            "Geo relay selected fallback {relay} for {pa}/{pb} ({})",
            pair.as_deref().unwrap_or("unknown")
        );
    }
    relay
}

fn lookup_country(reader: &Reader<Vec<u8>>, ip: IpAddr) -> Option<String> {
    let lookup = reader.lookup(ip).ok()?;
    let code: Option<String> = lookup
        .decode_path(&path!["country", "iso_code"])
        .ok()?;
    code.map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| value.len() == 2)
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

fn normalize_rule_key(raw: &str) -> Result<String, String> {
    let key = raw.trim().to_ascii_uppercase();
    if key == "DEFAULT" {
        return Ok(key);
    }

    let (a, b) = key
        .split_once('-')
        .ok_or_else(|| format!("invalid country-pair key: {raw}"))?;
    if !is_country_code(a) || !is_country_code(b) {
        return Err(format!("invalid country-pair key: {raw}"));
    }
    Ok(country_pair_key(a, b))
}

fn is_country_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|ch| ch.is_ascii_alphabetic())
}

fn country_pair_key(a: &str, b: &str) -> String {
    let a = a.trim().to_ascii_uppercase();
    let b = b.trim().to_ascii_uppercase();
    if a <= b {
        format!("{a}-{b}")
    } else {
        format!("{b}-{a}")
    }
}

fn parse_tiers(raw: &str) -> Result<Vec<Vec<String>>, String> {
    let tiers: Vec<Vec<String>> = raw
        .split('>')
        .map(|tier| {
            tier.split(',')
                .map(str::trim)
                .filter(|relay| !relay.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|tier| !tier.is_empty())
        .collect();

    if tiers.is_empty() {
        Err(format!("Geo relay rule has no relay servers: {raw}"))
    } else {
        Ok(tiers)
    }
}

fn select_from_tiers(tiers: &[Vec<String>], online_relays: &[String]) -> Option<String> {
    for tier in tiers {
        let available: Vec<&String> = tier
            .iter()
            .filter_map(|configured| {
                online_relays
                    .iter()
                    .find(|online| online.eq_ignore_ascii_case(configured))
            })
            .collect();
        if !available.is_empty() {
            let index = ROTATION.fetch_add(1, Ordering::SeqCst) % available.len();
            return Some(available[index].clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_country_pairs() {
        assert_eq!(normalize_rule_key("jp-cn").unwrap(), "CN-JP");
        assert_eq!(country_pair_key("US", "CN"), "CN-US");
        assert!(normalize_rule_key("china-jp").is_err());
    }

    #[test]
    fn parses_priorities_and_default() {
        let rules = RelayRules::parse(
            "CN-CN=hk-1,hk-2>jp;JP-CN=jp>hk-1;DEFAULT=jp>us",
        )
        .unwrap();
        assert_eq!(rules.by_pair["CN-CN"].len(), 2);
        assert_eq!(rules.by_pair["CN-JP"][0], vec!["jp"]);
        assert_eq!(rules.fallback[1], vec!["us"]);
    }

    #[test]
    fn chooses_first_online_priority_tier() {
        let tiers = parse_tiers("hk-1,hk-2>jp>us").unwrap();
        let online = vec!["jp".to_owned(), "us".to_owned()];
        assert_eq!(select_from_tiers(&tiers, &online), Some("jp".to_owned()));
    }

    #[test]
    fn ignores_relays_not_reported_online() {
        let tiers = parse_tiers("hk-1,hk-2").unwrap();
        let online = vec!["jp".to_owned()];
        assert_eq!(select_from_tiers(&tiers, &online), None);
    }
}
