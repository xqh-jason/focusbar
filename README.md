# FocusBar

极简 macOS 菜单栏专注计时器：菜单栏里显示倒计时，30 分钟专注 / 5 分钟休息，一轮结束自动暂停，
需要手点「开始」进入下一轮（和 WebPomodoro 的用法一致）。除此之外没有别的功能：没有窗口、
没有任务/日历、没有音乐、没有任何网络请求。

## 用起来

```bash
./install.sh            # 安装到 ~/Applications 并启动（推荐）
./install.sh --login    # 顺带装上登录自启（LaunchAgent）
./install.sh --uninstall
```

装好后菜单栏会出现倒计时：运行中显示 `29:59`，暂停时显示 `29:59 ⏸`。
点它弹出菜单：

| 菜单项 | 作用 |
| --- | --- |
| 开始 / 暂停 | 开始或暂停当前这一轮 |
| 已暂停 · 专注 · 30:00 · 已完成 N 轮专注 | 只读状态行 |
| 专注 30 分钟 | 切到专注阶段（按当前是否在计时决定继续跑还是停着） |
| 休息 5 分钟 | 切到休息阶段 |
| 重置本轮 | 把当前阶段的时间归零重来 |
| 退出 | 退出进程 |

一轮结束时：播放系统提示音 + 一条通知，然后**自动暂停**，等你点「开始」才进下一轮。
计时按墙钟推进，合盖/睡眠期间照常走，醒来显示的剩余时间是对的。

## 自测用的环境变量

```bash
FOCUSBAR_FOCUS_SECS=45 FOCUSBAR_BREAK_SECS=10 FOCUSBAR_AUTOSTART=1 \
  ~/Applications/FocusBar.app/Contents/MacOS/focusbar
```

- `FOCUSBAR_FOCUS_SECS` / `FOCUSBAR_BREAK_SECS`：覆盖阶段时长（秒），菜单文案也跟着变
- `FOCUSBAR_AUTOSTART=1`：启动即开始计时

## 为什么内存能压到这么低

不创建任何窗口 —— 不创建 window，tauri/wry 就不会拉起 WKWebView，也就没有
`WebKit.WebContent` / `WebKit.GPU` / `WebKit.Networking` 这三个辅助进程。
整个应用只有一个进程：Rust + AppKit 的菜单栏项。

另外每秒只在「要显示的文本真的变了」时才调 AppKit 改菜单栏，暂停状态下 CPU 为 0。

## 实测数据（本机 macOS 27.0.1，同日同机对比）

| 指标 | WebPomodoro 6.9（原生 Swift + WKWebView） | FocusBar（Tauri，纯菜单栏） |
| --- | --- | --- |
| 进程数 | 4（主进程 + WebContent + GPU + Networking） | 1 |
| 物理内存合计 | 165 MB（36 + 96 + 24 + 9） | 24 MB |
| RSS 合计 | 约 116 MB | 90 MB（含共享框架，参考值） |
| 计时中 CPU | ≈ 0.03%（几乎为 0） | ≈ 1%（单核，每秒刷新菜单栏文本所致） |
| 暂停时 CPU | ≈ 0 | 0 |
| App 体积 | 24 MB（其中 19 MB 是网页资源） | 7.5 MB |
| 网络 | 启动时校验 App Store 收据，含升级/内购弹窗逻辑 | 零网络 |

物理内存用 `footprint -p <pid>` 测，RSS 用 `ps -o rss`（RSS 会把共享框架算进去，偏大）。

## 开发

```bash
cargo build --release   # 编译
./make_app.sh           # 打出 target/FocusBar.app（手工组装 bundle，不需要 tauri-cli）
cargo fmt
```

依赖：Rust 1.99、tauri 2（`tray-icon` feature）。`tauri.conf.json` 里 `"windows": []`，
所以前端目录 `dist/` 只是个占位，永远不会被加载。
