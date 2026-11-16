## CI: KHook sniper build sees the runner's borrowed git objects (Nov 16)

Pushes to `main` started failing the `native (runtime)` shard: the runner's checkout began
borrowing objects from a runner-local mirror (`.git/objects/info/alternates`), which the
`rust:bullseye` container behind `scripts/test-khook-sniper-build.sh` did not mount, so
`git describe` in `scripts/framework-version.sh` failed and the shim's CMake configure stopped.
The script now mounts each absolute alternates store read-only at the same path. Pull-request
runs were unaffected.
