$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'platform-validation-lib.ps1')
$script:assertions = 0
function Assert-Contract {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "Contract failed: $Message" }
    $script:assertions++
}
function Assert-Rejected {
    param([scriptblock]$Action, [string]$Message)
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    Assert-Contract $rejected $Message
}

$all = @(Get-ValidationCargoArguments 'workflow-tools\Cargo.toml' 'install-ctl' 'all')
Assert-Contract (($all -join ' ') -eq 'test --locked --manifest-path workflow-tools/Cargo.toml -p install-ctl') 'all is not a Cargo filter'
$clippy = @(Get-ValidationCargoArguments 'workflow-tools/Cargo.toml' 'install-ctl' 'clippy-all-targets')
Assert-Contract (($clippy -join ' ') -eq 'clippy --locked --manifest-path workflow-tools/Cargo.toml -p install-ctl --all-targets -- -D warnings') 'clippy routing'
Assert-Rejected { Get-ValidationCargoArguments 'Cargo.toml' 'fixture' 'unowned' } 'unknown selector'
Assert-Rejected { Get-ValidationCargoArguments '' '' 'all' } 'missing ownership'
Assert-Contract (Test-ValidationSourcePath 'workflow-tools/install/install-ctl/src/lib.rs') 'tracked Rust'
Assert-Contract (Test-ValidationSourcePath 'workflow-tools/new/src/lib.rs' $true) 'in-scope untracked Rust'
Assert-Contract (-not (Test-ValidationSourcePath 'workflow-tools/.env')) 'no credentials'
Assert-Contract (-not (Test-ValidationSourcePath '.workflow-tools/session/record.json')) 'no entity stores'
Assert-Contract (-not (Test-ValidationSourcePath 'workflow-tools/target/output.rs')) 'no build cache'
Assert-Contract (-not (Test-ValidationSourcePath 'path-rendering-source.json')) 'no self-referential metadata'
Assert-Contract (-not (Test-ValidationSourcePath 'path-rendering-occurrences.toml')) 'ledger separately attested'
Assert-Contract (Test-ValidationSourcePath 'workflow-tools/install/docker-validation/run-platform-validation.ps1' $true) 'new harness source included'
Assert-ValidationImage 'source' 'source'
Assert-Rejected { Assert-ValidationImage 'stale-image' 'source' } 'stale image rejection'
Assert-ValidationSelection 'ledger' 'ledger' 'fixture-owner'
Assert-Rejected { Assert-ValidationSelection 'old-ledger' 'changed-ledger' 'fixture-owner' } 'altered ledger rejection'
Assert-Rejected { Assert-ValidationSelection 'ledger' 'ledger' '' } 'ledger ownership rejection'
$current = [ordered]@{ schema = 1; root_revision = 'revision'; source_digest = 'source'; gitlinks = @('link') }
Assert-ValidationMetadata $current $current
foreach ($field in @('source_digest', 'root_revision', 'gitlinks')) {
    $changed = [ordered]@{ schema = 1; root_revision = 'revision'; source_digest = 'source'; gitlinks = @('link') }
    $changed[$field] = 'changed'
    Assert-Rejected { Assert-ValidationMetadata $changed $current } "changed $field"
}
Assert-ValidationCapacity 30GB 6GB 0 1GB
Assert-Rejected { Assert-ValidationCapacity 12GB 6GB 0 1GB } 'capacity handoff keeps reserve'
Assert-Rejected { Assert-ValidationCapacity 200GB 6GB 119GB 1GB } 'campaign ceiling'
Assert-Contract ((Convert-ValidationSizeToBytes '4.096kB') -ge 4096) 'Docker cache size conservatively measured'
Assert-Rejected { Convert-ValidationSizeToBytes 'unknown' } 'unmeasurable cache rejected'
$fixtureSource = @{
    files = @(
        [pscustomobject]@{ path = 'owner/src/lib.rs'; hash = 'owner-bytes' }
        [pscustomobject]@{ path = 'dependency/src/lib.rs'; hash = 'dependency-bytes' }
        [pscustomobject]@{ path = 'unrelated/src/lib.rs'; hash = 'unrelated-bytes' }
    )
}
$fixtureGraph = @{
    packages = @(
        [pscustomobject]@{ name = 'owner'; id = 'owner-id'; manifest_path = '/source/owner/Cargo.toml' }
        [pscustomobject]@{ name = 'dependency'; id = 'dependency-id'; manifest_path = '/source/dependency/Cargo.toml' }
    )
    resolve = @{
        nodes = @(
            [pscustomobject]@{ id = 'owner-id'; dependencies = @('dependency-id') }
            [pscustomobject]@{ id = 'dependency-id'; dependencies = @() }
        )
    }
}
$identity = Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe'
$fixtureSource.files[2].hash = 'unrelated-change'
Assert-Contract ((Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe').digest -ceq $identity.digest) 'unrelated owner does not invalidate evidence'
$fixtureSource.files[1].hash = 'dependency-change'
Assert-Contract ((Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe').digest -cne $identity.digest) 'reachable dependency invalidates evidence'
Assert-Rejected { Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'missing' 'recipe' } 'missing resolved owner'
$fixtureGraph = [pscustomobject]$fixtureGraph
$fixtureGraph | Add-Member -NotePropertyName workspace_root -NotePropertyValue '/source'
foreach ($dependency in $fixtureGraph.packages) { $dependency | Add-Member -NotePropertyName version -NotePropertyValue '1.0.0' }
$fixtureSource.files += [pscustomobject]@{
    path = 'Cargo.lock'; hash = 'lock'
    cargo_packages = @(
        [pscustomobject]@{ name = 'owner'; version = '1.0.0'; hash = 'owner-lock' }
        [pscustomobject]@{ name = 'dependency'; version = '1.0.0'; hash = 'dependency-lock' }
        [pscustomobject]@{ name = 'unrelated'; version = '1.0.0'; hash = 'unrelated-lock' }
    )
}
$lockIdentity = Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe'
$fixtureSource.files[3].cargo_packages[2].hash = 'unrelated-lock-change'
Assert-Contract ((Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe').digest -ceq $lockIdentity.digest) 'unrelated lock section leaves owner evidence valid'
$fixtureSource.files[3].cargo_packages[1].hash = 'dependency-lock-change'
Assert-Contract ((Get-ValidationOwnerIdentity $fixtureSource $fixtureGraph 'owner' 'recipe').digest -cne $lockIdentity.digest) 'dependency lock section invalidates owner evidence'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ("validation-contract-" + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($scratch) | Out-Null
try {
    $shellFile = Join-Path $scratch 'fixture.sh'
    [IO.File]::WriteAllText($shellFile, "first`r`nsecond`r`n", [Text.UTF8Encoding]::new($false))
    Assert-Contract ([Text.Encoding]::UTF8.GetString((Get-ValidationCopyBytes $shellFile)) -ceq "first`nsecond`n") 'copied Bash uses LF'
    Assert-Rejected { Resolve-ValidationInput $scratch '..\outside.rs' } 'workspace traversal rejected'
} finally {
    Remove-Item -LiteralPath (Join-Path $scratch 'fixture.sh')
    Remove-Item -LiteralPath $scratch
}
Write-Host "Host harness contracts: $script:assertions passed"
