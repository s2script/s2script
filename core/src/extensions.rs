//! Native extensions (performance plan, Stage 6): third-party shared libraries that add natives
//! without a core change, as SourceMod extensions do. The C side of the contract is
//! `packages/sdk/ext/s2ext.h`; this module is the host side of it.
//!
//! - **Boot only.** `load_dir` runs once, when the loader learns the addon directory. Each
//!   `extensions/*.so` is `dlopen`ed, its `s2ext_info` is checked (struct size, ABI version, name,
//!   version), and `s2ext_init` registers its natives. Any failure refuses that one extension
//!   with a named reason, and the server keeps running. Nothing is ever unloaded: `dlclose` of
//!   code that V8 may still call is not safe.
//! - **Declared imports.** A plugin lists the extensions it needs in its manifest (`extensions`,
//!   `name → range`). A missing or mismatched extension refuses the plugin at load, and an
//!   undeclared name never resolves to an extension.
//! - **Binding** lives in `v8host::extensions_v8`. A native whose types are all fast-API types,
//!   and which cannot fail, is bound with V8's fast API pointing straight at the extension's
//!   function. Everything else goes through one generic slow callback that marshals arguments
//!   into registers by the System V rules (`sysv_call`), so no per-signature code exists.
//!
//! Engine-generic: extensions reach the engine only through the host services in `HostApi`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use v8::fast_api::{CFunctionInfo, CTypeInfo, Int64Representation, Type as FastType};

/// The ABI this host speaks (`S2EXT_ABI_VERSION`).
pub(crate) const ABI_VERSION: u32 = 1;

/// Native flag: the native may call `HostApi::fail`.
pub(crate) const FLAG_CAN_FAIL: u32 = 1;
const KNOWN_FLAGS: u32 = FLAG_CAN_FAIL;

/// Register budget per native (System V: six integer registers, the receiver takes one; eight
/// vector registers).
const MAX_INT_ARGS: usize = 5;
const MAX_FLOAT_ARGS: usize = 8;

/// `S2Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Type {
    Void,
    Bool,
    I32,
    U32,
    F32,
    F64,
    String,
    Entity,
}

