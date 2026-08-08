# crosslink Windows verification script (PS 5.1-friendly).
# Uses [System.Diagnostics.ProcessStartInfo] directly to launch crosslink.
# Avoids the Start-Process + cmd.exe /c + embedded-quotes param-eating bug
# that ate our --server/--client args (everything got stripped, only clap's
# Usage block landed in the log).
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
    Write-Error "[ERR] crosslink.exe not found. Build first (cargo build --release)."
    exit 1
}
Write-Host "Using binary: $bin"

# ---- Pre-clean ----
foreach ($f in @(".\srv_inject.log",".\cli_inject.log",".\srv_cap.log",".\cli_cap.log")) {
    if (Test-Path $f) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }
}
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# ---- Helper: launch crosslink via .NET ProcessStartInfo, async-capture streams ----
function Start-Xlink {
    param([string[]]$ArgArray, [string]$LogPath)
    if (Test-Path $LogPath) { Remove-Item -LiteralPath $LogPath -Force }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $bin
    $psi.Arguments = ($ArgArray | ForEach-Object {
            if ($_ -match '\s') { '"{0}"' -f $_ } else { $_ }
        }) -join ' '
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.WorkingDirectory = (Get-Location).Path

    $proc = [System.Diagnostics.Process]::Start($psi)

    # Async reads so the child never blocks on a full pipe buffer
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()

    return @{ Proc = $proc; StdoutTask = $stdoutTask; StderrTask = $stderrTask; LogPath = $LogPath }
}

# ---- Helper: stop process, write merged log ----
function Stop-Xlink {
    param($Runner)
    Start-Sleep -Milliseconds 500
    if (-not $Runner.Proc.HasExited) {
        try { $Runner.Proc.Kill() } catch {}
    }
    $Runner.Proc.WaitForExit(3000) | Out-Null
    $stdout = ""
    $stderr = ""
    try { $stdout = $Runner.StdoutTask.Result } catch {}
    try { $stderr = $Runner.StderrTask.Result } catch {}
    $content = $stdout + "`n" + $stderr
    [System.IO.File]::WriteAllText($Runner.LogPath, $content, [System.Text.Encoding]::UTF8)
}

# ---- Pick two random ports (avoid common defaults) ----
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
Write-Host "===== Summary ====="
Write-Host "injection: $injection"
Write-Host "capture:   $capture"
Write-Host ""
Write-Host "Logs (working dir): srv_inject.log, cli_inject.log, srv_cap.log, cli_cap.log"
Write-Host "Real cross-machine E2E requires TWO machines + this firewall rule on server:"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=$port1"
