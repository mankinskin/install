# syntax=docker/dockerfile:1
#
# Minimal-consumer installation validation: proves the public bootstrap
# contract (install.sh -> install-ctl, bootstrap.sh -> ticket/spec CLIs)
# against a freshly cloned workflow-minimal-demo consumer, independent of
# any local monorepo checkout path.
#
# The same image also backs the deterministic guidance fixture contract in
# run-docker-guidance-fixtures.sh, which overrides ENTRYPOINT at `docker run`
# time to run run-guidance-fixtures.sh instead of the network smoke path
# below.
ARG RUST_BASE_IMAGE=rust:1.91-bookworm

FROM ${RUST_BASE_IMAGE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workflow-tools
COPY . /workflow-tools

ENTRYPOINT ["bash", "install/docker-validation/run-in-container.sh"]
