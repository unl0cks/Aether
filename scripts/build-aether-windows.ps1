[CmdletBinding()]
param(
    [switch]$Metrics,
    [switch]$Tracy,
    [ValidateSet('dist', 'release')]
    [string]$Profile = 'dist'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust/Cargo was not found. Install Rust from rustup and reopen PowerShell.'
}

$features = [System.Collections.Generic.List[string]]::new()
if ($Metrics) { $features.Add('metrics') }
if ($Tracy) { $features.Add('tracy') }

$arguments = [System.Collections.Generic.List[string]]::new()
$arguments.Add('build')
$arguments.Add('-p')
$arguments.Add('aether_player')
$arguments.Add('--bin')
$arguments.Add('aether')

if ($Profile -eq 'dist') {
    $arguments.Add('--profile')
    $arguments.Add('dist')
} else {
    $arguments.Add('--release')
}

if ($features.Count -gt 0) {
    $arguments.Add('--features')
    $arguments.Add(($features -join ','))
}

$env:CFG_RELEASE_CHANNEL = 'aether-local'
Write-Host ('cargo ' + ($arguments -join ' '))
& cargo @arguments
if ($LASTEXITCODE -ne 0) {
    throw "Cargo failed with exit code $LASTEXITCODE."
}

$outputFolder = if ($Profile -eq 'dist') { 'dist' } else { 'release' }
$outputPath = Join-Path $repoRoot "target\$outputFolder\aether.exe"
if (-not (Test-Path $outputPath)) {
    throw "Build completed, but the expected executable was not found at $outputPath"
}

Write-Host "Built: $outputPath"
