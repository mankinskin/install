# install-ctl

`install-ctl` installs workspace tool binaries, runs the viewer lifecycle,
renders the artifact catalog, and plans/installs/repairs guidance corpora
under `.agents/**`. This file is the canonical operator contract for the
`guidance` command family; `../README.md` and `COMMANDS.md` link here rather
than repeating the same rules.

## Artifact registry and catalog

Rust binary installation first compares saved source and installed-binary
fingerprints. Matching artifacts skip compilation and installation entirely.
Otherwise it runs an online workspace warm-up build, then `cargo install
--locked` against that build's dependency resolution. This prevents a second
unlocked resolution from immediately making the installation appear stale.
`--offline` restricts the install step only; `--locked` also prevents the
warm-up build from changing Cargo.lock:

```bash
install-ctl install install-ctl
install-ctl install install-ctl --offline
install-ctl install install-ctl --locked
install-ctl install install-ctl --offline --locked
```

### Artifact status and updates

```bash
install-ctl status
install-ctl status ticket spec-cli
install-ctl status cli,mcp --root "$HOME/.local/workflow-tools"
install-ctl update
install-ctl update ticket spec-cli --root "$HOME/.local/workflow-tools" --offline --locked
install-ctl --dry-run update
install-ctl self-update
```

`status [selection...]` prints a tab-separated `artifact` / `status` table.
Each selected installable binary is `not-installed`, `up-to-date`, or
`outdated`. Selection accepts the same IDs, categories, comma-separated
tokens, and `all` as `install`, deduplicates entries, and defaults to all
installable artifacts. Inspect-only hooks and unsupported kinds are excluded
from `all` with diagnostics; explicitly selecting one fails.

Artifact commands resolve the installation root from `--root`, then the
running executable's `<root>/bin` location, then `CARGO_HOME`, then `~/.cargo`.
This includes `install`, `uninstall`, `self-uninstall`, `tui`, `status`, and
`update`. A development executable under `target/debug` or `target/release`
does not imply an installation root. `--root` always overrides detection;
roots are not merged or searched via PATH.

`status` **never compiles, installs, or accesses the network**. It compares
source fingerprints and installed binary hashes with receipts saved after
successful installation in `<root>/.install-ctl/`. Build-cache deletion does
not invalidate a receipt. The source is the **local checkout** resolved by
`INSTALL_CTL_REPO_ROOT` or the executable's ancestor directories, not the
latest remote revision.

Fingerprints cover package files and transitive local path/workspace/patch
dependencies, manifests, workspace lockfiles, enabled artifact features,
Cargo configuration, toolchain identity, and Rust/Cargo build environment
variables. Dependency discovery uses offline, locked `cargo metadata
--no-deps`; compiler identity uses `rustc -vV`, not a compilation. Optional,
target-specific, and development dependencies are conservatively included.
Build outputs (`target`, `node_modules`), Git internals, and workflow stores
are excluded. Unrelated package source changes do not invalidate a receipt,
but shared manifests/configuration/lockfiles do.

`not-installed` means no binary exists at the selected root. `up-to-date`
means both hashes match; otherwise the artifact is `outdated`. Existing or
prebuilt installations without receipts conservatively report `outdated`
with a provenance diagnostic: reinstall once to establish a fingerprint.
Malformed receipts and filesystem/metadata/toolchain errors fail explicitly.
This is a local-source cache, not universal build equivalence: arbitrary
external files or environment inputs consumed by build scripts are not
automatically discoverable, and remote updates require an explicit source
or lockfile refresh.

`update [selection...]` updates only installed artifacts with stale/missing
fingerprints or changed binary hashes. Missing binaries stay missing;
matching receipts skip all builds, even if the build cache has been deleted.
After a required rebuild, identical binaries are not replaced or stopped,
but their receipts are refreshed. Receipts are atomic per artifact and
recorded only after verification; detected source drift during installation
fails instead of recording a misleading receipt.
An empty installed selection is a no-op. Apply reports actual per-artifact
outcomes; a failure is nonzero and is not a batch rollback.
`--dry-run status` performs the same read-only status check;
`--dry-run update` previews conditional installation without builds or writes.

The former `update` operation (fetch and reinstall only install-ctl from its
canonical remote repository) is now named `self-update`.

`install-ctl tui` accepts optional file-based guidance inputs without
inventing a profile database: provide `--guidance-source`, one or more
`--guidance-profile <profile.toml>`, `--guidance-select <id>`, and
`--guidance-target <path>`. All four inputs are required when any profile is
provided; destination scope remains the TOML profile policy.

## Local prebuilt consumer bundles

