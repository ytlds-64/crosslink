# crosslink Windows verification script (PS 5.1-friendly).
# All heavy lifting is delegated to tools\win-verify-launcher.cmd (a .bat file).
# This script only: finds the binary, runs the launcher, inspects the logs, prints verdict.

$ErrorActionPreference = "Continue"

# Locate binary (prefer gnu target, then default msvc)
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
$launcher = Join-Path (Get-Location) "tools\win-verify-launcher.cmd"
if (-not (Test-Path $launcher)) {
    Write-Error "[ERR] launcher not found: $launcher"
    exit 1
}

# Cleanup leftovers before run
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
foreach ($f in @("srv_inject.log","cli_inject.log","srv_cap.log","cli_cap.log")) {
    Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue
}

# Run the .bat launcher (does all process orchestration).
# IMPORTANT: do NOT pass Arguments as an array - PowerShell 5.1 + Start-Process
# has quoting bugs with paths containing spaces. Build ONE cmd /c string instead,
# which lets cmd itself handle the parsing.
Write-Host ""
Write-Host "===== Running launcher (server + client pairs) ====="
$port1 = Get-Random -Minimum 4244 -Maximum 9000
$port2 = $port1 + 1
$cmdLine = '/c ""' + $launcher + '" "' + $bindir + '" ' + $port1 + ' ' + $port2 + '"'
$proc = Start-Process -FilePath "cmd.exe" -ArgumentList $cmdLine -NoNewWindow -Wait -PassThru
Write-Host "Launcher exited with code: $($proc.ExitCode)"

Start-Sleep -Seconds 1

# Inspect logs
function Read-Log {
    param([string]$Path)
    if (Test-Path $Path) {
        try { return (Get-Content -Path $Path -Raw -ErrorAction Stop) }
        catch { return "" }
    } else {
        return ""
    }
}

Write-Host ""
Write-Host "===== Test 1: Injection auth ====="
$srv1log = Read-Log "srv_inject.log"
$cli1log = Read-Log "cli_inject.log"
if ([string]::IsNullOrEmpty($cli1log)) {
    Write-Host "[FAIL] Test1: client log cli_inject.log MISSING (client process did not start)"
    $injection = "FAIL"
} elseif ($cli1log -match 'inject failed') {
    Write-Host "[FAIL] Test1: 'inject failed' found in cli_inject.log (run script as Administrator)"
    $injection = "FAIL"
} else {
    Write-Host "[OK]   Test1: client log created, no 'inject failed' found"
    $injection = "OK"
}

Write-Host ""
Write-Host "===== Test 2: Capture auth ====="
$srv2log = Read-Log "srv_cap.log"
if ([string]::IsNullOrEmpty($srv2log)) {
    Write-Host "[FAIL] Test2: server log srv_cap.log MISSING (server process did not start)"
    $capture = "FAIL"
} elseif ($srv2log -match 'handshake complete') {
    Write-Host "[OK]   Test2: server started and 'handshake complete' seen"
    $capture = "OK"
} else {
    Write-Host "[WARN] Test2: server started but 'handshake complete' not seen (check srv_cap.log)"
    $capture = "WARN"
}

Write-Host ""
Write-Host "===== Summary ====="
Write-Host "injection: $injection"
Write-Host "capture:   $capture"
Write-Host ""
Write-Host "Logs (working dir): srv_inject.log, cli_inject.log, srv_cap.log, cli_cap.log"
Write-Host "Real cross-machine E2E still requires TWO machines + firewall rule:"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=$port1"
