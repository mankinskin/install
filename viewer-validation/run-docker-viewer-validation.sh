#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../.." && pwd)
# shellcheck source=../validation-lib.sh
source "$repo_root/install/validation-lib.sh"

docker_validation_build_and_run \
    "$script_dir/Dockerfile" \
    "workflow-tools-viewer-api-validation" \
    "$repo_root" \
    --build-arg "NODE_BASE_IMAGE=${NODE_BASE_IMAGE:-node:20-bookworm-slim}"
