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

base_image=${RUST_BASE_IMAGE:-rust:1.91-bookworm}
tag=${DOCKER_IMAGE_TAG:-workflow-tools-guidance-fixtures-validation}

echo "[docker-build] Building $tag"
docker build \
    --build-arg "RUST_BASE_IMAGE=$base_image" \
    -f "$script_dir/Dockerfile" \
    -t "$tag" \
    "$repo_root"

echo "[docker-run] Running $tag (deterministic guidance fixture contract)"
docker run --rm --entrypoint bash "$tag" install/docker-validation/run-guidance-fixtures.sh
