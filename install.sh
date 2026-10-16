#!/usr/bin/env bash
# 安装 FocusBar 到 ~/Applications，可选装登录自启（LaunchAgent）
#   ./install.sh            只安装
#   ./install.sh --login    安装 + 登录时自动启动
#   ./install.sh --uninstall  卸载（应用 + 自启配置 + 运行时残留）
set -euo pipefail
cd "$(dirname "$0")"

DEST="$HOME/Applications/FocusBar.app"
AGENT="$HOME/Library/LaunchAgents/com.xqh.focusbar.plist"
SUPPORT="$HOME/Library/Application Support/FocusBar"
LABEL="com.xqh.focusbar"

if [[ "${1:-}" == "--uninstall" ]]; then
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  rm -f "$AGENT"
  pkill -f 'FocusBar.app/Contents/MacOS/focusbar' 2>/dev/null || true
  rm -rf "$DEST" "$SUPPORT"
  echo "已卸载 FocusBar（应用 / 自启配置 / 运行时残留都清了）"
  exit 0
fi

./make_app.sh

# 先停掉所有已有实例（含 LaunchAgent 里的），避免菜单栏出现两个倒计时
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
pkill -f 'FocusBar.app/Contents/MacOS/focusbar' 2>/dev/null || true

mkdir -p "$HOME/Applications"
# 先拷到临时位置，成功后再原子替换：中途失败不会把现有安装毁掉
TMP="$HOME/Applications/.FocusBar.app.tmp.$$"
rm -rf "$TMP"
trap 'rm -rf "$TMP"' EXIT
ditto target/FocusBar.app "$TMP"
rm -rf "$DEST"
mv "$TMP" "$DEST"
trap - EXIT

if [[ "${1:-}" == "--login" ]]; then
  mkdir -p "$(dirname "$AGENT")"
  # plist 里要写路径，XML 特殊字符必须转义（否则 HOME 含 & < > 时 plist 直接损坏）
  xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
  DEST_XML=$(xml_escape "$DEST")
  cat > "$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$LABEL</string>
    <key>ProgramArguments</key>
    <array><string>$DEST_XML/Contents/MacOS/focusbar</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><false/>
</dict>
</plist>
PLIST
  # 交给 launchd 启动，别再 open，否则会起第二个实例
  if launchctl bootstrap "gui/$(id -u)" "$AGENT"; then
    echo "已设置登录自启"
  else
    echo "警告：自启注册失败（plist 已写到 $AGENT），本次改用直接启动。" >&2
    open "$DEST"
  fi
  sleep 1
  # launchd 没把进程拉起来就兜底直接开，避免「装了但菜单栏没东西」
  if ! pgrep -f 'FocusBar.app/Contents/MacOS/focusbar' >/dev/null; then
    echo "警告：launchd 未拉起进程，直接启动一次" >&2
    open "$DEST"
  fi
else
  open "$DEST"
fi

echo "已安装并启动：$DEST"