impl Type {
    fn from_raw(v: u32) -> Option<Self> {
        Some(match v {
            0 => Type::Void,
            1 => Type::Bool,
            2 => Type::I32,
            3 => Type::U32,
            4 => Type::F32,
            5 => Type::F64,
            6 => Type::String,
            7 => Type::Entity,
            _ => return None,
        })
    }
    fn is_float(self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }
    fn fast(self) -> Option<CTypeInfo> {
        Some(match self {
            Type::Void => FastType::Void.as_info(),
            Type::Bool => FastType::Bool.as_info(),
            Type::I32 => FastType::Int32.as_info(),
            Type::U32 => FastType::Uint32.as_info(),
            Type::F32 => FastType::Float32.as_info(),
            Type::F64 => FastType::Float64.as_info(),
            Type::String | Type::Entity => return None,
        })
    }
    fn ts(self, result: bool) -> &'static str {
        match self {
            Type::Void => "void",
            Type::Bool => "boolean",
            Type::I32 | Type::U32 | Type::F32 | Type::F64 => "number",
            Type::String if result => "string | null",
            Type::String => "string",
            Type::Entity => "EntityRef",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The C structs (s2ext.h). Each grows only by appending; `size` says how much a peer filled in.
// ---------------------------------------------------------------------------------------------

#[repr(C)]
struct HostApi {
    size: u32,
    abi: u32,
    host_version: *const c_char,
    log: extern "C" fn(*const HostApi, i32, *const c_char),
    schema_offset: extern "C" fn(*const HostApi, *const c_char, *const c_char) -> i32,
    fail: extern "C" fn(*const HostApi, *const c_char, *const c_char),
}

/// What each extension's `host` pointer really points at: the public struct, then the host's own
/// bookkeeping, so a service knows which extension called it.
#[repr(C)]
struct HostBlock {
    api: HostApi,
    ext: usize,
}

#[repr(C)]
struct NativeDesc {
    size: u32,
    name: *const c_char,
    func: *mut c_void,
    ret: u32,
    args: *const u32,
    nargs: u32,
    flags: u32,
    doc: *const c_char,
    arg_names: *const *const c_char,
}

#[repr(C)]
struct RegistrarApi {
    size: u32,
    native: extern "C" fn(*mut Registrar, *const NativeDesc) -> i32,
}

#[repr(C)]
struct ExtInfo {
    size: u32,
    abi: u32,
    name: *const c_char,
    version: *const c_char,
}

/// The host side of `S2Registrar`: what `s2ext_init` registered, or why it was refused.
struct Registrar {
    ext: usize,
    natives: Vec<&'static Native>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------------------------

/// One registered native. Leaked: extensions are never unloaded, and V8 keeps pointers to it.
pub(crate) struct Native {
    pub name: String,
    pub func: *const c_void,
    pub ret: Type,
    pub args: Vec<Type>,
    pub arg_names: Vec<String>,
    pub flags: u32,
    pub doc: String,
    /// The fast-API signature, when every type is fast-eligible and the native cannot fail.
    pub fast: Option<&'static CFunctionInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Loaded,
    Refused(String),
}

pub(crate) struct Extension {
    pub file: PathBuf,
    pub name: String,
    pub version: String,
    pub state: State,
    pub natives: Vec<&'static Native>,
}

thread_local! {
    static EXTENSIONS: RefCell<Vec<Extension>> = const { RefCell::new(Vec::new()) };
    static LOADED_FROM: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// plugin id → (extension name → range), from each loaded manifest.
    static DECLARED: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
    /// True while a `FLAG_CAN_FAIL` native runs; `fail` records only then.
    static IN_FAILABLE: Cell<bool> = const { Cell::new(false) };
    static PENDING_FAIL: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// The extension whose `s2ext_init` is running, if any.
    static INITIALIZING: Cell<Option<usize>> = const { Cell::new(None) };
    static GAME_THREAD: Cell<bool> = const { Cell::new(false) };
}

fn lossy(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn ext_name(ext: usize) -> String {
    EXTENSIONS.with(|e| e.borrow().get(ext).map(|x| x.name.clone()).unwrap_or_else(|| "?".into()))
}

fn on_game_thread() -> bool {
    GAME_THREAD.with(Cell::get)
}

// ---------------------------------------------------------------------------------------------
// Host services
// ---------------------------------------------------------------------------------------------

fn block_ext(host: *const HostApi) -> Option<usize> {
    if host.is_null() {
        return None;
    }
    // Only ever handed out as the first field of a leaked `HostBlock`.
    Some(unsafe { (*(host as *const HostBlock)).ext })
}

extern "C" fn host_log(host: *const HostApi, level: i32, message: *const c_char) {
    let Some(ext) = block_ext(host) else { return };
    if !on_game_thread() {
        return;
    }
    let tag = match level {
        0 => "",
        1 => "WARN: ",
        _ => "ERROR: ",
    };
    let msg = lossy(message).unwrap_or_default();
    crate::v8host::log_warn(&format!("{tag}[ext {}] {msg}", ext_name(ext)));
}

extern "C" fn host_schema_offset(_host: *const HostApi, class: *const c_char, field: *const c_char) -> i32 {
    if !on_game_thread() {
        return -1;
    }
    let (Some(class), Some(field)) = (lossy(class), lossy(field)) else { return -1 };
    crate::v8host::schema_offset_cached(&class, &field)
}

fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 32 && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

extern "C" fn host_fail(host: *const HostApi, code: *const c_char, message: *const c_char) {
    let Some(ext) = block_ext(host) else { return };
    if !on_game_thread() {
        return;
    }
    if !IN_FAILABLE.with(Cell::get) {
        crate::v8host::log_warn(&format!(
            "WARN: [ext {}] fail() called outside a native registered with S2N_CAN_FAIL; ignored",
            ext_name(ext)
        ));
        return;
    }
    let code = lossy(code).filter(|c| valid_code(c)).unwrap_or_else(|| "failed".into());
    let message = lossy(message).unwrap_or_default();
    PENDING_FAIL.with(|p| *p.borrow_mut() = Some((code, message)));
}

/// Run a native's call with `fail` armed when `flags` allow it; returns the recorded failure.
pub(crate) fn with_fail_scope<R>(flags: u32, call: impl FnOnce() -> R) -> (R, Option<(String, String)>) {
    if flags & FLAG_CAN_FAIL == 0 {
        return (call(), None);
    }
    let outer = IN_FAILABLE.with(|f| f.replace(true));
    let r = call();
    IN_FAILABLE.with(|f| f.set(outer));
    (r, PENDING_FAIL.with(|p| p.borrow_mut().take()))
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && s.len() <= 64
}

/// The npm name grammar, lowercase: `name` or `@scope/name`.
fn valid_package_name(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && !p.starts_with('.')
            && !p.starts_with('_')
            && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~".contains(&b))
    };
    if s.len() > 214 {
        return false;
    }
    match s.strip_prefix('@') {
        Some(rest) => matches!(rest.split_once('/'), Some((scope, name)) if part(scope) && part(name)),
        None => part(s),
    }
}

fn check_desc(d: &NativeDesc) -> Result<(String, Vec<Type>, Type), String> {
    let name = lossy(d.name).ok_or("a native has no name")?;
    if !valid_ident(&name) {
        return Err(format!("native {name:?}: the name must be a JavaScript identifier of at most 64 characters"));
    }
    if d.func.is_null() {
        return Err(format!("native {name:?}: function pointer is null"));
    }
    let ret = Type::from_raw(d.ret).ok_or_else(|| format!("native {name:?}: unknown result type {}", d.ret))?;
    if ret == Type::Entity {
        return Err(format!("native {name:?}: an entity cannot be a result"));
    }
    if d.nargs > 32 {
        return Err(format!("native {name:?}: {} arguments", d.nargs));
    }
    if d.nargs > 0 && d.args.is_null() {
        return Err(format!("native {name:?}: nargs is {} but args is null", d.nargs));
    }
    let raw: &[u32] = if d.nargs == 0 { &[] } else { unsafe { std::slice::from_raw_parts(d.args, d.nargs as usize) } };
    let mut args = Vec::with_capacity(raw.len());
    for (i, t) in raw.iter().enumerate() {
        match Type::from_raw(*t) {
            Some(Type::Void) => return Err(format!("native {name:?}: argument {i} is void")),
            Some(t) => args.push(t),
            None => return Err(format!("native {name:?}: argument {i} has unknown type {t}")),
        }
    }
    let floats = args.iter().filter(|t| t.is_float()).count();
    if args.len() - floats > MAX_INT_ARGS {
        return Err(format!("native {name:?}: more than {MAX_INT_ARGS} integer-class arguments"));
    }
    if floats > MAX_FLOAT_ARGS {
        return Err(format!("native {name:?}: more than {MAX_FLOAT_ARGS} float arguments"));
    }
    if d.flags & !KNOWN_FLAGS != 0 {
        return Err(format!("native {name:?}: unknown flags {:#x}", d.flags & !KNOWN_FLAGS));
    }
    Ok((name, args, ret))
}

fn fast_info(args: &[Type], ret: Type, flags: u32) -> Option<&'static CFunctionInfo> {
    if flags & FLAG_CAN_FAIL != 0 {
        return None;
    }
    let mut infos = vec![FastType::V8Value.as_info()];
    for a in args {
        infos.push(a.fast()?);
    }
    let ret = ret.fast()?;
    let infos: &'static [CTypeInfo] = Box::leak(infos.into_boxed_slice());
    Some(Box::leak(Box::new(CFunctionInfo::new(ret, infos, Int64Representation::Number))))
}

extern "C" fn registrar_native(reg: *mut Registrar, desc: *const NativeDesc) -> i32 {
    if reg.is_null() || desc.is_null() {
        return 1;
    }
    let reg = unsafe { &mut *reg };
    if INITIALIZING.with(Cell::get) != Some(reg.ext) {
        return 1;
    }
    if reg.error.is_some() {
        return 1;
    }
    let size = unsafe { (*desc).size } as usize;
    if size < std::mem::size_of::<NativeDesc>() {
        reg.error = Some(format!("S2NativeDesc.size is {size}, expected at least {}", std::mem::size_of::<NativeDesc>()));
        return 1;
    }
    let d = unsafe { &*desc };
    let (name, args, ret) = match check_desc(d) {
        Ok(v) => v,
        Err(e) => {
            reg.error = Some(e);
            return 1;
        }
    };
    if reg.natives.iter().any(|n| n.name == name) {
        reg.error = Some(format!("native {name:?} is registered twice"));
        return 1;
    }
    let arg_names: Vec<String> = (0..args.len())
        .map(|i| {
            let named = if d.arg_names.is_null() { None } else { lossy(unsafe { *d.arg_names.add(i) }) };
            named.filter(|n| valid_ident(n)).unwrap_or_else(|| format!("arg{i}"))
        })
        .collect();
    let native = Native {
        fast: fast_info(&args, ret, d.flags),
        name,
        func: d.func,
        ret,
        args,
        arg_names,
        flags: d.flags,
        doc: lossy(d.doc).unwrap_or_default(),
    };
    reg.natives.push(Box::leak(Box::new(native)));
    0
}

// ---------------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------------

const RTLD_NOW: i32 = 2;
const RTLD_LOCAL: i32 = 0;

fn dl_error() -> String {
    let e = unsafe { libc::dlerror() };
    lossy(e).unwrap_or_else(|| "unknown dlopen error".into())
}

/// Load every `*.so` directly under `dir`, once per process. Later calls are no-ops.
pub(crate) fn load_dir(dir: &Path) {
    let first = LOADED_FROM.with(|l| {
        let mut l = l.borrow_mut();
        if l.is_some() {
            return false;
        }
        *l = Some(dir.to_path_buf());
        true
    });
    if !first {
        return;
    }
    GAME_THREAD.with(|g| g.set(true));
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        // `is_file` follows symlinks: an extension installed as a link loads like a copy.
        .filter(|p| p.extension().is_some_and(|x| x == "so") && p.is_file())
        .collect();
    files.sort();
    for file in files {
        load_one(&file);
    }
}

fn push(file: &Path, name: String, version: String, state: State, natives: Vec<&'static Native>) -> usize {
    match &state {
        State::Loaded => crate::v8host::log_warn(&format!(
            "[S2S] extension {name}@{version} loaded ({} native{})",
            natives.len(),
            if natives.len() == 1 { "" } else { "s" }
        )),
        State::Refused(reason) => crate::v8host::log_warn(&format!(
            "WARN: extension {} refused: {reason}",
            if name.is_empty() { file.display().to_string() } else { name.clone() }
        )),
    }
    EXTENSIONS.with(|e| {
        let mut e = e.borrow_mut();
        e.push(Extension { file: file.to_path_buf(), name, version, state, natives });
        e.len() - 1
    })
}

fn load_one(file: &Path) {
    if !cfg!(all(target_arch = "x86_64", unix)) {
        push(file, String::new(), String::new(), State::Refused("native extensions need x86-64 Linux in ABI 1".into()), vec![]);
        return;
    }
    let Ok(path) = CString::new(file.as_os_str().as_encoded_bytes()) else {
        push(file, String::new(), String::new(), State::Refused("path contains NUL".into()), vec![]);
        return;
    };
    let handle = unsafe { libc::dlopen(path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
    if handle.is_null() {
        push(file, String::new(), String::new(), State::Refused(format!("dlopen: {}", dl_error())), vec![]);
        return;
    }
    let sym = |n: &CStr| unsafe { libc::dlsym(handle, n.as_ptr()) };
    let (info_sym, init_sym) = (sym(c"s2ext_info"), sym(c"s2ext_init"));
    if info_sym.is_null() || init_sym.is_null() {
        unsafe { libc::dlclose(handle) };
        push(file, String::new(), String::new(), State::Refused("does not export s2ext_info and s2ext_init".into()), vec![]);
        return;
    }
    let info_fn: extern "C" fn() -> *const ExtInfo = unsafe { std::mem::transmute(info_sym) };
    let info = info_fn();
    let refuse_before_init = |reason: String, name: String, version: String| {
        unsafe { libc::dlclose(handle) };
        push(file, name, version, State::Refused(reason), vec![]);
    };
    if info.is_null() {
        return refuse_before_init("s2ext_info returned null".into(), String::new(), String::new());
    }
    let info = unsafe { &*info };
    let min = std::mem::size_of::<ExtInfo>() as u32;
    if info.size < min {
        return refuse_before_init(format!("S2ExtInfo.size is {}, expected at least {min}", info.size), String::new(), String::new());
    }
    let name = lossy(info.name).unwrap_or_default();
    let version = lossy(info.version).unwrap_or_default();
    if info.abi != ABI_VERSION {
        return refuse_before_init(format!("built for extension ABI {}, this host speaks ABI {ABI_VERSION}", info.abi), name, version);
    }
    if !valid_package_name(&name) {
        return refuse_before_init(format!("name {name:?} is not a valid package name"), name, version);
    }
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return refuse_before_init(format!("version {version:?} is not semver"), name, version);
    }
    let duplicate = EXTENSIONS.with(|e| e.borrow().iter().any(|x| x.name == name && x.state == State::Loaded));
    if duplicate {
        return refuse_before_init(format!("another file already loaded {name}"), name, version);
    }
    let ext = EXTENSIONS.with(|e| e.borrow().len());
    let host: &'static HostBlock = Box::leak(Box::new(HostBlock {
        api: HostApi {
            size: std::mem::size_of::<HostApi>() as u32,
            abi: ABI_VERSION,
            host_version: Box::leak(CString::new(env!("CARGO_PKG_VERSION")).unwrap().into_boxed_c_str()).as_ptr(),
            log: host_log,
            schema_offset: host_schema_offset,
            fail: host_fail,
        },
        ext,
    }));
    let rapi = RegistrarApi { size: std::mem::size_of::<RegistrarApi>() as u32, native: registrar_native };
    let mut reg = Registrar { ext, natives: Vec::new(), error: None };
    let init: extern "C" fn(*const HostApi, *mut Registrar, *const RegistrarApi) -> i32 = unsafe { std::mem::transmute(init_sym) };
    // The name is reserved while init runs so its log lines carry it.
    push_placeholder(file, &name, &version);
    INITIALIZING.with(|i| i.set(Some(ext)));
    let rc = init(&host.api, &mut reg, &rapi);
    INITIALIZING.with(|i| i.set(None));
    // From here on the library stays mapped whatever happens: init may have started threads or
    // handed out pointers.
    let state = match (rc, reg.error.take()) {
        (_, Some(e)) => State::Refused(e),
        (0, None) => State::Loaded,
        (rc, None) => State::Refused(format!("s2ext_init returned {rc}")),
    };
    let natives = if state == State::Loaded { reg.natives } else { Vec::new() };
    EXTENSIONS.with(|e| {
        e.borrow_mut().pop();
    });
    push(file, name, version, state, natives);
}

