@echo off
REM Register the FocusPlay native messaging host for Chrome
REM Run this script as administrator or for current user

set HOST_NAME=com.focusplay.host
set MANIFEST_PATH=%~dp0com.focusplay.host.json

REM Register for current user (HKCU)
reg add "HKCU\Software\Google\Chrome\NativeMessagingHosts\%HOST_NAME%" /ve /t REG_SZ /d "%MANIFEST_PATH%" /f

echo.
echo Native messaging host registered: %HOST_NAME%
echo Manifest: %MANIFEST_PATH%
echo.
echo IMPORTANT: Update the extension ID in com.focusplay.host.json
echo after loading the extension in Chrome.
pause
