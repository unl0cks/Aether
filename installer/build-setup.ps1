<#
.SYNOPSIS
    Builds Aether's self-contained Windows x64 setup executable and a portable ZIP.

.DESCRIPTION
    Three passes, in this order, because each one needs the previous:

      1. cargo build --release            -> aether.exe, the thing being installed
      2. dotnet publish Aether.Setup      -> a payload-LESS installer. It is not thrown away: it becomes
         (no AetherPayload)                  uninstall.exe inside the payload, so removing Aether never
                                             depends on keeping setup.exe around. It also packs the archive,
                                             via its own --pack mode, so packing and unpacking are the same code.
      3. dotnet publish Aether.Setup      -> the real setup.exe, single-file, with the archive embedded
         (AetherPayload=<archive>)
      4. dotnet publish Aether.Setup      -> OPTIONAL, -Launcher only. The same GUI with no payload, which
         (AetherLauncher=true)               downloads the published portable zip at install time instead of
                                             carrying one. A few megabytes rather than a hundred.

    The staging directory IS the payload: whatever ends up in it is what gets installed, so new runtime files
    are picked up without editing a file list.

.EXAMPLE
    pwsh .\installer\build-setup.ps1 -OpenOutputFolder

.EXAMPLE
    pwsh .\installer\build-setup.ps1 -Version 0.3.24 -SkipCargo
#>

