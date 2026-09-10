#!/usr/bin/env bash
# Runs inside the docker-validation image. Exercises the public minimal
# consumer install contract end to end: install.sh -> install-ctl,
# bootstrap.sh -> ticket/spec CLIs, against a freshly cloned consumer.
set -euo pipefail

consumer_repository="${CONSUMER_REPOSITORY:-https://github.com/mankinskin/workflow-minimal-demo}"
consumer_dir="/tmp/workflow-minimal-demo"
install_root="/tmp/workflow-tools-install"

echo "[docker-validation] cloning fresh consumer checkout: $consumer_repository"
rm -rf "$consumer_dir"
git clone --depth 1 "$consumer_repository" "$consumer_dir"

echo "[docker-validation] installing install-ctl via install.sh"
bash /workflow-tools/install.sh --root "$install_root"
test -x "$install_root/bin/install-ctl" \
    || { echo "[docker-validation] FAIL: install-ctl not found under $install_root/bin" >&2; exit 1; }
"$install_root/bin/install-ctl" --help >/dev/null

echo "[docker-validation] verifying installer oneliner updates when install-ctl is already installed"
bash /workflow-tools/install.sh --root "$install_root"
test -x "$install_root/bin/install-ctl" \
    || { echo "[docker-validation] FAIL: install-ctl not found after updating via install.sh" >&2; exit 1; }
"$install_root/bin/install-ctl" --help >/dev/null

echo "[docker-validation] bootstrapping consumer via bootstrap.sh"
WORKFLOW_TOOLS_BOOTSTRAP=/workflow-tools/bootstrap.sh bash "$consumer_dir/run-tutorial.sh"

echo "[docker-validation] verifying consumer store isolation"
canonical_store="$consumer_dir/.workflow-tools/ticket"
legacy_store="$consumer_dir/.ticket"
if [[ -d "$canonical_store" ]]; then
    ticket_store="$canonical_store"
elif [[ -d "$legacy_store" ]]; then
    ticket_store="$legacy_store"
else
    echo "[docker-validation] FAIL: no consumer ticket store found under $canonical_store or $legacy_store" >&2
    exit 1
fi

record_count=$("$consumer_dir/.workflow-tools/bin/ticket" \
    --json --index-root "$ticket_store" list --limit 5 \
    | grep -o '"count": *[0-9]*' | head -1 | grep -o '[0-9]*$')
if [[ -z "$record_count" || "$record_count" -lt 1 ]]; then
    echo "[docker-validation] FAIL: expected at least one ticket record, found '$record_count'" >&2
    exit 1
fi

echo "[docker-validation] OK: $record_count ticket record(s) read back from consumer store"

echo "[docker-validation] testing install.sh --uninstall"
bash /workflow-tools/install.sh --root "$install_root" --uninstall
if [[ -f "$install_root/bin/install-ctl" ]]; then
    echo "[docker-validation] FAIL: install-ctl still present after install.sh --uninstall" >&2
    exit 1
fi

echo "[docker-validation] re-installing install-ctl for self-uninstall test"
bash /workflow-tools/install.sh --root "$install_root"
test -x "$install_root/bin/install-ctl"

echo "[docker-validation] testing install-ctl self-uninstall"
"$install_root/bin/install-ctl" self-uninstall --root "$install_root"
if [[ -f "$install_root/bin/install-ctl" ]]; then
    echo "[docker-validation] FAIL: install-ctl still present after self-uninstall" >&2
    exit 1
fi

echo "[docker-validation] OK: uninstallation contract verified"
