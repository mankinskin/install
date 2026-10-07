Set-StrictMode -Version Latest

function Invoke-ValidationNative {
    param([string]$Program, [string[]]$Arguments)
    $output = & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program failed ($LASTEXITCODE): $($Arguments -join ' ')"
    }
    return $output
}

function Get-ValidationHash {
    param([byte[]]$Bytes)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Get-ValidationTextHash {
    param([string]$Text)
    return Get-ValidationHash ([Text.Encoding]::UTF8.GetBytes($Text))
}

function Get-ValidationCopyBytes {
    param([string]$Path)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ([IO.Path]::GetExtension($Path) -eq '.sh') {
        $text = [Text.Encoding]::UTF8.GetString($bytes).Replace("`r`n", "`n")
        return ,([Text.UTF8Encoding]::new($false).GetBytes($text))
    }
    return ,$bytes
}

function Resolve-ValidationInput {
    param([string]$Root, [string]$RelativePath)
    if ([IO.Path]::IsPathRooted($RelativePath)) { throw "Expected a workspace-relative path: $RelativePath" }
    $full = [IO.Path]::GetFullPath((Join-Path $Root $RelativePath))
    $prefix = $Root.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Input escapes the workspace: $RelativePath"
    }
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Missing input: $RelativePath" }
    $cursor = Get-Item -LiteralPath $full
    while ($cursor.FullName.Length -gt $Root.Length) {
        if (($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Linked input is not safe to copy: $RelativePath"
        }
        $cursor = if ($cursor -is [IO.FileInfo]) { $cursor.Directory } else { $cursor.Parent }
        if ($null -eq $cursor) { break }
    }
    return $full
}

function Test-ValidationSourcePath {
    param([string]$Path, [bool]$Untracked = $false)
    $name = $Path.Replace('\', '/')
    if ($name -match '(^|/)(\.git|\.workflow-tools|\.ticket|\.spec|\.session|\.feedback|\.test|target|node_modules|dist|build|\.next|transcripts)(/|$)') { return $false }
    if ($name -match '(^|/)(\.env[^/]*|credentials[^/]*|secrets?[^/]*|id_rsa[^/]*|id_ed25519[^/]*|path-rendering-source\.json|path-rendering-occurrences\.toml)$') { return $false }
    if ($name -match '\.(pem|key|pfx|p12|exe|dll|so|a|zip|7z|sqlite|db)$') { return $false }
    if ($Untracked) {
        return $name -match '(\.rs$|(^|/)(Cargo\.toml|Cargo\.lock)$)' -or
            $name -match '^workflow-tools/install/docker-validation/[^/]+\.(ps1|sh)$|^workflow-tools/install/docker-validation/Dockerfile\.platform$'
    }
    return $name -match '\.(rs|toml|lock|sh|ps1|json|md|txt|html|css|js|ts|tsx|jsx|svg|png|jpg|woff2|yml|yaml|ron|csv|snap)$|(^|/)(Dockerfile[^/]*|\.gitmodules|\.dockerignore|\.cargo/config)$'
}

function Resolve-ValidationOutput {
    param([string]$Root, [string]$RelativePath)
    if ([IO.Path]::IsPathRooted($RelativePath)) { throw "Expected workspace-relative output: $RelativePath" }
    $full = [IO.Path]::GetFullPath((Join-Path $Root $RelativePath))
    if (-not $full.StartsWith($Root.TrimEnd('\', '/') + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Output escapes the workspace: $RelativePath"
    }
    $cursor = $full
    while ($cursor.Length -gt $Root.Length) {
        if ((Test-Path -LiteralPath $cursor) -and
            ((Get-Item -LiteralPath $cursor).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Linked output is not safe: $RelativePath"
        }
        $cursor = Split-Path -Parent $cursor
    }
    return $full
}

function Get-ValidationSource {
    param([string]$Root)
    $rootRevision = (Invoke-ValidationNative git @('-C', $Root, 'rev-parse', 'HEAD')).Trim()
    $gitlinks = @(Invoke-ValidationNative git @('-C', $Root, 'submodule', 'status', '--recursive'))
    $repositories = @('')
    foreach ($line in $gitlinks) {
        if ($line -match '^[-U]') { throw "Uninitialized or conflicted submodule: $line" }
        if ($line -notmatch '^[ +]([0-9a-f]{40}) (.+?)(?: \(.+\))?$') { throw "Invalid submodule receipt: $line" }
        $repositories += $Matches[2]
    }
    $entries = [Collections.Generic.List[object]]::new()
    $excluded = [Collections.Generic.List[object]]::new()
    foreach ($repository in $repositories) {
        $repoRoot = if ($repository) { Join-Path $Root $repository } else { $Root }
        $tracked = @(Invoke-ValidationNative git @('-c', 'core.quotepath=false', '-C', $repoRoot, 'ls-files'))
        $untracked = @(Invoke-ValidationNative git @('-c', 'core.quotepath=false', '-C', $repoRoot, 'ls-files', '--others', '--exclude-standard'))
        foreach ($isUntracked in @($false, $true)) {
            $paths = if ($isUntracked) { $untracked } else { $tracked }
            foreach ($path in $paths) {
                $relative = if ($repository) { "$repository/$path" } else { $path }
                if (-not (Test-ValidationSourcePath $relative $isUntracked)) {
                    $excluded.Add([ordered]@{ path = $relative; reason = 'source-allowlist' })
                    continue
                }
                $full = [IO.Path]::GetFullPath((Join-Path $Root $relative))
                if (Test-Path -LiteralPath $full -PathType Container) { continue }
                if (-not (Test-Path -LiteralPath $full)) {
                    $excluded.Add([ordered]@{ path = $relative; reason = 'deleted-working-file' })
                    continue
                }
                $full = Resolve-ValidationInput $Root $relative
                $bytes = Get-ValidationCopyBytes $full
                $cargoPackages = @()
                if ([IO.Path]::GetFileName($full) -ceq 'Cargo.lock') {
                    foreach ($section in [regex]::Split([Text.Encoding]::UTF8.GetString($bytes), '(?m)^\[\[package\]\]\r?$')) {
                        $nameMatch = [regex]::Match($section, '(?m)^name = "([^"]+)"')
                        $versionMatch = [regex]::Match($section, '(?m)^version = "([^"]+)"')
                        if ($nameMatch.Success -and $versionMatch.Success) {
                            $cargoPackages += [pscustomobject]@{
                                name = $nameMatch.Groups[1].Value; version = $versionMatch.Groups[1].Value
                                hash = Get-ValidationTextHash $section
                            }
                        }
                    }
                }
                $entries.Add([pscustomobject][ordered]@{
                    path = $relative.Replace('\', '/'); hash = Get-ValidationHash $bytes
                    working_hash = Get-ValidationHash ([IO.File]::ReadAllBytes($full))
                    bytes = $bytes.Length; untracked = $isUntracked; cargo_packages = $cargoPackages
                })
            }
        }
    }
    $entries = @($entries | Sort-Object path -Unique)
    $description = ($entries | ForEach-Object { "$($_.path)`t$($_.hash)" }) -join "`n"
    $workingDescription = ($entries | ForEach-Object { "$($_.path)`t$($_.working_hash)" }) -join "`n"
    return [ordered]@{
        schema = 1; root_revision = $rootRevision; gitlinks = $gitlinks
        source_digest = Get-ValidationTextHash $description
        working_digest = Get-ValidationTextHash $workingDescription
        files = $entries; exclusions = @($excluded)
    }
}

function Assert-ValidationMetadata {
    param($Recorded, $Current)
    if ($Recorded.schema -ne 1 -or $Recorded.root_revision -ne $Current.root_revision -or
        $Recorded.source_digest -ne $Current.source_digest -or
        (($Recorded.gitlinks -join "`n") -cne ($Current.gitlinks -join "`n"))) {
        throw 'Source/root/gitlink metadata mismatch; refresh with -WriteMetadata after reviewing edits.'
    }

}

function Assert-ValidationImage {
    param([string]$ImageSource, [string]$ExpectedSource)
    if ($ImageSource -cne $ExpectedSource) { throw 'Image/source attestation mismatch.' }
}

function Assert-ValidationSelection {
    param([string]$RecordedDigest, [string]$CurrentDigest, [string]$Owner)
    if (-not $Owner) { throw 'Missing selector ownership.' }
    if ($RecordedDigest -cne $CurrentDigest) { throw 'Ledger selection digest mismatch.' }
}

function Assert-ValidationCapacity {
    param([long]$FreeBytes, [long]$EstimatedBytes, [long]$CampaignBytes, [long]$ContextBytes)
    $reserve = 10GB
    if ($CampaignBytes + $ContextBytes + $EstimatedBytes -gt 120GB) {
        throw 'Capacity handoff: campaign workload would exceed its 120-GiB ceiling.'
    }
    if ($FreeBytes -lt $reserve + $EstimatedBytes + $ContextBytes) {
        throw 'Capacity handoff: provide free space, keeping a 10-GiB reserve plus the measured next build footprint. No unrelated data was deleted.'
    }
}

function Convert-ValidationSizeToBytes {
    param([string]$Size)
    if ($Size -notmatch '^([0-9]+(?:\.[0-9]+)?)\s*(B|kB|MB|GB|TB|KiB|MiB|GiB)$') {
        throw "Unrecognized Docker cache size: $Size"
    }
    $factors = @{ B = 1; kB = 1000; MB = 1000000; GB = 1000000000; TB = 1000000000000
        KiB = 1024; MiB = 1048576; GiB = 1073741824 }
    $number = [double]::Parse($Matches[1], [Globalization.CultureInfo]::InvariantCulture)
    return [long][Math]::Ceiling($number * $factors[$Matches[2]] * 1.01)
}

function Get-ValidationRecipeHash {
    param($Source)
    $inputs = @($Source.files | Where-Object {
        $_.path -match '^workflow-tools/install/(docker-validation/|viewer-validation/|validation-lib\.sh$)'
    } | ForEach-Object { "$($_.path)`t$($_.hash)" })
    return Get-ValidationTextHash ($inputs -join "`n")
}

function Get-ValidationOwnerIdentity {
    param($Source, $CargoMetadata, [string]$Package, [string]$RecipeDigest)
    $owners = @($CargoMetadata.packages | Where-Object { $_.name -ceq $Package })
    if ($owners.Count -ne 1) { throw "Ambiguous or missing Cargo owner: $Package" }
    $reachable = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $queue = [Collections.Generic.Queue[string]]::new()
    $queue.Enqueue($owners[0].id)
    while ($queue.Count) {
        $id = $queue.Dequeue()
        if (-not $reachable.Add($id)) { continue }
        $nodes = @($CargoMetadata.resolve.nodes | Where-Object { $_.id -ceq $id })
        if ($nodes.Count -ne 1) { throw "Missing resolved dependency node: $id" }
        foreach ($dependency in $nodes[0].dependencies) { $queue.Enqueue($dependency) }
    }
    $localRoots = @()
    foreach ($dependency in $CargoMetadata.packages) {
        if ($reachable.Contains($dependency.id) -and $dependency.manifest_path.StartsWith('/source/')) {
            $localRoots += ($dependency.manifest_path.Substring(8) -replace '/Cargo\.toml$', '') + '/'
        }
    }
    $ownerFiles = @($Source.files | Where-Object {
        $file = $_.path
        @($localRoots | Where-Object { $file.StartsWith($_, [StringComparison]::Ordinal) }).Count -gt 0
    })
    if ($CargoMetadata.PSObject.Properties.Name -contains 'workspace_root') {
        $workspaceRoot = $CargoMetadata.workspace_root
        if (-not $workspaceRoot.StartsWith('/source/')) { throw 'Cargo workspace is outside copied source.' }
        $relativeWorkspace = $workspaceRoot.Substring(8) + '/'
        $ownerFiles += @($Source.files | Where-Object {
            $_.path -ceq ($relativeWorkspace + 'Cargo.toml') -or
            $_.path.StartsWith($relativeWorkspace + '.cargo/', [StringComparison]::Ordinal) -or
            $_.path.StartsWith('.cargo/', [StringComparison]::Ordinal)
        })
    }
    $input = @($RecipeDigest)
    $input += @($reachable | Sort-Object)
    $input += @($ownerFiles | ForEach-Object { "$($_.path)`t$($_.hash)" })
    if ($CargoMetadata.PSObject.Properties.Name -contains 'workspace_root') {
        $locks = @($Source.files | Where-Object { $_.path -ceq ($relativeWorkspace + 'Cargo.lock') })
        if ($locks.Count -ne 1) { throw 'Missing copied workspace lockfile for owner evidence.' }
        foreach ($dependency in $CargoMetadata.packages | Where-Object { $reachable.Contains($_.id) }) {
            $sections = @($locks[0].cargo_packages | Where-Object {
                $_.name -ceq $dependency.name -and $_.version -ceq $dependency.version
            })
            if (-not $sections.Count) { throw "Missing resolved lock section: $($dependency.name) $($dependency.version)" }
            $input += @($sections | ForEach-Object { "$($_.name)@$($_.version)`t$($_.hash)" })
        }
    }
    return [ordered]@{
        package = $Package; dependencies = @($reachable | Sort-Object)
        roots = $localRoots; digest = Get-ValidationTextHash ($input -join "`n")
    }
}

function Get-ValidationCargoArguments {
    param([string]$Manifest, [string]$Package, [string]$Selector)
    if (-not $Manifest -or -not $Package) { throw 'Cargo suite requires -Manifest and -Package.' }
    $common = @('--locked', '--manifest-path', $Manifest.Replace('\', '/'), '-p', $Package)
    switch ($Selector) {
        'all' { return @('test') + $common }
        'guidance' { return @('test') + $common + @('guidance') }
        'clippy-all-targets' { return @('clippy') + $common + @('--all-targets', '--', '-D', 'warnings') }
        default { throw "Unknown Cargo test selector: $Selector" }
    }
}

function Write-ValidationJson {
    param([string]$Path, $Value)
    $parent = Split-Path -Parent $Path
    if ($parent) { [IO.Directory]::CreateDirectory($parent) | Out-Null }
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($false))
}

function New-ValidationContext {
    param([string]$Root, [string]$Context, $Source)
    [IO.Directory]::CreateDirectory($Context) | Out-Null
    foreach ($entry in $Source.files) {
        $inputPath = Resolve-ValidationInput $Root $entry.path
        $destination = Join-Path $Context $entry.path
        [IO.Directory]::CreateDirectory((Split-Path -Parent $destination)) | Out-Null
        [IO.File]::WriteAllBytes($destination, (Get-ValidationCopyBytes $inputPath))
        if ((Get-ValidationHash ([IO.File]::ReadAllBytes($destination))) -cne $entry.hash) {
            throw "Source changed while copying: $($entry.path)"
        }
    }
}
