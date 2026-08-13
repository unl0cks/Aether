@echo off
rem Commit, tag, push and publish a GitHub release for whatever version aether\Cargo.toml says.
rem
rem Build first with installer\build-setup.cmd. This script publishes what is already in
rem installer\dist and refuses if the artifacts for that version are missing.
rem
rem Usage:
rem   publish-release.cmd                          uses release-notes.md next to this file
rem   publish-release.cmd notes\0.5.11.md          uses a specific notes file
rem   publish-release.cmd release-notes.md -DryRun shows what would happen, changes nothing
setlocal

where pwsh >nul 2>&1
if errorlevel 1 (
    echo PowerShell 7 ^(pwsh^) was not found. Install it with: winget install --id Microsoft.PowerShell
    pause
    exit /b 1
)

where gh >nul 2>&1
if errorlevel 1 (
    echo The GitHub CLI ^(gh^) was not found. Install it with: winget install --id GitHub.cli
    pause
    exit /b 1
)

rem The first argument is the notes file ONLY if it is not a switch. Without this test,
rem `publish-release.cmd -Force` looked for release notes named "-Force" and reported them
rem missing, which says nothing about what actually went wrong.
set "NOTES="
set "FIRST=%~1"
if not "%FIRST%"=="" if not "%FIRST:~0,1%"=="-" if not "%FIRST:~0,1%"=="/" (
    set "NOTES=%FIRST%"
    shift
)
if "%NOTES%"=="" set "NOTES=%~dp0release-notes.md"
if not exist "%NOTES%" (
    echo Release notes not found: %NOTES%
    echo Write them first, or pass the path as the first argument.
    pause
    exit /b 1
)

rem Everything left over is passed through as switches.
set "EXTRA="
:collect
if "%~1"=="" goto run
set "EXTRA=%EXTRA% %1"
shift
goto collect

:run
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\publish-release-fixed.ps1" -NotesFile "%NOTES%"%EXTRA%
if errorlevel 1 pause
endlocal
