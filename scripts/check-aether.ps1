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

# Tests. This script formatted and linted for a long time without ever running one, which is how a
# renderer test came to fail on main unnoticed: it asserted a texture-census total against a
# hardcoded 513 from when the table held 512 buckets, and nothing ran it after the table grew.
#
# `ruffle_core` is checked with the metrics features because most of Aether's own tests live behind
# them, and `--test-threads=1` because the census tests assert exact totals against process-global
# counters that any two of them would otherwise share.
#
# `aether_performance` is in the list because `aether/Cargo.toml` turns it on, so it ships -- and it
# was not here, which meant a whole module of shipped code had never been covered by this gate. The
# bitmap cache sweep lives in it.
cargo test -p ruffle_core --features aether_metrics,aether_performance -- --test-threads=1
if ($LASTEXITCODE -ne 0) { throw 'ruffle_core tests failed.' }

cargo test -p ruffle_render_wgpu --features aether_metrics -- --test-threads=1
if ($LASTEXITCODE -ne 0) { throw 'ruffle_render_wgpu tests failed.' }

cargo test -p aether_player
if ($LASTEXITCODE -ne 0) { throw 'aether_player tests failed.' }

Write-Host 'Aether checks passed.'