`prebuilt-install` installs delivered local binaries without compiling:
`install-ctl prebuilt-install ticket --bundle <dir> --manifest <manifest.json> --home <home>`.
Manifest version 1 contains `target` (`<os>-<arch>`) and `tools` entries with
registry `id`, relative `path`, and SHA-256 `sha256`. The manifest is an
integrity authority, not a signature. Verified blobs are retained at
`<home>/.workflow-tools/cache/<sha256>` and copied to
`<home>/.workflow-tools/bin/<registry-bin>`; later runs re-verify cache bytes.
Wrong target, missing/escaping path, or source/cache digest mismatch fails
without a Cargo fallback.

Example manifest (replace the digest with the delivered file's real SHA-256):

```json
{
  "version": 1,
  "target": "windows-x86_64",
  "tools": [
    {"id": "ticket", "path": "ticket.exe", "sha256": "<64 hexadecimal characters>"}
  ]
}
```

Prepare a bundle by building the platform-specific tools once, copying the
resulting binaries into a local directory, and recording their hashes in the
manifest. On PowerShell, `Get-FileHash -Algorithm SHA256 <binary>` computes a
digest. Keep the runtime registry available via `INSTALL_CTL_REPO_ROOT`.
The manifest does not replace that registry or authorize arbitrary output
filenames. The bundle must still be available for source verification on a
later install; the cache avoids rewriting verified blobs, not source checks.

For native interactive review:

```powershell
install-ctl tui --prebuilt-bundle C:\bundle `
  --prebuilt-manifest C:\bundle\manifest.json --prebuilt-home C:\consumer
```

Supply all three prebuilt options together. Review shows the local source,
manifest, selected tools, and initial installation root
`C:\consumer\.workflow-tools`. The existing `r` editor changes that tools root;
apply uses the edited root for both `bin` and `cache`, not the original flag.
Source and manifest paths are supplied on launch, not edited inside the TUI.
Cancellation performs no writes. Scripted `--dry-run prebuilt-install ...`
verifies inputs without writing. Existing Cargo-mode flags remain unchanged.

All selected source and existing cache hashes are checked before the first
write. Apply uses atomic per-file replacement and reports completed, failed,
and skipped actions; an I/O failure during apply is not a batch rollback.
Identical installed bytes report `unchanged`. Scripted replacement of the
currently executing installer is rejected; the TUI uses its existing Windows
pre-interaction shadow mechanism. No release upload or HTTPS fetch is included.

Scripted installation and confirmed TUI installation print a `result:` report
after applying the reviewed selection. Each action includes its identifier,
target, detail, and one of `written`, `unchanged`, `skipped`, or `failed`.
Matching source/binary receipts skip builds; otherwise binary replacement is
determined by comparing installed bytes with the fresh release build, not
merely by checking whether the target exists. Replacing a different binary
reports `written`. Build or install failures are reported
before exiting nonzero; later installation groups are skipped after a failure.
Guidance-only installation does not invoke a binary build. Guidance outcomes
include completed file writes even when a later file or profile fails; writes
are atomic per file, not transactional across the selection.

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
install-ctl guidance link      --repo <repo> --harness copilot-cli (--plan | --apply) [--json]
install-ctl guidance unlink    --repo <repo> --harness copilot-cli (--plan | --apply) [--json]
install-ctl guidance link      --repo <repo> --harness vscode --source NAME=PATH... [--migrate] (--plan | --apply) [--json]
install-ctl guidance unlink    --repo <repo> --harness vscode --source NAME=PATH... (--plan | --apply) [--json]
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
  the result reports `rolled_back: true`. An apply with unresolved findings
  but no safe operations fails with an actionable error instead of reporting
  a misleading successful `0 applied` run.
- `get` fetches only selected guidance and required references using typed
  Git protocol v2 with `filter` support. HTTP(S), SSH, Git and local/file
  sources remain supported when the server provides those capabilities.
  Unsupported sources fail explicitly; no full-clone fallback exists.
  System Git is still required for configuration, credentials, SSH and
  received-pack verification.
  - The remote HEAD is resolved to an immutable commit on every call.
    Tree metadata is permitted, but unrelated file contents are not fetched.
    Explicit missing paths fail after metadata discovery, without guidance
    downloads or destination writes. A bad selector never searches a different
    repository automatically.
  - `--profile` names a repository-relative TOML file, not a local file.
    Without a profile, `--select` values are direct paths. All selected IDs
    and inputs are preflighted before guidance download. Directories expand
    `.md`/`.toml` descendants; bare filenames retain deterministic lookup.
    Copy recipes preserve their input/output identity.
  - Submodules are entered only when needed by selection/reference traversal,
    at recorded gitlink commits rather than child HEAD. Nested relative URLs
    and contained Git symlinks are supported without OS symlink privileges;
    escaping/cyclic symlinks and unavailable required children fail explicitly.
    Bare-filename discovery may inspect candidate submodule metadata.
  - Verified objects are reused under the OS cache directory,
    `workflow-tools/install-ctl/guidance/v1`, segregated by credential-free
    repository identity, object format and OID. Each invocation checks remote
    access/freshness; a warm cache never hides authentication/network errors.
    Corruption produces a diagnostic and refetch. Failed selections retain
    valid cached metadata. No offline mode or cache-eviction interface is added.
  - The existing local planner/installer then consumes the selected-source
    snapshot; `--target` still defaults to `.` and destination overrides remain
    supported. Local `plan`/`install` never access the network.
  - Only invocation-owned temporary files are removed on success or error.
    `--keep-checkout` retains and names that invocation's selected-source
    snapshot, not a full Git checkout or the shared cache. With `--json`,
    stdout contains the unchanged plan object; progress, install counts and
    retained-snapshot paths go to stderr.

## Local Harness Links Without Copies

Use `guidance link` when the guidance already exists in a persistent local
repository and `.agents` must remain the single source. This mode does not
use the copy installer, fetch a repository, rewrite content, or materialize
templates. It creates real directory symbolic links with relative targets.

```bash
install-ctl guidance link --repo . --harness copilot-cli --plan
install-ctl guidance link --repo . --harness copilot-cli --apply
```

The `copilot-cli` layout remains:

| Harness Path | Canonical Source | Purpose |
|---|---|---|
| `.github/agents` | `.agents/agents` | Custom agent discovery |
| `.github/instructions` | `.agents/instructions` | Modular instruction discovery |
| `.claude/commands` | `.agents/prompts` | CLI's Claude-compatible Markdown command loader |

The harness paths remain visible, but are aliases rather than independent
guidance sources. Original edits and new files are immediately visible
through the aliases without synchronization. No `.copilot` alias is created.
Template bytes and metadata are preserved; this mode does not translate
VS Code-specific template features into CLI equivalents. Restart the CLI
after linking to refresh discovery.

`--repo` defaults to `.`; `--harness` and exactly one of `--plan`/`--apply`
are required. Plans are read-only. Missing optional source directories are
reported as skipped; linking fails when no source exists. Sources must
resolve inside the selected repository. Correct existing symbolic links are
unchanged. Ordinary destination files/directories, junctions, foreign links,
and indirect destination parents block the entire operation before mutation.
The command never overwrites a conflicting path.

On Windows, enable Developer Mode or run the command yourself from an
administrator terminal if symbolic-link creation is denied. There is **no
copy, hardlink, or junction fallback**, and the command never elevates itself.
A failed link operation rolls back only links and empty parent directories
created by that call, reporting any rollback failure.

Remove the harness aliases without removing their source files:

```bash
install-ctl guidance unlink --repo . --harness copilot-cli --plan
install-ctl guidance unlink --repo . --harness copilot-cli --apply
```

Unlink removes only matching symbolic links, including expected broken links.
Missing aliases are harmless. Foreign paths block removal. Unlink does not
remove `.agents`, template contents, or parent directories. Existing
`install`, `uninstall`, `get`, and `autofix` retain their separate semantics.

`--json` emits one object with `plan`, `applied`, and `error`. The plan contains
the resolved repository, harness, operation, source/destination/relative-target
entries, actions, skipped sources, and diagnostics. `applied` is true only
after successful application; any blocked or failed operation exits nonzero.

Focused validation is `cargo test -p install-ctl guidance::links`; CLI parsing
and read-only command tests run with `cargo test -p install-ctl guidance_link_cli`.
Real-symlink tests are explicitly ignored on Windows without a known
symlink-capable environment. With the prerequisite enabled, run them using
`cargo test -p install-ctl guidance::links -- --ignored`. Unix runs the same
real-symlink tests normally. Windows additionally tests actual junction
rejection without requiring symbolic-link privileges.

### VS Code: multiple named guidance sources

Use the separate `vscode` harness when the root workspace must expose guidance
from several repository-contained sources, including initialized submodules:

```powershell
install-ctl guidance link --repo . --harness vscode `
  --source root=.agents `
  --source workflow-tools=workflow-tools\.agents `
  --source context-engine=context-engine\.agents --plan --json
```

Review the plan, then repeat the command with `--apply` instead of `--plan`.
Running a command is an explicit activation of the selected repository;
documentation examples do not authorize activation of another workspace.

Each present kind becomes a named, relative directory symlink:

| Kind | Named destination | Root source target |
|---|---|---|
| agents | `.github/agents/root` | `../../.agents/agents` |
| instructions | `.github/instructions/root` | `../../.agents/instructions` |
| prompts | `.github/prompts/root` | `../../.agents/prompts` |

The kind containers are ordinary directories. Source names must begin with a
lowercase ASCII letter or digit and contain only lowercase letters, digits,
`-`, or `_`; Windows reserved device names are rejected. Sources are explicit
repository-relative paths, not recursively discovered. Absolute paths,
traversal, canonical escapes, and unavailable explicit sources block linking.
Each source needs at least one supported kind; individual missing kinds are
reported as skipped. The command never initializes or clones submodules.

VS Code does not recursively discover agents and prompts in these containers.
The transaction therefore adds forward-slash `true` entries to the workspace's
`.vscode/settings.json` location objects:

- `chat.agentFilesLocations`: `.github/agents/<name>`
- `chat.instructionsFilesLocations`: `.github/instructions/<name>`
- `chat.promptFilesLocations`: `.github/prompts/<name>`

The JSONC editor preserves comments, trailing commas, indentation, newline
style, and unrelated settings. Existing `true` entries are user-owned; false
or nonboolean selected entries, duplicate relevant properties, invalid JSONC,
and nonobject location settings block rather than being overwritten.
Existing discovery settings are not disabled. Source contents, `applyTo`,
frontmatter, agent names, and tool permissions remain unchanged. Named folders
namespace paths, not duplicate user-facing frontmatter names or VS Code's
consumer precedence rules. `.claude/commands` is not changed.

If a kind container is already a legacy single-source alias, the default plan
refuses conversion. Add `--migrate` only when the alias is a real, resolvable
symlink to the corresponding kind of an explicitly selected source. Foreign
aliases, broken legacy aliases, junctions, ordinary files, and indirect
ancestors remain blockers. Migration is invalid on unlink and both new flags
are invalid for `copilot-cli`.

Version-1 ownership is stored in `.vscode/install-ctl-guidance-links.json`,
including source mappings, destinations, raw relative targets, and which
settings properties the tool inserted. Do not hand-edit the record. All link,
settings, and ownership preconditions are checked before mutation. Apply
rechecks staleness and rolls back failed operations, restoring configuration
bytes and raw removed-link targets; rollback failures are reported explicitly.
Windows requires Developer Mode or a user-chosen privileged terminal, with no
automatic elevation or alternative link type.

To remove only one managed source:

```powershell
install-ctl guidance unlink --repo . --harness vscode `
  --source workflow-tools=workflow-tools\.agents --plan --json
# After review, repeat with --apply instead of --plan.
```

Unselected sources and user-owned settings survive. Unlink removes only
matching recorded links and still-`true` tool-inserted settings; source
directories may already be missing. Modified ownership/settings or selected
links without matching ownership block removal. No record and no selected
links is a no-op. The last unlink removes the empty record and removes the
settings file only if the tool originally created the file and no user
content remains. Ordinary parent directories are retained.

VS Code JSON reports contain `plan`, `applied`, and `error`; the plan includes
link actions and `configuration_changes`, without dumping existing settings
bytes. `applied` becomes true only after the complete transaction succeeds.
Refresh VS Code discovery after link/unlink. Verify agents, prompt invocation
in Local chat, and instruction attachment in a disposable workspace; generated
files alone do not prove consumer discovery.

Run from the parent `workflow-tools` Cargo workspace:

```powershell
cargo test -p install-ctl guidance::links::tests::vscode_integration -- --include-ignored
cargo test -p install-ctl guidance::links -- --include-ignored
cargo test -p install-ctl guidance_link_cli -- --include-ignored
cargo test -p install-ctl
```

On Windows, `--include-ignored` actually exercises privilege-dependent cases;
a skip or privilege failure is not passing symlink or VS Code consumer evidence.

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
curl -fsSL https://raw.githubusercontent.com/mankinskin/install/main/install.sh \
  | bash -s -- --root "$HOME/.local/workflow-tools"
```

```bash
"$HOME/.local/workflow-tools/bin/install-ctl" \
  guidance get \
  https://github.com/mankinskin/meta-workspace.git \
  --select workflow-tools/.agents \
  --select .agents \
  --select AGENTS.md \
  --target "$PWD" \
  --destination-scope repo
```

The command installs `install-ctl` and runs `guidance get`, which shallow-clones
`meta-workspace` (including its submodules), computes the full transitive Markdown
closure of all referenced instructions, agents, and prompts across `.agents`,
`workflow-tools/.agents`, and `AGENTS.md` (resolved to `workflow-tools/AGENTS.md`),
installs all guidance files into `$PWD/.agents/`, and automatically cleans up the
temporary clone. This exact flow is also exercised during containerized end-to-end
validation (`docker-validation/run-in-container.sh`).

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
  https://github.com/mankinskin/install.git \
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
