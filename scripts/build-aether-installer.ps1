[CmdletBinding()]
param(
    [string]$Version,
    [string]$BinaryPath,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($Version)) {
    $metadataJson = & cargo metadata `
        --manifest-path (Join-Path $repoRoot 'Cargo.toml') `
        --no-deps `
        --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to read the Aether package version (exit code $LASTEXITCODE)."
    }

    $aetherPackage = ($metadataJson | ConvertFrom-Json).packages |
        Where-Object { $_.name -eq 'aether_player' } |
        Select-Object -First 1
    if ($null -eq $aetherPackage) {
        throw 'Unable to find the aether_player package in Cargo metadata.'
    }
    $Version = $aetherPackage.version
}
if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $BinaryPath = Join-Path $repoRoot 'target\dist\aether.exe'
}
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path (Split-Path -Parent $repoRoot) 'Aether-Release\installers'
}

$binary = [System.IO.Path]::GetFullPath($BinaryPath)
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$wixSource = Join-Path $repoRoot 'packaging\windows\Aether.wxs'
$wixVersion = '4.0.6'
$toolDirectory = Join-Path $repoRoot ".tools\wix-$wixVersion"
$wix = Join-Path $toolDirectory 'wix.exe'

if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Aether executable was not found at: $binary"
}
if (-not (Test-Path -LiteralPath $wixSource -PathType Leaf)) {
    throw "WiX source was not found at: $wixSource"
}

if (-not (Test-Path -LiteralPath $wix -PathType Leaf)) {
    New-Item -ItemType Directory -Path $toolDirectory -Force | Out-Null
    & dotnet tool install --tool-path $toolDirectory wix --version $wixVersion
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to install the local WiX build tool (exit code $LASTEXITCODE)."
    }
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
$installer = Join-Path $output "Aether-$Version-x64.msi"

$env:AETHER_BINARY = $binary
$env:AETHER_PROJECT_ROOT = $repoRoot
$env:AETHER_VERSION = $Version

& $wix build $wixSource -arch x64 -pdbtype none -o $installer
if ($LASTEXITCODE -ne 0) {
    throw "WiX failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "WiX completed without producing the expected installer: $installer"
}

$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
$checksumPath = "$installer.sha256"
"$hash  $(Split-Path -Leaf $installer)" | Set-Content -LiteralPath $checksumPath -Encoding ascii

Write-Host "Installer: $installer"
Write-Host "SHA-256:  $hash"
Write-Host "Checksum: $checksumPath"
