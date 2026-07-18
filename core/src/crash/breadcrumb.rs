//! The breadcrumb: a fixed-size, pre-allocated #[repr(C)] POD in static memory that a signal
//! handler can read with plain memory loads. Written ONLY by the main thread (dispatch stamps);
//! torn reads are tolerated by design — the minidump is the source of truth for native stacks.

use std::cell::UnsafeCell;

pub const BREADCRUMB_MAGIC: u32 = 0x5332_4352; // "S2CR" (LE bytes: 52 43 32 53)
pub const BREADCRUMB_VERSION: u32 = 1;
pub const RING_LEN: usize = 16;
pub const PLUGIN_TABLE_LEN: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RingEntry {
    pub tick: u64,
    pub plugin: [u8; 32],
    pub dispatch: [u8; 48],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginSlot {
    pub id: [u8; 48],
    pub version: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CrashBreadcrumb {
    pub magic: u32,
    pub version: u32,
    // --- identity / treadmill ---
    pub s2_version: [u8; 32],
    pub api_version: u32,
    pub gamedata_fingerprint: [u8; 40],
    pub gamedata_generated_at: [u8; 32],
    pub hl2sdk_build: [u8; 32],
    pub schema_build: [u8; 40],
    pub gamedata_stale: u32, // 0/1
    pub game_name: [u8; 16],
    pub game_build: u32,
    pub map: [u8; 64],
    pub players: i32,
    // --- current context ---
    pub plugin: [u8; 32],
    pub dispatch: [u8; 48],
    pub engine_op: [u8; 32],
    pub js_location: [u8; 96],
    pub tick: u64,
    pub uptime_secs: u32,
    // --- ring buffer ---
    pub ring_head: u32, // next write index (mod RING_LEN)
    pub ring: [RingEntry; RING_LEN],
    // --- plugin table ---
    pub plugin_count: u32,
    pub plugins: [PluginSlot; PLUGIN_TABLE_LEN],
}

/// Static storage. Writers are main-thread-only (dispatch/engine ops run on the game thread);
/// the signal handler / panic hook read a best-effort snapshot and TOLERATE torn writes (spec
/// §6.1 threading note), so no lock is taken on either side.
struct BreadcrumbCell(UnsafeCell<CrashBreadcrumb>);
unsafe impl Sync for BreadcrumbCell {}

static BREADCRUMB: BreadcrumbCell = BreadcrumbCell(UnsafeCell::new(CrashBreadcrumb {
    magic: BREADCRUMB_MAGIC,
    version: BREADCRUMB_VERSION,
    s2_version: [0; 32],
    api_version: 0,
    gamedata_fingerprint: [0; 40],
    gamedata_generated_at: [0; 32],
    hl2sdk_build: [0; 32],
    schema_build: [0; 40],
    gamedata_stale: 0,
    game_name: [0; 16],
    game_build: 0,
    map: [0; 64],
    players: 0,
    plugin: [0; 32],
    dispatch: [0; 48],
    engine_op: [0; 32],
    js_location: [0; 96],
    tick: 0,
    uptime_secs: 0,
    ring_head: 0,
    ring: [RingEntry { tick: 0, plugin: [0; 32], dispatch: [0; 48] }; RING_LEN],
    plugin_count: 0,
    plugins: [PluginSlot { id: [0; 48], version: [0; 16] }; PLUGIN_TABLE_LEN],
}));

#[inline]
fn bc() -> &'static mut CrashBreadcrumb {
    // SAFETY: single-writer (main thread) by construction; readers tolerate tears.
    unsafe { &mut *BREADCRUMB.0.get() }
}

pub fn breadcrumb_ptr() -> *const u8 {
    BREADCRUMB.0.get() as *const u8
}

pub fn breadcrumb_size() -> u32 {
    std::mem::size_of::<CrashBreadcrumb>() as u32
}

/// Main-thread copy of the whole POD (used by the panic hook + JS-error path renderers).
pub fn snapshot() -> CrashBreadcrumb {
    unsafe { std::ptr::read(BREADCRUMB.0.get()) }
}

/// Bounded, NUL-terminated copy. Truncates to dst.len()-1 bytes; never allocates.
pub(crate) fn copy_cstr(dst: &mut [u8], src: &str) {
    let n = src.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    dst[n..].iter_mut().for_each(|b| *b = 0);
}

/// Read a NUL-terminated fixed buffer back into a String (lossy).
pub(crate) fn read_cstr(src: &[u8]) -> String {
    let end = src.iter().position(|&b| b == 0).unwrap_or(src.len());
    String::from_utf8_lossy(&src[..end]).into_owned()
}

pub fn set_identity(fingerprint: &str, generated_at: &str, hl2sdk: &str, schema_build: &str, stale: bool) {
    let b = bc();
    copy_cstr(&mut b.s2_version, env!("CARGO_PKG_VERSION"));
    b.api_version = crate::loader::HOST_API_VERSION_MAJOR;
    copy_cstr(&mut b.gamedata_fingerprint, fingerprint);
    copy_cstr(&mut b.gamedata_generated_at, generated_at);
    copy_cstr(&mut b.hl2sdk_build, hl2sdk);
    copy_cstr(&mut b.schema_build, schema_build);
    b.gamedata_stale = if stale { 1 } else { 0 };
}

pub fn set_game(name: &str, build: u32) {
    let b = bc();
    copy_cstr(&mut b.game_name, name);
    b.game_build = build;
}

pub fn set_map(map: &str) { copy_cstr(&mut bc().map, map); }
pub fn set_players(n: i32) { bc().players = n.max(0); }
pub fn note_tick(tick: u64, uptime_secs: u32) { let b = bc(); b.tick = tick; b.uptime_secs = uptime_secs; }
pub fn note_engine_op(op: &str) { copy_cstr(&mut bc().engine_op, op); }

/// "owner:line" without allocation-heavy formatting (a handful of byte stores per dispatch).
pub fn note_js_location(owner: &str, line: u32) {
    let b = bc();
    let mut buf = [0u8; 96];
    let n = owner.len().min(84);
    buf[..n].copy_from_slice(&owner.as_bytes()[..n]);
    buf[n] = b':';
    let mut digits = [0u8; 10];
    let mut v = line;
    let mut d = 0usize;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        d += 1;
        if v == 0 { break; }
    }
    for k in 0..d { buf[n + 1 + k] = digits[d - 1 - k]; }
    b.js_location = buf;
}

