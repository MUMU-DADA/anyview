@echo off
rem Anyview for Windows: starts the app built by the test script (run that first) for
rem manual testing. Drop an image or folder on this file to open it. Settings and
rem temporary files are kept in the anyview-build folder under TEMP (manual), so nothing
rem is written to your user profile; the cleanup script deletes it with the build.
rem Every window drag is measured and written to reports\windows\drags in the project folder
rem (how long each step took, what the UI thread was busy with), for finding the drag stutter.
rem (This file is ASCII only: cmd.exe reads batch files in the console code page.)
setlocal
for %%I in ("%~dp0..\..\reports\windows\drags") do set "DRAGLOG=%%~fI"
set "BASE=%TEMP%\anyview-build"
set "EXE=%BASE%\app\Anyview.exe"
if not exist "%EXE%" (
    echo Anyview.exe was not found in %BASE%\app. Run the test script first.
    pause
    exit /b 1
)
if not exist "%BASE%\manual\temp" mkdir "%BASE%\manual\temp"
set "TEMP=%BASE%\manual\temp"
set "TMP=%BASE%\manual\temp"
if "%~1"=="" (
    start "" "%EXE%" --settings "%BASE%\manual\settings.toml" --drag-log "%DRAGLOG%"
) else (
    start "" "%EXE%" --settings "%BASE%\manual\settings.toml" --drag-log "%DRAGLOG%" "%~1"
)
