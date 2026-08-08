@echo off
REM crosslink Windows verify launcher (batch) - single-machine smoke test.
REM Drives two capture/inject pairs in sequence with no input feedback loop.
REM Usage: invoke this from PowerShell:
REM     cmd /c "C:\...\cl_launcher.cmd <bindir> <port1> <port2>"
REM All stdout+stderr go to merged log files in the working directory.

setlocal
set BINDIR=%~1
set PORT1=%~2
set PORT2=%~3

REM --- Pre-clean ---
taskkill /F /IM crosslink.exe >nul 2>&1
del /Q srv_inject.log cli_inject.log srv_cap.log cli_cap.log 2>nul

REM --- Test 1: injection auth (server --no-capture --test-input + client injects) ---
echo === Test1 server ===
start "" /B /D "%BINDIR%" crosslink.exe --server --no-capture --test-input --port %PORT1% --name win-srv > "%~dp0srv_inject.log" 2>&1
timeout /t 2 /nobreak >nul
echo === Test1 client ===
start "" /B /D "%BINDIR%" crosslink.exe --client 127.0.0.1 --port %PORT1% --name win-cli > "%~dp0cli_inject.log" 2>&1
timeout /t 5 /nobreak >nul
taskkill /F /IM crosslink.exe >nul 2>&1
timeout /t 1 /nobreak >nul

REM --- Test 2: capture auth (server captures + client --no-inject) ---
echo === Test2 server ===
start "" /B /D "%BINDIR%" crosslink.exe --server --port %PORT2% --name win-srv > "%~dp0srv_cap.log" 2>&1
timeout /t 2 /nobreak >nul
echo === Test2 client ===
start "" /B /D "%BINDIR%" crosslink.exe --client 127.0.0.1 --port %PORT2% --name win-cli --no-inject > "%~dp0cli_cap.log" 2>&1
timeout /t 5 /nobreak >nul
taskkill /F /IM crosslink.exe >nul 2>&1
timeout /t 1 /nobreak >nul

echo === launcher done ===
endlocal