[CmdletBinding()]
param(
    [string] $Version,

    # Reuse whatever is already in target\release instead of rebuilding the client. Handy while iterating on
    # the installer itself, where the Rust side hasn't changed.
    [switch] $SkipCargo,

    [switch] $SkipPortableZip,

    # Also publish the launcher: the same GUI with no payload, which downloads the published portable archive
    # instead of carrying it. A few megabytes rather than a hundred, at the cost of needing a connection.
    [switch] $Launcher,

    [switch] $OpenOutputFolder,

    # Brotli quality 11 instead of the default. Worth it for a build you actually publish; not worth the
    # several extra minutes while iterating, because the payload is mostly incompressible binaries already.
    [switch] $MaxCompression,

    # Features passed to cargo. `metrics` is deliberately NOT the default: it is a diagnostic build.
    [string] $CargoFeatures = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step {
    param([string] $Text)
    Write-Host ''
    Write-Host "== $Text ==" -ForegroundColor Cyan
}

function Invoke-NativeCommand {
    param(
        [Parameter(Mandatory = $true)] [string] $FilePath,
        [Parameter(Mandatory = $true)] [string[]] $Arguments,
        [Parameter(Mandatory = $true)] [string] $FailureMessage,
        [string] $WorkingDirectory
    )

    if ($WorkingDirectory) { Push-Location -LiteralPath $WorkingDirectory }
    try {
        & $FilePath @Arguments
        $exitCode = $LASTEXITCODE
    }
    finally {
        if ($WorkingDirectory) { Pop-Location }
    }
    if ($exitCode -ne 0) { throw "$FailureMessage Exit code: $exitCode" }
}

function New-PayloadArchive {
    <#
      tar + Brotli, the exact pair Payload.ExtractAsync reads back: TarFile.CreateFromDirectory with no base
      directory, wrapped in a BrotliStream.

      This is done here, in-process, rather than by a --pack mode on the installer itself. That was the first
      design and it was wrong: app.manifest asks for administrator, so building the setup raised a UAC prompt
      merely to compress a folder, and the elevated packer then held the staging directory open where this
      script could neither wait on it nor clean up after it.

      Compression level is a real choice, not a default. Aether's payload is two already-compressed binaries —
      a Rust release build and a .NET single-file bundle — so SmallestSize (Brotli quality 11) spends minutes
      to save low single-digit percent. Optimal is the sane default; -MaxCompression is there for the build
      that actually gets published. Either is readable by the installer; Brotli decompression doesn't care
      what quality produced the stream.
    #>
    param(
        [Parameter(Mandatory = $true)] [string] $SourceDirectory,
        [Parameter(Mandatory = $true)] [string] $DestinationFile,
        [Parameter(Mandatory = $true)] [bool] $Maximum
    )

    $level = if ($Maximum) {
        [System.IO.Compression.CompressionLevel]::SmallestSize
    } else {
        [System.IO.Compression.CompressionLevel]::Optimal
    }

    [void](New-Item -ItemType Directory -Path (Split-Path -Parent $DestinationFile) -Force)
    if (Test-Path -LiteralPath $DestinationFile) { Remove-Item -LiteralPath $DestinationFile -Force }

    $outFile = [System.IO.File]::Create($DestinationFile)
    try {
        $brotli = [System.IO.Compression.BrotliStream]::new($outFile, $level)
        try {
            [System.Formats.Tar.TarFile]::CreateFromDirectory($SourceDirectory, $brotli, $false)
        }
        finally { $brotli.Dispose() }
    }
    finally { $outFile.Dispose() }
}

function Find-DotNet10 {
    $command = Get-Command dotnet.exe -ErrorAction SilentlyContinue
    if (-not $command) { $command = Get-Command dotnet -ErrorAction SilentlyContinue }
    if (-not $command) { throw '.NET SDK was not found. Install the .NET 10 SDK first.' }

    $sdkList = & $command.Source --list-sdks
    if ($LASTEXITCODE -ne 0) { throw 'dotnet --list-sdks failed.' }
    if (-not ($sdkList -match '(?m)^10\.')) {
        throw ".NET 10 SDK was not found.`nInstalled SDKs:`n$($sdkList -join "`n")"
    }
    return $command.Source
}

function Get-VersionFromCargoToml {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    $text = Get-Content -LiteralPath $Path -Raw
    # The first `version = "..."` after [package], which is aether_player's own.
    $match = [regex]::Match($text, '(?ms)^\s*\[package\].*?^\s*version\s*=\s*"([^"]+)"')
    if ($match.Success) { return $match.Groups[1].Value.Trim() }
    return $null
}

function Convert-ToFileVersion {
    param([string] $SemanticVersion)
    $base = ($SemanticVersion -split '[-+]')[0]
    $parts = @()
    foreach ($part in @($base -split '\.')) {
        if ($part -match '^\d+$') {
            $number = [int]$part
            if ($number -lt 0) { $number = 0 }
            if ($number -gt 65535) { $number = 65535 }
            $parts += $number
        }
        else { $parts += 0 }
    }
    while ($parts.Count -lt 4) { $parts += 0 }
    return ($parts[0..3] -join '.')
}

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

$installerDir = $PSScriptRoot
$root = Split-Path -Parent $installerDir
$setupProject = Join-Path $installerDir 'Aether.Setup\Aether.Setup.csproj'
$cargoToml = Join-Path $root 'aether\Cargo.toml'

if (-not (Test-Path -LiteralPath $setupProject -PathType Leaf)) {
    throw "Setup project not found: $setupProject"
}

if (-not $Version) { $Version = Get-VersionFromCargoToml -Path $cargoToml }
if (-not $Version) { throw "Could not read a version from $cargoToml. Pass -Version." }
if ($Version -notmatch '^[0-9A-Za-z][0-9A-Za-z._+\-]*$') { throw "Invalid version '$Version'." }

$fileVersion = Convert-ToFileVersion -SemanticVersion $Version

$buildRoot = Join-Path $installerDir '.build'
$configRoot = Join-Path $buildRoot 'Release'
$payloadDir = Join-Path $configRoot 'payload'      # <- this directory IS what gets installed
$bareDir = Join-Path $configRoot 'bare'            # <- payload-less installer / packer / uninstall.exe
$setupDir = Join-Path $configRoot 'setup'
$launcherDir = Join-Path $configRoot 'launcher'
$archive = Join-Path $configRoot 'Aether.payload'
$distDir = Join-Path $installerDir 'dist'
$setupPath = Join-Path $distDir "Aether-Setup-$Version-win-x64.exe"
$launcherPath = Join-Path $distDir "Aether-Launcher-$Version-win-x64.exe"
$portablePath = Join-Path $distDir "Aether-Portable-$Version-win-x64.zip"
$hashPath = Join-Path $distDir 'SHA256SUMS.txt'

Write-Host ''
Write-Host 'Aether Setup Builder' -ForegroundColor Green
Write-Host "  Root:         $root"
Write-Host "  Version:      $Version"
Write-Host "  File version: $fileVersion"
Write-Host "  Staging:      $payloadDir"
Write-Host "  Output:       $distDir"

$dotnet = Find-DotNet10

Write-Step 'Cleaning staging directory'
if (Test-Path -LiteralPath $configRoot) { Remove-Item -LiteralPath $configRoot -Recurse -Force }
[void](New-Item -ItemType Directory -Path $payloadDir -Force)
[void](New-Item -ItemType Directory -Path $distDir -Force)

# ---------------------------------------------------------------------------
# Pass 1 — the client
# ---------------------------------------------------------------------------

$releaseExe = Join-Path $root 'target\release\aether.exe'

if (-not $SkipCargo) {
    Write-Step "Building Aether ($Version, release)"
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) { throw 'cargo was not found. Install the Rust toolchain, or pass -SkipCargo.' }

    # Rust bakes absolute source paths into the binary, through file!() and through the log crate's
    # log.file field. Without remapping, every log line and crash report a player sends back quotes
    # the home directory, and therefore the Windows account name, of whoever built the release. A
    # tester reported reading a stranger's username throughout his own logs; that was this.
    #
    # This has to live here rather than only in scripts/build-release.sh, because the published
    # installer and portable zip are built by this script, and a remapping the release path can
    # forget is one that will be forgotten.
    #
    # CARGO_ENCODED_RUSTFLAGS rather than RUSTFLAGS: RUSTFLAGS splits on spaces, and the account
    # name that prompted this fix has spaces in it, so the plain form would mangle the very path it
    # is meant to remove. The encoded form separates on U+001F and has no such problem.
    # Three prefixes, because the account name reaches the binary by three different routes and
    # only two of them were being covered. `.rustup` is the one that was missed: with debug info on,
    # rustc rewrites the standard library's `/rustc/<hash>/library/...` paths to wherever the
    # `rust-src` component actually lives, so every `unwrap` message in `std` carries the toolchain
    # path, and the toolchain lives under the user's profile. That is 528 copies of the account name
    # in a build that was otherwise clean, and the check below is what caught it.
    #
    # Both separator forms are supplied for the toolchain: the paths land in the binary mixed
    # (`...\.rustup\toolchains\...\lib/rustlib/src/rust\library`), and matching costs nothing when
    # the prefix does not apply.
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
    $remap = @(
        "--remap-path-prefix=$($cargoHome -replace '\\', '/')=/cargo"
        "--remap-path-prefix=$($root -replace '\\', '/')=/aether"
        "--remap-path-prefix=$($rustupHome -replace '\\', '/')=/rust"
        "--remap-path-prefix=$rustupHome=/rust"
    )
    $previousRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
    $env:CARGO_ENCODED_RUSTFLAGS = $remap -join "`u{001f}"

    # Without this the channel defaults to "local", so a published build called itself
    # "0.5.10-local" in every log and crash report, and the guard that stops a developer build from
    # offering itself an update could never tell the two apart.
    $previousChannel = $env:CFG_RELEASE_CHANNEL
    $env:CFG_RELEASE_CHANNEL = 'release'

    try {
        $cargoArgs = @('build', '--release', '-p', 'aether_player')
        if ($CargoFeatures) { $cargoArgs += @('--features', $CargoFeatures) }
        Invoke-NativeCommand -FilePath $cargo.Source -Arguments $cargoArgs `
            -FailureMessage 'cargo build failed.' -WorkingDirectory $root
    }
    finally {
        $env:CARGO_ENCODED_RUSTFLAGS = $previousRustFlags
        $env:CFG_RELEASE_CHANNEL = $previousChannel
    }

    # Changing RUSTFLAGS changes the fingerprint, so this rebuilds rather than reusing a plain
    # cargo build. Verify rather than assume: the whole point is that a silent regression here is
    # invisible until a player posts a log.
    $accountName = Split-Path -Leaf $env:USERPROFILE
    $leaked = Select-String -Path $releaseExe -Pattern ([regex]::Escape($accountName)) `
        -Encoding utf8 -AllMatches -List -ErrorAction SilentlyContinue
    if ($leaked) {
        throw "aether.exe still contains the build account name '$accountName'. Path remapping did not take effect; publishing this would leak it into every player's logs."
    }
    Write-Host "  build paths remapped, no account name in the binary" -ForegroundColor DarkGray
}
else {
    Write-Step 'Skipping cargo build (-SkipCargo)'
}

if (-not (Test-Path -LiteralPath $releaseExe -PathType Leaf)) {
    throw "aether.exe was not found at $releaseExe. Build it, or drop -SkipCargo."
}

Write-Step 'Staging the install payload'
Copy-Item -LiteralPath $releaseExe -Destination (Join-Path $payloadDir 'aether.exe') -Force

# Anything else the client needs at run time goes in beside it. Aether is a single static binary today, so
# this loop is usually a no-op — it exists so that adding a DLL later needs no change here.
foreach ($extra in @('LICENSE.md', 'README.md')) {
    $src = Join-Path $root $extra
    if (Test-Path -LiteralPath $src -PathType Leaf) {
        Copy-Item -LiteralPath $src -Destination (Join-Path $payloadDir $extra) -Force
    }
}

# ---------------------------------------------------------------------------
# Pass 2 — the payload-less installer: uninstall.exe, and the packer
# ---------------------------------------------------------------------------

Write-Step 'Publishing the bare installer (becomes uninstall.exe)'
Invoke-NativeCommand -FilePath $dotnet -Arguments @(
    'publish', $setupProject,
    '-c', 'Release', '-r', 'win-x64', '--self-contained', 'true',
    '-o', $bareDir,
    '-p:PublishSingleFile=true',
    '-p:EnableCompressionInSingleFile=true',
    # NOT optional. Without it a single-file publish still drops libSkiaSharp/av_libglesv2/libHarfBuzzSharp
    # NEXT TO the exe, and since only the .exe is copied the installer starts with no renderer and dies
    # instantly — with no console to say why.
    '-p:IncludeNativeLibrariesForSelfExtract=true',
    '-p:PublishTrimmed=false', '-p:PublishReadyToRun=false',
    '-p:DebugType=none', '-p:DebugSymbols=false',
    "-p:Version=$Version", "-p:AssemblyVersion=$fileVersion",
    "-p:FileVersion=$fileVersion", "-p:InformationalVersion=$Version"
) -FailureMessage 'Publishing the bare installer failed.'

$bareExe = Join-Path $bareDir 'Aether.Setup.exe'
if (-not (Test-Path -LiteralPath $bareExe -PathType Leaf)) {
    throw "Expected $bareExe after publishing the bare installer."
}

Copy-Item -LiteralPath $bareExe -Destination (Join-Path $payloadDir 'uninstall.exe') -Force

$payloadFiles = @(Get-ChildItem -LiteralPath $payloadDir -File -Recurse)
$payloadBytes = ($payloadFiles | Measure-Object -Property Length -Sum).Sum
Write-Host ("Payload: {0} file(s), {1:0.0} MiB uncompressed" -f $payloadFiles.Count, ($payloadBytes / 1MB))

Write-Step ('Compressing the payload ({0})' -f $(if ($MaxCompression) { 'maximum' } else { 'balanced' }))
New-PayloadArchive -SourceDirectory $payloadDir -DestinationFile $archive -Maximum $MaxCompression.IsPresent

if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    throw "Packing reported success but $archive is not there."
}
$archiveBytes = (Get-Item -LiteralPath $archive).Length
if ($archiveBytes -le 0) { throw "The payload archive is empty: $archive" }
Write-Host ("Archive: {0:0.0} MiB compressed ({1:0}% of {2:0.0} MiB)" -f `
    ($archiveBytes / 1MB), (100 * $archiveBytes / $payloadBytes), ($payloadBytes / 1MB))

# ---------------------------------------------------------------------------
# Pass 3 — the real setup
# ---------------------------------------------------------------------------

Write-Step 'Publishing setup.exe with the payload embedded'
Invoke-NativeCommand -FilePath $dotnet -Arguments @(
    'publish', $setupProject,
    '-c', 'Release', '-r', 'win-x64', '--self-contained', 'true',
    '-o', $setupDir,
    '-p:PublishSingleFile=true',
    '-p:EnableCompressionInSingleFile=true',
    '-p:IncludeNativeLibrariesForSelfExtract=true',
    '-p:PublishTrimmed=false', '-p:PublishReadyToRun=false',
    '-p:DebugType=none', '-p:DebugSymbols=false',
    "-p:Version=$Version", "-p:AssemblyVersion=$fileVersion",
    "-p:FileVersion=$fileVersion", "-p:InformationalVersion=$Version",
    "-p:AetherPayload=$archive"
) -FailureMessage 'Publishing setup.exe failed.'

$builtSetup = Join-Path $setupDir 'Aether.Setup.exe'
if (-not (Test-Path -LiteralPath $builtSetup -PathType Leaf)) {
    throw "Expected $builtSetup after publishing setup.exe."
}

if (Test-Path -LiteralPath $setupPath) { Remove-Item -LiteralPath $setupPath -Force }
Copy-Item -LiteralPath $builtSetup -Destination $setupPath -Force

# ---------------------------------------------------------------------------
# Pass 4 (optional) — the launcher
# ---------------------------------------------------------------------------

# The same GUI as setup.exe, with no payload and the launcher flag set, so it fetches the published portable
# archive at install time instead of carrying one. That makes it a few megabytes instead of a hundred, which is
# the difference between a download people will start and one they put off.
#
# It has to be published AFTER the payload passes and never instead of them: the launcher installs whatever the
# releases page is currently offering, so the portable zip for this version has to exist and be published
# before a launcher built alongside it can install this version.
if ($Launcher) {
    Write-Step 'Publishing the launcher (downloads instead of carrying a payload)'
    Invoke-NativeCommand -FilePath $dotnet -Arguments @(
        'publish', $setupProject,
        '-c', 'Release', '-r', 'win-x64', '--self-contained', 'true',
        '-o', $launcherDir,
        '-p:PublishSingleFile=true',
        '-p:EnableCompressionInSingleFile=true',
        '-p:IncludeNativeLibrariesForSelfExtract=true',
        '-p:PublishTrimmed=false', '-p:PublishReadyToRun=false',
        '-p:DebugType=none', '-p:DebugSymbols=false',
        "-p:Version=$Version", "-p:AssemblyVersion=$fileVersion",
        "-p:FileVersion=$fileVersion", "-p:InformationalVersion=$Version",
        '-p:AetherLauncher=true'
    ) -FailureMessage 'Publishing the launcher failed.'

    $builtLauncher = Join-Path $launcherDir 'Aether.Setup.exe'
    if (-not (Test-Path -LiteralPath $builtLauncher -PathType Leaf)) {
        throw "Expected $builtLauncher after publishing the launcher."
    }

    # A launcher the size of the full setup means the payload leaked into it, and it would then install the
    # embedded copy rather than the published one. Cheap to check, silent and confusing if it ever happens.
    $launcherBytes = (Get-Item -LiteralPath $builtLauncher).Length
    if ($launcherBytes -ge $archiveBytes) {
        throw ("The launcher is {0:0.0} MiB, which is no smaller than the payload it is supposed to omit. It was built with a payload embedded." -f ($launcherBytes / 1MB))
    }

    if (Test-Path -LiteralPath $launcherPath) { Remove-Item -LiteralPath $launcherPath -Force }
    Copy-Item -LiteralPath $builtLauncher -Destination $launcherPath -Force
    Write-Host ("  Launcher: {0:0.0} MiB against {1:0.0} MiB for the full setup" -f `
        ($launcherBytes / 1MB), ((Get-Item -LiteralPath $setupPath).Length / 1MB))
}

# ---------------------------------------------------------------------------
# Portable ZIP + hashes
# ---------------------------------------------------------------------------

if (-not $SkipPortableZip) {
    Write-Step 'Creating portable ZIP'
    if (Test-Path -LiteralPath $portablePath) { Remove-Item -LiteralPath $portablePath -Force }

    # Staged separately rather than zipping $payloadDir. That directory has uninstall.exe added to it by
    # then, and a portable build shipping an uninstaller is both nonsense and expensive: it was 45.8 MB of
    # a 56.4 MB archive, four times the size of the program it came with. This is also the archive the
    # launcher downloads, so it holds exactly what a working Aether needs and nothing else.
    $portableStage = Join-Path $configRoot 'portable'
    if (Test-Path -LiteralPath $portableStage) { Remove-Item -LiteralPath $portableStage -Recurse -Force }
    [void](New-Item -ItemType Directory -Path $portableStage -Force)

    Copy-Item -LiteralPath (Join-Path $payloadDir 'aether.exe') -Destination $portableStage -Force
    foreach ($doc in @('LICENSE.md', 'README.md')) {
        $src = Join-Path $payloadDir $doc
        if (Test-Path -LiteralPath $src -PathType Leaf) {
            Copy-Item -LiteralPath $src -Destination $portableStage -Force
        }
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory(
        $portableStage, $portablePath, [System.IO.Compression.CompressionLevel]::Optimal, $false)
}

Write-Step 'Writing SHA256 checksums'
$hashTargets = @($setupPath)
if ($Launcher -and (Test-Path -LiteralPath $launcherPath -PathType Leaf)) {
    $hashTargets += $launcherPath
}
if (-not $SkipPortableZip -and (Test-Path -LiteralPath $portablePath -PathType Leaf)) {
    $hashTargets += $portablePath
}
$hashLines = foreach ($target in $hashTargets) {
    $hash = Get-FileHash -LiteralPath $target -Algorithm SHA256
    "$($hash.Hash.ToLowerInvariant())  $(Split-Path -Leaf $target)"
}
$hashLines | Set-Content -LiteralPath $hashPath -Encoding ASCII

$setupMiB = (Get-Item -LiteralPath $setupPath).Length / 1MB

Write-Host ''
Write-Host 'BUILD SUCCEEDED' -ForegroundColor Green
Write-Host ("  Setup:    $setupPath ({0:0.0} MiB)" -f $setupMiB)
if ($Launcher) { Write-Host "  Launcher: $launcherPath" }
if (-not $SkipPortableZip) { Write-Host "  Portable: $portablePath" }
Write-Host "  SHA256:   $hashPath"
Write-Host ''

if ($OpenOutputFolder) { Start-Process explorer.exe "/select,`"$setupPath`"" }
