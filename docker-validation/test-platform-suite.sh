#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin"
cat > "$scratch/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
if [[ "$1" != metadata ]]; then printf '%s\n' "$@" > "$CAPTURE"; fi
CARGO
cat > "$scratch/bin/docker" <<'DOCKER'
#!/usr/bin/env bash
printf '%s\n' "$@" >> "$DOCKER_CAPTURE"
DOCKER
chmod +x "$scratch/bin/cargo" "$scratch/bin/docker"
export PATH="$scratch/bin:$PATH" CAPTURE="$scratch/cargo.args" DOCKER_CAPTURE="$scratch/docker.args"
export VALIDATION_SOURCE_ROOT=/source
bash "$script_dir/run-platform-suite.sh" cargo test --locked --manifest-path workflow-tools/Cargo.toml -p install-ctl guidance
printf '%s\n' test --locked --manifest-path workflow-tools/Cargo.toml -p install-ctl guidance > "$scratch/expected"
cmp "$scratch/expected" "$CAPTURE"
if bash "$script_dir/run-platform-suite.sh" unsupported > "$scratch/unknown.log" 2>&1; then
    echo "Unknown suite did not fail" >&2
    exit 1
fi
grep -F 'Unsupported validation suite' "$scratch/unknown.log"
source "$script_dir/../validation-lib.sh"
unset RUST_BASE_IMAGE DOCKER_IMAGE_TAG
docker_validation_build_and_run 'fixture/Dockerfile' fixture-default /fixture --build-arg NODE_BASE_IMAGE=node-fixture
grep -Fx 'RUST_BASE_IMAGE=rust:1.91-bookworm' "$DOCKER_CAPTURE"
grep -Fx 'NODE_BASE_IMAGE=node-fixture' "$DOCKER_CAPTURE"
grep -Fx 'fixture-default' "$DOCKER_CAPTURE"
export RUST_BASE_IMAGE=rust-fixture DOCKER_IMAGE_TAG=tag-fixture
docker_validation_build_and_run 'fixture/Dockerfile' ignored /fixture
grep -Fx 'RUST_BASE_IMAGE=rust-fixture' "$DOCKER_CAPTURE"
grep -Fx 'tag-fixture' "$DOCKER_CAPTURE"
echo 'Container harness contracts: Cargo override, suite rejection, legacy defaults and viewer arguments passed'
