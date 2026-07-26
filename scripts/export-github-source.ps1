[CmdletBinding()]
param(
    [string]$Destination
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$releaseRoot = [System.IO.Path]::GetFullPath(
    (Join-Path (Split-Path -Parent $repoRoot) 'Aether-Release')
)
if ([string]::IsNullOrWhiteSpace($Destination)) {
    $Destination = Join-Path $releaseRoot 'Aether-GitHub-Source'
}

$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$releasePrefix = $releaseRoot.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
) + [System.IO.Path]::DirectorySeparatorChar
if (-not $destinationPath.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace a source export outside the intended release folder: $destinationPath"
}

if (Test-Path -LiteralPath $destinationPath) {
    $resolvedDestination = (Resolve-Path -LiteralPath $destinationPath).Path
    if (-not $resolvedDestination.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Resolved source export escaped the intended release folder: $resolvedDestination"
    }
    Remove-Item -LiteralPath $resolvedDestination -Recurse -Force
}

New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null

$excludedDirectories = @(
    (Join-Path $repoRoot 'target'),
    (Join-Path $repoRoot 'captures'),
    (Join-Path $repoRoot '.tools'),
    (Join-Path $repoRoot '.git'),
    (Join-Path $repoRoot '.github'),
    (Join-Path $repoRoot '.superpowers'),
    (Join-Path $repoRoot 'docs')
)

& robocopy $repoRoot $destinationPath /E /COPY:DAT /DCOPY:DAT /R:1 /W:1 /NFL /NDL /NJH /NJS /NP /XD $excludedDirectories
$robocopyExitCode = $LASTEXITCODE
if ($robocopyExitCode -ge 8) {
    throw "Source export failed with robocopy exit code $robocopyExitCode."
}

$generatedRootFiles = Get-ChildItem -LiteralPath $destinationPath -File | Where-Object {
    $_.Extension -eq '.jsonl' -or
    $_.Name -like 'build-*.log' -or
    $_.Name -eq 'spider.swf' -or
    $_.Name -like 'town-battleon-*.swf'
}
foreach ($file in $generatedRootFiles) {
    Remove-Item -LiteralPath $file.FullName -Force
}

$unnecessaryDirectoryNames = @(
    'tests',
    'test',
    'test_data',
    'testdata',
    'fixtures',
    'examples',
    'integration_tests',
    'assembly_tests'
)
$unnecessaryDirectories = Get-ChildItem -LiteralPath $destinationPath -Directory -Recurse |
    Where-Object { $unnecessaryDirectoryNames -contains $_.Name } |
    Sort-Object { $_.FullName.Length } -Descending
foreach ($directory in $unnecessaryDirectories) {
    if (Test-Path -LiteralPath $directory.FullName) {
        Remove-Item -LiteralPath $directory.FullName -Recurse -Force
    }
}

$unnecessaryFiles = Get-ChildItem -LiteralPath $destinationPath -File -Recurse | Where-Object {
    $_.Name -match '^(?i:readme)(?:[._-].*)?(?:\.[^.]+)?$' -or
    $_.Name -match '(?i)(codex|claude|chatgpt|handoff|prompt)' -or
    ($_.DirectoryName -eq $destinationPath -and $_.Name -in @(
        'AETHER_FILE_MANIFEST.txt',
        'CODE_OF_CONDUCT.md',
        'CONTRIBUTING.md',
        'movement-stop-console-v3.txt'
    )) -or
    ($_.DirectoryName -eq (Join-Path $destinationPath 'scripts') -and (
        $_.Name -like 'test-*.ps1' -or
        $_.Name -like 'capture-*.ps1' -or
        $_.Name -like 'compare-*.ps1' -or
        $_.Name -like 'summarize-*.ps1'
    ))
}
foreach ($file in $unnecessaryFiles) {
    Remove-Item -LiteralPath $file.FullName -Force
}

# Keep one canonical, Aether-focused README at the repository root. Historical
# implementation and hotfix READMEs are intentionally omitted from the export.
$canonicalReadmeSource = Join-Path $repoRoot 'README_AETHER_RELEASE.md'
$canonicalReadmeDestination = Join-Path $destinationPath 'README.md'
if (-not (Test-Path -LiteralPath $canonicalReadmeSource -PathType Leaf)) {
    throw "Canonical Aether README was not found: $canonicalReadmeSource"
}
Copy-Item -LiteralPath $canonicalReadmeSource -Destination $canonicalReadmeDestination -Force

# The exported folder intentionally omits standalone test crates. Remove those
# crates from the copied workspace member list so `cargo metadata` and normal
# Aether builds remain valid.
$workspaceManifestPath = Join-Path $destinationPath 'Cargo.toml'
$omittedWorkspaceMembers = @(
    '"exporter/integration_tests",',
    '"render/pixel_bender/assembly_tests",',
    '"tests",',
    '"tests/fs-tests-runner",',
    '"tests/input-format",',
    '"tests/socket-format",',
    '"tests/mocket",',
    '"tests/framework",'
)
$workspaceManifestLines = Get-Content -LiteralPath $workspaceManifestPath
$workspaceManifestLines = $workspaceManifestLines | Where-Object {
    $omittedWorkspaceMembers -notcontains $_.Trim()
}
[System.IO.File]::WriteAllLines(
    $workspaceManifestPath,
    $workspaceManifestLines,
    [System.Text.UTF8Encoding]::new($false)
)

$fileCount = (Get-ChildItem -LiteralPath $destinationPath -File -Recurse).Count
$sizeBytes = (
    Get-ChildItem -LiteralPath $destinationPath -File -Recurse |
        Measure-Object -Property Length -Sum
).Sum

Write-Host "GitHub-ready source folder: $destinationPath"
Write-Host "Files: $fileCount"
Write-Host ("Size:  {0:N2} MiB" -f ($sizeBytes / 1MB))
