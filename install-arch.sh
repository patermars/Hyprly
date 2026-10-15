#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
cache_root="${XDG_CACHE_HOME:-$HOME/.cache}/hyprly/makepkg"

cd "$repo_root"
exec makepkg -si "$@" \
    "BUILDDIR=$cache_root/build" \
    "SRCDEST=$cache_root/sources"
