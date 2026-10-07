#!/usr/bin/env bash
# Builds the docker-validation image and runs the deterministic guidance
# fixture contract (install-ctl guidance + audit-api markdown_links) against
# a fresh image, overriding the image's default network-smoke entrypoint.
# Kept separately labelled from run-docker-validation.sh's network smoke
# path per ticket 07601b9b: a failure here is a fixture/contract failure,
# never a network-dependent one.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../.." && pwd)

# shellcheck source=../validation-lib.sh
source "$repo_root/install/validation-lib.sh"

tag=workflow-tools-guidance-fixtures-validation
docker_validation_build "$script_dir/Dockerfile" "$tag" "$repo_root"

echo "[docker-run] Running $tag (deterministic guidance fixture contract)"
docker run --rm --entrypoint bash "${DOCKER_IMAGE_TAG:-$tag}" install/docker-validation/run-guidance-fixtures.sh
