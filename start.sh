#!/usr/bin/env bash

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
result_dir="$script_dir/result1" # result is for nix, result1 is for local build by ./build.sh
result_dir="${1:-$result_dir}"

fcitx_command="$(command -v fcitx5)"
fcitx_resolved="$(readlink -f -- "$fcitx_command")"
fcitx_unwrapped="$(dirname -- "$fcitx_resolved")/.fcitx5-wrapped"

if [[ -x "$fcitx_unwrapped" ]]; then
    # Nixのfcitx5 wrapperは自身のaddon directoryをFCITX_ADDON_DIRSの
    # 先頭へ追加するため、開発版よりsystem側の同名addonが優先される。
    # wrapper本体を迂回し、同じpackageの探索先を開発版の後ろへ配置する。
    fcitx_prefix="$(dirname -- "$(dirname -- "$fcitx_resolved")")"
    export XDG_DATA_DIRS="$result_dir/share:$fcitx_prefix/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
    export FCITX_ADDON_DIRS="$result_dir/lib/fcitx5:$fcitx_prefix/lib/fcitx5${FCITX_ADDON_DIRS:+:$FCITX_ADDON_DIRS}"
    exec "$fcitx_unwrapped" -r
fi

export XDG_DATA_DIRS="$result_dir/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
export FCITX_ADDON_DIRS="$result_dir/lib/fcitx5${FCITX_ADDON_DIRS:+:$FCITX_ADDON_DIRS}"
exec "$fcitx_command" -r
