#!/usr/bin/env bash
set -euo pipefail

crate_name=""
previous=""
for argument in "$@"; do
    if [[ "$previous" == --crate-name ]]; then
        crate_name="$argument"
        break
    fi
    if [[ "$argument" == --crate-name=* ]]; then
        crate_name="${argument#--crate-name=}"
        break
    fi
    previous="$argument"
done

case "$crate_name" in
    editor_app_ui|editor_build_ui|editor_core|editor|compiler|render|map_core|source_formats|native_runtime)
        script_path="$(realpath -- "$0")"
        exec flock --no-fork "${script_path%/*}/../target/hammerr-rustc-heavy.lock" "$@"
        ;;
esac

exec "$@"
