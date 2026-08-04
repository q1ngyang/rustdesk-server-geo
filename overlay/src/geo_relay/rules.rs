use serde::{de::DeserializeOwned, Deserialize, Deserializer};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::GeoFacts;

const RULE_VERSION: u8 = 2;

pub(super) struct RuleSet {
    rules: Vec<RelayRule>,
    syntax: RuleSyntax,
    requirements: DbRequirements,
}

pub(super) struct Selection {
    pub(super) relay: String,
    pub(super) rule_name: String,
}

#[derive(Clone, Copy, Default)]
pub(super) struct DbRequirements {
    pub(super) country: bool,
    pub(super) city: bool,
    pub(super) asn: bool,
}

#[derive(Clone, Copy)]
enum RuleSyntax {
    YamlV2,
    Legacy,
}

impl RuleSet {
    pub(super) fn empty() -> Self {
        Self {
            rules: Vec::new(),
            syntax: RuleSyntax::YamlV2,
            requirements: DbRequirements::default(),
        }
    }

    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("GEO_RELAY_RULES is empty".to_owned());
        }
        if looks_like_legacy(raw) {
            return Self::parse_legacy(raw);
        }

        let document: RuleDocument = serde_yml::from_str(raw)
            .map_err(|err| format!("invalid GEO_RELAY_RULES YAML: {err}"))?;
        if document.version != RULE_VERSION {
            return Err(format!(
                "unsupported GEO_RELAY_RULES version {}; expected {RULE_VERSION}",
                document.version
            ));
        }
        Self::compile(document.rules, RuleSyntax::YamlV2)
    }

    pub(super) fn select(
        &self,
        facts_a: &GeoFacts,
        facts_b: &GeoFacts,
        online_relays: &[String],
        rotation: &AtomicUsize,
    ) -> Option<Selection> {
        for rule in &self.rules {
            if !rule.matches(facts_a, facts_b) {
                continue;
            }
            if let Some(relay) = select_from_tiers(&rule.relay_tiers, online_relays, rotation) {
                return Some(Selection {
                    relay,
                    rule_name: rule.name.clone(),
                });
            }
        }
        None
    }

    pub(super) fn len(&self) -> usize {
        self.rules.len()
    }

    pub(super) fn syntax_name(&self) -> &'static str {
        match self.syntax {
            RuleSyntax::YamlV2 => "YAML v2",
            RuleSyntax::Legacy => "legacy single-line",
        }
    }

    pub(super) fn requirements(&self) -> DbRequirements {
        self.requirements
    }

    fn compile(configs: Vec<RelayRuleConfig>, syntax: RuleSyntax) -> Result<Self, String> {
        if configs.is_empty() {
            return Err("GEO_RELAY_RULES has no rules".to_owned());
        }

        let mut rules = Vec::with_capacity(configs.len());
        let mut requirements = DbRequirements::default();
        for (index, config) in configs.into_iter().enumerate() {
            let name = config.name.trim().to_owned();
            if name.is_empty() {
                return Err(format!("rule {} has an empty name", index + 1));
            }
            if rules.iter().any(|rule: &RelayRule| rule.name == name) {
                return Err(format!("duplicate rule name: {name}"));
            }
            config.matches.client_a.validate(&name)?;
            config.matches.client_b.validate(&name)?;
            requirements.merge(config.matches.client_a.requirements());
            requirements.merge(config.matches.client_b.requirements());

            let relay_tiers = normalize_tiers(config.relay_tiers, &name)?;
            rules.push(RelayRule {
                name,
                symmetric: config.symmetric,
                client_a: config.matches.client_a,
                client_b: config.matches.client_b,
                relay_tiers,
            });
        }

        Ok(Self {
            rules,
            syntax,
            requirements,
        })
    }

    fn parse_legacy(raw: &str) -> Result<Self, String> {
        let mut configs = Vec::new();
        for item in raw.split(';').map(str::trim).filter(|item| !item.is_empty()) {
            let (raw_key, raw_value) = item
                .split_once('=')
                .ok_or_else(|| format!("invalid legacy rule without '=': {item}"))?;
            let key = raw_key.trim().to_ascii_uppercase();
            let matches = if key == "DEFAULT" {
                EndpointPair::default()
            } else {
                let (country_a, country_b) = key
                    .split_once('-')
                    .ok_or_else(|| format!("invalid legacy country pair: {raw_key}"))?;
                if !is_country_code(country_a) || !is_country_code(country_b) {
                    return Err(format!("invalid legacy country pair: {raw_key}"));
                }
                EndpointPair {
                    client_a: Matcher::country(country_a),
                    client_b: Matcher::country(country_b),
                }
            };
            let relay_tiers = parse_legacy_tiers(raw_value)?
                .into_iter()
                .map(RelayTierConfig::Many)
                .collect();
            configs.push(RelayRuleConfig {
                name: format!("legacy-{key}"),
                symmetric: true,
                matches,
                relay_tiers,
            });
        }
        Self::compile(configs, RuleSyntax::Legacy)
    }
}