pub fn plugin_loaded(id: &str, version: &str) {
    let b = bc();
    // Update in place on reload.
    for i in 0..b.plugin_count as usize {
        if read_cstr(&b.plugins[i].id) == id {
            copy_cstr(&mut b.plugins[i].version, version);
            return;
        }
    }
    if (b.plugin_count as usize) < PLUGIN_TABLE_LEN {
        let i = b.plugin_count as usize;
        copy_cstr(&mut b.plugins[i].id, id);
        copy_cstr(&mut b.plugins[i].version, version);
        b.plugin_count += 1;
    } // else: table full → drop (fixed-size, never grows)
}

pub fn plugin_unloaded(id: &str) {
    let b = bc();
    let count = b.plugin_count as usize;
    for i in 0..count {
        if read_cstr(&b.plugins[i].id) == id {
            b.plugins[i] = b.plugins[count - 1];
            b.plugins[count - 1] = PluginSlot { id: [0; 48], version: [0; 16] };
            b.plugin_count -= 1;
            return;
        }
    }
}

pub fn clear_plugins() {
    let b = bc();
    b.plugins = [PluginSlot { id: [0; 48], version: [0; 16] }; PLUGIN_TABLE_LEN];
    b.plugin_count = 0;
    copy_cstr(&mut b.plugin, "core");
    copy_cstr(&mut b.dispatch, "idle");
}

/// RAII dispatch stamp: sets plugin+dispatch, pushes a ring entry; Drop restores the previous
/// stamp (supports nesting — e.g. an event fired from inside a frame handler).
pub struct DispatchGuard {
    prev_plugin: [u8; 32],
    prev_dispatch: [u8; 48],
}

