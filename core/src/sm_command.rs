//! `sm` — the runtime's own root console command, as in SourceMod's core.
//!
//! The runtime registers it at startup, so it exists with no plugin loaded and no plugin can
//! shadow it (`__s2_concommand` refuses the name). It is a server command:
//!
//! - the server console and RCON get everything: `sm version`, `sm plugins list|info|load|
//!   unload|reload`, and the registry operations `sm plugins install|update|remove|outdated|enable|
//!   disable`;
//! - a player who types `sm` in their own console gets what SourceMod gives one (its
//!   `PlayerManager::OnClientCommand`): `sm plugins` lists the running plugins, `sm exts` the
//!   running extensions, `sm credits` the credits, anything else the version banner. Nothing that changes the server, admins included;
//!   an in-game admin runs the rest through `sm_rcon sm …`.
//!
//! The command is registered client-executable so a player's `sm` reaches here at all, and the
//! ClientCommand hook matches it by name too (`commands::dispatch_client_command`), since it is
//! not in the plugin command table. The split between the two is decided by the caller slot, not
//! by hiding the command.

use crate::v8host::{engine_ops, log_warn};
use std::ffi::CString;

pub(crate) const NAME: &str = "sm";

const USAGE: &str = "Usage: sm plugins <list|info|load|unload|reload> [id] | info <#> | load <folder>/<id> \
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
    format!("s2script {}, by Gabriel Hirakawa", crate::VERSION)
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
    if out == Out::Server {
        finish_without_frames(queued_work(args));
    }
}

/// What a server-console `sm` leaves for the frame-driven loader to finish.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Queued {
    Nothing,
    /// `sm plugins load|unload|reload`: a loader request.
    Loader,
    /// A registry operation: its thread's report, then the files it swapped in to reload.
    Registry,
}

fn queued_work(args: &str) -> Queued {
    let argv: Vec<String> = args.split_whitespace().take(2).map(str::to_ascii_lowercase).collect();
    match argv.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["plugins", "load" | "unload" | "reload"] => Queued::Loader,
        ["plugins", "install" | "update" | "remove" | "outdated" | "enable" | "disable"] => Queued::Registry,
        _ => Queued::Nothing,
    }
}

