//! The incident envelope — the FROZEN schema_version 1 wire contract between the capture client
//! and the central backend (spec §6.5). Evolving any field requires bumping SCHEMA_VERSION.
use crate::crash::breadcrumb::{read_cstr, CrashBreadcrumb, RING_LEN};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct S2Block { pub version: String, pub api_version: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct GamedataBlock {
    pub fingerprint: String, pub generated_at: String, pub hl2sdk: String,
    pub schema_build: String, pub stale: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct GameBlock {
    pub name: String, pub build_number: u32, pub map: String, pub players: i32, pub uptime: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct HostBlock { pub server_id: String, pub os: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct RingJson { pub tick: u64, pub plugin: String, pub dispatch: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct BreadcrumbBlock {
    pub plugin: String, pub dispatch: String, pub engine_op: String,
    pub js_location: String, pub ring: Vec<RingJson>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct PluginJson { pub id: String, pub version: String }

/// detail differs per kind; untagged = the plain objects of §6.5 (each variant has a
/// distinguishing required field set, so untagged deserialization is unambiguous).
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(untagged)]
pub enum Detail {
    Native { minidump_ref: String },
    Js { stack: String, message: String, file: String, line: u32 },
    Panic { message: String, backtrace: String },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Envelope {
    pub schema_version: u32,
    pub incident_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    pub s2script: S2Block,
    pub gamedata: GamedataBlock,
    pub game: GameBlock,
    pub host: HostBlock,
    pub breadcrumb: BreadcrumbBlock,
    pub plugins: Vec<PluginJson>,
    pub detail: Detail,
}

/// Privacy scrub toggles (from crashreporter.json; Task 3 maps config → this).
pub struct Scrub { pub map: bool, pub players: bool }

/// ISO-8601 UTC from unix seconds (no chrono dep; Howard Hinnant's civil_from_days).
pub fn iso8601_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Render a breadcrumb snapshot + detail into the wire envelope. Pure (no I/O); the caller
/// supplies occurred_at (None for native at capture — the uploader reconstructs from mtime).
pub fn render(
    bc: &CrashBreadcrumb,
    kind: &str,
    detail: Detail,
    occurred_at: Option<String>,
    server_id: &str,
    scrub: &Scrub,
) -> Envelope {
    let mut ring = Vec::new();
    for k in 0..RING_LEN {
        let idx = (bc.ring_head as usize + k) % RING_LEN;
        let e = &bc.ring[idx];
        if e.plugin[0] == 0 && e.tick == 0 { continue; } // never-written slot
        ring.push(RingJson { tick: e.tick, plugin: read_cstr(&e.plugin), dispatch: read_cstr(&e.dispatch) });
    }
    let plugins = (0..bc.plugin_count as usize)
        .map(|i| PluginJson { id: read_cstr(&bc.plugins[i].id), version: read_cstr(&bc.plugins[i].version) })
        .collect();
    Envelope {
        schema_version: SCHEMA_VERSION,
        incident_id: uuid::Uuid::new_v4().to_string(),
        kind: kind.to_string(),
        occurred_at,
        s2script: S2Block {
            version: read_cstr(&bc.s2_version),
            api_version: bc.api_version.to_string(),
        },
        gamedata: GamedataBlock {
            fingerprint: read_cstr(&bc.gamedata_fingerprint),
            generated_at: read_cstr(&bc.gamedata_generated_at),
            hl2sdk: read_cstr(&bc.hl2sdk_build),
            schema_build: read_cstr(&bc.schema_build),
            stale: bc.gamedata_stale != 0,
        },
        game: GameBlock {
            name: read_cstr(&bc.game_name),
            build_number: bc.game_build,
            map: if scrub.map { String::new() } else { read_cstr(&bc.map) },
            players: if scrub.players { 0 } else { bc.players },
            uptime: bc.uptime_secs,
        },
        host: HostBlock { server_id: server_id.to_string(), os: std::env::consts::OS.to_string() },
        breadcrumb: BreadcrumbBlock {
            plugin: read_cstr(&bc.plugin),
            dispatch: read_cstr(&bc.dispatch),
            engine_op: read_cstr(&bc.engine_op),
            js_location: read_cstr(&bc.js_location),
            ring,
        },
        plugins,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash::breadcrumb;

    #[test]
    fn iso8601_epoch_and_known_date() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_752_710_400), "2025-07-17T00:00:00Z");
        assert_eq!(iso8601_utc(1_752_710_400 + 3661), "2025-07-17T01:01:01Z");
    }

    #[test]
    fn render_panic_envelope_matches_schema_v1() {
        breadcrumb::clear_plugins();
        breadcrumb::set_identity("fp-x", "1752710400", "sdk-a", "sch-b", false);
        breadcrumb::set_game("cs2", 14099);
        breadcrumb::set_map("de_inferno");
        breadcrumb::set_players(3);
        breadcrumb::plugin_loaded("myplugin", "1.2.3");
        let _g = breadcrumb::enter_dispatch("myplugin", "OnGameFrame:pre");
        let bc = breadcrumb::snapshot();
        let env = render(
            &bc,
            "panic",
            Detail::Panic { message: "boom".into(), backtrace: "bt".into() },
            Some(iso8601_utc(1_752_710_400)),
            "srv-1",
            &Scrub { map: false, players: false },
        );
        assert_eq!(env.schema_version, 1);
        assert_eq!(env.kind, "panic");
        assert_eq!(env.occurred_at.as_deref(), Some("2025-07-17T00:00:00Z"));
        assert_eq!(env.s2script.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(env.s2script.api_version, "2");
        assert_eq!(env.gamedata.fingerprint, "fp-x");
        assert!(!env.gamedata.stale);
        assert_eq!(env.game.name, "cs2");
        assert_eq!(env.game.build_number, 14099);
        assert_eq!(env.game.map, "de_inferno");
        assert_eq!(env.game.players, 3);
        assert_eq!(env.host.server_id, "srv-1");
        assert_eq!(env.host.os, std::env::consts::OS);
        assert_eq!(env.breadcrumb.plugin, "myplugin");
        assert_eq!(env.breadcrumb.dispatch, "OnGameFrame:pre");
        assert!(env.breadcrumb.ring.len() <= breadcrumb::RING_LEN);
        assert_eq!(env.plugins, vec![PluginJson { id: "myplugin".into(), version: "1.2.3".into() }]);
        assert!(!env.incident_id.is_empty());
        // Round-trip: the wire contract survives serialize → deserialize.
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
        match back.detail {
            Detail::Panic { message, .. } => assert_eq!(message, "boom"),
            other => panic!("wrong detail variant: {:?}", other),
        }
    }

    #[test]
    fn scrub_toggles_blank_map_and_players() {
        breadcrumb::set_map("de_nuke");
        breadcrumb::set_players(9);
        let bc = breadcrumb::snapshot();
        let env = render(&bc, "js",
            Detail::Js { stack: "s".into(), message: "m".into(), file: "f".into(), line: 1 },
            None, "srv", &Scrub { map: true, players: true });
        assert_eq!(env.game.map, "");
        assert_eq!(env.game.players, 0);
        assert!(env.occurred_at.is_none());
    }
}
