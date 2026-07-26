# Aether

Aether is an AQW-focused desktop client derived from Ruffle. This repository contains the clean
source tree for the current Windows testing build.

## Normal use

Launch `aether.exe` directly or use either shortcut created by the installer. No command-line
arguments are required.

In its normal no-argument AQW mode, Aether:

- opens the live AQW loader;
- targets 60 FPS natively;
- enables the AQW timeline-child compatibility repair;
- enables the premature movement-stop guard;
- coalesces continuous mouse motion at 60 Hz;
- enables the verified bitmap-cache, avatar-cache, AVM2 broadcast, and bounded texture-pool
  optimizations;
- preserves high vector quality and uses the high-power graphics preference.

Explicit command-line options remain available for diagnostics and compatibility testing. Passing
`--generic` or an explicit SWF path/URL keeps Aether in generic Ruffle behavior instead of applying
the AQW preset.

## Preview status

This is a testing build. AQW content changes continuously, and some recently released content may
still need compatibility work. Report the map, visible player count, equipped/shown items, and
reproduction steps with any visual or gameplay issue.

## Building from source

Install a current Rust toolchain, then run:

```powershell
.\scripts\check-aether.ps1
.\scripts\build-aether-windows.ps1 -Metrics
```

The optimized executable is written to `target\dist\aether.exe`.

To export a clean source tree and build the installer:

```powershell
.\scripts\export-github-source.ps1
.\scripts\build-aether-installer.ps1
```

The installer build script installs WiX as a repository-local .NET tool on first use. It does not
require a global WiX installation.

## Licensing

See `LICENSE.md`. Aether retains the licensing and attribution obligations of its Ruffle base and
its included dependencies.
