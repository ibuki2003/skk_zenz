#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
build_dir="$script_dir/build"
result_dir="$script_dir/result2"

cmake \
    -S "$script_dir" \
    -B "$build_dir" \
    -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    "$@"
cmake --build "$build_dir"
cmake --install "$build_dir" --prefix "$result_dir"

echo "Installed fcitx5 addon into: $result_dir"