fn push_placeholder(file: &Path, name: &str, version: &str) {
    EXTENSIONS.with(|e| {
        e.borrow_mut().push(Extension {
            file: file.to_path_buf(),
            name: name.to_string(),
            version: version.to_string(),
            state: State::Refused("initializing".into()),
            natives: Vec::new(),
        })
    });
}

// ---------------------------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------------------------

fn find_loaded(name: &str) -> Option<(usize, String)> {
    EXTENSIONS.with(|e| {
        e.borrow()
            .iter()
            .enumerate()
            .find(|(_, x)| x.name == name && x.state == State::Loaded)
            .map(|(i, x)| (i, x.version.clone()))
    })
}

fn unavailable_reason(name: &str, range: &str) -> Option<String> {
    match find_loaded(name) {
        Some((_, version)) if crate::interfaces::version_satisfies(range, &version) => None,
        Some((_, version)) => Some(format!("extension {name} is {version}, the plugin needs {range}")),
        None => {
            let refused = EXTENSIONS.with(|e| {
                e.borrow().iter().find(|x| x.name == name).and_then(|x| match &x.state {
                    State::Refused(r) => Some(r.clone()),
                    State::Loaded => None,
                })
            });
            Some(match refused {
                Some(r) => format!("extension {name} was refused at startup: {r}"),
                None => format!("extension {name} is not installed (its .so goes in addons/s2script/extensions/)"),
            })
        }
    }
}

