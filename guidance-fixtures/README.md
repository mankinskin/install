# Guidance Fixture Contract

The versioned manifest under `v1/manifest.toml` is the source of truth for deterministic guidance installer/audit scenarios. Each entry names a scenario, its owning test, expected outcome, and required assertions.

Fixture tests must use temporary source, target, repository, user, and system roots. They must never write to a developer's real agent configuration or workflow entity stores. Passing fixtures prove the requested closure, destination mapping, rewrites, final audit compatibility, and idempotence. Blocking fixtures prove a stable diagnostic and an unchanged destination.

To add a fixture:

1. Add a manifest entry with a unique id, category, test name, expected outcome, and at least one assertion.
2. Add the focused test under `install-ctl/src/guidance/tests.rs` or the owning audit fixture module.
3. Keep the test network-free and root-injected.
4. Run `cargo test -p install-ctl guidance`, `cargo test -p audit-api markdown_links`, and the Docker guidance fixture runner.
5. Update the expected contract only with a reviewed ticket change; do not weaken an existing assertion to make a failing implementation pass.

The Meta-Workspace network smoke test remains separate from this deterministic contract and cannot replace local fixture evidence.
