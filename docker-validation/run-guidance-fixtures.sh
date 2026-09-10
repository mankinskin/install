#!/usr/bin/env bash
# Deterministic fixture contract for the guidance installer/audit pipeline
# (install-ctl guidance plan/install/autofix + audit-api markdown_links).
# Runs entirely against temp-directory fixtures baked into the image; never
# clones an external repository, reaches a live service, or touches host
# agent/system directories or ticket/spec/test/session entity stores. This
# is intentionally separate from run-in-container.sh's network smoke path.
set -euo pipefail

cd /workflow-tools

manifest="install/guidance-fixtures/v1/manifest.toml"
test -s "$manifest" || {
    echo "[guidance-fixtures] FAIL: missing versioned fixture manifest: $manifest" >&2
    exit 1
}
echo "[guidance-fixtures] manifest: $manifest"

run_fixture_suite() {
    local label="$1"
    shift
    echo "[guidance-fixtures] running: $label ($*)"
    if ! "$@"; then
        echo "[guidance-fixtures] FAIL: $label" >&2
        exit 1
    fi
    echo "[guidance-fixtures] OK: $label"
}

run_fixture_suite "install-ctl guidance fixtures" cargo test -p install-ctl guidance
run_fixture_suite "audit-api markdown_links fixtures" cargo test -p audit-api markdown_links

echo "[guidance-fixtures] OK: deterministic fixture contract passed"
