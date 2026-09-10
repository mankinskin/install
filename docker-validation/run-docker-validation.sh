#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../.." && pwd)
# shellcheck source=../validation-lib.sh
source "$repo_root/install/validation-lib.sh"

docker_validation_build_and_run \
    "$script_dir/Dockerfile" \
    "workflow-tools-minimal-consumer-install-validation" \
    "$repo_root"
