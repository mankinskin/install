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

echo "[guidance-fixtures] testing autofix CLI plan/apply lifecycle"
autofix_root=$(mktemp -d)
trap 'rm -rf "$autofix_root"' EXIT
mkdir -p "$autofix_root/.agents"
printf '# target\n' > "$autofix_root/.agents/README.md"
printf '[target](missing.md)\n' > "$autofix_root/.agents/links.md"

plan_output=$(cargo run -p install-ctl --quiet -- guidance autofix \
    --repo-root "$autofix_root" \
    --plan \
    --rewrite missing.md=README.md)
printf '%s\n' "$plan_output"
grep -F "guidance autofix plan: 1 operation(s), 0 unresolved" <<<"$plan_output" >/dev/null \
    || { echo "[guidance-fixtures] FAIL: autofix plan did not produce one operation" >&2; exit 1; }

apply_output=$(cargo run -p install-ctl --quiet -- guidance autofix \
    --repo-root "$autofix_root" \
    --apply \
    --yes \
    --rewrite missing.md=README.md)
printf '%s\n' "$apply_output"
grep -F "guidance autofix apply: 1 applied, 0 refused, rolled_back=false" <<<"$apply_output" >/dev/null \
    || { echo "[guidance-fixtures] FAIL: autofix did not apply the planned rewrite" >&2; exit 1; }
grep -F '[target](README.md)' "$autofix_root/.agents/links.md" >/dev/null \
    || { echo "[guidance-fixtures] FAIL: autofix did not rewrite the source file" >&2; exit 1; }

second_apply_output=$(cargo run -p install-ctl --quiet -- guidance autofix \
    --repo-root "$autofix_root" \
    --apply \
    --yes \
    --rewrite missing.md=README.md)
printf '%s\n' "$second_apply_output"
grep -F "guidance autofix apply: 0 applied, 0 refused, rolled_back=false" <<<"$second_apply_output" >/dev/null \
    || { echo "[guidance-fixtures] FAIL: autofix was not idempotent" >&2; exit 1; }

unresolved_root=$(mktemp -d)
mkdir -p "$unresolved_root/.agents"
printf '[gone](missing.md)\n' > "$unresolved_root/.agents/links.md"
if cargo run -p install-ctl --quiet -- guidance autofix \
    --repo-root "$unresolved_root" \
    --apply \
    --yes >"$unresolved_root/output.txt" 2>&1; then
    cat "$unresolved_root/output.txt"
    echo "[guidance-fixtures] FAIL: unresolved autofix unexpectedly succeeded" >&2
    exit 1
fi
grep -F "no safe operations" "$unresolved_root/output.txt" >/dev/null \
    || { cat "$unresolved_root/output.txt"; echo "[guidance-fixtures] FAIL: unresolved autofix error was not actionable" >&2; exit 1; }
rm -rf "$unresolved_root"
echo "[guidance-fixtures] OK: autofix CLI plan/apply lifecycle"

echo "[guidance-fixtures] OK: deterministic fixture contract passed"
