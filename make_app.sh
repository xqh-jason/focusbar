#!/usr/bin/env bash
# 打出 FocusBar.app（不依赖 tauri-cli，直接手工组装 bundle）
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release

APP="target/FocusBar.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# 图标：png -> icns
ICNS="$APP/Contents/Resources/AppIcon.icns"
sips -s format icns icons/icon.png --out "$ICNS" >/dev/null

cp target/release/focusbar "$APP/Contents/MacOS/focusbar"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>zh_CN</string>
    <key>CFBundleExecutable</key><string>focusbar</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIdentifier</key><string>com.xqh.focusbar</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>FocusBar</string>
    <key>CFBundleDisplayName</key><string>FocusBar</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "提示：本地 ad-hoc 签名失败，不影响本机运行"

echo "已生成：$(pwd)/$APP"
