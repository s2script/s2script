#!/usr/bin/env bash
# Build s2script against the CS2 server runtime (Steam Runtime 3 sniper = Debian
# bullseye, glibc 2.31). Run INSIDE a rust:bullseye container with the repo at /repo.
# Host-built (Arch glibc 2.43) binaries require GLIBC_2.38/2.34 and won't load on
# the server; this produces binaries needing <= GLIBC_2.31.
# note: no `pipefail` — the `objdump | ... | tail` glibc checks SIGPIPE harmlessly
set -eu

echo "=== install C/C++ build deps (g++ 10, binutils, curl) ==="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq build-essential binutils curl >/dev/null

# bullseye ships cmake 3.18; the shim needs >= 3.20. Drop in a newer cmake binary.
echo "=== install cmake 3.28 (bullseye's 3.18 is too old) ==="
curl -fsSL https://github.com/Kitware/CMake/releases/download/v3.28.6/cmake-3.28.6-linux-x86_64.tar.gz | tar xz -C /opt
export PATH="/opt/cmake-3.28.6-linux-x86_64/bin:$PATH"

cd /repo
echo "=== container toolchain ==="
gcc --version | head -1; cmake --version | head -1; cargo --version; ldd --version | head -1

echo "=== force a CLEAN core relink in bullseye (host artifact looks up-to-date to cargo) ==="
cargo clean -p s2script-core --release 2>/dev/null || true
echo "=== build Rust core cdylib (relinks v8 149.4.0 prebuilt against glibc 2.31) ==="
cargo build --release -p s2script-core

echo "=== CORE glibc requirement (the decisive V8-prebuilt check; must be <= 2.31) ==="
objdump -T target/release/libs2script_core.so | grep -oE 'GLIBC_[0-9.]+' | sort -V | tail -3

echo "=== build C++ shim (links the just-built core) ==="
rm -rf build/shim
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build build/shim -j

echo "=== package addon ==="
./scripts/package-addon.sh

echo "=== GLIBC requirement after bullseye build (must be <= 2.31) ==="
echo -n "s2script.so      needs: "; objdump -T build/shim/s2script.so | grep -oE 'GLIBC_[0-9.]+' | sort -V | tail -1
echo -n "libs2script_core needs: "; objdump -T target/release/libs2script_core.so | grep -oE 'GLIBC_[0-9.]+' | sort -V | tail -1

# return ownership to the host user that owns the bind-mounted repo
# (container runs as root; hardcoded 1000:1000 breaks GitHub Actions uid 1001)
HOST_UID=$(stat -c %u /repo)
HOST_GID=$(stat -c %g /repo)
chown -R "$HOST_UID:$HOST_GID" /repo/target /repo/build /repo/dist /repo/docker/metamod 2>/dev/null || true
echo "=== DONE ==="
