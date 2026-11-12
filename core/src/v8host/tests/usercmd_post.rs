//! The usercmd hook's "post" phase: once per command the server ran, after its movement, over a
//! read-only command.
//!
//! The fake engine here plays the shim's part of one command: the pre dispatch, then the "movement"
//! (it moves a fake pawn and updates the movement-services buttons), then the post dispatch. That
//! ordering is the whole contract of the phase, so it is what these tests drive.
use super::*;
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};

/// The fake pawn's entity index, and where in it the tests keep "origin x" (an f32).
const PAWN: i32 = 9;
const ORIGIN_X: i32 = 0x40;

thread_local! {
    static PAWN_BUF: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// The command in scope: forwardMove (field 0) and the button mask.
    static CMD_FORWARD: Cell<f64> = const { Cell::new(0.0) };
    static CMD_BUTTONS: Cell<u64> = const { Cell::new(0) };
    /// Every write op that reached the "shim", by name.
    static WRITES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    /// Movement-services pressed / changed / scroll.
    static MOVE_BUTTONS: Cell<[u64; 3]> = const { Cell::new([0; 3]) };
    static PRE_INSTALLS: Cell<i32> = const { Cell::new(0) };
    static POST_INSTALLS: Cell<i32> = const { Cell::new(0) };
}

extern "C" fn fake_ent_resolve(index: c_int, _serial: c_int) -> *mut c_void {
    if index == PAWN { PAWN_BUF.with(|b| b.get()) as *mut c_void } else { std::ptr::null_mut() }
}
extern "C" fn fake_read(field: c_int) -> f64 { if field == 0 { CMD_FORWARD.with(|c| c.get()) } else { 0.0 } }
extern "C" fn fake_write(field: c_int, value: f64) {
    WRITES.with(|w| w.borrow_mut().push("write"));
    if field == 0 { CMD_FORWARD.with(|c| c.set(value)); }
}
extern "C" fn fake_read_buttons() -> u64 { CMD_BUTTONS.with(|c| c.get()) }
extern "C" fn fake_write_buttons(mask: u64) {
    WRITES.with(|w| w.borrow_mut().push("write_buttons"));
    CMD_BUTTONS.with(|c| c.set(mask));
}
extern "C" fn fake_clear_subtick() { WRITES.with(|w| w.borrow_mut().push("clear_subtick")); }
extern "C" fn fake_move_buttons(which: c_int) -> u64 {
    MOVE_BUTTONS.with(|b| b.get()).get(which as usize).copied().unwrap_or(0)
}
extern "C" fn fake_pre_install() -> c_int { PRE_INSTALLS.with(|c| c.set(c.get() + 1)); 1 }
extern "C" fn fake_post_install() -> c_int { POST_INSTALLS.with(|c| c.set(c.get() + 1)); 1 }

fn origin_x() -> f32 { unsafe { *(PAWN_BUF.with(|b| b.get()).add(ORIGIN_X as usize) as *const f32) } }
fn set_origin_x(v: f32) { unsafe { *(PAWN_BUF.with(|b| b.get()).add(ORIGIN_X as usize) as *mut f32) = v } }

/// A fresh host with the fake engine wired, a live fake pawn, and plugin `p` holding a ref to it.
fn setup() {
    crate::entity_live::reset_for_tests();
    let _ = init(dummy_logger());
    let buf: &'static mut [u8; 4096] = Box::leak(Box::new([0u8; 4096]));
    PAWN_BUF.with(|b| b.set(buf.as_mut_ptr()));
    CMD_FORWARD.with(|c| c.set(0.0));
    CMD_BUTTONS.with(|c| c.set(0));
    WRITES.with(|w| w.borrow_mut().clear());
    MOVE_BUTTONS.with(|b| b.set([0; 3]));
    PRE_INSTALLS.with(|c| c.set(0));
    POST_INSTALLS.with(|c| c.set(0));
    let id = crate::entity_live::on_created(PAWN, 1);
    set_engine_ops(Some(S2EngineOps {
        ent_resolve: Some(fake_ent_resolve),
        usercmd_hook_install: Some(fake_pre_install),
        usercmd_post_hook_install: Some(fake_post_install),
        usercmd_read: Some(fake_read),
        usercmd_write: Some(fake_write),
        usercmd_read_buttons: Some(fake_read_buttons),
        usercmd_write_buttons: Some(fake_write_buttons),
        usercmd_clear_subtick: Some(fake_clear_subtick),
        usercmd_read_move_buttons: Some(fake_move_buttons),
        ..mock_event_ops()
    }));
    create_plugin_context("p");
    let _ = id;
    eval_in_context("p", &format!(
        "globalThis.pawn = new (__s2require('@s2script/sdk/entity').EntityRef)({PAWN}, __s2_ent_id_for_index({PAWN}));\
         globalThis.log = [];"
    )).unwrap();
}

fn teardown() {
    set_engine_ops(None);
    shutdown();
}

