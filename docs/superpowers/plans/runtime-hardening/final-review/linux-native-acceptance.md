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

## Release package

The same compiler environment built the optimized core and release-linked shim, then packaged the addon. All 18 base-plugin builds passed; the release contains the 14 enabled base archives. The shim requires at most GLIBC 2.17 and the core at most 2.30, within the documented 2.31 server ceiling. Core SHA-256: `f1d3519dd9deafd24970d14aa5707664e6dc67382c4011179b1bca9610880331`; shim SHA-256: `5a6b58f96421abc9bfe98a91471911577d5ddc02e4b55bb399f6541501c1326d`.

The 24,473,784-byte compressed release archive has 45 files, each re-read and verified against the [release manifest](final-linux-release-manifest.json). It is a prepared test artifact and has **not** been installed on Nebula. The [release build log](final-local-linux-release.log) records compilation and GLIBC checks. Existing test-server configs, data and live fixtures must be preserved during the owned test installation.
