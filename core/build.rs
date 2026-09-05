fn main() {
    // Keep libs2script_core.so resident for the process lifetime so the V8
    // platform survives a Metamod `meta unload` / `meta load` cycle (see ARCHITECTURE §2.1 / spec §5).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-z,nodelete");
    }
}