/// The load gate: every hard extension must be loaded at a satisfying version.
pub(crate) fn check_manifest(hard: &HashMap<String, String>) -> Result<(), String> {
    let mut names: Vec<&String> = hard.keys().collect();
    names.sort();
    for name in names {
        if let Some(reason) = unavailable_reason(name, &hard[name]) {
            return Err(reason);
        }
    }
    Ok(())
}

/// Record a loading plugin's declarations (replacing a previous version's).
pub(crate) fn register_plugin(id: &str, extensions: &HashMap<String, String>) {
    DECLARED.with(|d| {
        let mut d = d.borrow_mut();
        if extensions.is_empty() {
            d.remove(id);
        } else {
            d.insert(id.to_string(), extensions.clone());
        }
    });
}

pub(crate) enum Resolution {
    /// The plugin did not declare this name as an extension.
    Undeclared,
    /// Declared, but not loaded at a satisfying version (the load gate makes this unreachable for
    /// a running plugin): `null`.
    Unavailable,
    Module(usize),
}

pub(crate) fn resolve(plugin: &str, name: &str) -> Resolution {
    let Some(range) = DECLARED.with(|d| d.borrow().get(plugin).and_then(|m| m.get(name).cloned())) else {
        return Resolution::Undeclared;
    };
    match find_loaded(name) {
        Some((i, version)) if crate::interfaces::version_satisfies(&range, &version) => Resolution::Module(i),
        _ => Resolution::Unavailable,
    }
}

