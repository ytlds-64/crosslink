#!/bin/zsh
# CrossLink M3 双机验证 - macOS 客户端启动器
# 复用已授权的 .app bundle（辅助功能），无需重新配置 TCC。
#
# 用法：
#   zsh crosslink-m3-client.sh <Windows_IP> [--fingerprint <FP>]
#
# 示例：
#   zsh crosslink-m3-client.sh 192.168.1.50
#   zsh crosslink-m3-client.sh 192.168.1.50 --fingerprint AB:CD:EF:...
#
# 布局：默认 Mac 在 Windows 右侧（client --side left）。
#       若 Mac 实际在 Windows 左侧，在末尾加：--side right

set -u

BIN=/Users/xiaohua/Applications/crosslink.app/Contents/MacOS/crosslink

if [ $# -lt 1 ]; then
  echo "用法: zsh $0 <Windows_IP> [--fingerprint <FP>] [--side left|right|top|bottom]"
  exit 1
fi

WIN_IP=$1
shift

# 清掉可能残留的旧进程，避免抢真实光标
pkill -f 'crosslink' 2>/dev/null
sleep 1

if [ ! -x "$BIN" ]; then
  echo "❌ 找不到 .app 内二进制: $BIN"
  echo "   请先 cargo build --release 并把二进制 cp 进 .app/Contents/MacOS/crosslink"
  exit 1
fi

echo "▶ 启动 Mac client（--switch），连接 Windows server @ $WIN_IP"
echo "  （首次连接无需 --fingerprint，TOFU 自动信任）"
"$BIN" --client "$WIN_IP" --switch --name mac-cli "$@"
