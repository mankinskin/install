# syntax=docker/dockerfile:1
#
# viewer-api validation: proves the relocated workflow-tools/viewer-api
# submodule (and its viewer-api-dioxus frontend crate) build cleanly from a
# full workflow-tools checkout, independent of the old context-engine-local
# path. Shares the RUST_BASE_IMAGE convention with install/docker-validation.
ARG RUST_BASE_IMAGE=rust:1.91-bookworm
ARG NODE_BASE_IMAGE=node:20-bookworm-slim

FROM ${NODE_BASE_IMAGE} AS node

FROM ${RUST_BASE_IMAGE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=node /usr/local /usr/local
# viewer-api-dioxus's WebGPU web-sys bindings require the
# web_sys_unstable_apis cfg flag.
RUN rustup target add wasm32-unknown-unknown \
    && cargo install trunk --locked
ENV RUSTFLAGS="--cfg=web_sys_unstable_apis"

# Anonymous HTTPS clones of public GitHub repos intermittently fail through
# cargo's built-in libgit2 fetcher inside minimal containers; shell out to
# the git CLI instead (same fix docker-validation's consumer clone relies on).
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /workflow-tools
COPY . /workflow-tools

ENTRYPOINT ["bash", "install/viewer-validation/run-in-container.sh"]