/// One command, the way the shim runs it: pre dispatch, the movement, post dispatch.
fn run_command(slot: i32, tick: i32, number: i32) {
    let _ = dispatch_usercmd(slot);
    let forward = CMD_FORWARD.with(|c| c.get()) as f32;
    set_origin_x(origin_x() + forward);
    let pressed = CMD_BUTTONS.with(|c| c.get());
    MOVE_BUTTONS.with(|b| { let [old, _, _] = b.get(); b.set([pressed, old ^ pressed, 0]) });
    dispatch_usercmd_post(slot, tick, number);
}

fn log() -> String { eval_in_context_string("p", "log.join(' ')") }

#[test]
fn post_sees_the_origin_after_the_commands_movement() {
    setup();
    eval_in_context("p", &format!(r#"
        const U = __s2pkg_usercmd.UserCmd;
        U.onRun(function (cmd, info) {{ log.push('pre:' + pawn.readFloat32({ORIGIN_X})); }});
        U.onRun(function (cmd, info) {{ log.push('post:' + pawn.readFloat32({ORIGIN_X})); }}, {{ phase: 'post' }});
    "#)).unwrap();
    CMD_FORWARD.with(|c| c.set(5.0));
    run_command(2, 100, 1);
    run_command(2, 101, 2);
    assert_eq!(log(), "pre:0 post:5 pre:5 post:10",
               "pre reads the pawn before the command moved it, post after");
    // Standing still: pre and post agree.
    CMD_FORWARD.with(|c| c.set(0.0));
    eval_in_context("p", "log = [];").unwrap();
    run_command(2, 102, 3);
    assert_eq!(log(), "pre:10 post:10");
    teardown();
}

#[test]
fn post_fires_once_per_executed_command_with_its_tick_and_number() {
    setup();
    eval_in_context("p", r#"
        globalThis.infos = [];
        __s2pkg_usercmd.UserCmd.onRun(function (cmd, info) {
          log.push(info.slot + '@' + info.tick + '#' + info.commandNumber);
          infos.push(info);
        }, { phase: 'post' });
    "#).unwrap();
    // Tick 100 runs no command (a lost packet), 101 catches up with three, 102 runs one.
    run_command(4, 101, 7);
    run_command(4, 101, 8);
    run_command(4, 101, 9);
    run_command(4, 102, 10);
    assert_eq!(log(), "4@101#7 4@101#8 4@101#9 4@102#10");
    assert!(eval_in_context_bool("p", "infos.every(i => i === infos[0])"),
            "one info object, reused for every call");
    assert_eq!(eval_in_context_string("p", "JSON.stringify(infos[0])"),
               r#"{"slot":4,"tick":102,"commandNumber":10}"#);
    teardown();
}

#[test]
fn post_is_read_only_and_ignores_returns() {
    setup();
    eval_in_context("p", r#"
        const U = __s2pkg_usercmd.UserCmd;
        U.onRun(function (cmd) {
          cmd.forwardMove = 99; cmd.sideMove = 1; cmd.upMove = 1; cmd.impulse = 100;
          cmd.buttons = 7n; cmd.viewAngles = { x: 1, y: 2, z: 3 }; cmd.clearSubtickMoves();
          cmd.pressedButtons = 1n;
          log.push('a:' + cmd.forwardMove + ':' + cmd.buttons);
          return 3;   // Stop: must neither truncate the next subscriber nor reach the engine
        }, { phase: 'post' });
        U.onRun(function () { log.push('b'); }, { phase: 'post' });
        globalThis.preCmd = null;
        U.onRun(function (cmd) { preCmd = cmd; });
    "#).unwrap();
    CMD_FORWARD.with(|c| c.set(2.0));
    CMD_BUTTONS.with(|c| c.set(1));
    run_command(1, 50, 1);
    assert_eq!(log(), "a:2:1 b", "post writes change nothing and a Stop return does not truncate");
    assert!(WRITES.with(|w| w.borrow().is_empty()), "no post write reached the engine");
    assert_eq!(CMD_FORWARD.with(|c| c.get()), 2.0);
    assert_eq!(CMD_BUTTONS.with(|c| c.get()), 1);
    // Pre stays writable, with the same UserCmdView.
    eval_in_context("p", r#"
        __s2pkg_usercmd.UserCmd.onRun(function (cmd) {
          log.push(cmd === preCmd ? 'same-view' : 'other-view');
          cmd.forwardMove = 3;
        });
    "#).unwrap();
    run_command(1, 51, 2);
    assert_eq!(WRITES.with(|w| w.borrow().clone()), vec!["write"], "the pre write went through");
    assert_eq!(CMD_FORWARD.with(|c| c.get()), 3.0);
    assert!(log().contains("same-view"));
    teardown();
}

#[test]
fn move_buttons_read_what_the_command_left() {
    setup();
    eval_in_context("p", r#"
        const U = __s2pkg_usercmd.UserCmd;
        U.onRun(function (cmd) { log.push('pre:' + cmd.pressedButtons); });
        U.onRun(function (cmd) {
          log.push('post:' + [cmd.pressedButtons, cmd.changedButtons, cmd.scrollButtons].join('/') +
                   ':' + typeof cmd.pressedButtons);
        }, { phase: 'post' });
    "#).unwrap();
    CMD_BUTTONS.with(|c| c.set(2));   // IN_JUMP
    run_command(0, 10, 1);
    CMD_BUTTONS.with(|c| c.set(0));
    run_command(0, 11, 2);
    assert_eq!(log(), "pre:0 post:2/2/0:bigint pre:2 post:0/2/0:bigint");
    teardown();
}

#[test]
fn each_phase_installs_its_own_detour_on_its_first_subscriber() {
    setup();
    dispatch_usercmd_post(0, 1, 1);   // nobody subscribed: nothing runs, nothing installs
    assert_eq!((PRE_INSTALLS.with(|c| c.get()), POST_INSTALLS.with(|c| c.get())), (0, 0));
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () {});").unwrap();
    assert_eq!((PRE_INSTALLS.with(|c| c.get()), POST_INSTALLS.with(|c| c.get())), (1, 0),
               "a pre subscriber installs only the pre detour");
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () {}, { phase: 'post' });").unwrap();
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () {}, { phase: 'post' });").unwrap();
    assert_eq!((PRE_INSTALLS.with(|c| c.get()), POST_INSTALLS.with(|c| c.get())), (1, 1),
               "the first post subscriber installs the post detour, even with pre subscribers present");
    teardown();

    // And the other way round: a post subscriber first must not stop pre from installing.
    setup();
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () {}, { phase: 'post' });").unwrap();
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () {});").unwrap();
    assert_eq!((PRE_INSTALLS.with(|c| c.get()), POST_INSTALLS.with(|c| c.get())), (1, 1));
    teardown();
}

#[test]
fn bad_phase_throws_at_the_call_site() {
    setup();
    assert_eq!(
        eval_in_context_string("p", r#"
            try { __s2pkg_usercmd.UserCmd.onRun(function () {}, { phase: 'later' }); 'no throw' }
            catch (e) { e instanceof TypeError ? 'TypeError' : String(e) }
        "#),
        "TypeError",
    );
    assert_eq!(POST_INSTALLS.with(|c| c.get()) + PRE_INSTALLS.with(|c| c.get()), 0);
    teardown();
}

#[test]
fn unloading_the_subscriber_stops_post_delivery() {
    setup();
    create_plugin_context("q");
    eval_in_context("q", "globalThis.n = 0; __s2pkg_usercmd.UserCmd.onRun(function () { n++; }, { phase: 'post' });")
        .unwrap();
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function () { log.push('p'); }, { phase: 'post' });").unwrap();
    run_command(0, 1, 1);
    assert_eq!(eval_in_context_string("q", "String(n)"), "1");
    unload_plugin("q");
    run_command(0, 2, 2);
    assert_eq!(log(), "p p", "the cached subscriber list follows an unload");
    teardown();
}

/// A steady-state post dispatch makes no Rust allocation: the subscriber list is cached, no
/// arguments are built, and the shared fan-out's liveness check and re-entrancy marker reuse
/// their storage. (V8's own heap is outside this counter; the prelude side reuses its objects.)
#[test]
fn post_dispatch_makes_no_rust_allocation() {
    setup();
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function (cmd, info) { info.slot; }, { phase: 'post' });").unwrap();
    for i in 0..200 { dispatch_usercmd_post(0, i, i); }   // warm the cached list and the JIT
    let (allocs, _) = super::allocations::measure(|| {
        for i in 0..1000 { dispatch_usercmd_post(0, i, i); }
    });
    assert_eq!(allocs, 0, "post dispatch allocated {allocs} times for 1000 calls x 1 subscriber");
    teardown();
}

/// `cargo test -p s2script-core usercmd_post_cost -- --ignored --nocapture`: host overhead of one
/// post dispatch to one subscribed plugin, the number the PR quotes.
#[test]
#[ignore]
fn usercmd_post_cost() {
    setup();
    let n = 200_000;
    let t = std::time::Instant::now();
    for i in 0..n { dispatch_usercmd_post(0, i, i); }
    let idle = t.elapsed().as_nanos() as f64 / n as f64;
    eval_in_context("p", "__s2pkg_usercmd.UserCmd.onRun(function (cmd, info) { info.slot; }, { phase: 'post' });").unwrap();
    for i in 0..20_000 { dispatch_usercmd_post(0, i, i); }
    let t = std::time::Instant::now();
    for i in 0..n { dispatch_usercmd_post(0, i, i); }
    let per = t.elapsed().as_nanos() as f64 / n as f64;
    println!("usercmd post dispatch: {idle:.1} ns/call with no subscriber, {per:.0} ns/call with 1");
    teardown();
}

