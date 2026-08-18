//! Commands: the ConCommand registry, client-command listeners, chat-trigger dispatch,
//! unmatched-say fan-out, and this feature's teardown.
//!
//! One inbound path. A line or a ConCommand arrives from server console, client console, or a
//! `!`/`/` chat trigger; `ReplySource` names the channel. A matching ConCommand **supersedes**.
//! A client-command listener **observes** unless it returns `>= Handled`. An unmatched say line
//! fans out to `ctx.clients.onSay` subscribers; `Handled` consumes the line once (the documented
//! exception — a menu pick must not also cast a vote).
//!
//! Engine-generic: names and slots are the caller's strings and integers. Nothing here knows a
//! specific game.
//!
//! Extracted from `v8host.rs` under the core-stabilization program — see `crate::usermsg` for the
//! shape (a feature owns its state, natives, dispatch and teardown together).
//!
//! # What deliberately did NOT move
//! Outbound `Chat.toSlot`/`toAll` (client print). `Server.command`. TopMenu. Plugin list/load.
//! `Chat.color`. `S2EngineOps` stays whole in `v8host.rs`.

use crate::dispatch::{fan_out, fan_out_collapsing, Instrument, StopAt};
use crate::multiplexer::HookResult;
use crate::v8host::{
    current_plugin, engine_ops, log_warn, plugin_generation, set_native, subscribe_into,
};
use std::ffi::CString;

thread_local! {
    /// `name → (owner, generation, Global<Function>)` map for registered ConCommands. Owner-tracked
    /// so `dispatch_concommand` runs the handler in the REGISTERING plugin's context (liveness-gated)
    /// and unload can drop commands owned by the departing plugin. The shim calls back via
    /// `s2script_core_dispatch_concommand` when a registered command fires.
    static CONCOMMANDS: std::cell::RefCell<std::collections::HashMap<String, (String, u64, v8::Global<v8::Function>)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// `name → flags` sidecar for registered ConCommands (backing `sm_help` / `Commands.list()`).
    /// `flags` encodes the required admin bit mask: `0` = anyone, `-1` = console/server-only sentinel,
    /// else the `ADMFLAG` bit mask (`registerAdmin`). Pure `i64` — no V8 handles. `__s2_commands_list`
    /// joins on live `CONCOMMANDS` keys (a stale meta entry is ignored).
    static COMMAND_META: std::cell::RefCell<std::collections::HashMap<String, i64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Client-command listener mux: `Commands.onClientCommand(name, h)` subscribers, keyed by the
    /// COMMAND NAME. SourceMod `AddCommandListener` — exists because `CONCOMMANDS` cannot serve an
    /// engine-owned name (`player_ping`, `jointeam`, `drop`). Semantics invert `CONCOMMANDS`: a
    /// matching ConCommand supersedes; a listener observes and passes through unless it returns
    /// `>= HookResult.Handled`.
    static CLIENT_CMD_SUBS: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
    /// Unmatched-say subscriber mux: `ctx.clients.onSay(h)` subscribers, keyed by the constant "".
    /// Handlers receive `(slot, text, teamonly)` and may return a HookResult (`>= Handled` suppresses
    /// the broadcast). The Host_Say detour is always installed, so there is no per-subscribe engine-op.
    static CHAT_MSG_SUBS: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
}

/// Where a command was invoked from — SM's reply source. Decides where `ctx.reply` lands.
///
/// Crosses into JS as the command wrapper's 3rd argument (a plain number) and is mapped back to a
/// string in `__s2cmd_ctx`. Engine-generic: it names invocation channels, never a game concept.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub(crate) enum ReplySource {
    /// The server console or rcon (caller slot -1). Replies go to the server console.
    Server = 0,
    /// A player's own developer console — the `ISource2GameClients::ClientCommand` hook.
    Console = 1,
    /// A `!`/`/` chat trigger — the `Host_Say` detour.
    Chat = 2,
}

impl ReplySource {
    /// Derive the source for the shared ConCommand trampoline, which carries only a slot: `-1` is
    /// the server console/rcon; a player slot means a client-run ConCommand that the `ClientCommand`
    /// hook did not already SUPERCEDE, which is still that player's console — never their chat.
    pub(crate) fn from_slot(slot: i32) -> Self {
        if slot < 0 { ReplySource::Server } else { ReplySource::Console }
    }
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// Native `__s2_concommand(name: string, fn: (slot: number, argString: string) => void, flags?: number)`.
/// Stores the JS callback keyed by command name in `CONCOMMANDS`, records the optional flags in
/// `COMMAND_META`, and registers the raw ConCommand engine-side via the shim's ops table.
fn s2_concommand(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 {
            return;
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(1)) {
            Ok(f) => f,
            Err(_) => return,
        };
        let global = v8::Global::new(scope.as_ref(), func_local);

        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = plugin_generation(&owner);
        CONCOMMANDS.with(|m| m.borrow_mut().insert(name.clone(), (owner, generation, global)));

        let flags = if args.length() >= 3 { args.get(2).integer_value(scope).unwrap_or(0) } else { 0 };
        COMMAND_META.with(|m| m.borrow_mut().insert(name.clone(), flags));

        let Some(ops) = engine_ops() else {
            log_warn("WARN: __s2_concommand: no engine ops table");
            return;
        };
        let Some(func) = ops.concommand_register else {
            log_warn("WARN: __s2_concommand: concommand_register not wired in ops");
            return;
        };
        let Ok(cname) = CString::new(name.as_str()) else { return };
        func(cname.as_ptr());
    }));
}

