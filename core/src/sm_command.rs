//! `sm` — the runtime's own root console command, as in SourceMod's core.
//!
//! The runtime registers it at startup, so it exists with no plugin loaded and no plugin can
//! shadow it (`__s2_concommand` refuses the name). It is a server command:
//!
//! - the server console and RCON get everything: `sm version`, `sm plugins list|load|unload|
//!   reload`, and the registry operations `sm plugins install|update|remove|outdated|enable|
//!   disable`;
//! - a player who types `sm` in their own console gets what SourceMod gives one (its
//!   `PlayerManager::OnClientCommand`): `sm plugins` lists the running plugins, `sm credits` the
//!   credits, anything else the version banner. Nothing that changes the server, admins included;
//!   an in-game admin runs the rest through `sm_rcon sm …`.
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

fn version_line() -> String {
    format!("s2script {}, by Gabriel Hirakawa", env!("CARGO_PKG_VERSION"))
}

/// What a bare `sm` prints, as SourceMod's does.
fn banner() -> Vec<String> {
    vec![
        version_line(),
        "To see running plugins, type \"sm plugins\"".to_string(),
        "To see credits, type \"sm credits\"".to_string(),
        "Visit https://www.s2script.com/".to_string(),
    ]
}

fn credits() -> Vec<String> {
    vec![
        "s2script would not be possible without:".to_string(),
        " Gabriel Hirakawa".to_string(),
        "s2script is open source under the MIT or Apache-2.0 license.".to_string(),
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

/// A player's `sm`, admin or not, as SourceMod answers one: the running plugins, the credits, or
/// the version banner. Read-only; nothing here changes the server.
fn client_lines(args: &str) -> Vec<String> {
    match args.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str() {
        "plugins" => {
            let running: Vec<String> = crate::loader::plugin_list()
                .into_iter()
                .filter(|(_, state)| state == "running")
                .map(|(id, _)| id)
                .collect();
            let mut lines = vec![format!("Listing {} plugin(s):", running.len())];
            lines.extend(running.iter().enumerate().map(|(i, id)| format!("  {} \"{id}\"", i + 1)));
            lines
        }
        "credits" => credits(),
        _ => banner(),
    }
}

/// The server console's `sm`: the lines to print for `args`.
fn run(args: &str) -> Vec<String> {
    let argv: Vec<&str> = args.split_whitespace().collect();
    let sub = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match sub.as_str() {
        "" => {
            let mut lines = vec![version_line()];
            lines.push("Commands: sm version | sm credits | sm plugins … | sm prof …".to_string());
            lines
        }
        "version" => vec![version_line(), "Visit https://www.s2script.com/".to_string()],
        "credits" => credits(),
        "plugins" => plugins(&argv[1..]),
        "prof" => prof(&argv[1..]),
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

const PROF_USAGE: &str = "Usage: sm prof start [seconds] [js] | stop | status | dump [rows] | clear";

/// `sm prof …`: the runtime's profiler (see `profiler`). Server console / RCON only.
fn prof(argv: &[&str]) -> Vec<String> {
    let action = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match action.as_str() {
        "start" => match prof_options(&argv[1..]) {
            Ok(options) => crate::profiler::request(crate::profiler::Request::Start(options)),
            Err(e) => vec![e],
        },
        "stop" => crate::profiler::request(crate::profiler::Request::Stop),
        "status" => crate::profiler::status(),
        "dump" => match argv.get(1).map(|n| n.parse::<usize>()) {
            None => crate::profiler::dump(crate::profiler::Summary::DEFAULT_ROWS),
            Some(Ok(n)) if n > 0 => crate::profiler::dump(n),
            Some(_) => vec![PROF_USAGE.to_string()],
        },
        "clear" => crate::profiler::clear(),
        _ => vec![PROF_USAGE.to_string()],
    }
}

/// `start`'s arguments, in any order: a duration in seconds, and `js` for JS sampling.
fn prof_options(argv: &[&str]) -> Result<crate::profiler::Options, String> {
    let mut options = crate::profiler::Options::default();
    for arg in argv {
        match arg.to_ascii_lowercase().as_str() {
            "js" | "--js" => options.js = true,
            n => match n.parse::<f64>() {
                Ok(secs) if secs > 0.0 && secs <= 24.0 * 3600.0 => {
                    options.duration = Some(std::time::Duration::from_secs_f64(secs))
                }
                _ => return Err(PROF_USAGE.to_string()),
            },
        }
    }
    Ok(options)
}

fn list() -> Vec<String> {
    let plugins = crate::loader::plugin_list();
    let mut lines = vec![format!("[S2S] Plugins ({}):", plugins.len())];
    if plugins.is_empty() {
        lines.push(format!("[S2S] {}", crate::loader::empty_reason(&crate::loader::progress())));
    }
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
    fn server_sm_shows_version_commands_and_credits() {
        let lines = run("");
        assert_eq!(lines[0], version_line());
        assert!(lines[1].contains("sm plugins"), "{lines:?}");
        assert_eq!(run("version")[0], version_line());
        assert_eq!(run("CREDITS"), credits());
    }

    #[test]
    fn unknown_subcommand_is_named() {
        assert_eq!(run("frobnicate"), vec!["[S2S] Unknown sub-command 'frobnicate'. Try: sm plugins list"]);
    }

    /// An empty list says why instead of a bare "Plugins (0)".
    #[test]
    fn an_empty_list_says_why() {
        crate::loader::shutdown_worker();
        let lines = list();
        assert_eq!(lines[0], "[S2S] Plugins (0):");
        assert!(lines[1].contains("plugin loader is not running"), "{lines:?}");
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
    fn prof_arguments_parse_in_any_order() {
        let o = prof_options(&["60", "js"]).unwrap();
        assert_eq!((o.duration, o.js), (Some(std::time::Duration::from_secs(60)), true));
        let o = prof_options(&["--JS", "0.5"]).unwrap();
        assert_eq!((o.duration, o.js), (Some(std::time::Duration::from_millis(500)), true));
        assert_eq!(prof_options(&[]).unwrap(), crate::profiler::Options::default());
        for bad in [&["0"][..], &["-5"], &["forever"], &["90000"]] {
            assert_eq!(prof_options(bad), Err(PROF_USAGE.to_string()), "{bad:?}");
        }
        assert_eq!(prof(&["bogus"]), vec![PROF_USAGE.to_string()]);
        assert_eq!(prof(&["dump", "x"]), vec![PROF_USAGE.to_string()]);
        assert_eq!(run("prof"), vec![PROF_USAGE.to_string()]);
    }

    /// A player can never start the profiler: their `sm prof` is the version banner.
    #[test]
    fn a_player_cannot_profile() {
        assert_eq!(client_lines("prof start"), banner());
    }

    /// SourceMod's client `sm`: `plugins` lists, `credits` credits, anything else the banner.
    /// Nothing a player types reaches an action, admin or not.
    #[test]
    fn a_player_gets_what_sourcemod_gives_one() {
        assert_eq!(client_lines(""), banner());
        assert_eq!(client_lines("version"), banner());
        assert_eq!(client_lines("CREDITS"), credits());
        for args in ["plugins install @evil/x", "plugins unload @s2script/basecommands", "plugins reload x"] {
            let lines = client_lines(args);
            assert!(lines[0].starts_with("Listing "), "{args}: {lines:?}");
        }
        for args in ["frobnicate", "exts", "plugins_install"] {
            assert_eq!(client_lines(args), banner(), "{args}");
        }
        let banner = banner();
        assert!(banner.iter().any(|l| l == "To see running plugins, type \"sm plugins\""));
        assert!(!banner.iter().chain(credits().iter()).any(|l| l.contains("rcon")), "no admin call-out");
    }
}
