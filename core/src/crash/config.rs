//! crashreporter.json — operator config (runtime infra, NOT plugin-permissioned; spec §6.7).
//! Fail-off: absent/malformed file → all defaults, reporter effectively disabled.
use serde::Deserialize;

pub const DEFAULT_ENDPOINT: &str = "https://s2script.com/api/crash/v1/ingest";

fn default_endpoint() -> String { DEFAULT_ENDPOINT.to_string() }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CrashConfig {
    #[serde(default)]
    pub enabled: bool, // opt-in: default FALSE
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub include_minidump: bool,
    #[serde(default)]
    pub scrub_map: bool,
    #[serde(default)]
    pub scrub_players: bool,
    /// Dev-only: arms the deliberate-crash harness natives (Task 6). Never enable in production.
    #[serde(default)]
    pub dev_test: bool,
}

impl Default for CrashConfig {
    fn default() -> Self {
        CrashConfig {
            enabled: false,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            api_key: String::new(),
            include_minidump: true,
            scrub_map: false,
            scrub_players: false,
            dev_test: false,
        }
    }
}

pub fn parse(json: Option<&str>) -> CrashConfig {
    json.and_then(|s| serde_json::from_str(&crate::config::strip_line_comments(s)).ok())
        .unwrap_or_default()
}

/// Read + parse configs/crashreporter.json via the config_read engine op (shim file I/O).
pub fn load() -> CrashConfig {
    parse(crate::v8host::read_engine_config("crashreporter").as_deref())
}

pub fn scrub(cfg: &CrashConfig) -> crate::crash::envelope::Scrub {
    crate::crash::envelope::Scrub { map: cfg.scrub_map, players: cfg.scrub_players }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_opt_out_and_fail_off() {
        let c = parse(None);
        assert!(!c.enabled, "enabled MUST default to false (opt-in)");
        assert_eq!(c.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(c.api_key, "");
        assert!(c.include_minidump);
        assert!(!c.scrub_map);
        assert!(!c.scrub_players);
        assert!(!c.dev_test);
    }

    #[test]
    fn parse_overrides_and_tolerates_jsonc_comments() {
        let c = parse(Some(
            r#"{
                // operator opted in
                "enabled": true,
                "endpoint": "http://127.0.0.1:9/ingest",
                "api_key": "k-123",
                "include_minidump": false,
                "scrub_map": true
            }"#,
        ));
        assert!(c.enabled);
        assert_eq!(c.endpoint, "http://127.0.0.1:9/ingest");
        assert_eq!(c.api_key, "k-123");
        assert!(!c.include_minidump);
        assert!(c.scrub_map);
        assert!(!c.scrub_players); // unspecified key keeps its default
    }

    #[test]
    fn malformed_json_degrades_to_defaults() {
        let c = parse(Some("{ not json"));
        assert!(!c.enabled);
        assert_eq!(c.endpoint, DEFAULT_ENDPOINT);
    }
}
