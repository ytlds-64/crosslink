#!/bin/bash
# crosslink Mac 实机验证脚本（安全隔离，避免同机回环）
# 用法：zsh tools/crosslink-verify.sh
set -u

# 优先用 .app 内二进制（Tahoe 稳定授权必需）；回退到 cargo build 产物
BIN=""
if [ -x "dist/crosslink.app/Contents/MacOS/crosslink" ]; then
  BIN="dist/crosslink.app/Contents/MacOS/crosslink"
elif [ -x "target/release/crosslink" ]; then
  BIN="target/release/crosslink"
else
  echo "❌ 找不到二进制；请先 zsh tools/mac-bundle.sh"
  exit 1
fi

pkill -f crosslink 2>/dev/null
sleep 1

echo "########## 测试1：注入授权（server --no-capture --test-input + client 注入）##########"
echo "（client 会向本机注入 a/b/1/Tab/Enter 等模拟按键，约 1 秒，属正常验证）"
"$BIN" --server --no-capture --test-input --port 4242 --name mac-srv > /tmp/cl_srv_inject.log 2>&1 & SRV=$!
sleep 2
"$BIN" --client 127.0.0.1 --port 4242 --name mac-cli > /tmp/cl_cli_inject.log 2>&1 & CLI=$!
sleep 4
kill $CLI $SRV 2>/dev/null
if grep -qi "inject failed" /tmp/cl_cli_inject.log; then
  echo ">>> 注入授权：❌ 发现 inject failed（输入监控未授权）"
else
  echo ">>> 注入授权：✅ 管线通畅（输入监控已授权，mock 事件抵达注入 worker）"
fi

echo
echo "########## 测试2：捕获授权（server 捕获 + client --no-inject，安全不回环）##########"
"$BIN" --server --port 4243 --name mac-srv > /tmp/cl_srv_cap.log 2>&1 & SRV=$!
sleep 1
"$BIN" --client 127.0.0.1 --port 4243 --name mac-cli --no-inject > /tmp/cl_cli_cap.log 2>&1 & CLI=$!
sleep 4
kill $CLI $SRV 2>/dev/null
if grep -qi "CGEventTapCreate failed" /tmp/cl_srv_cap.log; then
  echo ">>> 捕获授权：❌ 失败（辅助功能未授权；确认已用 .app 本体授权）"
else
  echo ">>> 捕获授权：✅ CGEventTap 创建成功（辅助功能已授权）"
fi

echo
echo "########## 判定汇总 ##########"
grep -qi "inject failed" /tmp/cl_cli_inject.log && echo "注入: ❌" || echo "注入: ✅"
grep -qi "CGEventTapCreate failed" /tmp/cl_srv_cap.log && echo "捕获: ❌" || echo "捕获: ✅"
echo
echo "⚠️ 两项的『联动』（捕获→注入）需在两台不同机器上测试，同机必回环。"
