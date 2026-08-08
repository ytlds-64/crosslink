# crosslink Windows verification script (PS 5.1-friendly, Start-Process variant).
# Uses Start-Process -ArgumentList <array> directly. Each ArgArray element
# becomes one argv entry to crosslink.exe - no shell-style repassing, no
# embedded-quote arg-eating.
# Run as Administrator if you want SendInput to land on UAC-elevated windows.

$ErrorActionPreference = "Continue"

# ---- Force UTF-8 console so env_logger writes readable bytes ----
chcp 65001 > $null
$OutputEncoding = [System.Text.Encoding]::UTF8

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
    Write-Error "[ERR] crosslink.exe not found. Build first (cargo build --release) and cd into the crosslink repo root before running this script."
    exit 1
}
Write-Host "Using binary: $bin"

# ---- Pre-clean ----
$cleanFiles = @(
    ".\srv_inject.log",".\cli_inject.log",".\srv_cap.log",".\cli_cap.log",
    ".\srv_inject.log.out",".\srv_inject.log.err",
    ".\cli_inject.log.out",".\cli_inject.log.err",
    ".\srv_cap.log.out",".\srv_cap.log.err",
    ".\cli_cap.log.out",".\cli_cap.log.err"
)
foreach ($f in $cleanFiles) {
    if (Test-Path $f) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }
}
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# ---- Helper: launch crosslink via Start-Process -ArgumentList <array> ----
function Start-Xlink {
    param([string[]]$ArgArray, [string]$LogPath)

    Write-Host ("  argv: " + ($ArgArray -join ' '))

    $proc = Start-Process `
        -FilePath $bin `
        -ArgumentList $ArgArray `
        -NoNewWindow `
        -PassThru `
        -RedirectStandardOutput ("$LogPath.out") `
        -RedirectStandardError  ("$LogPath.err")

    return @{ Proc = $proc; LogPath = $LogPath }
}

# ---- Helper: stop process, merge .out/.err into LogPath ----
function Stop-Xlink {
    param($Runner)
    Start-Sleep -Milliseconds 500
    if (-not $Runner.Proc.HasExited) {
        try { $Runner.Proc.Kill() } catch {}
    }
    $Runner.Proc.WaitForExit(3000) | Out-Null

    $stdout = ""
    $stderr = ""
    if (Test-Path "$($Runner.LogPath).out") { $stdout = (Get-Content "$($Runner.LogPath).out" -Raw -ErrorAction SilentlyContinue) }
    if (Test-Path "$($Runner.LogPath).err") { $stderr = (Get-Content "$($Runner.LogPath).err" -Raw -ErrorAction SilentlyContinue) }

    [System.IO.File]::WriteAllText($Runner.LogPath, ($stdout + "`n" + $stderr), [System.Text.Encoding]::UTF8)

    Remove-Item "$($Runner.LogPath).out","$($Runner.LogPath).err" -Force -ErrorAction SilentlyContinue
}

# ---- Pick two random ports ----
$port1 = Get-Random -Minimum 4244 -Maximum 8999
$port2 = $port1 + 1

# ============================================================
Write-Host ""
Write-Host "===== Test 1: Injection auth (server test-input + client) ====="
$T1Srv = Start-Xlink @('--server','--no-capture','--test-input','--port',"$port1",'--name','win-srv') ".\srv_inject.log"
Start-Sleep -Seconds 3
$T1Cli = Start-Xlink @('--client','127.0.0.1','--port',"$port1",'--name','win-cli') ".\cli_inject.log"
Start-Sleep -Seconds 6
Stop-Xlink $T1Srv
Stop-Xlink $T1Cli

Write-Host ""
Write-Host "===== Test 2: Capture auth (server captures + client --no-inject) ====="
$T2Srv = Start-Xlink @('--server','--port',"$port2",'--name','win-srv') ".\srv_cap.log"
Start-Sleep -Seconds 3
$T2Cli = Start-Xlink @('--client','127.0.0.1','--port',"$port2",'--name','win-cli','--no-inject') ".\cli_cap.log"
Start-Sleep -Seconds 6
Stop-Xlink $T2Srv
Stop-Xlink $T2Cli

