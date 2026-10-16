#!/usr/bin/env bash
# 安装 FocusBar 到 ~/Applications，可选装登录自启（LaunchAgent）
#   ./install.sh          只安装
#   ./install.sh --login  安装 + 登录时自动启动
#   ./install.sh --uninstall  卸载（应用 + 自启配置）
set -euo pipefail
cd "$(dirname "$0")"

DEST="$HOME/Applications/FocusBar.app"
AGENT="$HOME/Library/LaunchAgents/com.xqh.focusbar.plist"

if [[ "${1:-}" == "--uninstall" ]]; then
  launchctl bootout "gui/$(id -u)/com.xqh.focusbar" 2>/dev/null || true
  rm -f "$AGENT"
  pkill -f 'FocusBar.app/Contents/MacOS/focusbar' 2>/dev/null || true
  rm -rf "$DEST"
  echo "已卸载 FocusBar"
  exit 0
fi

./make_app.sh
mkdir -p "$HOME/Applications"
rm -rf "$DEST"
cp -R target/FocusBar.app "$DEST"

# 先停掉所有已有实例（含 LaunchAgent 里的），避免菜单栏出现两个倒计时
launchctl bootout "gui/$(id -u)/com.xqh.focusbar" 2>/dev/null || true
pkill -f 'FocusBar.app/Contents/MacOS/focusbar' 2>/dev/null || true
sleep 1

if [[ "${1:-}" == "--login" ]]; then
  mkdir -p "$(dirname "$AGENT")"
  cat > "$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.xqh.focusbar</string>
    <key>ProgramArguments</key>
    <array><string>$DEST/Contents/MacOS/focusbar</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><false/>
</dict>
</plist>
PLIST
  # 交给 launchd 启动，别再 open，否则会起第二个实例
  launchctl bootstrap "gui/$(id -u)" "$AGENT"
  echo "已设置登录自启"
else
  open "$DEST"
fi

echo "已安装并启动：$DEST"