/// How long a console command on a hibernating server waits for its registry operation. Each
/// registry request has its own 30 s timeout; this only bounds how long the console is held.
const REGISTRY_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// On a server that runs frames, everything `sm` queued finishes by itself within a second. An
/// empty server hibernates and runs none, so finish it here: wait for the registry operation, then
/// reload what it changed and print its report. Nobody is connected to notice the console being
/// held meanwhile.
fn finish_without_frames(queued: Queued) {
    if queued == Queued::Nothing || crate::loader::frames_running() {
        return;
    }
    if queued == Queued::Registry {
        let started = std::time::Instant::now();
        while crate::server_update::console_op_active() {
            if started.elapsed() >= REGISTRY_WAIT {
                Out::Server.line(&format!(
                    "[S2S] Still working after {}s; the report follows once the server runs frames.",
                    REGISTRY_WAIT.as_secs()
                ));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    // The report first, so it reads before the reload lines the loader logs.
    drain_reports();
    crate::loader::settle_without_frames();
    drain_reports();
}

/// A player's `sm`, admin or not, as SourceMod answers one: the running plugins, the running
/// extensions, the credits, or the version banner. Read-only; nothing here changes the server, so
/// `sm plugins info|load|unload|reload|update…` stay on the server console / RCON.
fn client_lines(args: &str) -> Vec<String> {
    match args.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str() {
        "plugins" => {
            let running: Vec<String> = crate::loader::plugin_list()
                .into_iter()
                .filter(|(_, state)| state == "running")
                .map(|(id, _)| id)
                .collect();
            client_plugin_lines(&running, crate::loader::plugin_meta)
        }
        "exts" => crate::extensions::client_lines(),
        "credits" => credits(),
        _ => banner(),
    }
}

/// A player's `sm plugins`: one ` "id" (version) by author` line per running plugin.
fn client_plugin_lines(
    running: &[String],
    meta_of: impl Fn(&str) -> Option<crate::loader::PluginMeta>,
) -> Vec<String> {
    if running.is_empty() {
        return vec!["[S2S] No plugins found.".to_string()];
    }
    running.iter().map(|id| format!(" {}", identity(id, meta_of(id).as_ref()))).collect()
}

/// The server console's `sm`: the lines to print for `args`.
fn run(args: &str) -> Vec<String> {
    let argv: Vec<&str> = args.split_whitespace().collect();
    let sub = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match sub.as_str() {
        "" => {
            let mut lines = vec![version_line()];
            lines.push("Commands: sm version | sm credits | sm plugins … | sm ext … | sm prof …".to_string());
            lines
        }
        "version" => vec![version_line(), "Visit https://www.s2script.com/".to_string()],
        "credits" => credits(),
        "plugins" => plugins(&argv[1..]),
        "prof" => prof(&argv[1..]),
        "ext" | "exts" | "extensions" => ext(&argv[1..]),
        other => vec![format!("[S2S] Unknown sub-command '{other}'. Try: sm plugins list")],
    }
}

fn plugins(argv: &[&str]) -> Vec<String> {
    let action = argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    match action.as_str() {
        "" | "list" => list(),
        "info" => match argv.get(1) {
            Some(target) => info(target),
            None => vec!["Usage: sm plugins info <#|id>".to_string()],
        },
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

const EXT_USAGE: &str = "Usage: sm ext list | types <name>";

/// `sm ext …`: native extensions (see `extensions`). Server console / RCON only.
fn ext(argv: &[&str]) -> Vec<String> {
    match argv.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default().as_str() {
        "" | "list" => crate::extensions::list_lines(),
        "types" => match argv.get(1) {
            Some(name) => match crate::extensions::types(name) {
                Some(dts) => dts.lines().map(str::to_string).collect(),
                None => vec![format!("[S2S] No loaded extension named {name}")],
            },
            None => vec![EXT_USAGE.to_string()],
        },
        _ => vec![EXT_USAGE.to_string()],
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

/// `"<id>" (<version>) by <author>`: how SourceMod names a plugin in its lists. The version and
/// author are left out when the manifest has none (an `.s2sp` built before they were carried).
fn identity(id: &str, meta: Option<&crate::loader::PluginMeta>) -> String {
    let mut s = format!("\"{id}\"");
    if let Some(meta) = meta {
        if !meta.version.is_empty() {
            s.push_str(&format!(" ({})", meta.version));
        }
        if let Some(author) = &meta.author {
            s.push_str(&format!(" by {author}"));
        }
    }
    s
}

/// The lifecycle state as `sm plugins list` / `info` print it.
fn status_text(state: &str) -> &'static str {
    match state {
        "running" => "Running",
        "loading" => "Loading",
        "waiting" => "Waiting",
        "failed" => "Failed",
        "unloaded" => "Unloaded",
        _ => "Unknown",
    }
}

fn list() -> Vec<String> {
    let plugins = crate::loader::plugin_list();
    let rows: Vec<(String, String, Option<crate::loader::PluginMeta>, Option<String>)> = plugins
        .into_iter()
        .map(|(id, state)| {
            let meta = crate::loader::plugin_meta(&id);
            let folder = crate::loader::session_folder(&id);
            (id, state, meta, folder)
        })
        .collect();
    list_lines(&rows, |id| crate::v8host::failed_reason(id))
}

/// `sm plugins list`, as SourceMod prints it: one numbered line per plugin with its version and
/// author, a `<State>` tag on any plugin that is not running, then the reason for each failure.
fn list_lines(
    rows: &[(String, String, Option<crate::loader::PluginMeta>, Option<String>)],
    failed_reason: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut lines = vec![format!("[S2S] Listing {} plugin{}:", rows.len(), if rows.len() == 1 { "" } else { "s" })];
    if rows.is_empty() {
        lines.push(format!("[S2S] {}", crate::loader::empty_reason(&crate::loader::progress())));
    }
    let mut errors = Vec::new();
    for (i, (id, state, meta, folder)) in rows.iter().enumerate() {
        let tag = if state == "running" { String::new() } else { format!("<{}> ", status_text(state)) };
        let mut line = format!("  {:02} {tag}{}", i + 1, identity(id, meta.as_ref()));
        if let Some(folder) = folder {
            line.push_str(&format!(" [from plugins/{folder}/, this run only]"));
        }
        lines.push(line);
        if state == "failed" {
            errors.push(format!("  {id}: {}", failed_reason(id).unwrap_or_else(|| "unknown error".to_string())));
        }
    }
    if !errors.is_empty() {
        lines.push("Errors:".to_string());
        lines.extend(errors);
    }
    lines
}

/// `sm plugins info <#|id>`: a plugin's file, title, author, version, URL and status, as
/// SourceMod prints them. `#` is the plugin's number in `sm plugins list`.
fn info(target: &str) -> Vec<String> {
    let plugins = crate::loader::plugin_list();
    let Some((id, state)) = find_plugin(&plugins, target, crate::loader::plugin_file) else {
        return vec![format!("[S2S] Plugin {target} is not loaded.")];
    };
    info_lines(
        &id,
        &state,
        crate::loader::plugin_meta(&id).as_ref(),
        crate::loader::plugin_file(&id).as_deref(),
        crate::loader::session_folder(&id).as_deref(),
        crate::v8host::failed_reason(&id).as_deref(),
    )
}

/// The list entry `target` names: its number in `sm plugins list`, its id, `<folder>/<id>`, or
/// its file name under plugins/.
fn find_plugin(
    plugins: &[(String, String)],
    target: &str,
    file_of: impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    if let Ok(n) = target.parse::<usize>() {
        return n.checked_sub(1).and_then(|i| plugins.get(i)).cloned();
    }
    let by_id = |id: &str| plugins.iter().find(|(p, _)| p == id).cloned();
    by_id(target)
        .or_else(|| crate::loader::split_folder_spec(target).and_then(|(_, id)| by_id(&id)))
        .or_else(|| {
            plugins.iter().find(|(p, _)| {
                file_of(p).is_some_and(|f| f == target || f.rsplit('/').next() == Some(target))
            }).cloned()
        })
}

fn info_lines(
    id: &str,
    state: &str,
    meta: Option<&crate::loader::PluginMeta>,
    file: Option<&str>,
    folder: Option<&str>,
    failed_reason: Option<&str>,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(file) = file {
        lines.push(format!("  Filename: {file}"));
    }
    let description = meta.and_then(|m| m.description.as_deref());
    lines.push(match description {
        Some(d) => format!("  Title: {id} ({d})"),
        None => format!("  Title: {id}"),
    });
    if let Some(author) = meta.and_then(|m| m.author.as_deref()) {
        lines.push(format!("  Author: {author}"));
    }
    if let Some(version) = meta.map(|m| m.version.as_str()).filter(|v| !v.is_empty()) {
        lines.push(format!("  Version: {version}"));
    }
    if let Some(url) = meta.and_then(|m| m.url.as_deref()) {
        lines.push(format!("  URL: {url}"));
    }
    lines.push(format!("  Status: {}", status_text(state)));
    if state == "failed" {
        lines.push(format!("  Error: {}", failed_reason.unwrap_or("unknown error")));
    }
    if let Some(folder) = folder {
        lines.push(format!("  Loaded from plugins/{folder}/ for this run only"));
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

    /// The server-console commands whose work the frame-driven loader finishes, which a
    /// hibernating server has to finish in the command itself.
    #[test]
    fn queued_work_names_what_waits_for_a_frame() {
        assert_eq!(queued_work("plugins update"), Queued::Registry);
        assert_eq!(queued_work("PLUGINS Install @edge/rtv"), Queued::Registry);
        assert_eq!(queued_work("plugins outdated"), Queued::Registry);
        assert_eq!(queued_work("plugins reload rtv"), Queued::Loader);
        assert_eq!(queued_work("plugins list"), Queued::Nothing);
        assert_eq!(queued_work("plugins"), Queued::Nothing);
        assert_eq!(queued_work("version"), Queued::Nothing);
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
        assert_eq!(lines[0], "[S2S] Listing 0 plugins:");
        assert!(lines[1].contains("plugin loader is not running"), "{lines:?}");
    }

    fn meta(version: &str, author: Option<&str>, description: Option<&str>) -> crate::loader::PluginMeta {
        crate::loader::PluginMeta {
            version: version.into(),
            author: author.map(Into::into),
            description: description.map(Into::into),
            url: Some("https://www.s2script.com/".into()),
        }
    }

    /// SourceMod's list: `NN "name" (version) by author`, a `<State>` tag on anything not
    /// running, and each failure's reason after the list.
    #[test]
    fn the_list_shows_versions_like_sourcemod() {
        let rows = vec![
            ("@s2script/basecommands".to_string(), "running".to_string(), Some(meta("0.15.0", Some("s2script"), None)), None),
            ("@edge/jb".to_string(), "failed".to_string(), Some(meta("2.1.0", None, None)), None),
            ("@edge/old".to_string(), "unloaded".to_string(), None, None),
            ("@edge/rtv".to_string(), "running".to_string(), Some(meta("1.0.0", Some("edge"), None)), Some("disabled".to_string())),
        ];
        let lines = list_lines(&rows, |id| (id == "@edge/jb").then(|| "OnPluginStart threw: boom".to_string()));
        assert_eq!(
            lines,
            vec![
                "[S2S] Listing 4 plugins:",
                "  01 \"@s2script/basecommands\" (0.15.0) by s2script",
                "  02 <Failed> \"@edge/jb\" (2.1.0)",
                "  03 <Unloaded> \"@edge/old\"",
                "  04 \"@edge/rtv\" (1.0.0) by edge [from plugins/disabled/, this run only]",
                "Errors:",
                "  @edge/jb: OnPluginStart threw: boom",
            ]
        );
        let one = list_lines(&rows[..1], |_| None);
        assert_eq!(one[0], "[S2S] Listing 1 plugin:");
    }

    #[test]
    fn info_prints_what_sourcemod_prints() {
        let m = meta("0.15.0", Some("s2script"), Some("Basic admin commands"));
        assert_eq!(
            info_lines("@s2script/basecommands", "running", Some(&m), Some("_s2script_basecommands.s2sp"), None, None),
            vec![
                "  Filename: _s2script_basecommands.s2sp",
                "  Title: @s2script/basecommands (Basic admin commands)",
                "  Author: s2script",
                "  Version: 0.15.0",
                "  URL: https://www.s2script.com/",
                "  Status: Running",
            ]
        );
        let bare = crate::loader::PluginMeta { version: "1.0.0".into(), ..Default::default() };
        assert_eq!(
            info_lines("@edge/jb", "failed", Some(&bare), None, Some("disabled"), Some("boom")),
            vec![
                "  Title: @edge/jb",
                "  Version: 1.0.0",
                "  Status: Failed",
                "  Error: boom",
                "  Loaded from plugins/disabled/ for this run only",
            ]
        );
    }

    #[test]
    fn info_finds_a_plugin_by_number_id_folder_or_file() {
        let plugins = vec![
            ("@a/one".to_string(), "running".to_string()),
            ("@b/two".to_string(), "failed".to_string()),
        ];
        let file_of = |id: &str| Some(format!("disabled/{}.s2sp", id.replace(['@', '/'], "_")));
        let find = |t: &str| find_plugin(&plugins, t, file_of).map(|(id, _)| id);
        assert_eq!(find("1").as_deref(), Some("@a/one"));
        assert_eq!(find("2").as_deref(), Some("@b/two"));
        assert_eq!(find("0"), None);
        assert_eq!(find("3"), None);
        assert_eq!(find("@b/two").as_deref(), Some("@b/two"));
        assert_eq!(find("disabled/@b/two").as_deref(), Some("@b/two"));
        assert_eq!(find("_a_one.s2sp").as_deref(), Some("@a/one"));
        assert_eq!(find("disabled/_a_one.s2sp").as_deref(), Some("@a/one"));
        assert_eq!(find("@c/three"), None);
        assert_eq!(plugins_info_usage(), vec!["Usage: sm plugins info <#|id>".to_string()]);
        assert_eq!(info("@nope/x"), vec!["[S2S] Plugin @nope/x is not loaded.".to_string()]);
    }

    fn plugins_info_usage() -> Vec<String> {
        plugins(&["info"])
    }

    /// A player's `sm plugins` names each running plugin's version and author too.
    #[test]
    fn identity_names_version_and_author() {
        assert_eq!(identity("@a/one", None), "\"@a/one\"");
        assert_eq!(identity("@a/one", Some(&meta("1.2.3", Some("A"), None))), "\"@a/one\" (1.2.3) by A");
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

    /// A player's plugin list reads like SourceMod's: no numbers, no state tags, no errors.
    #[test]
    fn a_players_plugin_list_names_version_and_author() {
        let running = vec!["@s2script/basecommands".to_string(), "old".to_string()];
        let lines = client_plugin_lines(&running, |id| {
            (id == "@s2script/basecommands").then(|| meta("1.2.0", Some("s2script"), None))
        });
        assert_eq!(lines, vec![" \"@s2script/basecommands\" (1.2.0) by s2script".to_string(), " \"old\"".to_string()]);
        assert_eq!(client_plugin_lines(&[], |_| None), vec!["[S2S] No plugins found.".to_string()]);
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
        let list = client_lines("plugins");
        for args in ["plugins install @evil/x", "plugins unload @s2script/basecommands", "plugins reload x", "plugins info 1", "plugins 11"] {
            assert_eq!(client_lines(args), list, "{args}");
        }
        assert_eq!(client_lines("exts"), crate::extensions::client_lines());
        for args in ["frobnicate", "ext list", "plugins_install"] {
            assert_eq!(client_lines(args), banner(), "{args}");
        }
        let banner = banner();
        assert!(banner.iter().any(|l| l == "To see running plugins, type \"sm plugins\""));
        assert!(!banner.iter().chain(credits().iter()).any(|l| l.contains("rcon")), "no admin call-out");
    }
}
