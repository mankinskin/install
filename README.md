# install-ctl

`install-ctl` installs workspace tool binaries, runs the viewer lifecycle,
renders the artifact catalog, and plans/installs/repairs guidance corpora
under `.agents/**`. This file is the canonical operator contract for the
`guidance` command family; `../README.md` and `COMMANDS.md` link here rather
than repeating the same rules.

## Artifact registry and catalog

`artifacts.toml` is the runtime-read registry of installable binaries and
hooks. `COMMANDS.md` at the repository root is its generated projection:

```bash
cargo run -p install-ctl -- catalog --check
```

`guidance plan`/`install`/`autofix`/`get` are subcommands of the
already-registered `install-ctl` binary artifact, not separate artifacts. A
user-authored guidance profile (a `.toml` file naming a corpus and a
destination) is runtime input to those subcommands, never a registry entry —
the catalog projects installable binaries, not profiles a caller supplies on
the command line.

## `guidance` command family

```bash
install-ctl guidance plan      --source <repo> --profile <profile.toml> --select <id>... --target <repo> [--destination-scope repo|user|system|explicit] [--destination-path <path>] [--json]
install-ctl guidance install   --source <repo> --profile <profile.toml> --select <id>... --target <repo> [--destination-scope ...] [--destination-path ...] [--json]
install-ctl guidance uninstall --source <repo> --profile <profile.toml> --select <id>... --target <repo> [--destination-scope ...] [--destination-path ...] [--json]
install-ctl guidance autofix   --repo-root <repo> [--scope <path>] [--rewrite old=new]... (--plan | --apply --yes) [--json]
install-ctl guidance get       <repository-url> --select <id>... [--profile <repo-relative-path>] [--target <repo>] [--destination-scope ...] [--destination-path ...] [--keep-checkout] [--json]
```

- `plan` computes a read-only installation plan: no writes, no recipe
  execution, no network access. It fails with a non-zero exit and prints
  every diagnostic when the plan is blocking.
- `install` recomputes the same plan, then writes only if the plan has zero
  diagnostics. Every artifact's final bytes (including any link rewrite) are
  staged in memory first; a read failure on any one artifact leaves the
  destination completely untouched, and per-file writes are atomic
  (temp file + rename). A second `install` run is idempotent and reports
  unchanged files instead of rewriting them.
- `uninstall` computes the guidance plan and safely removes installed guidance
  files mapped by the plan from the target destination, cleaning up empty parent
  directories up to the destination root.
- `autofix` targets existing blocking guidance-audit findings (missing
  target, non-guidance target, unsupported dependency, unsafe path,
  unreadable artifact). `--plan` is read-only. `--apply` is the only mutation
  path and requires `--yes`. Only a finding whose raw link destination
  exactly matches an explicit `--rewrite old=new` pair becomes a candidate
  operation — autofix never invents a transformation. Before writing, apply
  re-hashes each source file against the hash captured at plan time and
  refuses (does not write) any operation whose source changed since
  (`stale plan`). After applying, it reruns the guidance audit; if any
  touched source still has a blocking finding for the same destination,
  every write from this call is rolled back from its in-memory backup and
  the result reports `rolled_back: true`.
- `get` is the one-command surface: it shallow-clones `<repository-url>`
  with the system `git` binary into a managed temporary checkout, resolves
  a profile (an explicit `--profile <repo-relative-path>`, resolved inside
  the clone — not the caller's filesystem — or, when omitted, a synthesized
  single-item direct-corpus profile from the bare `--select` path(s)), then
  runs the same `plan`/`install` logic unchanged. `--target` defaults to
  `.`. The managed checkout is deleted on every exit path — success,
  blocking plan, or error — unless `--keep-checkout` is given, in which case
  its path is printed. No Git-client crate is added; the clone shells out to
  `git`, styled after `workflow-tools/session/crates/worktree-ctl/src/git.rs`.

## Artifact & Self Uninstallation

Every software artifact and guidance corpus installed by `install-ctl` defines a full removal path:

```bash
install-ctl uninstall <selection> [--root <install-root>]
install-ctl self-uninstall [--root <install-root>]
install-ctl guidance uninstall --source <repo> --profile <profile.toml> --select <id>... [--target <repo>]
install.sh --root <install-root> --uninstall [--dry-run]
```

- `install-ctl uninstall` removes specified binary artifacts (or `all`) from `<install-root>/bin`.
- `install-ctl self-uninstall` removes the `install-ctl` binary itself from `<install-root>/bin`.
- `install-ctl guidance uninstall` computes the guidance plan and safely removes installed guidance files mapped by the plan from the target destination, cleaning up empty parent directories.
- `install.sh --uninstall` provides shell-level removal of `install-ctl` from `<install-root>/bin`, cleaning up empty parent directories.

## Profile schema

A profile is one TOML file; there is no global profile registry:

```toml
[profile]
id = "example"

[[corpus]]
id = "readme"
paths = ["workflow-tools/README.md"]      # direct corpus item

[[corpus]]
id = "generated-doc"
[[corpus.recipe.step]]
kind = "copy"
from = "workflow-tools/README.md"          # repo-relative, source root
to = "workflow-tools/GENERATED.md"          # repo-relative, destination-side id

[destination]
scope = "repo"                              # repo | user | system | explicit
# path = "..."                              # required only when scope = "explicit"
```

- A **direct** corpus item lists repo-relative source paths.
- A **recipe** corpus item has typed steps. Only `kind = "copy"` is
  implemented; any other step `kind` is recorded as an `UnsupportedRecipeStep`
  blocking diagnostic and is never executed (no network access, no shell-out)
  — recipes are declarative, not scripts.
- `to` for a copy step must end in `.md` or `.toml`; a non-guidance target
  is an `UnsafeTarget` diagnostic, not a silent write.

