#!/bin/bash
# crosslink macOS .app 打包脚本（macOS Tahoe 上稳定获取 TCC 授权必需）
# 用法：zsh tools/mac-bundle.sh
set -euo pipefail

APP_NAME=crosslink
BUNDLE_ID=com.crosslink.app
SRC=target/release/crosslink
OUT=dist/$APP_NAME.app

if [ ! -x "$SRC" ]; then
  echo "❌ 未找到 release 二进制: $SRC"
  echo "   请先运行: cargo build --release"
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "$SRC" "$OUT/Contents/MacOS/$APP_NAME"

cat > "$OUT/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>CrossLink</string>
    <key>CFBundleDisplayName</key>
    <string>CrossLink</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# ad-hoc 签名（无开发者证书也能让 TCC 稳定授权；失败也不致命）
codesign --force --deep --sign - "$OUT" 2>/dev/null || echo "⚠️ codesign 失败（可跳过，但 TCC 授权可能不稳定）"

echo "✅ 已生成: $OUT"
echo "   下一步：把 $OUT 本体加入 系统设置 → 隐私与安全性 → 辅助功能 / 输入监控 并勾选"
