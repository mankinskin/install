#!/usr/bin/env bash
set -euo pipefail

suite=${1:?validation suite is required}
shift
source_root=${VALIDATION_SOURCE_ROOT:-/source}
cd "$source_root"
case "$suite" in
    cargo)
        manifest=
        previous=
        for argument in "$@"; do
            if [[ "$previous" == --manifest-path ]]; then manifest=$argument; fi
            previous=$argument
        done
        if [[ -z "$manifest" ]]; then
            echo "Cargo suite has no manifest owner" >&2
            exit 2
        fi
        if [[ -d /validation-results ]]; then
            cargo metadata --locked --format-version 1 --manifest-path "$manifest" > /validation-results/cargo-metadata.json
        fi
        exec cargo "$@"
        ;;
    harness-contract)
        exec bash "$source_root/workflow-tools/install/docker-validation/test-platform-suite.sh"
        ;;
    install-smoke|guidance-fixtures)
        cd "$source_root/workflow-tools"
        # Existing installer fixtures use /workflow-tools as their container root.
        ln -s "$PWD" /workflow-tools
        if [[ "$suite" == install-smoke ]]; then
            exec bash install/docker-validation/run-in-container.sh
        fi
        exec bash install/docker-validation/run-guidance-fixtures.sh
        ;;
    *)
        echo "Unsupported validation suite: $suite" >&2
        exit 2
        ;;
esac