## Dependency closure and diagnostics

Starting from the `--select`ed roots, the planner extracts Markdown links
and classifies each one:

| Link shape | Class | Closure behavior |
|---|---|---|
| `http(s)://`, `mailto:`, `#fragment` | External | Skipped, never added to the closure |
| ends in `.md` | Guidance | Added to the closure (`GuidanceReference` edge) |
| ends in `.toml` | Profile | Added to the closure (`ProfileReference` edge) |
| other relative path | Non-guidance | Skipped, recorded, never installed |
| absolute or drive-qualified path | Absolute source | `AbsoluteSourcePath` diagnostic |

Typed dependency edges (`GuidanceReference`, `ProfileReference`,
`GeneratedInput` for recipe copies, `SubmoduleOwnership`, `Unsupported`,
`Unsafe`) and their status (`Resolved`, `Missing`, `Blocked`) are included in
the plan output for inspection.

A plan is **blocking** (`is_blocking() == true`, non-zero exit, no writes)
when it contains any diagnostic: `MissingDependency`, `Cycle`,
`DuplicateDestination`, `AmbiguousOwnership`, `PathTraversal`,
`AbsoluteSourcePath`, `SymlinkEscape`, `UnsupportedRecipeStep`, or
`UnsafeTarget`. A symlink is resolved to its real path before the
source-root containment check, so a symlink escaping the source root is
rejected the same way a `..` traversal is.

## Destination scopes

| Scope | Root | Notes |
|---|---|---|
| `repo` | `--target` | Prefers an existing initialized submodule (has a `.git` *file*, not directory) that owns the artifact's `.agents/`-relative id; falls back to the canonical `.agents/...` path under the target root when no matching submodule is initialized. Intra-artifact relative links are rewritten to stay valid at the final location. |
| `user` | platform config dir (e.g. `~/.config/install-ctl/guidance`) | Injected in tests via `DestinationPaths`; never a caller's real home directory during a test run. |
| `system` | `/etc/install-ctl/guidance` (Unix) / `C:/ProgramData/install-ctl/guidance` (Windows) | Same injection contract as `user`. |
| `explicit` | `--destination-path` or the profile's `destination.path` | Required for this scope; a path containing a leading `..` that would escape itself is a `PathTraversal` diagnostic, not a partial write. |

`--destination-scope`/`--destination-path` override whatever the profile
declares; the profile's own `[destination]` table is the default.

## Fixture, Docker, and network validation

- `guidance-fixtures/README.md` and `guidance-fixtures/v1/manifest.toml` are
  the deterministic fixture contract: every fixture uses temporary source,
  target, user, and system roots and never touches a real developer
  environment or entity store.
  ```bash
  cargo test -p install-ctl guidance
  cargo test -p audit-api markdown_links
  ```
- `docker-validation/run-docker-guidance-fixtures.sh` builds the validation
  image and runs the same deterministic contract inside a fresh container,
  overriding the image's default entrypoint:
  ```bash
  bash install/docker-validation/run-docker-guidance-fixtures.sh
  ```
- `docker-validation/run-docker-validation.sh` (via `run-in-container.sh`) is
  a **separate, network-dependent smoke path**. It is supplementary evidence
  only and never substitutes for the deterministic fixture contract above.

## Quickstart: install guidance in another repository

The public `install.sh` entry point installs a pinned `install-ctl` binary
into a caller-owned directory. The primary, most important use case is
installing all guidance from `meta-workspace` into an external consumer
repository under its `.agents/` tree:

```bash
curl -fsSL https://raw.githubusercontent.com/mankinskin/workflow-tools/main/install.sh \
  | bash -s -- --root "$HOME/.local/workflow-tools" \
  && "$HOME/.local/workflow-tools/bin/install-ctl" \
       guidance get \
       https://github.com/mankinskin/meta-workspace.git \
       --select context-engine/AGENTS.md \
       --select .agents/agents/implement.agent.md \
       --select workflow-tools/.agents/agents/orchestrator.agent.md \
       --select workflow-tools/.agents/prompts/iteration.prompt.md \
       --target "$PWD" \
       --destination-scope repo
```

The command installs `install-ctl` and runs `guidance get`, which shallow-clones
`meta-workspace`, computes the full transitive Markdown closure of all referenced
instructions, agents, and prompts, installs all guidance files into `$PWD/.agents/`,
and automatically cleans up the temporary clone.

### Install specific selected agents or guidance files

If you only want a subset of agents or guidance files rather than the complete corpus,
`--select` accepts specific repo-relative file paths and can be repeated. When the
source is the `meta-workspace` monorepo, include the appropriate prefix:

```bash
install-ctl guidance get \
  https://github.com/mankinskin/meta-workspace.git \
  --select workflow-tools/.agents/agents/implement.agent.md \
  --select workflow-tools/.agents/agents/research.agent.md \
  --target "$PWD" \
  --destination-scope repo
```

When cloning the standalone `workflow-tools` repository, the same selections
start at `.agents/` instead:

```bash
install-ctl guidance get \
  https://github.com/mankinskin/workflow-tools.git \
  --select .agents/agents/implement.agent.md \
  --select .agents/instructions/workflow/agent-world-model.instructions.md \
  --target "$PWD" \
  --destination-scope repo
```

For a repeatable curated corpus, pass `--profile <repo-relative-path>`; the
profile is resolved inside the freshly cloned source repository. Without a
profile, every `--select` value is treated as a direct source path. Use
`--keep-checkout` when inspecting the fetched source after installation.

See `../README.md` for the initial CLI/MCP bundle installation via
`bootstrap.sh` and `install.sh`; the commands above cover guidance corpus
installation after `install-ctl` is available.
