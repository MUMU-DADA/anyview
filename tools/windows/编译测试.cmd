@echo off
rem Anyview for Windows: builds the app on this PC and runs its self-tests and the cold
rem start measurement (test.ps1 next to this file). Nothing is installed: the .NET SDK,
rem the NuGet packages, the build and copies of the test images stay in the anyview-build
rem folder under TEMP; the cleanup script next to this file deletes that folder.
rem Results are written to reports\windows on the shared folder.
rem (This file is ASCII only: cmd.exe reads batch files in the console code page.)
setlocal
set "SCRIPT=%~dp0test.ps1"
where pwsh >nul 2>nul
if not errorlevel 1 (
    pwsh -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%"
) else (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%"
)
echo.
pause
