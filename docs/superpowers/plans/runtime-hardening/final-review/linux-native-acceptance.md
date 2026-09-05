# Final Linux native validation

Source: `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`. This is the independently reviewed production tree; subsequent commits only record evidence.

The complete `scripts/ci-native.sh` exited zero in an isolated Linux/amd64 Docker container on the Mac. Rust 1.98.0, GCC 10.2.1, CMake 3.28.6; four CPU quota and 7 GiB memory. The builder derives from rust:bullseye and has image ID `sha256:16384f64da341d31dd0946a53ce7c109ee514f42ec8b867744de61a9713bc1cc`. Source and submodules were cloned separately; Linux target/cargo volumes did not overwrite the Mac build.

- Core: 797 passed, zero failed, three ignored, 11.40 seconds.
- All three isolated pressure processes pass.
- C++ unit and sanitizer gates, generated source, license freshness, boundary and ABI checks pass.
- The full shim compiles and links, including the extracted config translation unit; its tokenizer test passes.
- All 47 required `s2script_core_*` entry points are defined in the built core library.

The game-library symbol-resolution phase explicitly skipped because this isolated checkout has no CS2 installation. That check, installed-engine execution and the final 60-minute mixed soak still require the designated test environment. Script success is not a claim that the skipped phase or live acceptance passed. The [raw log](final-local-linux-ci-native.log) preserves the skip and complete results.

Initial image setup failed because the Docker credential helper was absent from PATH, then timed out inside the helper. A task-local Docker configuration allowed anonymous retrieval of the public Rust image; the user's Docker configuration was unchanged. No sanitizer or test was disabled to obtain the passing run.
