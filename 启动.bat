@echo off
setlocal
set "SRC=%~dp0tuneport.exe"
if not exist "%SRC%" set "SRC=%~dp0src-tauri\target\release\tuneport.exe"
set "DST=%LOCALAPPDATA%\TunePort-portable"

if not exist "%SRC%" goto nosrc
if not exist "%DST%" mkdir "%DST%" >nul 2>&1
copy /y "%SRC%" "%DST%\tuneport.exe" >nul 2>&1
if errorlevel 1 goto nocopy
start "" "%DST%\tuneport.exe"
exit /b 0

:nosrc
echo [ERROR] tuneport.exe not found next to this script.
pause
exit /b 1

:nocopy
echo [ERROR] cannot copy to %DST%
pause
exit /b 1
