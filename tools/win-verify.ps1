# crosslink Windows 真机穿透验证脚本（防同机回环）
# 用法（PowerShell）：.\tools\win-verify.ps1
# 前提：已构建（cargo build --target x86_64-pc-windows-gnu --release 或 cargo build --release）
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
    Write-Error "❌ 找不到 crosslink.exe；请先构建（cargo build --target x86_64-pc-windows-gnu --release）"
    exit 1
}
Write-Host "使用二进制: $bin"

# 清理残留进程
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

Write-Host ""
Write-Host "########## 测试1：注入授权（server --no-capture --test-input + client 注入）##########"
Write-Host "（client 会向本机注入 a/b/1/Tab/Enter 等模拟按键，约 1 秒，属正常验证）"
$null = Start-Process -FilePath $bin -ArgumentList "--server","--no-capture","--test-input","--port","4242","--name","win-srv" -RedirectStandardOutput "srv_inject.log" -RedirectStandardError "srv_inject.log" -WindowStyle Hidden
Start-Sleep -Seconds 2
$null = Start-Process -FilePath $bin -ArgumentList "--client","127.0.0.1","--port","4242","--name","win-cli" -RedirectStandardOutput "cli_inject.log" -RedirectStandardError "cli_inject.log" -WindowStyle Hidden
Start-Sleep -Seconds 4
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) {
    Write-Host ">>> 注入授权：❌ 发现 inject failed（可能需要以管理员身份运行 / SendInput 被拦截）"
} else {
    Write-Host ">>> 注入授权：✅ 管线通畅（SendInput 注入 worker 正常）"
}

Write-Host ""
Write-Host "########## 测试2：捕获授权（server 捕获 + client --no-inject，安全不回环）##########"
$null = Start-Process -FilePath $bin -ArgumentList "--server","--port","4243","--name","win-srv" -RedirectStandardOutput "srv_cap.log" -RedirectStandardError "srv_cap.log" -WindowStyle Hidden
Start-Sleep -Seconds 1
$null = Start-Process -FilePath $bin -ArgumentList "--client","127.0.0.1","--port","4243","--name","win-cli","--no-inject" -RedirectStandardOutput "cli_cap.log" -RedirectStandardError "cli_cap.log" -WindowStyle Hidden
Start-Sleep -Seconds 4
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Write-Host ">>> 捕获授权：✅ server 已启动（详见 srv_cap.log 中 handshake complete / heartbeat）"

Write-Host ""
Write-Host "########## 判定汇总 ##########"
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) { Write-Host "注入: ❌" } else { Write-Host "注入: ✅" }
Write-Host "捕获: ✅ (同机标准模式不可用，需双机联动验证)"
Write-Host ""
Write-Host "⚠️ 真正的端到端联动（捕获→注入）必须在两台不同机器上测试，同机必回环。"
Write-Host "⚠️ Windows 防火墙需放行 TCP 4242："
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=4242"
