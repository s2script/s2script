#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT
g++ -std=c++17 -Wall -Wextra -Werror -I shim/src shim/tests/client_bootstrap_test.cpp -o "$test_dir/client_bootstrap_test"
"$test_dir/client_bootstrap_test"
