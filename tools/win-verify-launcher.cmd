@echo off
REM crosslink Windows verify launcher (batch) - single-machine smoke test.
REM Drives two capture/inject pairs in sequence with no input feedback loop.
REM Drives launches via: start /B "" cmd /c "<binary> ... > log 2>&1"
REM   (because `start` alone's stdout redirect does NOT propagate to the child process;
REM    wrapping in cmd /c ensures the shell redirection actually applies.)
REM Usage: launched from PowerShell as:
REM     cmd.exe /c "\"<launcher_path>\" <bindir> <port1> <port2>"

setlocal EnableExtensions
set BINDIR=%~1
set PORT1=%~2
set PORT2=%~3

if "%BINDIR%"=="" goto :usage
if "%PORT1%"=="" goto :usage
if "%PORT2%"=="" goto :usage

REM --- Pre-clean ---
taskkill /F /IM crosslink.exe >nul 2>&1
del /Q srv_inject.log cli_inject.log srv_cap.log cli_cap.log 2>nul

REM --- Test 1: injection auth ---
echo === Test1 server ===
start /B "" cmd /c "\"%BINDIR%\crosslink.exe\" --server --no-capture --test-input --port %PORT1% --name win-srv > \"%~dp0srv_inject.log\" 2>&1"
timeout /t 2 /nobreak >nul
echo === Test1 client ===
start /B "" cmd /c "\"%BINDIR%\crosslink.exe\" --client 127.0.0.1 --port %PORT1% --name win-cli > \"%~dp0cli_inject.log\" 2>&1"
timeout /t 6 /nobreak >nul
taskkill /F /IM crosslink.exe >nul 2>&1
timeout /t 1 /nobreak >nul

REM --- Test 2: capture auth ---
echo === Test2 server ===
start /B "" cmd /c "\"%BINDIR%\crosslink.exe\" --server --port %PORT2% --name win-srv > \"%~dp0srv_cap.log\" 2>&1"
timeout /t 2 /nobreak >nul
echo === Test2 client ===
start /B "" cmd /c "\"%BINDIR%\crosslink.exe\" --client 127.0.0.1 --port %PORT2% --name win-cli --no-inject > \"%~dp0cli_cap.log\" 2>&1"
timeout /t 6 /nobreak >nul
taskkill /F /IM crosslink.exe >nul 2>&1
timeout /t 1 /nobreak >nul

echo === launcher done ===
exit /b 0

:usage
echo Usage: %~nx0 ^<bindir^> ^<port1^> ^<port2^>
exit /b 1
