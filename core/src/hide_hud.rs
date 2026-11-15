//! `@s2script/sdk/hud` — a multiplexed, per-player HUD-hide mask over `CBasePlayerPawn::m_iHideHUD`.
//!
//! Every plugin holds its own bits per slot; the pawn carries the OR of every plugin's bits on top of
//! whatever the game itself put there. Writing the field directly would let two plugins stomp each
//! other and leave bits behind when one unloads, so the field is owned here:
//!
//! - **Additive, never destructive.** The host remembers which bits IT last applied to a pawn and
//!   only ever adds or removes those, so a bit the game set (or another tool set) survives.
//! - **Follows the player, not the pawn.** The mask is keyed by slot. The pawn is re-resolved every
//!   sweep through the controller's `m_hPawn` (books-gated, never a raw pointer across time), so a
//!   respawn, a fresh pawn or the switch to an observer pawn picks the bits up within a frame, and
//!   the pawn left behind is given back its own value.
//! - **Re-asserted.** If the game resets the field (spawn does), the next sweep puts the bits back.
//! - **Ledgered.** Unload removes only that plugin's bits; disconnect clears the slot for everyone,
//!   because a mask is about the player who held the seat.
//!
//! Engine-generic: `CBasePlayerController::m_hPawn` and `CBasePlayerPawn::m_iHideHUD` exist in every
//! Source 2 game. What each BIT means is the game's business (`@s2script/cs2` names CS2's).
use std::cell::RefCell;
use std::collections::HashMap;

use crate::v8host::{current_plugin, engine_ops_ref, schema_offset_cached, set_native};

const SLOTS: usize = 64;
const PAWN_CLASS: &str = "CBasePlayerPawn";
const HIDE_FIELD: &str = "m_iHideHUD";
const CONTROLLER_CLASS: &str = "CBasePlayerController";
const PAWN_HANDLE_FIELD: &str = "m_hPawn";

/// Which bits the host last put on which pawn for a slot. `bits == 0` means nothing of ours is on it.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Applied {
    index: i32,
    id: u64,
    bits: u32,
}

thread_local! {
    /// owner -> slot -> bits. The source of truth; the pawn field is derived from it.
    static MASKS: RefCell<HashMap<String, HashMap<i32, u32>>> = RefCell::new(HashMap::new());
    static APPLIED: RefCell<[Applied; SLOTS]> = RefCell::new([Applied::default(); SLOTS]);
}

fn merged(slot: i32) -> u32 {
    MASKS.with(|m| m.borrow().values().filter_map(|r| r.get(&slot)).fold(0, |a, b| a | b))
}

fn own(owner: &str, slot: i32) -> u32 {
    MASKS.with(|m| m.borrow().get(owner).and_then(|r| r.get(&slot)).copied().unwrap_or(0))
}

fn store(owner: &str, slot: i32, bits: u32) {
    MASKS.with(|m| {
        let mut m = m.borrow_mut();
        if bits == 0 {
            if let Some(r) = m.get_mut(owner) {
                r.remove(&slot);
                if r.is_empty() { m.remove(owner); }
            }
        } else {
            m.entry(owner.to_string()).or_default().insert(slot, bits);
        }
    });
}

/// The slot's current pawn, as books-gated (index, id). None when the controller or pawn is not live.
fn pawn_for_slot(slot: i32) -> Option<(i32, u64)> {
    let handle_off = schema_offset_cached(CONTROLLER_CLASS, PAWN_HANDLE_FIELD);
    if handle_off < 0 { return None; }
    let ctrl_index = slot + 1;
    let (ctrl_id, _) = crate::entity_live::lookup(ctrl_index)?;
    let ctrl = crate::entity::entity_resolve_ptr(ctrl_index, ctrl_id);
    if ctrl.is_null() { return None; }
    let handle = crate::entity::read_u32(ctrl, handle_off);
    let (index, serial) = crate::entity::decode_handle(handle);
    let id = crate::entity_live::adopt(index, serial)?;
    Some((index, id))
}

