.PHONY: all core shim package check-boundary docker-test clean ci ci-native ci-js

all: core shim package

core:
	cargo build --release

check-boundary:
	./scripts/check-core-boundary.sh

shim: core
	cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release
	cmake --build build/shim -j

package:
	./scripts/package-addon.sh

docker-test:
	docker compose -f docker/docker-compose.yml up

clean:
	cargo clean
	rm -rf build dist

# The gate suite. These scripts are exactly what CI runs — local green means CI green.
# npm ci is skipped on a local run; use `CI=1 make ci-js` to include the lockfile guard.
ci-js:
	./scripts/ci-js.sh

ci-native:
	./scripts/ci-native.sh

# Both suites — the command CLAUDE.md tells you to run before every PR.
ci: ci-native ci-js
