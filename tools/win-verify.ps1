# crosslink Windows end-to-end verification script (single-machine, no feedback loop)
# Usage (PowerShell): .\tools\win-verify.ps1
# Prereq: built binary at target\release\crosslink.exe (or target\x86_64-pc-windows-gnu\release\)
# Compatibility: Windows PowerShell 5.1 (cannot redirect stdout+stderr to same file; uses cmd /c)
# Note: Write-Host strings are ASCII-only to avoid PS 5.1 code-page issues with UTF-8 no-BOM.
$ErrorActionPreference = "Stop"

$bin = $null
$candidates = @(
    "target\x86_64-pc-windows-gnu\release\crosslink.exe",
    "target\release\crosslink.exe"
)
foreach ($c in $candidates) {
    if (Test-Path $c) { $bin = $c; break }
}
if (-not $bin) {
    Write-Error "[ERR] crosslink.exe not found; build first (cargo build --release)"
    exit 1
}
Write-Host "Using binary: $bin"

# Cleanup leftover processes and previous logs
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
foreach ($f in @("srv_inject.log","cli_inject.log","srv_cap.log","cli_cap.log")) {
    Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 1

# Launcher helper: Windows PowerShell 5.1 forbids RedirectStandardOutput and
# RedirectStandardError pointing to the same file, so we shell out to cmd /c
# with classic redirection 2>&1 to merge both streams into one log.
function Start-Crosslink {
    param([string[]]$Args, [string]$LogFile)
    $cmdLine = '/c "' + $bin + '" ' + ($Args -join ' ') + ' > "' + $LogFile + '" 2>&1'
    Start-Process -FilePath cmd.exe -ArgumentList $cmdLine -WindowStyle Hidden
}

Write-Host ""
Write-Host "===== Test 1: Injection auth (server --no-capture --test-input + client injects) ====="
Write-Host "(client will inject simulated keys a/b/1/Tab/Enter to localhost; ~1s, expected)"
Start-Crosslink -Args @("--server","--no-capture","--test-input","--port","4242","--name","win-srv") -LogFile "srv_inject.log"
Start-Sleep -Seconds 2
Start-Crosslink -Args @("--client","127.0.0.1","--port","4242","--name","win-cli") -LogFile "cli_inject.log"
Start-Sleep -Seconds 4
Get-Process crosslink, cmd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) {
    Write-Host "[FAIL] injection: 'inject failed' found (run as Administrator, or SendInput blocked)"
} else {
    Write-Host "[OK] injection: pipeline clear (SendInput worker functioning)"
}

Write-Host ""
Write-Host "===== Test 2: Capture auth (server captures + client --no-inject, safe no-loop) ====="
Start-Crosslink -Args @("--server","--port","4243","--name","win-srv") -LogFile "srv_cap.log"
Start-Sleep -Seconds 1
Start-Crosslink -Args @("--client","127.0.0.1","--port","4243","--name","win-cli","--no-inject") -LogFile "cli_cap.log"
Start-Sleep -Seconds 4
Get-Process crosslink, cmd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Select-String -Path "srv_cap.log" -Pattern "handshake complete" -Quiet) {
    Write-Host "[OK] capture: server started and handshake complete (see srv_cap.log)"
} else {
    Write-Host "[WARN] capture: handshake not seen, check srv_cap.log"
}

Write-Host ""
Write-Host "===== Summary ====="
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) {
    Write-Host "injection: FAIL"
} else {
    Write-Host "injection: OK"
}
if (Select-String -Path "srv_cap.log" -Pattern "handshake complete" -Quiet) {
    Write-Host "capture: OK"
} else {
    Write-Host "capture: WARN"
}
Write-Host ""
Write-Host "NOTE: real end-to-end (capture->inject across machines) requires TWO machines."
Write-Host "NOTE: server side needs firewall rule (run as admin):"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=4242"