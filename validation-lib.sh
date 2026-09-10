#!/usr/bin/env bash
# Shared build+run driver for workflow-tools Docker validation containers.
# Sourced by install/*/run-docker-*.sh drivers; not meant to be run directly.
set -euo pipefail

# docker_validation_build_and_run <dockerfile> <default_tag> <context_dir> [extra docker build args...]
#
# Builds and runs a validation image using the common RUST_BASE_IMAGE
# build-arg and DOCKER_IMAGE_TAG override convention shared by every
# workflow-tools Docker validation container.
docker_validation_build_and_run() {
    local dockerfile=$1
    local default_tag=$2
    local context_dir=$3
    shift 3

    local base_image=${RUST_BASE_IMAGE:-rust:1.91-bookworm}
    local tag=${DOCKER_IMAGE_TAG:-$default_tag}

    echo "[docker-build] Building $tag"
    docker build \
        --build-arg "RUST_BASE_IMAGE=$base_image" \
        "$@" \
        -f "$dockerfile" \
        -t "$tag" \
        "$context_dir"

    echo "[docker-run] Running $tag"
    docker run --rm "$tag"
}
