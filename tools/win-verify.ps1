# crosslink Windows verification script (PS 5.1-friendly).
# Pure PowerShell — no .bat / .cmd subprocesses.
# Avoids PS 5.1 same-file dual-redirect ban by spawning each crosslink via
# `cmd.exe /c "... > log 2>&1"` — the redirect happens inside cmd, not on Start-Process.
# Run as Administrator if you want SendInput to actually land on UAC-elevated windows.

$ErrorActionPreference = "Continue"

# ---- Find binary ----
$bin = $null
$candidates = @(
    "target\x86_64-pc-windows-gnu\release\crosslink.exe",
    "target\release\crosslink.exe"
)
foreach ($c in $candidates) {
    if (Test-Path $c) { $bin = (Resolve-Path $c).Path; break }
}
if (-not $bin) {
    Write-Error "[ERR] crosslink.exe not found. Build first (cargo build --release)."
    exit 1
}
Write-Host "Using binary: $bin"
$bindir = Split-Path -Parent $bin

# ---- Pre-clean ----
foreach ($f in @(".\srv_inject.log",".\cli_inject.log",".\srv_cap.log",".\cli_cap.log")) {
    if (Test-Path $f) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }
}
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# ---- Pick two free-ish ports ----
$port1 = Get-Random -Minimum 4244 -Maximum 8999
$port2 = $port1 + 1

# Helper: launch crosslink via cmd /c with stdout+stderr merged into $LogPath
# This works around PS 5.1's ban on same-file dual-redirect for Start-Process.
function Start-Xlink {
    param([string]$Args, [string]$LogPath)
    if (Test-Path $LogPath) { Remove-Item -LiteralPath $LogPath -Force }
    $cmdLine = '/c ""' + $bin + '" ' + $Args + ' > "' + $LogPath + '" 2>&1"'
    Start-Process -FilePath cmd.exe -ArgumentList $cmdLine -NoNewWindow
}

# ============================================================
Write-Host ""
Write-Host "===== Test 1: Injection auth (server test-input + client) ====="
Start-Xlink "--server --no-capture --test-input --port $port1 --name win-srv" ".\srv_inject.log"
Start-Sleep -Seconds 3
Start-Xlink "--client 127.0.0.1 --port $port1 --name win-cli" ".\cli_inject.log"
Start-Sleep -Seconds 6
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

Write-Host ""
Write-Host "===== Test 2: Capture auth (server captures + client --no-inject) ====="
Start-Xlink "--server --port $port2 --name win-srv" ".\srv_cap.log"
Start-Sleep -Seconds 3
Start-Xlink "--client 127.0.0.1 --port $port2 --name win-cli --no-inject" ".\cli_cap.log"
Start-Sleep -Seconds 6
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# ---- Inspect logs ----
function Read-Log([string]$Path) {
    if (Test-Path $Path) {
        try { return (Get-Content -Path $Path -Raw -ErrorAction Stop) }
        catch { return "" }
    } else { return "" }
}

Write-Host ""
Write-Host "===== Verdict ====="
$cli1log = Read-Log ".\cli_inject.log"
if ([string]::IsNullOrEmpty($cli1log)) {
    Write-Host "[FAIL] injection: cli_inject.log MISSING (client failed to start)"
    $injection = "FAIL"
} elseif ($cli1log -match 'inject failed') {
    Write-Host "[FAIL] injection: 'inject failed' found (re-run as Administrator)"
    $injection = "FAIL"
} else {
    Write-Host "[OK]   injection: client log captured, no 'inject failed' marker"
    $injection = "OK"
}

$srv2log = Read-Log ".\srv_cap.log"
if ([string]::IsNullOrEmpty($srv2log)) {
    Write-Host "[FAIL] capture: srv_cap.log MISSING (server failed to start)"
    $capture = "FAIL"
} elseif ($srv2log -match 'handshake complete') {
    Write-Host "[OK]   capture: server captured 'handshake complete'"
    $capture = "OK"
} else {
    Write-Host "[WARN] capture: server log present but 'handshake complete' not seen"
    $capture = "WARN"
}

Write-Host ""
Write-Host "===== Summary ====="
Write-Host "injection: $injection"
Write-Host "capture:   $capture"
Write-Host ""
Write-Host "Logs (working dir): srv_inject.log, cli_inject.log, srv_cap.log, cli_cap.log"
Write-Host "Real cross-machine E2E requires TWO machines + this firewall rule on server:"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=$port1"
