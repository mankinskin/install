# Local platform validation

Run from the meta-workspace root on Windows:

```powershell
$driver = '.\workflow-tools\install\docker-validation\run-platform-validation.ps1'
& $driver -Platform linux -Root (Get-Location).Path -Suite harness-contract
& $driver -Platform linux -Root (Get-Location).Path -Suite cargo `
  -Manifest workflow-tools/Cargo.toml -Package install-ctl -TestSelector guidance `
  -Metadata path-rendering-source.json -WriteMetadata
```

The host invokes Docker, never host Cargo. The Linux image includes the complete
allowlisted recursive source tree, including sibling Cargo patches. Its default
network-smoke entry point is not used for Cargo tests.

Suites currently implemented: `cargo`, `ledger`, `harness-contract`, `install-smoke`,
`guidance-fixtures`, and `viewer`. The standalone-consumer
adapter is added by its prerequisite work package. Windows and engine
switching fail explicitly until the terminal Windows adapter exists.

Cargo selectors are `all` (no test filter), `guidance`, and
`clippy-all-targets`. Every Cargo command uses `--locked`. Legacy Bash drivers
remain supported; `RUST_BASE_IMAGE`, `DOCKER_IMAGE_TAG`, and viewer
`NODE_BASE_IMAGE` conventions are unchanged.

`-WriteMetadata` refreshes source identity after reviewed edits. Without it,
the source revision, recursive checkout, copied bytes and recipe must match.
Every run rebuilds its source-derived image, verifies the image label, runs one
container with 4 CPUs/8 GiB, then checks that inputs did not change during the
test. JSON receipts under `.workflow-tools/validation/receipts` contain command,
source and image identities and an explicit passed/failed outcome.

Source copying excludes Git metadata, entity stores, credentials, caches and
generated receipts. Tracked source/fixtures and in-scope untracked Rust/manifests
are included; new harness scripts are explicitly included during bootstrap.
Symlink/reparse inputs are rejected rather than copying outside the workspace.
Exclusions are recorded in the source receipt.
Copied Bash scripts have CRLF normalized to LF for Linux execution; receipts
record both working-file hashes and the actual copied-byte hashes.
The Rust base is pulled and resolved to its immutable repository digest before
building. Cargo receipts include the selected package's resolved dependency
graph and local source/test hashes; unrelated owners are not included in that
package digest. Changing a reachable dependency or build recipe invalidates it.
The copied snapshot remains exactly attested. Post-run checks reject changes to
the selected Cargo owner, its dependencies or harness recipe; concurrent changes
to unrelated owners do not discard valid owner evidence. An explicit metadata
refresh is still required before using a different full-source snapshot.
The dedicated `workflow-platform-validation` BuildKit builder is limited to four
CPUs and 16 GiB, without changing shared Docker Desktop allocation. Its measured
cache and campaign-labelled images count toward the capacity ceiling; only that
dedicated cache may be reclaimed automatically under space pressure. Validation
containers run with four CPUs and 8 GiB. A workspace-scoped exclusive lock
prevents overlapping driver runs. Preflight failures also produce failed receipts.

On Windows hosts with script execution disabled, use a process-scoped invocation:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command '& .\workflow-tools\install\docker-validation\run-platform-validation.ps1 -Platform linux -Root (Get-Location).Path -Suite harness-contract'
```

This does not change the machine or user's persistent execution policy.

Capacity checks keep a 10-GiB safety reserve plus an initial 6-GiB build estimate
and copied context size, bounded by the 120-GiB campaign ceiling. Insufficient
capacity fails with an operator handoff; the driver never prunes unrelated data.
The driver refuses a second campaign container and does not switch engines.
Existing unrelated containers are not stopped.

## Inventory and ledger suites

Create the initial pending census without claiming any classification:

```powershell
& $driver -Platform linux -Root (Get-Location).Path -Suite ledger `
  -Ledger path-rendering-occurrences.toml -LedgerSelector inventory-generate `
  -Stage inventory -Metadata path-rendering-source.json -WriteMetadata
```

Generation refuses to overwrite a ledger. Subsequent runs use exact emitted
`batch:<group>:<nnn>` selectors or the stage-specific aggregate selectors
documented in [the inventory package](../../path-render-inventory/README.md).
The ledger and earlier receipts are separate attested inputs, not image/source
hash inputs. `-RunSelectedTests`, `-CheckLedger` and `-VerifyComplete` retain their
distinct roles; complete gates fail on unresolved classification or stale owning
platform proofs. Windows batches check their requested platform; the terminal
both-platform aggregate is what requires the complete paired proof set.
