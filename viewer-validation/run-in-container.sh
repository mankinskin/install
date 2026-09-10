#!/usr/bin/env bash
# Runs inside the viewer-validation image. Proves the relocated
# workflow-tools/viewer-api submodule builds cleanly, native and wasm,
# from a fresh full-repo checkout (no context-engine-local path involved).
set -euo pipefail

echo "[viewer-validation] cargo check -p viewer-api"
cargo check -p viewer-api

echo "[viewer-validation] cargo check --target wasm32-unknown-unknown -p viewer-api-dioxus"
cargo check --target wasm32-unknown-unknown -p viewer-api-dioxus

echo "[viewer-validation] OK: viewer-api and viewer-api-dioxus build cleanly from workflow-tools/viewer-api"