# ---- Test 3: M3 edge-switch wiring (loopback; no real cursor move needed) ----
$port3 = $port2 + 1
Write-Host ""
Write-Host "===== Test 3: Edge-switch wiring (--switch loopback) ====="
$T3Srv = Start-Xlink @('--server','--switch','--port',"$port3",'--name','srv-sw') ".\srv_sw.log"
Start-Sleep -Seconds 3
$T3Cli = Start-Xlink @('--client','127.0.0.1','--port',"$port3",'--switch','--name','cli-sw') ".\cli_sw.log"
Start-Sleep -Seconds 5
Stop-Xlink $T3Srv
Stop-Xlink $T3Cli

# ---- Inspect logs ----
function Read-Log {
    param([string]$Path)
    if (Test-Path $Path) {
        try { return [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8) }
        catch {
            try { return (Get-Content -Path $Path -Raw -ErrorAction Stop) } catch { return "" }
        }
    } else { return "" }
}

Write-Host ""
Write-Host "===== Verdict ====="
$cli1log = Read-Log ".\cli_inject.log"
if ([string]::IsNullOrWhiteSpace($cli1log)) {
    Write-Host "[FAIL] injection: cli_inject.log empty/missing"
    $injection = "FAIL"
} elseif ($cli1log -match '(?m)^\s*Usage:') {
    Write-Host "[FAIL] injection: client printed Usage only (args not delivered)"
    $injection = "FAIL"
} elseif ($cli1log -match 'inject failed') {
    Write-Host "[FAIL] injection: 'inject failed' found (re-run as Administrator)"
    $injection = "FAIL"
} else {
    Write-Host "[OK]   injection: client log captured, no 'inject failed' marker"
    $injection = "OK"
}

$srv2log = Read-Log ".\srv_cap.log"
if ([string]::IsNullOrWhiteSpace($srv2log)) {
    Write-Host "[FAIL] capture: srv_cap.log empty/missing"
    $capture = "FAIL"
} elseif ($srv2log -match '(?m)^\s*Usage:') {
    Write-Host "[FAIL] capture: server printed Usage only (args not delivered)"
    $capture = "FAIL"
} elseif ($srv2log -match 'handshake complete') {
    Write-Host "[OK]   capture: server captured 'handshake complete'"
    $capture = "OK"
} else {
    Write-Host "[WARN] capture: server log present but 'handshake complete' not seen"
    $capture = "WARN"
}

Write-Host ""
$srv3log = Read-Log ".\srv_sw.log"
$cli3log = Read-Log ".\cli_sw.log"
if ([string]::IsNullOrWhiteSpace($srv3log) -or [string]::IsNullOrWhiteSpace($cli3log)) {
    Write-Host "[FAIL] switch: srv_sw.log or cli_sw.log empty/missing"
    $switchRes = "FAIL"
} elseif (($srv3log -match '(?m)^\s*Usage:') -or ($cli3log -match '(?m)^\s*Usage:')) {
    Write-Host "[FAIL] switch: a side printed Usage only (args not delivered)"
    $switchRes = "FAIL"
} elseif (($srv3log -match 'switch monitor starting') -and ($cli3log -match 'switch monitor starting') -and ($srv3log -match 'peer screen geometry') -and ($cli3log -match 'peer screen geometry')) {
    Write-Host "[OK]   switch: both sides booted monitor + exchanged screen geometry"
    $switchRes = "OK"
} else {
    Write-Host "[WARN] switch: booted but monitor/geometry handshake not fully seen"
    $switchRes = "WARN"
}

Write-Host "===== Summary ====="
Write-Host "injection: $injection"
Write-Host "capture:   $capture"
Write-Host "switch:    $switchRes"
Write-Host ""
Write-Host "Logs (working dir): srv_inject.log, cli_inject.log, srv_cap.log, cli_cap.log, srv_sw.log, cli_sw.log"
Write-Host "Real cross-machine E2E (cursor handoff) requires TWO machines + this firewall rule on server:"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=$port1"
Write-Host "For --switch cursor roaming: run server with --switch on machine A, client with --switch on"
Write-Host "machine B; push the cursor to the shared edge to hand off. Tune --side if layout isn't left/right."
