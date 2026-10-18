//! `sm` — the runtime's own root console command, as in SourceMod's core.
//!
//! The runtime registers it at startup, so it exists with no plugin loaded and no plugin can
//! shadow it (`__s2_concommand` refuses the name). It is a server command:
//!
//! - the server console and RCON get everything: `sm version`, `sm plugins list|load|unload|
//!   reload`, and the registry operations `sm plugins install|update|remove|outdated|enable|
//!   disable`;
//! - a player who types `sm` in their own console gets the credits only, admins included. An
//!   in-game admin runs the rest through `sm_rcon sm …`.
//!
//! The command is registered client-executable so a player's `sm` reaches here at all; the split
//! between the two is decided by the caller slot, not by hiding the command.

use crate::v8host::{engine_ops, log_warn};
use std::ffi::CString;

pub(crate) const NAME: &str = "sm";

const USAGE: &str = "Usage: sm plugins <list|load|unload|reload> [id] | load <folder>/<id> \
| install <name>[@range]... [--with <member>] [--disabled] | update [name...] | remove <name>... \
| enable <name>... | disable <name>... | outdated";

/// Register `sm` with the engine. Idempotent on the shim side (a second call is skipped).
pub(crate) fn register() {
    let Some(func) = engine_ops().and_then(|ops| ops.concommand_register) else {
        log_warn("WARN: sm: concommand_register not wired in ops — the sm command is unavailable");
        return;
    };
    if let Ok(name) = CString::new(NAME) {
        func(name.as_ptr());
    }
}

fn credits() -> [String; 2] {
    [
        format!(
            "[S2S] s2script {} — a TypeScript plugin framework for Source 2, by Gabriel Hirakawa.",
            env!("CARGO_PKG_VERSION")
        ),
        "[S2S] github.com/s2script/s2script".to_string(),
    ]
}

/// Where `sm`'s output goes: the server console, or one player's own console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Out {
    Server,
    Client(i32),
}

impl Out {
    fn line(self, msg: &str) {
        match self {
            Out::Server => log_warn(msg),
            Out::Client(slot) => {
                let Some(func) = engine_ops().and_then(|ops| ops.client_console_print) else { return };
                if let Ok(c) = CString::new(format!("{msg}\n")) {
                    func(slot, c.as_ptr());
                }
            }
        }
    }
}

/// Handle one `sm` invocation. `slot` is -1 for the server console / RCON.
pub(crate) fn handle(slot: i32, args: &str) {
    let (out, lines) = if slot >= 0 { (Out::Client(slot), client_lines(args)) } else { (Out::Server, run(args)) };
    for line in lines {
        out.line(&line);
    }
}

/// A player's `sm`, admin or not: the credits, and a pointer to `sm_rcon` for anything else.
fn client_lines(args: &str) -> Vec<String> {
    let mut lines = credits().to_vec();
    let sub = args.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if !sub.is_empty() && sub != "version" && sub != "credits" {
        lines.push("[S2S] The rest of sm runs from the server console. In game, admins use: sm_rcon sm …".to_string());
    }
    lines
}

/// The server console's `sm`: the lines to print for `args`.
fn run(args: &str) -> Vec<String> {
    let argv: Vec<&str> = args.split_whitespace().collect();
    let sub = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match sub.as_str() {
        "" | "version" | "credits" => {
            let mut lines = credits().to_vec();
            if sub.is_empty() {
                lines.push("[S2S] Commands: sm version | sm plugins …".to_string());
            }
            lines
        }
        "plugins" => plugins(&argv[1..]),
        other => vec![format!("[S2S] Unknown sub-command '{other}'. Try: sm plugins list")],
    }
}

fn plugins(argv: &[&str]) -> Vec<String> {
    let action = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match action.as_str() {
        "" | "list" => list(),
        "install" | "update" | "remove" | "outdated" | "enable" | "disable" => registry(&action, &argv[1..]),
        "load" | "unload" | "reload" => {
            let Some(id) = argv.get(1) else { return vec![USAGE.to_string()] };
            vec![match action.as_str() {
                "load" if crate::loader::request_load(id) => format!("[S2S] Loading '{id}'…"),
                "load" => format!(
                    "[S2S] Nothing to load as {id}. A plugin in a folder under plugins/ loads as \
                     <folder>/<plugin id>, e.g. disabled/@edgegamers/jailbreak"
                ),
                "unload" if crate::loader::request_unload(id) => format!("[S2S] Unloading '{id}'…"),
                "unload" => format!("[S2S] Not a loaded plugin: {id}"),
                _ if crate::loader::request_reload(id) => format!("[S2S] Reloading '{id}'…"),
                _ => format!("[S2S] No such plugin: {id}"),
            }]
        }
        _ => vec![USAGE.to_string()],
    }
}