/// Read-modify-write the field on one pawn: drop `remove`, add `add`. Notifies only on a change.
/// Returns false when the pawn or the field cannot be reached.
fn write_bits(index: i32, id: u64, remove: u32, add: u32) -> bool {
    let off = schema_offset_cached(PAWN_CLASS, HIDE_FIELD);
    if off < 0 { return false; }
    let ptr = crate::entity::entity_resolve_ptr(index, id);
    if ptr.is_null() { return false; }
    let cur = crate::entity::read_u32(ptr, off);
    let next = (cur & !remove) | add;
    if next != cur {
        crate::entity::write_u32(ptr, off, next);
        if let Some(f) = engine_ops_ref().and_then(|ops| ops.ent_state_changed) {
            f(ptr as *mut std::ffi::c_void, off);
        }
    }
    true
}

/// Bring one slot's pawn in line with the merged mask. Cheap when nothing changed: one books lookup
/// and one read. Returns whether the merged bits are on a live pawn afterwards.
fn sync(slot: i32) -> bool {
    if !(0..SLOTS as i32).contains(&slot) { return false; }
    let want = merged(slot);
    let prev = APPLIED.with(|a| a.borrow()[slot as usize]);
    let pawn = pawn_for_slot(slot);
    // The pawn changed (respawn, observer, replacement): hand the old one back its own value.
    if prev.bits != 0 && pawn.is_none_or(|(i, d)| i != prev.index || d != prev.id) {
        let _ = write_bits(prev.index, prev.id, prev.bits, 0);
    }
    let Some((index, id)) = pawn else {
        APPLIED.with(|a| a.borrow_mut()[slot as usize] = Applied::default());
        return false;
    };
    let ours = if prev.index == index && prev.id == id { prev.bits } else { 0 };
    // Re-add `want` even when nothing moved: the game may have cleared the field since.
    let ok = (want == 0 && ours == 0) || write_bits(index, id, ours & !want, want);
    APPLIED.with(|a| a.borrow_mut()[slot as usize] = Applied { index, id, bits: if ok { want } else { 0 } });
    ok
}

/// Per-frame re-assert for every slot that has, or recently had, bits.
pub(crate) fn sweep() {
    for slot in 0..SLOTS as i32 {
        let active = APPLIED.with(|a| a.borrow()[slot as usize].bits != 0) || merged(slot) != 0;
        if active { let _ = sync(slot); }
    }
}

/// Disconnect: the next occupant of the seat must not inherit anyone's mask.
pub(crate) fn clear_slot(slot: i32) {
    if !(0..SLOTS as i32).contains(&slot) { return; }
    MASKS.with(|m| {
        let mut m = m.borrow_mut();
        for r in m.values_mut() { r.remove(&slot); }
        m.retain(|_, r| !r.is_empty());
    });
    let _ = sync(slot);
}

fn remove_owner(owner: &str) {
    let slots: Vec<i32> = MASKS.with(|m| m.borrow_mut().remove(owner).map(|r| r.into_keys().collect()).unwrap_or_default());
    for s in slots { let _ = sync(s); }
}

fn slot_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<i32> {
    let s = v.integer_value(scope)? as i32;
    (0..SLOTS as i32).contains(&s).then_some(s)
}

fn bits_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<u32> {
    let n = v.number_value(scope)?;
    (n.is_finite() && n >= 0.0 && n <= u32::MAX as f64 && n.fract() == 0.0).then_some(n as u32)
}

/// `__s2_hidehud_set(slot, bits, mode)` — mode 0 replaces this plugin's bits, 1 adds, 2 removes.
/// Returns this plugin's bits after the change, or -1 on a bad argument. The pawn write happens now
/// when the pawn is live, and on the first frame it is otherwise.
fn s2_hidehud_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(slot) = slot_arg(scope, args.get(0)) else { return };
        let Some(bits) = bits_arg(scope, args.get(1)) else { return };
        let mode = args.get(2).integer_value(scope).unwrap_or(0);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let cur = own(&owner, slot);
        let next = match mode { 1 => cur | bits, 2 => cur & !bits, _ => bits };
        store(&owner, slot, next);
        let _ = sync(slot);
        rv.set_double(next as f64);
    }));
}

