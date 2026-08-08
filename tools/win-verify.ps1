# crosslink Windows 真机穿透验证脚本（防同机回环）
# 用法（PowerShell）：.\tools\win-verify.ps1
# 前提：已构建（cargo build --target x86_64-pc-windows-gnu --release 或 cargo build --release）
# 兼容性：Windows PowerShell 5.1（不允许同一文件同时做 stdout/stderr 重定向，本脚本用 cmd /c 合并）
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

# 清理残留进程和上次日志
Get-Process crosslink -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
foreach ($f in @("srv_inject.log","cli_inject.log","srv_cap.log","cli_cap.log")) {
    Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 1

# 启动辅助：Windows PowerShell 5.1 不允许 RedirectStandardOutput/Error 指向同一文件，
# 这里用 cmd /c + shell 重定向 (2>&1) 让一个文件同时收两份流。
function Start-Crosslink {
    param([string[]]$Args, [string]$LogFile)
    # 把参数拼成单一命令行，外层包 cmd /c 让 cmd 自身解析 > 和 2>&1
    $cmdLine = '/c "' + $bin + '" ' + ($Args -join ' ') + ' > "' + $LogFile + '" 2>&1'
    Start-Process -FilePath cmd.exe -ArgumentList $cmdLine -WindowStyle Hidden
}

Write-Host ""
Write-Host "########## 测试1：注入授权（server --no-capture --test-input + client 注入）##########"
Write-Host "（client 会向本机注入 a/b/1/Tab/Enter 等模拟按键，约 1 秒，属正常验证）"
Start-Crosslink -Args @("--server","--no-capture","--test-input","--port","4242","--name","win-srv") -LogFile "srv_inject.log"
Start-Sleep -Seconds 2
Start-Crosslink -Args @("--client","127.0.0.1","--port","4242","--name","win-cli") -LogFile "cli_inject.log"
Start-Sleep -Seconds 4
Get-Process crosslink, cmd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) {
    Write-Host ">>> 注入授权：❌ 发现 inject failed（可能需要以管理员身份运行 / SendInput 被拦截）"
} else {
    Write-Host ">>> 注入授权：✅ 管线通畅（SendInput 注入 worker 正常）"
}

Write-Host ""
Write-Host "########## 测试2：捕获授权（server 捕获 + client --no-inject，安全不回环）##########"
Start-Crosslink -Args @("--server","--port","4243","--name","win-srv") -LogFile "srv_cap.log"
Start-Sleep -Seconds 1
Start-Crosslink -Args @("--client","127.0.0.1","--port","4243","--name","win-cli","--no-inject") -LogFile "cli_cap.log"
Start-Sleep -Seconds 4
Get-Process crosslink, cmd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Select-String -Path "srv_cap.log" -Pattern "handshake complete" -Quiet) {
    Write-Host ">>> 捕获授权：✅ server 启动 + 握手完成（详见 srv_cap.log）"
} else {
    Write-Host ">>> 捕获授权：⚠️ 握手未完成，查看 srv_cap.log"
}

Write-Host ""
Write-Host "########## 判定汇总 ##########"
if (Select-String -Path "cli_inject.log" -Pattern "inject failed" -Quiet) { Write-Host "注入: ❌" } else { Write-Host "注入: ✅" }
if (Select-String -Path "srv_cap.log" -Pattern "handshake complete" -Quiet) { Write-Host "捕获: ✅" } else { Write-Host "捕获: ⚠️" }
Write-Host ""
Write-Host "⚠️ 真正的端到端联动（捕获→注入）必须在两台不同机器上测试，同机必回环。"
Write-Host "⚠️ Windows 防火墙需放行 TCP 4242（server 那台执行，需管理员）："
Write-Host "    netsh advfirewall firewall add rule name=crosslink dir=in action=allow protocol=TCP localport=4242"