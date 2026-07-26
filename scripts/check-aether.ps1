[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust/Cargo was not found.'
}

$env:CFG_RELEASE_CHANNEL = 'aether-check'

cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw 'cargo fmt failed.' }

cargo check -p aether_player --bin aether
if ($LASTEXITCODE -ne 0) { throw 'Normal Aether cargo check failed.' }

cargo check -p aether_player --bin aether --features metrics
if ($LASTEXITCODE -ne 0) { throw 'Metrics Aether cargo check failed.' }

cargo clippy --no-deps -p aether_player --bin aether --features metrics -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Aether clippy failed.' }

Write-Host 'Aether checks passed.'
