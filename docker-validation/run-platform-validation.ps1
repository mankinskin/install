[CmdletBinding()]
param(
    [ValidateSet('linux', 'windows')][string]$Platform = 'linux',
    [Parameter(Mandatory = $true)][string]$Root,
    [ValidateSet('cargo', 'ledger', 'harness-contract', 'standalone-consumer', 'install-smoke', 'guidance-fixtures', 'viewer')]
    [string]$Suite = 'cargo',
    [string]$Manifest, [string]$Package, [string]$TestSelector = 'all',
    [string]$Metadata, [switch]$WriteMetadata,
    [string]$Ledger, [string]$LedgerSelector,
    [ValidateSet('inventory', 'migration', 'final')][string]$Stage = 'inventory',
    [switch]$RunSelectedTests, [switch]$CheckLedger, [switch]$VerifyComplete,
    [string]$DependencyRevision, [switch]$AllowEngineSwitch
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'platform-validation-lib.ps1')
$Root = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\', '/')
$workId = [guid]::NewGuid().ToString('N')
$work = Join-Path ([IO.Path]::GetTempPath()) "workflow-validation-$workId"
$context = Join-Path $work 'source'
$receiptDirectory = Join-Path $Root '.workflow-tools\validation\receipts'
$receiptPath = Join-Path $receiptDirectory "$workId.json"
$receipt = [ordered]@{
    schema = 1; suite = $Suite; platform = $Platform; status = 'started'
    source = $null; commands = @(); image = $null; base_image = $null
    selector = $TestSelector; owner = $Manifest; owner_identity = $null
    started_at = (Get-Date).ToUniversalTime().ToString('o')
}
$lock = $null
try {
    [IO.Directory]::CreateDirectory((Split-Path -Parent $receiptDirectory)) | Out-Null
    $lock = [IO.File]::Open((Join-Path (Split-Path -Parent $receiptDirectory) 'platform.lock'),
        [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
if ($Platform -ne 'linux') { throw 'Windows adapter is not implemented yet; no Linux fallback is permitted.' }
if ($AllowEngineSwitch) { throw 'Engine switching belongs to the terminal Windows adapter, not the Linux bootstrap.' }
if ($Suite -in @('ledger', 'standalone-consumer')) {
    throw "$Suite adapter is not implemented yet; its prerequisite waypoint must complete first."
}
if ($Ledger -or $LedgerSelector -or $RunSelectedTests -or $CheckLedger -or $VerifyComplete) {
    throw 'Ledger options are valid only for the ledger suite; bootstrap does not require a ledger.'
}

$cargoArguments = @()
if ($Suite -eq 'cargo') {
    $null = Resolve-ValidationInput $Root $Manifest
    $cargoArguments = @(Get-ValidationCargoArguments $Manifest $Package $TestSelector)
}
if ($Suite -eq 'harness-contract') {
    & (Join-Path $PSScriptRoot 'test-platform-validation.ps1')
}

$serverOs = & docker info --format '{{.OSType}}' 2>$null
if ($LASTEXITCODE -ne 0) {
    $desktop = Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'
    if (-not (Test-Path -LiteralPath $desktop)) { throw 'Docker Desktop is not installed.' }
    Start-Process -FilePath $desktop | Out-Null
    $deadline = (Get-Date).AddSeconds(180)
    do {
        Start-Sleep -Seconds 3
        $serverOs = & docker info --format '{{.OSType}}' 2>$null
        if ($LASTEXITCODE -eq 0) { break }
    } while ((Get-Date) -lt $deadline)
    if ($LASTEXITCODE -ne 0) { throw 'Docker did not become responsive within 180 seconds.' }
}
if ($serverOs -ne 'linux') { throw "Expected existing Linux engine, got $serverOs; no engine switch was performed." }
$active = @(Invoke-ValidationNative docker @('ps', '--filter', 'label=org.workflow-tools.validation=platform', '--format', '{{.ID}}'))
if ($active.Count) { throw "Another campaign container is active: $($active -join ', ')" }

$source = Get-ValidationSource $Root
$baseImage = if ($env:RUST_BASE_IMAGE) { $env:RUST_BASE_IMAGE } else { 'rust:1.91-bookworm' }
$recipeFileHash = Get-ValidationRecipeHash $source
$metadataPath = if ($Metadata) { Resolve-ValidationOutput $Root $Metadata } else { $null }
if ($metadataPath -and -not $WriteMetadata) {
    if (-not (Test-Path -LiteralPath $metadataPath)) { throw 'Missing metadata; first run requires -WriteMetadata.' }
    $recorded = Get-Content -Raw -LiteralPath $metadataPath | ConvertFrom-Json
    Assert-ValidationMetadata $recorded $source
}

$contextBytes = [long](($source.files | Measure-Object -Property bytes -Sum).Sum)
$drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($work))
$sizes = @(Invoke-ValidationNative docker @('image', 'ls', '--filter', 'label=org.workflow-tools.validation=platform', '-q') | Sort-Object -Unique)
$campaignBytes = 0L
foreach ($image in $sizes) {
    $campaignBytes += [long](Invoke-ValidationNative docker @('image', 'inspect', '--format', '{{.Size}}', $image))
}
$estimate = 6GB
$builder = 'workflow-platform-validation'
$builderNames = @(Invoke-ValidationNative docker @('buildx', 'ls', '--format', '{{.Name}}'))
if ($builder -notin $builderNames) {
    Assert-ValidationCapacity $drive.AvailableFreeSpace $estimate $campaignBytes $contextBytes
    Invoke-ValidationNative docker @('buildx', 'create', '--name', $builder, '--driver', 'docker-container',
        '--driver-opt', 'memory=16g', '--driver-opt', 'memory-swap=16g',
        '--driver-opt', 'cpu-period=100000', '--driver-opt', 'cpu-quota=400000',
        '--driver-opt', 'env.WORKFLOW_VALIDATION_BUILDER=platform', '--bootstrap') | Out-Host
} else {
    Invoke-ValidationNative docker @('buildx', 'inspect', $builder, '--bootstrap') | Out-Host
}
$builderInfo = ((Invoke-ValidationNative docker @('inspect', "buildx_buildkit_${builder}0")) -join "`n") | ConvertFrom-Json
if ('WORKFLOW_VALIDATION_BUILDER=platform' -notin $builderInfo[0].Config.Env -or
    $builderInfo[0].HostConfig.Memory -ne 16GB -or $builderInfo[0].HostConfig.CpuQuota -ne 400000 -or
    $builderInfo[0].HostConfig.CpuPeriod -ne 100000) {
    throw 'Existing builder ownership/resource configuration does not match the campaign.'
}
$cacheRows = @(Invoke-ValidationNative docker @('buildx', 'du', '--builder', $builder, '--format', 'json') |
    ForEach-Object { $_ | ConvertFrom-Json })
$cacheBytes = 0L
foreach ($row in $cacheRows) { $cacheBytes += Convert-ValidationSizeToBytes $row.Size }
$receipt.cache = [ordered]@{ builder = $builder; bytes = $cacheBytes; records = @($cacheRows | ForEach-Object { $_.ID }) }
try {
    Assert-ValidationCapacity $drive.AvailableFreeSpace $estimate ($campaignBytes + $cacheBytes) $contextBytes
} catch {
    if ($cacheRows.Count) {
        Invoke-ValidationNative docker @('buildx', 'prune', '--builder', $builder, '--all', '--force') | Out-Host
        $receipt.cache.cleanup = 'Enumerated dedicated campaign-builder cache removed; evidence images retained.'
    }
    $cacheRows = @(Invoke-ValidationNative docker @('buildx', 'du', '--builder', $builder, '--format', 'json') |
        ForEach-Object { $_ | ConvertFrom-Json })
    $cacheBytes = 0L
    foreach ($row in $cacheRows) { $cacheBytes += Convert-ValidationSizeToBytes $row.Size }
    Assert-ValidationCapacity $drive.AvailableFreeSpace $estimate ($campaignBytes + $cacheBytes) $contextBytes
}
Invoke-ValidationNative docker @('pull', $baseImage) | Out-Host
$baseInfo = ((Invoke-ValidationNative docker @('image', 'inspect', $baseImage)) -join "`n") | ConvertFrom-Json
if (-not $baseInfo[0].RepoDigests.Count) { throw 'Base image has no immutable repository digest.' }
$baseDigest = $baseInfo[0].RepoDigests[0]
$nodeDigest = ''
if ($Suite -eq 'viewer') {
    $nodeImage = if ($env:NODE_BASE_IMAGE) { $env:NODE_BASE_IMAGE } else { 'node:20-bookworm-slim' }
    Invoke-ValidationNative docker @('pull', $nodeImage) | Out-Host
    $nodeInfo = ((Invoke-ValidationNative docker @('image', 'inspect', $nodeImage)) -join "`n") | ConvertFrom-Json
    if (-not $nodeInfo[0].RepoDigests.Count) { throw 'Node base image has no immutable repository digest.' }
    $nodeDigest = $nodeInfo[0].RepoDigests[0]
}
$recipeDigest = Get-ValidationTextHash "linux`n$Suite`n$baseDigest`n$nodeDigest`n$recipeFileHash"
$source.recipe_digest = $recipeDigest
if ($metadataPath -and -not $WriteMetadata -and $recorded.recipe_digest -cne $recipeDigest) {
    throw 'Build recipe/base-image metadata mismatch.'
}
$tagDigest = Get-ValidationTextHash "$recipeDigest`n$($source.source_digest)"
$tag = if ($env:DOCKER_IMAGE_TAG) { $env:DOCKER_IMAGE_TAG } else { "workflow-platform:$($tagDigest.Substring(0, 24))" }
$receipt.source = $source
$receipt.base_image = $baseDigest
$receipt.node_base_image = $nodeDigest

    New-ValidationContext $Root $context $source
    $dockerfile = Join-Path $context 'workflow-tools\install\docker-validation\Dockerfile.platform'
    $buildArgs = @('buildx', 'build', '--builder', $builder, '--load', '--label', 'org.workflow-tools.validation=platform',
        '--label', "org.workflow-tools.source=$($source.source_digest)",
        '--build-arg', "RUST_BASE_IMAGE=$baseDigest", '-f', $dockerfile, '-t', $tag, $context)
    if ($Suite -eq 'viewer') {
        $buildArgs = @('buildx', 'build', '--builder', $builder, '--load', '--label', 'org.workflow-tools.validation=platform',
            '--label', "org.workflow-tools.source=$($source.source_digest)",
            '--build-arg', "RUST_BASE_IMAGE=$baseDigest", '--build-arg', "NODE_BASE_IMAGE=$nodeDigest",
            '-f', (Join-Path $context 'workflow-tools\install\viewer-validation\Dockerfile'), '-t', $tag,
            (Join-Path $context 'workflow-tools'))
    }
    $receipt.commands += ,(@('docker') + $buildArgs)
    Invoke-ValidationNative docker $buildArgs | Out-Host
    $imageInfo = (Invoke-ValidationNative docker @('image', 'inspect', $tag)) -join "`n" | ConvertFrom-Json
    $imageDigest = $imageInfo[0].Id
    Assert-ValidationImage $imageInfo[0].Config.Labels.'org.workflow-tools.source' $source.source_digest
    $receipt.image = $imageDigest
    $runArgs = @('run', '--rm', '--name', "workflow-validation-$workId", '--cpus', '4', '--memory', '8g',
        '--label', 'org.workflow-tools.validation=platform', '--entrypoint', 'bash')
    $results = Join-Path $work 'results'
    [IO.Directory]::CreateDirectory($results) | Out-Null
    $runArgs += @('--mount', "type=bind,source=$results,target=/validation-results")
    if ($Suite -eq 'viewer') {
        $runArgs += @($imageDigest, 'install/viewer-validation/run-in-container.sh')
    } else {
        $runArgs += @($imageDigest, '/source/workflow-tools/install/docker-validation/run-platform-suite.sh', $Suite)
        $runArgs += $cargoArguments
    }
    $receipt.commands += ,(@('docker') + $runArgs)
    Invoke-ValidationNative docker $runArgs | Out-Host
    if ($Suite -eq 'cargo') {
        $cargoMetadata = Get-Content -Raw -LiteralPath (Join-Path $results 'cargo-metadata.json') | ConvertFrom-Json
        $receipt.owner_identity = Get-ValidationOwnerIdentity $source $cargoMetadata $Package $recipeDigest
    }
    $after = Get-ValidationSource $Root
    if ((Get-ValidationRecipeHash $after) -cne $recipeFileHash) { throw 'Validation recipe changed during the run.' }
    if ($Suite -eq 'cargo') {
        $afterOwner = Get-ValidationOwnerIdentity $after $cargoMetadata $Package $recipeDigest
        if ($afterOwner.digest -cne $receipt.owner_identity.digest) { throw 'Selected owner or dependency changed during the run.' }
    } elseif ($Suite -ne 'harness-contract') {
        Assert-ValidationMetadata $source $after
    }
    $receipt.status = 'passed'
    if ($metadataPath) { Write-ValidationJson $metadataPath $source }
} catch {
    $receipt.status = 'failed'
    $receipt.error = $_.Exception.Message
    throw
} finally {
    $receipt.finished_at = (Get-Date).ToUniversalTime().ToString('o')
    Write-ValidationJson $receiptPath $receipt
    Write-Host "Validation receipt: $receiptPath ($($receipt.status))"
    if (Test-Path -LiteralPath $context) { Remove-Item -LiteralPath $context -Recurse -Force }
    if (Test-Path -LiteralPath (Join-Path $work 'results')) { Remove-Item -LiteralPath (Join-Path $work 'results') -Recurse -Force }
    if ($lock) { $lock.Dispose() }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work }
}
