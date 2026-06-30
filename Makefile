.PHONY: all core shim package check-boundary docker-test clean

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