pub(crate) fn natives_of(ext: usize) -> Vec<&'static Native> {
    EXTENSIONS.with(|e| e.borrow().get(ext).map(|x| x.natives.clone()).unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------
// The generic call
// ---------------------------------------------------------------------------------------------

/// One argument, already converted.
#[derive(Clone, Copy)]
pub(crate) enum Arg {
    Int(u64),
    F32(f32),
    F64(f64),
}

/// A native's raw result registers.
pub(crate) struct Raw {
    pub rax: u64,
    pub xmm0: f64,
}

/// Call `func(recv = null, args…)` by the System V rules: integer-class arguments fill rsi, rdx,
/// rcx, r8, r9 in order after the receiver in rdi; float arguments fill xmm0–xmm7 in order. The
/// registration checks guarantee they all fit, so nothing goes on the stack.
///
/// # Safety
/// `func` must be a native registered with exactly this argument list.
#[cfg(all(target_arch = "x86_64", unix))]
pub(crate) unsafe fn call(func: *const c_void, args: &[Arg]) -> Raw {
    let mut ints = [0u64; 6];
    let mut floats = [0f64; 8];
    let (mut ni, mut nf) = (1usize, 0usize);
    for a in args {
        match *a {
            Arg::Int(v) => {
                ints[ni] = v;
                ni += 1;
            }
            // A float travels in the low 32 bits of its vector register.
            Arg::F32(v) => {
                floats[nf] = f64::from_bits(v.to_bits() as u64);
                nf += 1;
            }
            Arg::F64(v) => {
                floats[nf] = v;
                nf += 1;
            }
        }
    }
    let rax: u64;
    let xmm0: f64;
    std::arch::asm!(
        "call {f}",
        f = in(reg) func,
        in("rdi") ints[0],
        in("rsi") ints[1],
        in("rdx") ints[2],
        in("rcx") ints[3],
        in("r8") ints[4],
        in("r9") ints[5],
        inout("xmm0") floats[0] => xmm0,
        in("xmm1") floats[1],
        in("xmm2") floats[2],
        in("xmm3") floats[3],
        in("xmm4") floats[4],
        in("xmm5") floats[5],
        in("xmm6") floats[6],
        in("xmm7") floats[7],
        // al = vector registers used, which a variadic callee reads; harmless otherwise.
        inout("rax") nf as u64 => rax,
        clobber_abi("C"),
    );
    Raw { rax, xmm0 }
}

#[cfg(not(all(target_arch = "x86_64", unix)))]
pub(crate) unsafe fn call(_func: *const c_void, _args: &[Arg]) -> Raw {
    unreachable!("load_one refuses every extension on this target")
}

// ---------------------------------------------------------------------------------------------
// Typings and status
// ---------------------------------------------------------------------------------------------

/// The `.d.ts` for a loaded extension, generated from what it registered.
pub(crate) fn types(name: &str) -> Option<String> {
    let (ext, version) = find_loaded(name)?;
    let natives = natives_of(ext);
    let mut out = format!(
        "// Generated by s2script from the native extension {name} {version} (ABI {ABI_VERSION}).\n// Regenerate with `sm ext types {name}` on a server that has it installed.\n"
    );
    let entity = natives.iter().any(|n| n.args.contains(&Type::Entity));
    if entity {
        out.push_str("import type { EntityRef } from \"@s2script/sdk/entity\";\n");
    }
    out.push('\n');
    for n in &natives {
        let mut doc: Vec<String> = n.doc.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).collect();
        if n.flags & FLAG_CAN_FAIL != 0 {
            doc.push("@throws {Error} an `S2Error` when the extension reports a failure.".into());
        }
        if n.args.contains(&Type::Entity) {
            doc.push("@throws {Error} an `S2Error` with code `\"stale_entity\"` when an entity is no longer live.".into());
        }
        if !doc.is_empty() {
            out.push_str("/**\n");
            for l in doc {
                out.push_str(&format!(" * {}\n", l.replace("*/", "* /")));
            }
            out.push_str(" */\n");
        }
        let params: Vec<String> = n.args.iter().zip(&n.arg_names).map(|(t, a)| format!("{a}: {}", t.ts(false))).collect();
        out.push_str(&format!("export declare function {}({}): {};\n", n.name, params.join(", "), n.ret.ts(true)));
    }
    Some(out)
}