/// `__s2_chat_on_message(handler)` — subscribe a JS fn to unmatched player say. Owner-tracked;
/// the Host_Say detour is installed at Load, so no per-subscribe engine registration is needed.
/// The public authoring seam is `ctx.clients.onSay`; this native is what that (and the prelude
/// menu/votes helpers) call.
fn s2_chat_on_message(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &CHAT_MSG_SUBS, "", 0) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_client_command_listen(name, handler)` — subscribe a JS fn to a CLIENT COMMAND by name
/// (the SourceMod `AddCommandListener` seam). The handler receives `(slot, argString)` and may
/// return a HookResult; `>= Handled` suppresses the engine's own handling.
fn s2_client_command_listen(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        if name.is_empty() { return; }
        let Some((sub_id, _first)) = subscribe_into(scope, &args, &CLIENT_CMD_SUBS, &name, 1) else { return };
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// `__s2_commands_list() -> string` — JSON array of `{name, flags}` for `Commands.list()` / `sm_help`.
/// Joins on live `CONCOMMANDS` keys (a stale `COMMAND_META` entry is ignored); `flags` defaults to 0.
fn s2_commands_list(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let items: Vec<serde_json::Value> = CONCOMMANDS.with(|c| {
            COMMAND_META.with(|meta| {
                let meta = meta.borrow();
                c.borrow().keys()
                    .map(|name| serde_json::json!({
                        "name": name,
                        "flags": meta.get(name).copied().unwrap_or(0),
                    }))
                    .collect()
            })
        });
        let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
        if let Some(js) = v8::String::new(scope, &json) { rv.set(js.into()); }
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_concommand", s2_concommand);
    set_native(scope, global_obj, "__s2_chat_on_message", s2_chat_on_message);
    set_native(scope, global_obj, "__s2_client_command_listen", s2_client_command_listen);
    set_native(scope, global_obj, "__s2_commands_list", s2_commands_list);
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a ConCommand callback to the registered JS function.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_concommand`. Routes through `fan_out` so the
/// isolate stay inside the host: snapshot the one handler, then the shared preamble owns
/// re-entrancy, liveness, TryCatch, and the crash breadcrumb.
pub(crate) fn dispatch_concommand(name: &str, slot: i32, args: &str, src: ReplySource) {
    let snap = CONCOMMANDS.with(|m| {
        m.borrow()
            .get(name)
            .map(|(o, g, f)| vec![(o.clone(), *g, f.clone())])
            .unwrap_or_default()
    });
    if snap.is_empty() { return; }
    let tag = format!("command:{}", name);
    let _ = fan_out(&snap, &tag, Instrument::full(&tag), |tc| {
        let slot_val: v8::Local<v8::Value> = v8::Number::new(tc, slot as f64).into();
        let Some(args_str) = v8::String::new(tc, args) else { return None };
        let src_val: v8::Local<v8::Value> = v8::Integer::new(tc, src as i32).into();
        Some(vec![slot_val, args_str.into(), src_val])
    });
}

/// Parse a chat line for a command trigger (`!cmd` / `/cmd`) and dispatch it.
///
/// Called from `ffi.rs`'s `s2script_core_dispatch_chat` (the shim's Host_Say detour). Reuses the
/// ConCommand registry so a chat trigger runs the SAME registered handler as the console command,
/// with `ReplySource::Chat`. SM convention: `!kick` tries `kick`, then falls back to `sm_kick`.
///
/// On a MATCHED command trigger, returns `silent` (suppress iff the trigger was `/`) after
/// dispatching. Otherwise the raw line is delivered to `ctx.clients.onSay` subscribers; a return
/// of `>= HookResult.Handled` (2) suppresses the broadcast.
pub(crate) fn dispatch_chat(slot: i32, text: &str, teamonly: bool) -> bool {
    let (silent, is_trigger) = match text.as_bytes().first() {
        Some(b'!') => (false, true),
        Some(b'/') => (true, true),
        _ => (false, false),
    };
    if is_trigger {
        let rest = text[1..].trim();
        if !rest.is_empty() {
            let (name, args) = match rest.find(char::is_whitespace) {
                Some(i) => (rest[..i].to_string(), rest[i..].trim_start().to_string()),
                None => (rest.to_string(), String::new()),
            };
            let sm_name = format!("sm_{}", name);
            let matched = CONCOMMANDS.with(|m| {
                let map = m.borrow();
                if map.contains_key(&name) { Some(name.clone()) }
                else if map.contains_key(&sm_name) { Some(sm_name) }
                else { None }
            });
            if let Some(cmd) = matched {
                dispatch_concommand(&cmd, slot, &args, ReplySource::Chat);
                return silent;
            }
        }
    }
    dispatch_chat_message(slot, text, teamonly)
}

/// Deliver a raw say line to the `ctx.clients.onSay` subscribers. StopAt::Handled — the documented
/// exception to the standard collapse: a chat line is consumed ONCE (a shop menu, a nominate menu
/// and a live vote must not all see the same "2").
fn dispatch_chat_message(slot: i32, text: &str, teamonly: bool) -> bool {
    let snap = CHAT_MSG_SUBS.with(|m| m.borrow().snapshot(""));
    let result = fan_out_collapsing(
        &snap,
        "dispatch_chat: onSay",
        Instrument::none(),
        StopAt::Handled,
        |tc| {
            Some(vec![
                v8::Integer::new(tc, slot).into(),
                match v8::String::new(tc, text) {
                    Some(s) => s.into(),
                    None => v8::undefined(tc).into(),
                },
                v8::Boolean::new(tc, teamonly).into(),
            ])
        },
    );
    result >= HookResult::Handled
}

/// Dispatch a player's CONSOLE command (from the ClientCommand hook). Match the EXACT registered
/// name only — never an `sm_` fallback (that would hijack a real engine command like `say`).
/// Returns true iff a registered command matched + was dispatched (the caller then SUPERCEDEs).
pub(crate) fn dispatch_client_command(slot: i32, name: &str, args: &str) -> bool {
    // Listeners are NOT run here. They are driven from the shim's `DispatchConCommand` hook.
    let matched = CONCOMMANDS.with(|m| m.borrow().contains_key(name));
    if matched {
        dispatch_concommand(name, slot, args, ReplySource::Console);
        return true;
    }
    false
}

