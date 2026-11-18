@echo off
rem Anyview: deletes what the Windows test script left on this PC, that is the
rem anyview-build folder under TEMP (cleanup.ps1 next to this file).
rem Test reports on the shared folder are kept.
rem (This file is ASCII only: cmd.exe reads batch files in the console code page.)
setlocal
set "SCRIPT=%~dp0cleanup.ps1"
where pwsh >nul 2>nul
if not errorlevel 1 (
    pwsh -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%"
) else (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%"
)
echo.
pause
