<#
.SYNOPSIS
    Commits the working tree, tags it, and publishes a GitHub release with the built artifacts.

.DESCRIPTION
    Everything a release needs, in the order it needs to happen, with the checks that have caught
    real mistakes built in rather than remembered:

      1. The version comes from aether\Cargo.toml. Nothing is passed by hand, so the tag, the
         artifact names and the binary can never disagree.
      2. The artifacts for that version must already exist. This script never builds; run
         installer\build-setup.cmd first.
      3. The shipped binary is checked for the build account name. A release that leaks it into
         every player's logs is stopped here rather than discovered by a player.
      4. Release notes come from a file you write. There is no generated text, because notes that
         nobody read are worse than none.
      5. Commit, tag, push, upload.

    Re-running against an existing tag is refused. Use -Force only to replace a release you have
    just published and nobody has downloaded.

.EXAMPLE
    pwsh .\scripts\publish-release.ps1 -NotesFile .\notes-0.5.11.md

.EXAMPLE
    pwsh .\scripts\publish-release.ps1 -NotesFile .\notes.md -DryRun
#>

[CmdletBinding()]
param(
    # Markdown file holding the release notes. Required: see point 4.
    [Parameter(Mandatory = $true)] [string] $NotesFile,

    # Commit message subject. The body is taken from the notes file.
    [string] $CommitMessage,

    # Show what would happen and change nothing.
    [switch] $DryRun,

    # Replace an existing release and tag of the same version.
    [switch] $Force,

    # Mark the GitHub release as a pre-release, so it is not offered to anyone as the latest
    # version and the updater leaves it alone.
    [switch] $PreRelease
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string] $Text) Write-Host ''; Write-Host "== $Text ==" -ForegroundColor Cyan }
function Write-Ok   { param([string] $Text) Write-Host "  $Text" -ForegroundColor DarkGray }

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)] [string] $FilePath,
        [Parameter(Mandatory = $true)] [string[]] $Arguments,
        [Parameter(Mandatory = $true)] [string] $FailureMessage
    )
    if ($DryRun) {
        Write-Host "  would run: $FilePath $($Arguments -join ' ')" -ForegroundColor Yellow
        return ''
    }
    $output = & $FilePath @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        $output | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
        throw $FailureMessage
    }
    return $output
}

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    # -- Version ------------------------------------------------------------------------------
    Write-Step 'Reading version'
    $cargoToml = Join-Path $root 'aether\Cargo.toml'
    $version = (Select-String -Path $cargoToml -Pattern '^version\s*=\s*"([^"]+)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
    if (-not $version) { throw "Could not read a version from $cargoToml." }
    $tag = "v$version"
    Write-Ok "version $version, tag $tag"

    # -- Preconditions ------------------------------------------------------------------------
    Write-Step 'Checking preconditions'

    foreach ($tool in 'git', 'gh') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "$tool was not found on PATH."
        }
    }

    $notesPath = Resolve-Path -LiteralPath $NotesFile -ErrorAction SilentlyContinue
    if (-not $notesPath) { throw "Release notes not found at $NotesFile." }
    $notes = Get-Content -LiteralPath $notesPath -Raw
    if ([string]::IsNullOrWhiteSpace($notes)) { throw "$NotesFile is empty." }
    # Em dashes read as machine-written and are not the house style.
    if ($notes -match '[—–]') { throw "$NotesFile contains an em or en dash. Rewrite those lines." }
    Write-Ok "notes: $notesPath"

    $distDir = Join-Path $root 'installer\dist'
    $assets = @(
        (Join-Path $distDir "Aether-Setup-$version-win-x64.exe"),
        (Join-Path $distDir "Aether-Portable-$version-win-x64.zip")
    )
    foreach ($asset in $assets) {
        if (-not (Test-Path -LiteralPath $asset -PathType Leaf)) {
            throw "Missing $([System.IO.Path]::GetFileName($asset)). Run installer\build-setup.cmd first."
        }
    }
    # The launcher is optional, since it is only built with -Launcher. It is uploaded when it is there and
    # not demanded when it is not, because a release without one is still a complete release.
    $launcher = Join-Path $distDir "Aether-Launcher-$version-win-x64.exe"
    if (Test-Path -LiteralPath $launcher -PathType Leaf) { $assets += $launcher }
    Write-Ok "artifacts present for $version ($($assets.Count) files)"

    if (-not $Force) {
        $existing = & git tag --list $tag
        if ($existing) { throw "Tag $tag already exists. Bump the version, or pass -Force to replace it." }
    }

    # -- The check that has already caught one shipped release --------------------------------
    Write-Step 'Checking the shipped binary for the build account name'
    $accountName = Split-Path -Leaf $env:USERPROFILE
    $staging = Join-Path ([System.IO.Path]::GetTempPath()) "aether-publish-$version"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    Expand-Archive -LiteralPath (Join-Path $distDir "Aether-Portable-$version-win-x64.zip") `
        -DestinationPath $staging -Force
    $shipped = Join-Path $staging 'aether.exe'
    if (-not (Test-Path -LiteralPath $shipped -PathType Leaf)) {
        throw "The portable zip does not contain aether.exe."
    }
    $leaked = Select-String -Path $shipped -Pattern ([regex]::Escape($accountName)) `
        -Encoding utf8 -List -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $staging -Recurse -Force
    if ($leaked) {
        throw "The shipped aether.exe contains the build account name '$accountName'. Rebuild with installer\build-setup.cmd, which remaps build paths, and publish that."
    }
    Write-Ok "clean, no account name in aether.exe"

    # -- Checksums ----------------------------------------------------------------------------
    Write-Step 'Writing SHA256SUMS.txt'
    $sumsPath = Join-Path $distDir 'SHA256SUMS.txt'
    $lines = foreach ($asset in $assets) {
        $hash = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([System.IO.Path]::GetFileName($asset))"
    }
    if (-not $DryRun) { Set-Content -LiteralPath $sumsPath -Value $lines -Encoding ascii }
    $lines | ForEach-Object { Write-Ok $_ }

    # -- Commit, tag, push --------------------------------------------------------------------
    Write-Step 'Committing'
    $dirty = & git status --porcelain
    if ($dirty) {
        if (-not $CommitMessage) { $CommitMessage = "Release $version" }
        Invoke-Native git @('add', '-A') 'git add failed.'
        $messageFile = Join-Path ([System.IO.Path]::GetTempPath()) "aether-commit-$version.txt"
        Set-Content -LiteralPath $messageFile -Value (@($CommitMessage, '', $notes) -join "`n") -Encoding utf8
        Invoke-Native git @('commit', '-F', $messageFile) 'git commit failed.'
        Remove-Item -LiteralPath $messageFile -Force -ErrorAction SilentlyContinue
        Write-Ok "committed: $CommitMessage"
    }
    else {
        Write-Ok 'working tree already clean, nothing to commit'
    }

    Write-Step "Tagging $tag"
    if ($Force) {
        Invoke-Native git @('tag', '-f', '-a', $tag, '-m', "Aether $version") 'git tag failed.'
    }
    else {
        Invoke-Native git @('tag', '-a', $tag, '-m', "Aether $version") 'git tag failed.'
    }

    Write-Step 'Pushing'
    Invoke-Native git @('push', 'origin', 'HEAD') 'git push failed.'
    $pushTag = if ($Force) { @('push', '--force', 'origin', $tag) } else { @('push', 'origin', $tag) }
    Invoke-Native git $pushTag 'git push --tags failed.'

    # -- Release ------------------------------------------------------------------------------
    Write-Step 'Publishing the release'
    if ($Force) {
        # Delete only the GitHub release. Keep the tag: it was just recreated locally and
        # force-pushed above, and the new release needs that exact tag to still exist.
        if ($DryRun) {
            Write-Host "  would run: gh release delete $tag --yes" -ForegroundColor Yellow
        }
        else {
            & gh release delete $tag --yes 2>&1 | Out-Null
            # A missing release is fine under -Force: the tag may exist without a release.
            # Any existing tag has already been replaced by the forced tag push above.
        }
    }
    $releaseArgs = @(
        'release', 'create', $tag,
        '--title', "Aether $version",
        '--notes-file', $notesPath.Path
    )
    if ($PreRelease) { $releaseArgs += '--prerelease' }
    $releaseArgs = $releaseArgs + $assets + @($sumsPath)
    Invoke-Native gh $releaseArgs 'gh release create failed.'

    Write-Host ''
    if ($DryRun) {
        Write-Host "Dry run finished. Nothing was changed." -ForegroundColor Yellow
    }
    else {
        Write-Host "Published Aether $version" -ForegroundColor Green
        Write-Host "  https://github.com/unl0cks/Aether/releases/tag/$tag"
    }
}
finally {
    Pop-Location
}