/// `__s2_hidehud_get(slot, merged)` — this plugin's bits, or (merged = true) every plugin's OR.
fn s2_hidehud_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(slot) = slot_arg(scope, args.get(0)) else { return };
        let all = args.get(1).boolean_value(scope);
        let bits = if all { merged(slot) } else {
            let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
            own(&owner, slot)
        };
        rv.set_double(bits as f64);
    }));
}

/// `__s2_hidehud_reset_all()` — drop every bit this plugin holds, on every slot.
fn s2_hidehud_reset_all(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        remove_owner(&owner);
    }));
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_hidehud_set", s2_hidehud_set);
    set_native(scope, global_obj, "__s2_hidehud_get", s2_hidehud_get);
    set_native(scope, global_obj, "__s2_hidehud_reset_all", s2_hidehud_reset_all);
}

/// Unload removes only that plugin's bits; process teardown empties the table without touching pawns.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "HIDE_HUD",
        Box::new(remove_owner),
        Box::new(|_ids| {}),
        Box::new(|| {
            MASKS.with(|m| m.borrow_mut().clear());
            APPLIED.with(|a| *a.borrow_mut() = [Applied::default(); SLOTS]);
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, unload_plugin, S2EngineOps};
    use std::cell::Cell;
    use std::ffi::{c_char, c_int, c_void, CStr};

    const SLOT: i32 = 2;
    const CTRL: i32 = SLOT + 1;
    const PAWN_A: i32 = 9;
    const PAWN_B: i32 = 10;
    const HANDLE_OFF: i32 = 0x10;
    const HIDE_OFF: i32 = 0x20;

    thread_local! {
        static BUFS: Cell<[*mut u8; 3]> = const { Cell::new([std::ptr::null_mut(); 3]) };
        static NOTIFIES: Cell<i32> = const { Cell::new(0) };
    }
    fn buf(i: usize) -> *mut u8 { BUFS.with(|b| b.get()[i]) }
    extern "C" fn resolve(index: c_int, _serial: c_int) -> *mut c_void {
        (match index { CTRL => buf(0), PAWN_A => buf(1), PAWN_B => buf(2), _ => std::ptr::null_mut() }) as *mut c_void
    }
    extern "C" fn offset(class: *const c_char, field: *const c_char) -> c_int {
        let (c, f) = unsafe { (CStr::from_ptr(class).to_bytes(), CStr::from_ptr(field).to_bytes()) };
        match (c, f) {
            (b"CBasePlayerController", b"m_hPawn") => HANDLE_OFF,
            (b"CBasePlayerPawn", b"m_iHideHUD") => HIDE_OFF,
            _ => -1,
        }
    }
    extern "C" fn changed(_ent: *mut c_void, off: c_int) {
        assert_eq!(off, HIDE_OFF);
        NOTIFIES.with(|n| n.set(n.get() + 1));
    }
    fn hide(pawn: usize) -> u32 { crate::entity::read_u32(buf(pawn), HIDE_OFF) }
    fn set_hide(pawn: usize, v: u32) { crate::entity::write_u32(buf(pawn), HIDE_OFF, v) }
    fn point_controller_at(index: i32, serial: i32) {
        crate::entity::write_u32(buf(0), HANDLE_OFF, ((serial as u32) << crate::entity::HANDLE_ENTRY_BITS) | index as u32);
    }

    fn setup() {
        crate::entity_live::reset_for_tests();
        init(dummy_logger()).unwrap();
        let bufs = [0, 1, 2].map(|_| Box::leak(Box::new([0u8; 256])).as_mut_ptr());
        BUFS.with(|b| b.set(bufs));
        NOTIFIES.with(|n| n.set(0));
        crate::entity_live::on_created(CTRL, 1);
        crate::entity_live::on_created(PAWN_A, 5);
        crate::entity_live::on_created(PAWN_B, 6);
        point_controller_at(PAWN_A, 5);
        set_engine_ops(Some(S2EngineOps {
            ent_resolve: Some(resolve), schema_offset: Some(offset), ent_state_changed: Some(changed),
            ..S2EngineOps::none()
        }));
        create_plugin_context("a");
        create_plugin_context("b");
    }

    #[test]
    fn plugins_or_together_and_keep_the_games_own_bits() {
        setup();
        set_hide(1, 0x100); // a bit the game set
        assert_eq!(eval_in_context_string("a", &format!("String(__s2pkg_hud.Hud.hide({SLOT}, 2 | 32))")), "34");
        assert_eq!(eval_in_context_string("b", &format!("String(__s2pkg_hud.Hud.hide({{ slot: {SLOT} }}, 2 | 8))")), "10");
        assert_eq!(hide(1), 0x100 | 2 | 8 | 32);
        // b shows the radar: a still hides it.
        eval_in_context_string("b", &format!("String(__s2pkg_hud.Hud.show({SLOT}, 2))"));
        assert_eq!(hide(1), 0x100 | 2 | 8 | 32);
        assert_eq!(eval_in_context_string("b", &format!(
            "[__s2pkg_hud.Hud.hidden({SLOT}), __s2pkg_hud.Hud.hiddenByAll({SLOT}), __s2pkg_hud.Hud.isHidden({SLOT}, 34), __s2pkg_hud.Hud.isHidden({SLOT}, 4)].join()")),
            "8,42,true,false");
        // a unloads: only a's bits leave the pawn.
        unload_plugin("a");
        assert_eq!(hide(1), 0x100 | 8);
        eval_in_context_string("b", &format!("__s2pkg_hud.Hud.reset({SLOT}); ''"));
        assert_eq!(hide(1), 0x100, "the game's bit survives every change");
        shutdown();
    }

    #[test]
    fn the_mask_follows_the_player_and_survives_a_reset() {
        setup();
        eval_in_context_string("a", &format!("__s2pkg_hud.Hud.set({SLOT}, 2); ''"));
        assert_eq!(hide(1), 2);
        let n = NOTIFIES.with(|n| n.get());
        sweep();
        assert_eq!(NOTIFIES.with(|n| n.get()), n, "a sweep with nothing to do writes nothing");
        // The game resets the field (spawn): the next sweep puts the bit back.
        set_hide(1, 0);
        sweep();
        assert_eq!(hide(1), 2);
        // The player moves to another pawn (observer, replacement): the bit moves with them and the
        // old pawn gets its own value back.
        set_hide(2, 0x40);
        point_controller_at(PAWN_B, 6);
        sweep();
        assert_eq!((hide(1), hide(2)), (0, 0x40 | 2));
        // A dangling handle never resolves: nothing is written anywhere.
        point_controller_at(PAWN_A, 99);
        sweep();
        assert_eq!((hide(1), hide(2)), (0, 0x40));
        shutdown();
    }

    #[test]
    fn disconnect_clears_the_slot_and_bad_arguments_throw() {
        setup();
        eval_in_context_string("a", &format!("__s2pkg_hud.Hud.hide({SLOT}, 16); ''"));
        clear_slot(SLOT);
        assert_eq!(hide(1), 0);
        assert_eq!(eval_in_context_string("a", &format!("String(__s2pkg_hud.Hud.hidden({SLOT}))")), "0");
        assert_eq!(eval_in_context_string("a", r#"[
            function () { __s2pkg_hud.Hud.hide(64, 1); },
            function () { __s2pkg_hud.Hud.hide(-1, 1); },
            function () { __s2pkg_hud.Hud.hide(1.5, 1); },
            function () { __s2pkg_hud.Hud.hide(0, -1); },
            function () { __s2pkg_hud.Hud.hide(0, 2 ** 32); },
            function () { __s2pkg_hud.Hud.hide(0, "2"); }
        ].map(function (f) { try { f(); return "ok"; } catch (e) { return e.name; } }).join()"#),
            "RangeError,RangeError,RangeError,TypeError,TypeError,TypeError");
        assert_eq!(eval_in_context_string("a", "String(__s2pkg_hud.Hud.hide(0, 0xFFFFFFFF))"), "4294967295");
        shutdown();
    }
}
