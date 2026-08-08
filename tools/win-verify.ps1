# crosslink Windows end-to-end verification script (single-machine, no feedback loop)
# Usage (PowerShell): .\tools\win-verify.ps1
# Prereq: built binary at target\release\crosslink.exe (or target\x86_64-pc-windows-gnu\release\)
# Compatibility: Windows PowerShell 5.1
#   - PS 5.1 forbids RedirectStandardOutput+RedirectStandardError pointing to same file.
#   - PS 5.1 cmd /c argument quoting with embedded ">" can silently fail to start the binary.
#   Fix: write a temp .cmd file that runs the binary with shell redirection, then invoke it.
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

# Launcher: write a temp .cmd that runs the binary with stdout+stderr merged into one file.
# Avoids both PS 5.1 cmd /c quoting quirks and the same-file dual-redirect restriction.
function Start-Crosslink {
    param([string[]]$Args, [string]$LogFile)
    $binPath = (Resolve-Path $bin).Path
    $tmpBat = [System.IO.Path]::Combine($env:TEMP, "crosslink_$([System.IO.Path]::GetRandomFileName()).cmd")
    $line = '@echo off' + [Environment]::NewLine
    $line += '"' + $binPath + '" ' + ($Args -join ' ') + ' > "' + $LogFile + '" 2>&1' + [Environment]::NewLine
    [System.IO.File]::WriteAllText($tmpBat, $line, [System.Text.Encoding]::ASCII)
    Start-Process -FilePath $tmpBat -WindowStyle Hidden | Out-Null
}

function Cleanup-Crosslink {
    Get-Process crosslink, cmd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Get-ChildItem -Path $env:TEMP -Filter "crosslink_*.cmd" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}

function Assert-Log {
    param([string]$Path, [string]$Label)
    if (Test-Path $Path) {
        Write-Host "[OK]   $Label log created: $Path"
        return $true
    } else {
        Write-Host "[FAIL] $Label log MISSING: $Path (process likely failed to start)"
        return $false
    }
}

Write-Host ""
Write-Host "===== Test 1: Injection auth (server --no-capture --test-input + client injects) ====="
Write-Host "(client will inject simulated keys a/b/1/Tab/Enter to localhost; ~1s, expected)"
Start-Crosslink -Args @("--server","--no-capture","--test-input","--port","4242","--name","win-srv") -LogFile "srv_inject.log"
Start-Sleep -Seconds 2
Start-Crosslink -Args @("--client","127.0.0.1","--port","4242","--name","win-cli") -LogFile "cli_inject.log"
Start-Sleep -Seconds 4
Cleanup-Crosslink
Start-Sleep -Seconds 1
$srv1 = Assert-Log "srv_inject.log" "Test1-server"
$cli1 = Assert-Log "cli_inject.log" "Test1-client"
if ($cli1 -and (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet)) {
    Write-Host "[FAIL] injection: 'inject failed' found (run as Administrator, or SendInput blocked)"
    $injection = "FAIL"
} elseif ($cli1) {
    Write-Host "[OK]   injection: pipeline clear (SendInput worker functioning)"
    $injection = "OK"
} else {
    $injection = "UNKNOWN"
}

Write-Host ""
Write-Host "===== Test 2: Capture auth (server captures + client --no-inject, safe no-loop) ====="
Start-Crosslink -Args @("--server","--port","4243","--name","win-srv") -LogFile "srv_cap.log"
Start-Sleep -Seconds 1
Start-Crosslink -Args @("--client","127.0.0.1","--port","4243","--name","win-cli","--no-inject") -LogFile "cli_cap.log"
Start-Sleep -Seconds 4
Cleanup-Crosslink
Start-Sleep -Seconds 1
$srv2 = Assert-Log "srv_cap.log" "Test2-server"
$cli2 = Assert-Log "cli_cap.log" "Test2-client"
if ($srv2 -and (Select-String -Path "srv_cap.log" -Pattern "handshake complete" -Quiet)) {
    Write-Host "[OK]   capture: server started and handshake complete (see srv_cap.log)"
    $capture = "OK"
} elseif ($srv2) {
    Write-Host "[WARN] capture: handshake not seen, check srv_cap.log"
    $capture = "WARN"
} else {
    $capture = "UNKNOWN"
}

Write-Host ""
Write-Host "===== Summary ====="
Write-Host "injection: $injection"
Write-Host "capture:   $capture"
Write-Host ""
Write-Host "NOTE: real end-to-end (capture->inject across machines) requires TWO machines."
Write-Host "NOTE: server side needs firewall rule (run as admin):"
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=4242"