pub fn enter_dispatch(plugin: &str, dispatch: &str) -> DispatchGuard {
    let b = bc();
    let g = DispatchGuard { prev_plugin: b.plugin, prev_dispatch: b.dispatch };
    copy_cstr(&mut b.plugin, plugin);
    copy_cstr(&mut b.dispatch, dispatch);
    let idx = (b.ring_head as usize) % RING_LEN;
    b.ring[idx].tick = b.tick;
    b.ring[idx].plugin = b.plugin;
    b.ring[idx].dispatch = b.dispatch;
    b.ring_head = ((idx + 1) % RING_LEN) as u32;
    g
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        let b = bc();
        b.plugin = self.prev_plugin;
        b.dispatch = self.prev_dispatch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_guard_stamps_and_restores() {
        clear_plugins();
        {
            let _g = enter_dispatch("pluginA", "OnGameFrame:pre");
            let s = snapshot();
            assert_eq!(read_cstr(&s.plugin), "pluginA");
            assert_eq!(read_cstr(&s.dispatch), "OnGameFrame:pre");
            {
                let _inner = enter_dispatch("pluginB", "event:round_start");
                let s2 = snapshot();
                assert_eq!(read_cstr(&s2.plugin), "pluginB");
            }
            // inner drop restores the outer stamp
            let s3 = snapshot();
            assert_eq!(read_cstr(&s3.plugin), "pluginA");
        }
        let s4 = snapshot();
        assert_eq!(read_cstr(&s4.plugin), "core"); // guard drop restores the idle stamp
    }

    #[test]
    fn ring_records_last_16_in_order() {
        clear_plugins();
        for i in 0..20u64 {
            note_tick(i, 0);
            let _g = enter_dispatch(&format!("p{}", i), "d");
        }
        let s = snapshot();
        // head points at the next write slot; the 16 entries are ticks 4..=19
        let mut ticks: Vec<u64> = Vec::new();
        for k in 0..RING_LEN {
            let idx = (s.ring_head as usize + k) % RING_LEN;
            ticks.push(s.ring[idx].tick);
        }
        assert_eq!(ticks, (4..20).collect::<Vec<u64>>());
        assert_eq!(read_cstr(&s.ring[(s.ring_head as usize + RING_LEN - 1) % RING_LEN].plugin), "p19");
    }

    #[test]
    fn plugin_table_add_remove_and_overflow() {
        clear_plugins();
        plugin_loaded("a", "1.0.0");
        plugin_loaded("b", "2.0.0");
        plugin_loaded("a", "1.0.1"); // reload updates in place, no duplicate
        let s = snapshot();
        assert_eq!(s.plugin_count, 2);
        let ids: Vec<(String, String)> = (0..s.plugin_count as usize)
            .map(|i| (read_cstr(&s.plugins[i].id), read_cstr(&s.plugins[i].version)))
            .collect();
        assert!(ids.contains(&("a".into(), "1.0.1".into())));
        plugin_unloaded("a");
        assert_eq!(snapshot().plugin_count, 1);
        // overflow: table is fixed-size; extra loads are dropped, never grow/realloc
        for i in 0..100 {
            plugin_loaded(&format!("p{}", i), "0.0.1");
        }
        assert_eq!(snapshot().plugin_count as usize, PLUGIN_TABLE_LEN);
    }

    #[test]
    fn identity_game_map_players_stamp() {
        set_identity("fp123", "1752710400", "hl2sdk-abc", "schema-def", true);
        set_game("cs2", 14099);
        set_map("de_dust2");
        set_players(7);
        note_engine_op("ent_ref_read");
        note_js_location("myplugin", 42);
        let s = snapshot();
        assert_eq!(s.magic, BREADCRUMB_MAGIC);
        assert_eq!(s.version, BREADCRUMB_VERSION);
        assert_eq!(read_cstr(&s.gamedata_fingerprint), "fp123");
        assert_eq!(s.gamedata_stale, 1);
        assert_eq!(read_cstr(&s.game_name), "cs2");
        assert_eq!(s.game_build, 14099);
        assert_eq!(read_cstr(&s.map), "de_dust2");
        assert_eq!(s.players, 7);
        assert_eq!(read_cstr(&s.engine_op), "ent_ref_read");
        assert_eq!(read_cstr(&s.js_location), "myplugin:42");
        assert_eq!(read_cstr(&s.s2_version), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn copy_cstr_truncates_and_terminates() {
        let mut buf = [0u8; 8];
        copy_cstr(&mut buf, "12345678901234");
        assert_eq!(read_cstr(&buf), "1234567"); // 7 chars + NUL
        copy_cstr(&mut buf, "ab");
        assert_eq!(read_cstr(&buf), "ab");
    }
}