/// `sm ext list`.
pub(crate) fn list_lines() -> Vec<String> {
    let running: Vec<String> = crate::loader::plugin_list().into_iter().map(|(id, _)| id).collect();
    let users = |name: &str| -> Vec<String> {
        DECLARED.with(|d| {
            let d = d.borrow();
            let mut v: Vec<String> = d
                .iter()
                .filter(|(id, m)| m.contains_key(name) && running.contains(id))
                .map(|(id, _)| id.clone())
                .collect();
            v.sort();
            v
        })
    };
    EXTENSIONS.with(|e| {
        let e = e.borrow();
        let mut lines = vec![format!("[S2S] Extensions ({}):", e.len())];
        if e.is_empty() {
            let dir = LOADED_FROM.with(|l| l.borrow().clone());
            lines.push(match dir {
                Some(d) => format!("[S2S] No extension files in {}.", d.display()),
                None => "[S2S] The extension directory has not been scanned.".into(),
            });
        }
        for (i, x) in e.iter().enumerate() {
            let label = if x.name.is_empty() { x.file.display().to_string() } else { format!("{}@{}", x.name, x.version) };
            lines.push(match &x.state {
                State::Loaded => {
                    let u = users(&x.name);
                    let used = if u.is_empty() { String::new() } else { format!(", used by {}", u.join(", ")) };
                    format!("  {} {label} ({} natives{used})", i + 1, x.natives.len())
                }
                State::Refused(r) => format!("  {} {label} (refused: {r})", i + 1),
            });
        }
        lines
    })
}

