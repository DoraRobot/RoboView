#!/usr/bin/env bash
# Spec A9 guard: production code must never hardcode a data file path.
#
# Scope: crates/*/src Rust sources. Test fixtures are exempt by design
# (they are in-memory byte arrays and carry no path strings); any file
# path literal ending in .ply/.pcd in production code fails the guard.
set -u

hits=$(grep -rnE "[\"'][^\"']*\.(ply|pcd)['\"]" crates/*/src --include='*.rs' | grep -vE "^\S+:[0-9]+:[[:space:]]*//" || true)

if [ -n "$hits" ]; then
    echo "A9 violation: hardcoded data file path(s) found:"
    echo "$hits"
    exit 1
fi

echo "A9: no hardcoded data file paths."
