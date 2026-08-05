@echo off
rem Convenience wrapper so the builder can be double-clicked. All arguments pass straight through.
setlocal
where pwsh >nul 2>&1
if errorlevel 1 (
    echo PowerShell 7 ^(pwsh^) was not found. Install it with: winget install --id Microsoft.PowerShell
    pause
    exit /b 1
)
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-setup.ps1" %*
if errorlevel 1 pause
endlocal