/// Forget everything (tests). Loaded libraries stay mapped.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    EXTENSIONS.with(|e| e.borrow_mut().clear());
    LOADED_FROM.with(|l| l.borrow_mut().take());
    DECLARED.with(|d| d.borrow_mut().clear());
    PENDING_FAIL.with(|p| p.borrow_mut().take());
    IN_FAILABLE.with(|f| f.set(false));
}

#[cfg(test)]
pub(crate) fn state_of(name_or_file: &str) -> Option<State> {
    EXTENSIONS.with(|e| {
        e.borrow()
            .iter()
            .find(|x| x.name == name_or_file || x.file.file_name().is_some_and(|f| f == name_or_file))
            .map(|x| x.state.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names() {
        for ok in ["fastmath", "@community/fastmath", "@a/b-c.d_e~f", "x1"] {
            assert!(valid_package_name(ok), "{ok}");
        }
        for bad in ["", "@x", "@/x", "Fast", "@a/", "../x", ".x", "_x", "a b", "@a/b/c"] {
            assert!(!valid_package_name(bad), "{bad}");
        }
    }

    #[test]
    fn identifiers_and_codes() {
        assert!(valid_ident("clamp") && valid_ident("_x") && valid_ident("$y1"));
        assert!(!valid_ident("1x") && !valid_ident("a-b") && !valid_ident(""));
        assert!(valid_code("bad_number") && !valid_code("Bad") && !valid_code("") && !valid_code(&"a".repeat(33)));
    }
}