/// Deliver a client command to the `Commands.onClientCommand(name, …)` listeners.
/// StopAt::Stop: listeners all run and their results OR together.
pub(crate) fn dispatch_command_listeners(slot: i32, name: &str, args: &str) -> bool {
    let snap = CLIENT_CMD_SUBS.with(|m| m.borrow().snapshot(name));
    let result = fan_out_collapsing(
        &snap,
        &format!("dispatch_client_command on '{}'", name),
        Instrument::none(),
        StopAt::Stop,
        |tc| {
            Some(vec![
                v8::Integer::new(tc, slot).into(),
                match v8::String::new(tc, args) {
                    Some(s) => s.into(),
                    None => v8::undefined(tc).into(),
                },
            ])
        },
    );
    result >= HookResult::Handled
}

// ---------------------------------------------------------------------------
// Teardown
// ---------------------------------------------------------------------------

/// Owner-scoped stores. Host_Say / ClientCommand hooks stay installed for the process lifetime.
pub(crate) fn register_stores() {
    crate::owner_stores::register(
        "CHAT_MSG_SUBS",
        Box::new(|owner| { CHAT_MSG_SUBS.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { CHAT_MSG_SUBS.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            CHAT_MSG_SUBS.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
    crate::owner_stores::register(
        "CLIENT_CMD_SUBS",
        Box::new(|owner| { CLIENT_CMD_SUBS.with(|m| m.borrow_mut().remove_by_owner(owner)); }),
        Box::new(|ids| { CLIENT_CMD_SUBS.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| {
            CLIENT_CMD_SUBS.with(|m| *m.borrow_mut() = crate::channels::Channels::new());
        }),
    );
    crate::owner_stores::register(
        "CONCOMMANDS",
        Box::new(|owner| {
            let dropped_cmds: Vec<String> = CONCOMMANDS.with(|m| {
                let mut b = m.borrow_mut();
                let names: Vec<String> = b.iter().filter(|(_, (o, _, _))| o == owner).map(|(n, _)| n.clone()).collect();
                b.retain(|_, (o, _, _)| o != owner);
                names
            });
            COMMAND_META.with(|m| { let mut b = m.borrow_mut(); for n in &dropped_cmds { b.remove(n); } });
        }),
        Box::new(|_ids| {}),
        Box::new(|| {
            CONCOMMANDS.with(|m| m.borrow_mut().clear());
            COMMAND_META.with(|m| m.borrow_mut().clear());
        }),
    );
}

// ---------------------------------------------------------------------------
// Tests
//
// These drive the SHARED in-isolate harness (`v8host::frame_tests`). The harness stays
// in `v8host`; only the per-feature assertions live here.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{
        dummy_logger, eval_in_context_string, load_body, logger, LOG,
    };
    use crate::v8host::{
        create_plugin_context, eval_in_context, frame_async_drain, init, shutdown, unload_plugin,
    };

    /// `__s2_concommand` stores the JS callback in CONCOMMANDS; `dispatch_concommand` invokes it
    /// with (slot, argString) in the REGISTERING PLUGIN'S context (owner-tracked, liveness-gated).
    /// This test exercises the store + dispatch path without the engine
    /// (calls `dispatch_concommand` directly, bypassing ConCommand registration).
    #[test]
    fn concommand_callback_receives_slot_and_args() {
        init(dummy_logger()).unwrap();
        // Register the raw native from a PLUGIN context (dispatch is now owner-tracked; registering
        // from the shared HOST context would produce owner="legacy" with no REGISTRY entry → skipped).
        load_body("cc_test", r#"
            globalThis.__cc = null;
            __s2_concommand("s2_test", function (slot, args) { globalThis.__cc = slot + ":" + args; });
        "#, "{}");
        // Simulate the engine invoking the command (bypasses ConCommand registration):
        dispatch_concommand("s2_test", 3, "1234", ReplySource::from_slot(3));
        assert_eq!(eval_in_context_string("cc_test", "String(globalThis.__cc)"), "3:1234");
        shutdown();
    }

    /// `__s2_commands_list` returns valid JSON: `[]` when no commands are registered, and a
    /// `[{name, flags}]` entry (with the flags passed to `__s2_concommand`) once one is registered.
    /// Mirrors the plugins-list style; exercises the store + list join without the engine.
    #[test]
    fn commands_list_returns_name_and_flags() {
        init(dummy_logger()).unwrap();
        // Load a plugin whose body: (1) confirms the list is empty BEFORE any registration, then
        // (2) registers two commands with distinct flag masks (2nd arg is the callback, 3rd is the flags).
        load_body("cl_test", r#"
            globalThis.__cl_empty = __s2_commands_list();               // must be "[]" — nothing registered yet
            __s2_concommand("s2_open", function () {}, 0);              // 0 = anyone
            __s2_concommand("s2_admin", function () {}, 6);             // an ADMFLAG bit mask
            var list = JSON.parse(__s2_commands_list());
            var byName = {};
            for (var i = 0; i < list.length; i++) { byName[list[i].name] = list[i].flags; }
            globalThis.__cl = list.length + "|" + byName["s2_open"] + "|" + byName["s2_admin"];
        "#, "{}");
        // Empty (valid JSON) before registration, then both commands surface with their flags.
        assert_eq!(eval_in_context_string("cl_test", "String(globalThis.__cl_empty)"), "[]");
        assert_eq!(eval_in_context_string("cl_test", "String(globalThis.__cl)"), "2|0|6");
        // Native still returns valid JSON directly.
        assert_eq!(eval_in_context_string("cl_test", "typeof __s2_commands_list()"), "string");
        shutdown();
    }

    /// `Commands.register` builds a typed ctx (callerSlot/args/argString/reply); reply routes to
    /// console.log for slot<0, to Chat.toSlot for slot>=0.  Unload drops the command → later
    /// dispatch is a no-op.  A throwing handler is caught (no panic).
    ///
    /// Slice 6.1 Task 2.  Calls `dispatch_concommand` directly (simulates the engine trampoline).
    #[test]
    fn command_dispatch_builds_ctx_and_routes_reply() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // A plugin registers sm_test; capture the ctx it receives.
        load_body("cmd", r#"
            var C = __s2pkg_commands.Commands;
            C.register("sm_test", function (ctx) {
                globalThis.__seen = ctx.callerSlot + "|" + ctx.args.join(",") + "|" + ctx.argString;
                // SM-parity arg API (Slice 6.10): arg/argInt/argFloat/argsFrom/argCount.
                globalThis.__argapi = [ctx.argCount, ctx.arg(0), ctx.argInt(1), ctx.argFloat(1),
                                        ctx.argsFrom(2), ctx.arg(99), ctx.argInt(99, 7)].join("|");
                if (ctx.callerSlot < 0) ctx.reply("console-reply");   // routes to console.log
            });
        "#, "{}");
        // Simulate the engine firing the command from the server console (slot -1).
        dispatch_concommand("sm_test", -1, "foo bar", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("cmd", "String(globalThis.__seen)"), "-1|foo,bar|foo bar");
        // The arg API: dispatch "target 42 hello world" and verify typed retrieval.
        dispatch_concommand("sm_test", -1, "target 42 hello world", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("cmd", "String(globalThis.__argapi)"),
                   "4|target|42|42|hello world||7",
                   "argCount|arg(0)|argInt(1)|argFloat(1)|argsFrom(2)|arg(99)=''|argInt(99,7)=7");
        assert!(LOG.lock().unwrap().iter().any(|m| m.contains("console-reply")), "console reply routed to log");
        // A throwing handler is caught (no panic).
        load_body("cmd2", r#" __s2pkg_commands.Commands.register("sm_boom", function(){ throw new Error("x"); }); "#, "{}");
        dispatch_concommand("sm_boom", -1, "", ReplySource::from_slot(-1));   // must not panic
        // Unload drops the command → a later dispatch is a no-op.
        unload_plugin("cmd");
        eval_in_context("cmd2", "globalThis.__afterUnload = 'unchanged';").unwrap();
        dispatch_concommand("sm_test", -1, "again", ReplySource::from_slot(-1));   // cmd is gone → no handler → no-op
        shutdown();
    }

    /// Command reply source, PR 1: the explicit reply targets. `replyToConsole` prints to the
    /// CALLER'S developer console with every C0 control byte stripped (chat colour control bytes
    /// occupy the C0 range on this engine — including \x09, \x0A and \x0D — so the strip takes the
    /// whole range with no tab/newline/carriage-return exemption); `replyToChat` goes to their chat
    /// RAW, one frame later. Both the native and the chat module fn are resolved through
    /// `globalThis` at call time, so the test stubs them as in-isolate spies.
    #[test]
    fn explicit_reply_targets_route_and_strip() {
        init(dummy_logger()).unwrap();
        load_body("rt", r#"
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(slot + "|" + msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            __s2pkg_commands.Commands.register("sm_t", function (ctx) {
                ctx.replyToConsole("\x04a\x09b\x0Ac");
                ctx.replyToChat("\x04a\x09b\x0Ac");
            });
        "#, "{}");
        dispatch_concommand("sm_t", 3, "", ReplySource::from_slot(3));
        // console: immediate, stripped, newline-terminated (matches Client.print).
        assert_eq!(eval_in_context_string("rt", "globalThis.__con.join(';')"), "3|abc\n");
        // chat: deferred one frame — nothing has landed yet.
        assert_eq!(eval_in_context_string("rt", "String(globalThis.__cht.length)"), "0");
        frame_async_drain();
        frame_async_drain();
        // chat: RAW — colour is content the caller owns.
        assert_eq!(eval_in_context_string("rt", "globalThis.__cht.join(';')"), "3|\u{4}a\u{9}b\u{a}c");
        shutdown();
    }

    /// Command reply source, PR 1: at the server console (slot -1) there is no client channel, so
    /// BOTH explicit targets degrade to the server console (`console.log`, captured in `LOG`) with
    /// control bytes stripped, and neither throws.
    #[test]
    fn explicit_reply_targets_degrade_at_slot_minus_one() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("rd", r#"
            __s2pkg_commands.Commands.register("sm_d", function (ctx) {
                ctx.replyToConsole("\x04con-degrade");
                ctx.replyToChat("\x04chat-degrade");
            });
        "#, "{}");
        dispatch_concommand("sm_d", -1, "", ReplySource::from_slot(-1));   // must not throw
        let log = LOG.lock().unwrap().clone();
        assert!(log.iter().any(|l| l.contains("con-degrade")), "replyToConsole at slot -1 → server console");
        assert!(log.iter().any(|l| l.contains("chat-degrade")), "replyToChat at slot -1 → server console");
        assert!(!log.iter().any(|l| l.contains('\u{4}')), "control bytes stripped on both degrade paths");
        shutdown();
    }

    /// Command reply source, PR 1: the ctx reply methods must survive being DETACHED from the ctx
    /// object. A plugin that hands `cmd.reply` to a helper as a bare function reference (a real
    /// pattern in the shipped plugin suite) would otherwise hit an undefined receiver and throw —
    /// and the dispatch wrapper swallows handler throws, so the reply would silently vanish. The
    /// methods close over their context instead of depending on `this`.
    ///
    /// Driven through the JS `Commands.dispatch` rather than the Rust `dispatch_concommand` so the
    /// test survives the later commits that change that signature. `reply`'s destination differs
    /// across those commits (chat before routing lands, the caller's console after), so the
    /// invariant pinned here is delivery, not channel.
    #[test]
    fn reply_methods_survive_being_detached_from_ctx() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("rx", r#"
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(slot + "|" + msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            var C = __s2pkg_commands.Commands;
            C.register("sm_x", function (ctx) {
                var toConsole = ctx.replyToConsole, toChat = ctx.replyToChat, reply = ctx.reply;
                toConsole("detached-console");   // detached, exactly as plugins/disabled/funvotes does
                toChat("detached-chat");
                reply("detached-reply");         // the one that used to depend on `this`
            });
            C.dispatch("sm_x", 5, "");           // a PLAYER caller — the slot the receiver bug bit
            globalThis.__sawAll = function (needle) {
                return (globalThis.__con.join(";") + ";" + globalThis.__cht.join(";")).indexOf(needle) >= 0;
            };
        "#, "{}");
        frame_async_drain();
        frame_async_drain();
        assert_eq!(eval_in_context_string("rx", "String(__sawAll('5|detached-console'))"), "true",
                   "detached replyToConsole delivered");
        assert_eq!(eval_in_context_string("rx", "String(__sawAll('5|detached-chat'))"), "true",
                   "detached replyToChat delivered");
        assert_eq!(eval_in_context_string("rx", "String(__sawAll('detached-reply'))"), "true",
                   "detached reply delivered (channel varies by commit; delivery does not)");
        shutdown();
    }

    /// Command reply source, PR 2: each dispatch entry point stamps its own `ctx.replySource` —
    /// the shared ConCommand trampoline (server console / rcon) → "server", the ClientCommand hook
    /// (a player's own developer console) → "console", the Host_Say chat trigger → "chat".
    /// `Commands.onClientCommand` — the AddCommandListener seam. The listener must see a command
    /// NOBODY registered (the whole point: engine-owned names like `player_ping` cannot be
    /// registered), and must NOT supersede it, so the engine still does its own work.
    #[test]
    fn client_command_listener_observes_without_superseding() {
        init(dummy_logger()).unwrap();
        load_body("ccl", r#"
            globalThis.__seen = [];
            __s2pkg_commands.Commands.onClientCommand("player_ping", function (slot, args) {
                globalThis.__seen.push(slot + "|" + args);
            });
        "#, "{}");
        // No ConCommand named "player_ping" exists — without the listener mux this returns false
        // and the handler never runs.
        let superseded = dispatch_command_listeners(7, "player_ping", "a b");
        assert_eq!(eval_in_context_string("ccl", "globalThis.__seen.join(',')"), "7|a b",
                   "listener saw an unregistered (engine-owned) client command");
        assert!(!superseded, "an observing listener must let the engine handle the command");
    }

    /// A listener returning `>= HookResult.Handled` suppresses the engine's handling.
    #[test]
    fn client_command_listener_can_suppress() {
        init(dummy_logger()).unwrap();
        load_body("ccs", r#"
            __s2pkg_commands.Commands.onClientCommand("drop", function () { return 2; });
        "#, "{}");
        assert!(dispatch_command_listeners(3, "drop", ""), "Handled from a listener supersedes");
        assert!(!dispatch_command_listeners(3, "buy", ""), "an unlistened command is untouched");
    }

    /// A listener and a registered ConCommand of the same name both run, and the command still
    /// supersedes on its own account.
    #[test]
    fn client_command_listener_coexists_with_a_registered_command() {
        init(dummy_logger()).unwrap();
        load_body("ccc", r#"
            globalThis.__order = [];
            __s2pkg_commands.Commands.onClientCommand("sm_both", function () { globalThis.__order.push("listener"); });
            __s2pkg_commands.Commands.register("sm_both", function () { globalThis.__order.push("command"); });
        "#, "{}");
        // The two seams are independent: DispatchConCommand drives listeners, the ConCommand
        // trampoline drives the owning command. Each fires exactly once.
        assert!(!dispatch_command_listeners(1, "sm_both", ""), "an observing listener does not supersede");
        assert!(dispatch_client_command(1, "sm_both", ""), "a registered command still supersedes");
        assert_eq!(eval_in_context_string("ccc", "globalThis.__order.join(',')"), "listener,command",
                   "each seam fired its own handler exactly once");
    }

    #[test]
    fn reply_source_derives_from_entry_point() {
        init(dummy_logger()).unwrap();
        load_body("rs", r#"
            globalThis.__src = "";
            __s2pkg_commands.Commands.register("sm_s", function (ctx) { globalThis.__src = ctx.replySource; });
        "#, "{}");
        dispatch_concommand("sm_s", -1, "", ReplySource::from_slot(-1));
        assert_eq!(eval_in_context_string("rs", "globalThis.__src"), "server");
        dispatch_client_command(4, "sm_s", "");
        assert_eq!(eval_in_context_string("rs", "globalThis.__src"), "console");
        dispatch_chat(4, "!s", false);
        assert_eq!(eval_in_context_string("rs", "globalThis.__src"), "chat");
        // A client-run ConCommand the ClientCommand hook did not SUPERCEDE is still that player's
        // console, never their chat.
        dispatch_concommand("sm_s", 4, "", ReplySource::from_slot(4));
        assert_eq!(eval_in_context_string("rs", "globalThis.__src"), "console");
        shutdown();
    }

    /// Command reply source, PR 2: a `Commands.dispatch` with no source (SM's FakeClientCommand
    /// path) falls back to the slot — the server console at -1, else that player's own console.
    #[test]
    fn reply_source_falls_back_to_slot() {
        init(dummy_logger()).unwrap();
        load_body("rf", r#"
            globalThis.__src = "";
            var C = __s2pkg_commands.Commands;
            C.register("sm_f", function (ctx) { globalThis.__src = ctx.replySource; });
            C.dispatch("sm_f", -1, "");   globalThis.__a = globalThis.__src;
            C.dispatch("sm_f", 4, "");    globalThis.__b = globalThis.__src;
        "#, "{}");
        assert_eq!(eval_in_context_string("rf", "globalThis.__a"), "server");
        assert_eq!(eval_in_context_string("rf", "globalThis.__b"), "console");
        shutdown();
    }

    /// Command reply source, PR 3 (THE FIX): `reply` lands in the channel the caller used — the
    /// server console for "server", the CALLER'S own developer console for "console", their chat
    /// for "chat". Before this, every reply from a player went to chat, so a player who typed
    /// `sm_help` at their console got ten lines of pagination spammed into chat instead.
    #[test]
    fn reply_routes_by_reply_source() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("rr", r#"
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(slot + "|" + msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            __s2pkg_commands.Commands.register("sm_r", function (ctx) { ctx.reply("\x04hi-" + ctx.replySource); });
        "#, "{}");
        // "server" → the server console (console.log → LOG); no client channel is touched.
        dispatch_concommand("sm_r", -1, "", ReplySource::from_slot(-1));
        assert!(LOG.lock().unwrap().iter().any(|l| l.contains("hi-server")), "server source → server console");
        assert!(!LOG.lock().unwrap().iter().any(|l| l.contains('\u{4}')), "server reply strips control bytes");
        assert_eq!(eval_in_context_string("rr", "String(globalThis.__con.length)"), "0");
        // "console" → the caller's own developer console, immediately.
        dispatch_client_command(6, "sm_r", "");
        assert_eq!(eval_in_context_string("rr", "globalThis.__con.join(';')"), "6|hi-console\n");
        // "chat" → their chat, one frame later.
        dispatch_chat(6, "!r", false);
        assert_eq!(eval_in_context_string("rr", "String(globalThis.__cht.length)"), "0", "chat reply is deferred");
        frame_async_drain();
        frame_async_drain();
        assert_eq!(eval_in_context_string("rr", "globalThis.__cht.join(';')"), "6|\u{4}hi-chat");
        // …and the chat trigger did NOT also print to the console.
        assert_eq!(eval_in_context_string("rr", "globalThis.__con.join(';')"), "6|hi-console\n");
        shutdown();
    }

    /// Command reply source, PR 3: `Commands.handleChatTrigger` is the chat-trigger entry point, so
    /// the command it dispatches must answer in CHAT — not in the caller's console, which is where
    /// the bare slot fallback would send it once `reply` routes on the source.
    #[test]
    fn handle_chat_trigger_replies_to_chat() {
        init(dummy_logger()).unwrap();
        load_body("hc", r#"
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(slot + "|" + msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            var C = __s2pkg_commands.Commands;
            C.register("sm_h", function (ctx) { ctx.reply("via-" + ctx.replySource); });
            C.handleChatTrigger(4, "!h");
        "#, "{}");
        frame_async_drain();
        frame_async_drain();
        assert_eq!(eval_in_context_string("hc", "globalThis.__cht.join(';')"), "4|via-chat");
        assert_eq!(eval_in_context_string("hc", "String(globalThis.__con.length)"), "0",
                   "a chat trigger must not answer in the caller's console");
        shutdown();
    }

    /// Command reply source, PR 3: the explicit targets IGNORE `replySource` — a chat-triggered
    /// command can force its answer into the caller's console, and a console-invoked one into chat.
    #[test]
    fn explicit_reply_targets_override_source() {
        init(dummy_logger()).unwrap();
        load_body("ro", r#"
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(msg); };
            __s2pkg_commands.Commands.register("sm_o", function (ctx) {
                if (ctx.replySource === "chat") ctx.replyToConsole("forced-console");
                else ctx.replyToChat("forced-chat");
            });
        "#, "{}");
        dispatch_chat(2, "!o", false);              // source "chat" → forced to the console
        assert_eq!(eval_in_context_string("ro", "globalThis.__con.join(';')"), "forced-console\n");
        dispatch_client_command(2, "sm_o", "");     // source "console" → forced to chat
        frame_async_drain();
        frame_async_drain();
        assert_eq!(eval_in_context_string("ro", "globalThis.__cht.join(';')"), "forced-chat");
        shutdown();
    }

    /// Command reply source, PR 3: `replyT` routes through `reply`, so it inherits the fix — a
    /// player who ran the command at their console gets the TRANSLATED line in their console, not
    /// in chat.
    #[test]
    fn replyt_inherits_routing() {
        init(dummy_logger()).unwrap();
        load_body("rl", r#"
            globalThis.__con = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(msg); };
            __s2pkg_translations.Translations.load('c', { Kicked: 'Kicked {1}' });
            __s2pkg_commands.Commands.register("sm_l", function (ctx) { ctx.replyT('Kicked', 'Bob'); });
        "#, "{}");
        dispatch_client_command(7, "sm_l", "");
        let got = eval_in_context_string("rl", "globalThis.__con.join(';')");
        assert!(got.contains("Kicked"), "replyT landed in the caller's console, got {:?}", got);
        shutdown();
    }

    /// Command reply source, PR 4: `Commands.dispatch` takes an optional trailing reply source, and
    /// `handleChatTrigger` always dispatches as "chat" — the caller typed it in chat, whatever the
    /// slot would otherwise imply. An unrecognised token degrades to the slot fallback rather than
    /// failing the dispatch.
    #[test]
    fn commands_dispatch_reply_source_param() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("rp", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__src = "";
            globalThis.__cht = [];
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            C.register("sm_p", function (ctx) { globalThis.__src = ctx.replySource; ctx.reply("r-" + ctx.replySource); });
            C.dispatch("sm_p", 4, "");            globalThis.__a = globalThis.__src;  // default → console
            C.dispatch("sm_p", 4, "", "chat");    globalThis.__b = globalThis.__src;  // explicit
            C.dispatch("sm_p", -1, "", "chat");   globalThis.__c = globalThis.__src;  // explicit beats the slot
            C.handleChatTrigger(4, "!p");         globalThis.__d = globalThis.__src;  // always chat
            C.handleChatTrigger(4, "/p");          globalThis.__g = globalThis.__src;  // silent trigger → still chat
            globalThis.__e = String(C.dispatch("sm_p", 4, "", "bogus"));              // unknown token
            globalThis.__f = globalThis.__src;
        "#, "{}");
        assert_eq!(eval_in_context_string("rp", "globalThis.__a"), "console");
        assert_eq!(eval_in_context_string("rp", "globalThis.__b"), "chat");
        assert_eq!(eval_in_context_string("rp", "globalThis.__c"), "chat");
        assert_eq!(eval_in_context_string("rp", "globalThis.__d"), "chat", "handleChatTrigger forces chat");
        assert_eq!(eval_in_context_string("rp", "globalThis.__g"), "chat",
                   "the silent / trigger still answers in chat");
        assert_eq!(eval_in_context_string("rp", "globalThis.__e"), "true", "an unknown token still dispatches");
        assert_eq!(eval_in_context_string("rp", "globalThis.__f"), "console", "an unknown token falls back to the slot");
        // The "chat" + slot -1 dispatch (__c) has no chat channel and must degrade to the server
        // console synchronously, landing on LOG rather than the chat spy.
        assert!(LOG.lock().unwrap().iter().any(|l| l.contains("r-chat")),
                 "\"chat\" reply source at slot -1 degrades to the server console");
        assert_eq!(eval_in_context_string("rp", "String(globalThis.__cht.length)"), "0",
                   "the -1 + \"chat\" dispatch must not land in the chat spy");
        shutdown();
    }

    /// Slice 6.11: chat-trigger parsing + same-context dispatch (a player's "!cmd" runs the command).
    #[test]
    fn chat_triggers_parse_and_dispatch() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("ct", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__ran = "";
            C.register("sm_test", function (ctx) { globalThis.__ran = ctx.callerSlot + ":" + ctx.argString; });
            globalThis.__p1 = JSON.stringify(C.parseChatTrigger("!kick Bob Smith"));
            globalThis.__p2 = JSON.stringify(C.parseChatTrigger("/who"));
            globalThis.__p3 = String(C.parseChatTrigger("hello world"));            // null -> "null"
            globalThis.__h  = JSON.stringify(C.handleChatTrigger(5, "!test foo bar")); // sm_ prepend -> sm_test
            globalThis.__hMiss = JSON.stringify(C.handleChatTrigger(5, "!nope x"));   // no such command -> ran:false
        "#, "{}");
        assert_eq!(eval_in_context_string("ct", "globalThis.__p1"), r#"{"silent":false,"name":"kick","argString":"Bob Smith"}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__p2"), r#"{"silent":true,"name":"who","argString":""}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__p3"), "null");
        assert_eq!(eval_in_context_string("ct", "globalThis.__ran"), "5:foo bar", "sm_test dispatched via !test");
        assert_eq!(eval_in_context_string("ct", "globalThis.__h"), r#"{"silent":false,"ran":true}"#);
        assert_eq!(eval_in_context_string("ct", "globalThis.__hMiss"), r#"{"silent":false,"ran":false}"#, "trigger consumed even if the command is unknown");
        shutdown();
    }

    /// Slice 6.11b: the core Host_Say chat dispatch. `dispatch_chat(slot, text)` parses a !cmd / /cmd
    /// trigger, dispatches the matching (or `sm_`-prefixed) command in its owner context with the
    /// speaker's slot, and returns whether to SUPPRESS the broadcast — a matched silent `/` only.
    /// This is exactly the fn the shim's Host_Say detour calls.
    #[test]
    fn chat_dispatch_host_say_parses_and_suppresses() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("hs", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__ran = "";
            C.register("sm_test", function (ctx) { globalThis.__ran = ctx.callerSlot + ":" + ctx.argString; });
        "#, "{}");
        // Public `!test` → dispatches sm_test (sm_ fallback) with slot 5 + args, and NEVER suppresses.
        assert_eq!(dispatch_chat(5, "!test foo bar", false), false, "! trigger never suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "5:foo bar", "!test dispatched sm_test");
        // Silent `/test` → dispatches AND suppresses (matched silent trigger).
        eval_in_context("hs", "globalThis.__ran = '';").unwrap();
        assert_eq!(dispatch_chat(7, "/test", false), true, "matched / trigger suppresses");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "7:", "/test dispatched with empty args");
        // Ordinary chat (no trigger char) → no dispatch, no suppress.
        eval_in_context("hs", "globalThis.__ran = 'untouched';").unwrap();
        assert_eq!(dispatch_chat(5, "hello world", false), false, "ordinary chat is not a trigger");
        assert_eq!(eval_in_context_string("hs", "globalThis.__ran"), "untouched", "ordinary chat did not dispatch");
        // Unknown `/nope` → no command match → NOT suppressed (never swallow a non-command message).
        assert_eq!(dispatch_chat(5, "/nope", false), false, "unmatched silent trigger is not suppressed");
        shutdown();
    }

    /// Slice 6.13b Task 3: the raw-chat subscriber mechanism (`__s2_chat_on_message`). A non-command chat
    /// line is delivered to `CHAT_MSG_SUBS` subscribers with `(slot, text, teamonly)`; if a live
    /// subscriber returns `>= HookResult.Handled` (2) the broadcast is suppressed (`dispatch_chat`
    /// returns true). `Continue`/`undefined`/non-number → no suppress. A matched command trigger
    /// takes the command path and never reaches the subscriber loop. Engine-generic: core passes
    /// only slot/text/teamonly (no game type).
    #[test]
    fn chat_message_subscriber_suppresses_on_handled() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("cm", r#"
            globalThis.__got = null;
            globalThis.__block = false;
            __s2_chat_on_message(function (slot, text, teamonly) {
                globalThis.__got = slot + "|" + text + "|" + teamonly;
                return globalThis.__block ? 2 /*Handled*/ : 0 /*Continue*/;
            });
        "#, "{}");
        // Continue (return 0) → not suppressed; the subscriber still saw slot/text/teamonly.
        assert_eq!(dispatch_chat(3, "hello world", true), false, "Continue does not suppress");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "3|hello world|true", "subscriber saw slot/text/teamonly");
        // Handled (return 2) → suppressed; teamonly=false threads through as `false`.
        eval_in_context("cm", "globalThis.__block = true;").unwrap();
        assert_eq!(dispatch_chat(4, "hi again", false), true, ">= Handled suppresses");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "4|hi again|false", "subscriber saw the second line");
        // A command trigger with NO subscriber-reachable path: `!nope` doesn't match a command, so it
        // falls to the raw-chat subscriber loop — the subscriber (blocking) suppresses it too.
        eval_in_context("cm", "globalThis.__got = 'x';").unwrap();
        assert_eq!(dispatch_chat(5, "!nope", false), true, "unmatched trigger reaches subscribers (blocking)");
        assert_eq!(eval_in_context_string("cm", "globalThis.__got"), "5|!nope|false", "unmatched trigger delivered raw to subscriber");
        shutdown();
    }

    /// Slice 6.13b Task 3: `__s2_chat_on_message` degrades safely with no engine ops present — the
    /// native only touches CHAT_MSG_SUBS (no engine-op), so subscribing must not panic, and a
    /// dispatch with no subscriber-return-value change (handler returns nothing) does not suppress.
    #[test]
    fn chat_on_message_native_degrades_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();               // no engine ops set
        create_plugin_context("p");
        // Subscribing must not throw even with no ops.
        eval_in_context("p", "__s2_chat_on_message(function (slot, text, teamonly) { /* returns undefined */ });").unwrap();
        // A handler returning undefined ⇒ Continue ⇒ no suppress.
        assert_eq!(dispatch_chat(1, "plain line", false), false, "undefined return ⇒ Continue ⇒ no suppress");
        shutdown();
    }

    #[test]
    fn command_trio_server_and_admin_gating() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_body("t", r#"
            var C = __s2pkg_commands.Commands;
            globalThis.__con = []; globalThis.__cht = [];
            globalThis.__s2_client_console_print = function (slot, msg) { globalThis.__con.push(slot + "|" + msg); };
            globalThis.__s2pkg_chat.Chat.toSlot = function (slot, msg) { globalThis.__cht.push(slot + "|" + msg); };
            C.registerServer("sm_srv", function(ctx){ globalThis.__srv = ctx.callerSlot; });
            C.registerAdmin("sm_adm", 512 /*CHAT=1<<9*/, function(ctx){ globalThis.__adm = ctx.callerSlot; });
            // Install a fake admin-check: slot 5 allowed, others denied.
            globalThis.__s2_admin_check = function(slot, mask){ return slot === 5; };
        "#, "{}");
        // registerServer: console (-1) runs; a player (3) denied.
        dispatch_concommand("sm_srv", -1, "", ReplySource::from_slot(-1)); assert_eq!(eval_in_context_string("t", "String(globalThis.__srv)"), "-1");
        eval_in_context("t", "globalThis.__srv = 'none';").unwrap();
        dispatch_concommand("sm_srv", 3, "", ReplySource::from_slot(3)); assert_eq!(eval_in_context_string("t", "String(globalThis.__srv)"), "none"); // stayed
        // registerAdmin: console (-1) = root runs; slot 5 (hook true) runs; slot 3 (hook false) denied.
        dispatch_concommand("sm_adm", -1, "", ReplySource::from_slot(-1)); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "-1");
        eval_in_context("t", "globalThis.__adm = 'none';").unwrap();
        dispatch_concommand("sm_adm", 5, "", ReplySource::from_slot(5)); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "5");
        eval_in_context("t", "globalThis.__adm = 'none';").unwrap();
        // The headline live-gate row: a non-admin typing sm_adm at their OWN CONSOLE (the
        // ClientCommand path — source "console") must be refused IN THAT CONSOLE, never in chat.
        eval_in_context("t", "globalThis.__con = [];").unwrap();   // isolate from the registerServer denial above
        dispatch_client_command(3, "sm_adm", "");
        assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "none"); // denied
        assert_eq!(eval_in_context_string("t", "globalThis.__con.join(';')"), "3|[SM] You do not have access to this command.\n",
                   "the console-sourced denial lands in the caller's console");
        assert_eq!(eval_in_context_string("t", "String(globalThis.__cht.length)"), "0",
                   "the console-sourced denial must not land in chat");
        // Fail-safe: with NO admin-check hook installed, a player is DENIED (never accidentally granted).
        eval_in_context("t", "delete globalThis.__s2_admin_check; globalThis.__adm = 'none'; globalThis.__con = [];").unwrap();
        dispatch_concommand("sm_adm", 3, "", ReplySource::from_slot(3)); assert_eq!(eval_in_context_string("t", "String(globalThis.__adm)"), "none"); // no hook → denied
        shutdown();
    }
}
