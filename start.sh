#!/usr/bin/env bash

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
result_dir="$script_dir/result"

export XDG_DATA_DIRS="$result_dir/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
export FCITX_ADDON_DIRS="${FCITX_ADDON_DIRS:+$FCITX_ADDON_DIRS:}$result_dir/lib/fcitx5"
fcitx5 -r
