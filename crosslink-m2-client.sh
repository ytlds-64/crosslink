#!/bin/zsh
# CrossLink M2 双机验证 - macOS 客户端启动器（Win 主控 → Mac 被控）
# 复用已授权的 .app bundle（输入监控），无需重新配置 TCC。
#
# 用法：
#   zsh crosslink-m2-client.sh <Windows_IP> [--fingerprint <FP>]
#
# 示例：
#   zsh crosslink-m2-client.sh 192.168.1.50
#   zsh crosslink-m2-client.sh 192.168.1.50 --fingerprint AB:CD:EF:...
#
# 说明：M2 是默认转发模式（不加 --switch）。Mac 作为 client 只做注入，
#       收到 Windows server 转发的键鼠事件后用 CGEventPost 注入本机，
#       实现「Win 鼠标/键盘持续控制 Mac」。Mac 自身触控板仍本地可用。

set -u

BIN=/Users/xiaohua/Applications/crosslink.app/Contents/MacOS/crosslink

if [ $# -lt 1 ]; then
  echo "用法: zsh $0 <Windows_IP> [--fingerprint <FP>]"
  exit 1
fi

WIN_IP=$1
shift

# 清掉可能残留的旧进程，避免抢真实光标 / 端口占用
pkill -f 'crosslink' 2>/dev/null
sleep 1

if [ ! -x "$BIN" ]; then
  echo "❌ 找不到 .app 内二进制: $BIN"
  echo "   请先 cargo build --release 并把二进制 cp 进 .app/Contents/MacOS/crosslink"
  exit 1
fi

echo "▶ 启动 Mac client（M2 默认转发模式），连接 Windows server @ $WIN_IP"
echo "  （首次连接无需 --fingerprint，TOFU 自动信任；Mac 需已授权「输入监控」）"
"$BIN" --client "$WIN_IP" --name mac-cli "$@"
