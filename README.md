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

装好后菜单栏会出现倒计时：计时中显示 `29:59` 并逐秒走，暂停时显示 `29:59 ⏸`。

每次改菜单栏文本都要走一次 CoreAnimation + WindowServer 的 XPC（实测约 9ms），所以逐秒刷的代价是
约 1% 单核。想省掉它就用 `FOCUSBAR_ECONOMY_TITLE=1` 启动：平时只显示整分钟（`29m`），最后一分钟才逐秒。

点它弹出菜单：

| 菜单项 | 作用 |
| --- | --- |
| 开始 / 暂停 | 开始或暂停当前这一轮 |
| 计时中 · 专注 · 29:59 · 已完成 N 轮专注 | 只读状态行 |
| 专注 30 分钟 | 切到专注阶段（按当前是否在计时决定继续跑还是停着） |
| 休息 5 分钟 | 切到休息阶段 |
| 重置本轮 | 把当前阶段的时间归零重来 |
| 退出 | 退出进程 |

一轮结束时：播放系统提示音 + 一条通知，然后**自动暂停**，等你点「开始」才进下一轮。
计时按墙钟推进，合盖/睡眠期间照常走，醒来显示的剩余时间是对的。

只允许一个实例运行（Unix socket 占位）：重复启动会直接退出，不会在菜单栏出现两个倒计时。

## 自测用的环境变量

```bash
FOCUSBAR_FOCUS_SECS=45 FOCUSBAR_BREAK_SECS=10 FOCUSBAR_AUTOSTART=1 \
  ~/Applications/FocusBar.app/Contents/MacOS/focusbar
```

- `FOCUSBAR_FOCUS_SECS` / `FOCUSBAR_BREAK_SECS`：覆盖阶段时长（秒），菜单文案也跟着变
- `FOCUSBAR_AUTOSTART=1`：启动即开始计时
- `FOCUSBAR_ECONOMY_TITLE=1`：菜单栏平时只显示整分钟（省约 1% 单核）

## CPU 与内存

### 为什么 CPU 是 1%

每次改菜单栏文本，AppKit 都会走一次 CoreAnimation 提交 + 到 WindowServer 的 XPC（`sample` 采样里能看到
`CA::Transaction::commit` → BaseBoard），实测**每次约 9ms**。逐秒刷 mm:ss 就等于每秒烧 9ms ≈ 1% 单核。
这已经是最省的实现：文本没变就不调 AppKit（暂停态因此是 0%）。真要再降只能牺牲秒数精度，见
`FOCUSBAR_ECONOMY_TITLE`。

## 为什么内存能压到这么低

不创建任何窗口 —— 不创建 window，tauri/wry 就不会拉起 WKWebView，也就没有
`WebKit.WebContent` / `WebKit.GPU` / `WebKit.Networking` 这三个辅助进程。
整个应用只有一个进程：Rust + AppKit 的菜单栏项。

## 实测数据（本机 macOS 27.0.1，同日同机对比）

| 指标 | WebPomodoro 6.9（原生 Swift + WKWebView） | FocusBar（Tauri，纯菜单栏） |
| --- | --- | --- |
| 进程数 | 4（主进程 + WebContent + GPU + Networking） | 1 |
| 物理内存合计 | 169 MB（36 + 109 + 15 + 9） | 23 MB |
| RSS 合计 | 约 103 MB（波动，参考值） | 90 MB（含共享框架，参考值） |
| 计时中 CPU（默认，逐秒 mm:ss） | ≈ 0.03% | 0.92% |
| 计时中 CPU（`FOCUSBAR_ECONOMY_TITLE=1`，平时整分钟） | — | 0.00%（60 秒窗口里只剩整分钟边界那一次刷新） |
| 暂停时 CPU | ≈ 0 | 0 |
| App 体积 | 24 MB（其中 19 MB 是网页资源） | 7.5 MB |
| 网络 | 启动时校验 App Store 收据，含升级/内购弹窗逻辑 | 零网络 |

CPU 是 `ps -o cputime` 前后差除以 60 秒窗口算出来的（`ps -o %cpu` 是生命周期均值，不能用）；
物理内存用 `footprint -p <pid>`，RSS 用 `ps -o rss`（RSS 会把共享框架算进去，偏大）。

## 开发

```bash
cargo build --release   # 编译
./make_app.sh           # 打出 target/FocusBar.app（手工组装 bundle，不需要 tauri-cli）
cargo fmt
```

依赖：Rust 1.99、tauri 2（`tray-icon` feature）。`tauri.conf.json` 里 `"windows": []`，
所以前端目录 `dist/` 只是个占位，永远不会被加载。