fn list() -> Vec<String> {
    let plugins = crate::loader::plugin_list();
    let mut lines = vec![format!("[S2S] Plugins ({}):", plugins.len())];
    for (i, (id, state)) in plugins.iter().enumerate() {
        let n = i + 1;
        lines.push(match crate::loader::session_folder(id) {
            Some(folder) => format!("  {n} \"{id}\" (running from plugins/{folder}/, this run only)"),
            None if state == "running" => format!("  {n} \"{id}\" (running)"),
            None => format!("  {n} \"{id}\" ({state})"),
        });
    }
    lines
}

/// Parse a registry operation's arguments into the request `server_update::parse_op` reads.
fn registry_request(action: &str, argv: &[&str]) -> Result<String, String> {
    let mut names = Vec::new();
    let mut with = Vec::new();
    let mut disabled = false;
    let mut i = 0;
    while i < argv.len() {
        match argv[i] {
            "--with" if i + 1 < argv.len() => {
                with.push(argv[i + 1]);
                i += 1;
            }
            "--disabled" => disabled = true,
            a => names.push(a),
        }
        i += 1;
    }
    if action != "update" && action != "outdated" && names.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(serde_json::json!({ "op": action, "names": names, "with": with, "disabled": disabled }).to_string())
}

fn registry(action: &str, argv: &[&str]) -> Vec<String> {
    let started = (|| -> Result<(), String> {
        let request = registry_request(action, argv)?;
        let op = crate::server_update::parse_op(&request)?;
        let root = crate::loader::plugins_dir()
            .and_then(|d| d.parent().map(std::path::Path::to_path_buf))
            .ok_or("the plugin loader is not running")?;
        crate::server_update::spawn_console_op(root, op, crate::server_update::console_token(), action)
    })();
    match started {
        Ok(()) => vec![format!("[S2S] sm plugins {action}: working — the report follows here when it finishes.")],
        Err(e) if e == USAGE => vec![e],
        Err(e) => vec![format!("[S2S] sm plugins {action} failed: {e}")],
    }
}

/// Print the reports of finished registry operations (main thread, from the loader's poll).
pub(crate) fn drain_reports() {
    for line in crate::server_update::take_console_report() {
        Out::Server.line(&format!("[S2S] {line}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_sm_shows_credits_and_commands() {
        let lines = run("");
        assert!(lines[0].starts_with("[S2S] s2script "), "{lines:?}");
        assert_eq!(lines[1], "[S2S] github.com/s2script/s2script");
        assert!(lines[2].contains("sm plugins"));
        assert_eq!(run("version").len(), 2);
        assert_eq!(run("CREDITS").len(), 2);
    }

    #[test]
    fn unknown_subcommand_is_named() {
        assert_eq!(run("frobnicate"), vec!["[S2S] Unknown sub-command 'frobnicate'. Try: sm plugins list"]);
    }

    #[test]
    fn plugins_load_without_an_id_prints_usage() {
        for action in ["load", "unload", "reload"] {
            assert_eq!(plugins(&[action]), vec![USAGE.to_string()]);
        }
        assert_eq!(plugins(&["bogus"]), vec![USAGE.to_string()]);
    }

    #[test]
    fn registry_requests_parse_like_basecommands_did() {
        let req = registry_request("install", &["@edge/rtv@^1", "--with", "@edge/rtv-extra", "--disabled"]).unwrap();
        assert_eq!(
            crate::server_update::parse_op(&req).unwrap(),
            crate::server_update::Op::Install {
                names: vec!["@edge/rtv@^1".into()],
                with: vec!["@edge/rtv-extra".into()],
                disabled: true,
            }
        );
        assert_eq!(
            crate::server_update::parse_op(&registry_request("update", &[]).unwrap()).unwrap(),
            crate::server_update::Op::Update { names: vec![] }
        );
        assert_eq!(registry_request("remove", &[]), Err(USAGE.to_string()));
        assert_eq!(registry_request("enable", &[]), Err(USAGE.to_string()));
    }

    #[test]
    fn a_player_only_ever_gets_the_credits() {
        assert_eq!(client_lines(""), credits().to_vec());
        assert_eq!(client_lines("version"), credits().to_vec());
        for args in ["plugins list", "plugins install @evil/x", "plugins unload @s2script/basecommands"] {
            let lines = client_lines(args);
            assert_eq!(lines[..2], credits(), "{args}");
            assert_eq!(lines.len(), 3, "{args}");
            assert!(lines[2].contains("sm_rcon"), "{args}");
        }
    }
}