impl DbRequirements {
    fn merge(&mut self, other: Self) {
        self.country |= other.country;
        self.city |= other.city;
        self.asn |= other.asn;
    }
}

struct RelayRule {
    name: String,
    symmetric: bool,
    client_a: Matcher,
    client_b: Matcher,
    relay_tiers: Vec<Vec<String>>,
}

impl RelayRule {
    fn matches(&self, facts_a: &GeoFacts, facts_b: &GeoFacts) -> bool {
        let direct = self.client_a.matches(facts_a) && self.client_b.matches(facts_b);
        direct
            || (self.symmetric
                && self.client_a.matches(facts_b)
                && self.client_b.matches(facts_a))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDocument {
    version: u8,
    rules: Vec<RelayRuleConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayRuleConfig {
    name: String,
    #[serde(default = "default_true")]
    symmetric: bool,
    #[serde(rename = "match")]
    matches: EndpointPair,
    relay_tiers: Vec<RelayTierConfig>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RelayTierConfig {
    One(String),
    Many(Vec<String>),
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointPair {
    #[serde(default)]
    client_a: Matcher,
    #[serde(default)]
    client_b: Matcher,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Matcher {
    #[serde(default)]
    all: Vec<Matcher>,
    #[serde(default)]
    any: Vec<Matcher>,
    #[serde(default)]
    not: Option<Box<Matcher>>,
    #[serde(default, deserialize_with = "one_or_many")]
    continent: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    country: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    subdivision: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    city: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    city_geoname_id: Vec<u32>,
    #[serde(default, deserialize_with = "one_or_many")]
    asn: Vec<u32>,
    #[serde(default, deserialize_with = "one_or_many")]
    asn_org_contains: Vec<String>,
}

impl Matcher {
    fn country(country: &str) -> Self {
        Self {
            country: vec![country.trim().to_ascii_uppercase()],
            ..Self::default()
        }
    }

    fn matches(&self, facts: &GeoFacts) -> bool {
        if !matches_optional(&facts.continent, &self.continent)
            || !matches_optional(&facts.country, &self.country)
            || !matches_any_value(
                &facts.subdivision_codes,
                &facts.subdivision_names,
                &self.subdivision,
            )
            || !matches_values(&facts.city_names, &self.city)
            || !matches_number(facts.city_geoname_id, &self.city_geoname_id)
            || !matches_number(facts.asn, &self.asn)
            || !matches_contains(&facts.asn_org, &self.asn_org_contains)
        {
            return false;
        }
        if !self.all.iter().all(|matcher| matcher.matches(facts)) {
            return false;
        }
        if !self.any.is_empty() && !self.any.iter().any(|matcher| matcher.matches(facts)) {
            return false;
        }
        if self
            .not
            .as_ref()
            .map(|matcher| matcher.matches(facts))
            .unwrap_or(false)
        {
            return false;
        }
        true
    }

    fn validate(&self, rule_name: &str) -> Result<(), String> {
        validate_strings(&self.continent, "continent", rule_name)?;
        validate_strings(&self.country, "country", rule_name)?;
        validate_strings(&self.subdivision, "subdivision", rule_name)?;
        validate_strings(&self.city, "city", rule_name)?;
        validate_strings(&self.asn_org_contains, "asn_org_contains", rule_name)?;
        if self.asn.contains(&0) {
            return Err(format!("rule '{rule_name}' contains invalid ASN 0"));
        }
        for matcher in self.all.iter().chain(self.any.iter()) {
            matcher.validate(rule_name)?;
        }
        if let Some(matcher) = self.not.as_ref() {
            matcher.validate(rule_name)?;
        }
        Ok(())
    }

    fn requirements(&self) -> DbRequirements {
        let mut requirements = DbRequirements {
            country: !self.country.is_empty() || !self.continent.is_empty(),
            city: !self.subdivision.is_empty()
                || !self.city.is_empty()
                || !self.city_geoname_id.is_empty(),
            asn: !self.asn.is_empty() || !self.asn_org_contains.is_empty(),
        };
        for matcher in self.all.iter().chain(self.any.iter()) {
            requirements.merge(matcher.requirements());
        }
        if let Some(matcher) = self.not.as_ref() {
            requirements.merge(matcher.requirements());
        }
        requirements
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

fn one_or_many<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    match OneOrMany::<T>::deserialize(deserializer)? {
        OneOrMany::One(value) => Ok(vec![value]),
        OneOrMany::Many(values) => Ok(values),
    }
}

fn normalize_tiers(
    tiers: Vec<RelayTierConfig>,
    rule_name: &str,
) -> Result<Vec<Vec<String>>, String> {
    let mut normalized = Vec::new();
    for tier in tiers {
        let relays = match tier {
            RelayTierConfig::One(relay) => vec![relay],
            RelayTierConfig::Many(relays) => relays,
        };
        let relays: Vec<String> = relays
            .into_iter()
            .map(|relay| relay.trim().to_owned())
            .filter(|relay| !relay.is_empty())
            .collect();
        if relays.is_empty() {
            return Err(format!("rule '{rule_name}' contains an empty relay tier"));
        }
        normalized.push(relays);
    }
    if normalized.is_empty() {
        return Err(format!("rule '{rule_name}' has no relay_tiers"));
    }
    Ok(normalized)
}

fn select_from_tiers(
    tiers: &[Vec<String>],
    online_relays: &[String],
    rotation: &AtomicUsize,
) -> Option<String> {
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
            let index = rotation.fetch_add(1, Ordering::SeqCst) % available.len();
            return Some(available[index].clone());
        }
    }
    None
}

fn matches_optional(actual: &Option<String>, allowed: &[String]) -> bool {
    allowed.is_empty()
        || actual
            .as_ref()
            .map(|actual| {
                allowed
                    .iter()
                    .any(|allowed| actual.eq_ignore_ascii_case(allowed.trim()))
            })
            .unwrap_or(false)
}

fn matches_values(actual: &[String], allowed: &[String]) -> bool {
    allowed.is_empty()
        || actual.iter().any(|actual| {
            allowed
                .iter()
                .any(|allowed| actual.eq_ignore_ascii_case(allowed.trim()))
        })
}

fn matches_any_value(first: &[String], second: &[String], allowed: &[String]) -> bool {
    allowed.is_empty() || matches_values(first, allowed) || matches_values(second, allowed)
}

fn matches_number(actual: Option<u32>, allowed: &[u32]) -> bool {
    allowed.is_empty() || actual.map(|actual| allowed.contains(&actual)).unwrap_or(false)
}

fn matches_contains(actual: &Option<String>, needles: &[String]) -> bool {
    if needles.is_empty() {
        return true;
    }
    let Some(actual) = actual.as_ref() else {
        return false;
    };
    let actual = actual.to_ascii_lowercase();
    needles.iter().any(|needle| {
        let needle = needle.trim().to_ascii_lowercase();
        !needle.is_empty() && actual.contains(&needle)
    })
}

fn validate_strings(values: &[String], field: &str, rule_name: &str) -> Result<(), String> {
    if values.iter().any(|value| value.trim().is_empty()) {
        Err(format!(
            "rule '{rule_name}' contains an empty value in {field}"
        ))
    } else {
        Ok(())
    }
}

fn looks_like_legacy(raw: &str) -> bool {
    !raw.contains('\n') && raw.contains('=') && !raw.trim_start().starts_with('{')
}

fn parse_legacy_tiers(raw: &str) -> Result<Vec<Vec<String>>, String> {
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
        Err(format!("legacy rule has no relay servers: {raw}"))
    } else {
        Ok(tiers)
    }
}

fn is_country_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|ch| ch.is_ascii_alphabetic())
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(country: &str, city: &str, asn: u32, org: &str) -> GeoFacts {
        GeoFacts {
            country: Some(country.to_owned()),
            city_names: vec![city.to_owned()],
            asn: Some(asn),
            asn_org: Some(org.to_owned()),
            ..GeoFacts::default()
        }
    }

    #[test]
    fn parses_multiline_yaml_and_uses_first_matching_rule() {
        let rules = RuleSet::parse(
            r#"
version: 2
rules:
  - name: China Telecom to Tokyo
    match:
      client_a:
        country: CN
        asn: 4134
      client_b:
        country: JP
        city: [Tokyo, 東京]
    relay_tiers:
      - [tokyo-1, tokyo-2]
      - osaka
  - name: CN-JP general
    match:
      client_a: { country: CN }
      client_b: { country: JP }
    relay_tiers: [general]
"#,
        )
        .unwrap();
        let online = vec!["tokyo-1".to_owned(), "general".to_owned()];
        let rotation = AtomicUsize::new(0);
        let selected = rules
            .select(
                &facts("CN", "Shanghai", 4134, "China Telecom"),
                &facts("JP", "Tokyo", 2516, "KDDI"),
                &online,
                &rotation,
            )
            .unwrap();
        assert_eq!(selected.relay, "tokyo-1");
        assert_eq!(selected.rule_name, "China Telecom to Tokyo");
    }

    #[test]
    fn supports_recursive_all_any_and_not() {
        let matcher: Matcher = serde_yml::from_str(
            r#"
all:
  - country: CN
  - any:
      - city: Shanghai
      - asn: 4134
not:
  asn_org_contains: China Mobile
"#,
        )
        .unwrap();
        assert!(matcher.matches(&facts(
            "CN",
            "Beijing",
            4134,
            "China Telecom"
        )));
        assert!(!matcher.matches(&facts(
            "CN",
            "Shanghai",
            9808,
            "China Mobile"
        )));
    }

    #[test]
    fn symmetric_rules_match_reversed_clients() {
        let rules = RuleSet::parse(
            "CN-JP=jp>hk;DEFAULT=us",
        )
        .unwrap();
        let online = vec!["jp".to_owned(), "us".to_owned()];
        let selected = rules
            .select(
                &facts("JP", "Tokyo", 2516, "KDDI"),
                &facts("CN", "Shanghai", 4134, "China Telecom"),
                &online,
                &AtomicUsize::new(0),
            )
            .unwrap();
        assert_eq!(selected.relay, "jp");
    }

    #[test]
    fn continues_to_lower_rule_if_matching_relays_are_offline() {
        let rules = RuleSet::parse(
            r#"
version: 2
rules:
  - name: preferred
    match:
      client_a: { country: CN }
      client_b: { country: JP }
    relay_tiers: [offline]
  - name: default
    match: {}
    relay_tiers: [online]
"#,
        )
        .unwrap();
        let selected = rules
            .select(
                &facts("CN", "Shanghai", 4134, "China Telecom"),
                &facts("JP", "Tokyo", 2516, "KDDI"),
                &["online".to_owned()],
                &AtomicUsize::new(0),
            )
            .unwrap();
        assert_eq!(selected.relay, "online");
        assert_eq!(selected.rule_name, "default");
    }

    #[test]
    fn rejects_unknown_fields_and_duplicate_names() {
        let unknown = RuleSet::parse(
            "version: 2\nrules:\n  - name: bad\n    match: { client_a: { isp: x } }\n    relay_tiers: [x]",
        );
        assert!(unknown.is_err());

        let duplicate = RuleSet::parse(
            "version: 2\nrules:\n  - { name: same, match: {}, relay_tiers: [a] }\n  - { name: same, match: {}, relay_tiers: [b] }",
        );
        assert!(duplicate.is_err());
    }
}
