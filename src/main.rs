//! FocusBar — 极简菜单栏专注计时器。
//!
//! 只做一件事：菜单栏显示倒计时，30 分钟专注 / 5 分钟休息，一轮结束自动暂停，
//! 需要人手点「开始」进入下一轮。
//!
//! 刻意不创建任何窗口：不创建 window 就不会拉起 WebKit 的 WebContent / GPU 进程，
//! 物理内存能压到 25MB 上下，而「原生 App + WKWebView」的同类方案实测在 165MB。
//!
//! 计时用 SystemTime（墙钟）而非 Instant：合盖/睡眠期间也要照常走。
//! 代价是系统时间被改动会影响剩余时间（前拨会立刻结束、回拨会拉长），这是墙钟方案的固有取舍。

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{ActivationPolicy, AppHandle, Manager, RunEvent};

const DEFAULT_FOCUS_SECS: u64 = 30 * 60;
const DEFAULT_BREAK_SECS: u64 = 5 * 60;
/// 1 秒足够（显示精度就是秒），把唤醒次数压到最低
const TICK_MS: u64 = 1000;

/// 阶段时长，可用环境变量覆盖（用于验收/自测，不改代码）：
/// FOCUSBAR_FOCUS_SECS / FOCUSBAR_BREAK_SECS
fn durations() -> (u64, u64) {
    static D: OnceLock<(u64, u64)> = OnceLock::new();
    *D.get_or_init(|| {
        let read = |key: &str, default: u64| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(default)
        };
        (
            read("FOCUSBAR_FOCUS_SECS", DEFAULT_FOCUS_SECS),
            read("FOCUSBAR_BREAK_SECS", DEFAULT_BREAK_SECS),
        )
    })
}

fn human(secs: u64) -> String {
    if secs >= 60 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Focus,
    Break,
}

impl Phase {
    fn secs(self) -> u64 {
        let (focus, brk) = durations();
        match self {
            Phase::Focus => focus,
            Phase::Break => brk,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Phase::Focus => "专注",
            Phase::Break => "休息",
        }
    }
    fn next(self) -> Phase {
        match self {
            Phase::Focus => Phase::Break,
            Phase::Break => Phase::Focus,
        }
    }
}

struct Timer {
    phase: Phase,
    /// 当前阶段剩余秒数（暂停时的快照；运行中以 deadline 为准）
    remaining: u64,
    /// 运行中为未来的结束时刻（用系统时钟，合盖/睡眠期间也照常走）
    deadline: Option<SystemTime>,
    /// 本次进程内完成的专注轮数（只做展示，不落盘）
    done_focus: u32,
}

impl Timer {
    fn new() -> Self {
        Timer {
            phase: Phase::Focus,
            remaining: Phase::Focus.secs(),
            deadline: None,
            done_focus: 0,
        }
    }

    fn running(&self) -> bool {
        self.deadline.is_some()
    }

    /// 把剩余秒数折算成结束时刻。多给 200ms 是为了「显示不提前跳到 0」，
    /// 这点偏差由 tick() 里的 min 夹住，不会出现 30:01。
    fn arm(&mut self) {
        let nanos = Duration::from_secs(self.remaining) + Duration::from_millis(200);
        self.deadline = Some(SystemTime::now() + nanos);
    }

    fn start(&mut self) {
        if !self.running() {
            if self.remaining == 0 {
                self.remaining = self.phase.secs();
            }
            self.arm();
        }
    }

    fn pause(&mut self) {
        self.tick();
        self.deadline = None;
    }

    /// 重置本轮：剩余时间恢复成整段；本来在计时就继续计时，本来停着就停着。
    fn reset(&mut self) {
        let running = self.running();
        self.deadline = None;
        self.remaining = self.phase.secs();
        if running {
            self.arm();
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        let running = self.running();
        self.phase = phase;
        self.deadline = None;
        self.remaining = phase.secs();
        if running {
            self.arm();
        }
    }

    /// 按墙钟推进一次；返回 true 表示这一轮刚刚结束（需要提示 + 自动暂停）
    fn tick(&mut self) -> bool {
        let Some(deadline) = self.deadline else {
            return false;
        };
        let now = SystemTime::now();
        if now >= deadline {
            let finished = self.phase;
            self.done_focus += u32::from(finished == Phase::Focus);
            self.phase = finished.next();
            self.remaining = self.phase.secs();
            // 自动暂停：不自动接续下一轮，等用户点「开始」
            self.deadline = None;
            return true;
        }
        let left = deadline.duration_since(now).unwrap_or_default();
        // 向上取整（还剩 0.4 秒也显示 00:01），再夹在本阶段总长内：
        // arm() 给的 200ms 余量会让第一秒算出 1801，夹住就不会闪一下 30:01。
        self.remaining = (left.as_millis() as u64)
            .div_ceil(1000)
            .min(self.phase.secs());
        false
    }

    fn clock(&self) -> String {
        format!("{:02}:{:02}", self.remaining / 60, self.remaining % 60)
    }

    /// 菜单栏显示文本。默认全程逐秒 mm:ss（要的是一眼看到秒数）。
    /// 代价：每次改菜单栏文本都会触发一次 CoreAnimation + WindowServer 的 XPC 提交（实测约 9ms），
    /// 逐秒刷就是约 1% 单核。想省这点 CPU 就用 FOCUSBAR_ECONOMY_TITLE=1：
    /// 非最后一分钟只显示整分钟（29m → 28m …），最后一分钟才逐秒。
    fn display(&self) -> String {
        static ECONOMY: OnceLock<bool> = OnceLock::new();
        let economy = *ECONOMY.get_or_init(|| std::env::var("FOCUSBAR_ECONOMY_TITLE").is_ok());
        if economy && self.running() && self.remaining > 60 {
            format!("{}m", self.remaining / 60)
        } else {
            self.clock()
        }
    }

    fn menu_title(&self) -> String {
        if self.running() {
            self.display()
        } else {
            format!("{} ⏸", self.display())
        }
    }

    fn status_line(&self) -> String {
        format!(
            "{} · {} · {} · 已完成 {} 轮专注",
            if self.running() {
                "计时中"
            } else {
                "已暂停"
            },
            self.phase.label(),
            self.display(),
            self.done_focus
        )
    }

    fn toggle_label(&self) -> String {
        (if self.running() { "暂停" } else { "开始" }).to_owned()
    }
}

/// 计时器状态 + 上一次真正写进 AppKit 的文本，放同一把锁里：
/// 「读快照 → 比对 → 记录已写文本」是一次原子操作，
/// 避免 tick 线程读到旧状态、菜单线程刚改完，导致菜单栏短暂显示错状态。
struct State {
    timer: Timer,
    last_title: String,
    last_status: String,
    last_toggle: String,
}

struct Ui {
    state: Mutex<State>,
    toggle: MenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
}

/// 起一个外部命令并回收它，避免留下僵尸进程（afplay / osascript 都是短命进程）
fn spawn_quiet(cmd: &mut Command) {
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("[focusbar] 外部命令启动失败：{e}"),
    }
}

/// 一轮结束时：系统提示音 + 一条通知（提示失败绝不影响计时）
fn notify(phase_finished: Phase, next: Phase) {
    println!(
        "[focusbar] {} 结束 -> 自动暂停，切到{} {}",
        phase_finished.label(),
        next.label(),
        human(next.secs())
    );
    spawn_quiet(Command::new("afplay").arg("/System/Library/Sounds/Glass.aiff"));

    // msg 只由本程序里的常量与数字拼成，没有外部输入。
    // 以后若要往通知里塞用户可控内容，必须走参数传递，不能直接插进 AppleScript 字符串。
    let msg = format!(
        "{}结束，点菜单栏「开始」进入{} {}",
        phase_finished.label(),
        next.label(),
        human(next.secs())
    );
    spawn_quiet(Command::new("osascript").arg("-e").arg(format!(
        "display notification \"{msg}\" with title \"FocusBar\""
    )));
}

/// 把当前状态同步到菜单栏：只有文本真的变了才调 AppKit（每次约 9ms）
fn refresh(app: &AppHandle) {
    let ui = app.state::<Ui>();
    let (title, status, toggle) = {
        let mut state = ui.state.lock().unwrap();
        let title = state.timer.menu_title();
        let status = state.timer.status_line();
        let toggle = state.timer.toggle_label();
        let title_changed = state.last_title != title;
        let status_changed = state.last_status != status;
        let toggle_changed = state.last_toggle != toggle;
        if title_changed {
            state.last_title.clone_from(&title);
        }
        if status_changed {
            state.last_status.clone_from(&status);
        }
        if toggle_changed {
            state.last_toggle.clone_from(&toggle);
        }
        (
            title_changed.then_some(title),
            status_changed.then_some(status),
            toggle_changed.then_some(toggle),
        )
    };

    if let Some(title) = title {
        if let Some(tray) = app.tray_by_id("focusbar") {
            let _ = tray.set_title(Some(&title));
        }
    }
    if let Some(status) = status {
        let _ = ui.status.set_text(status);
    }
    if let Some(toggle) = toggle {
        let _ = ui.toggle.set_text(toggle);
    }
}

/// 已占用的 socket 路径，进程退出时删掉，不留残留
static SOCK_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 单实例保护：菜单栏应用起两个实例，顶部就会出现两个倒计时。
/// 用 Unix socket 占位（不引入额外依赖）：绑定成功 = 拿到所有权；
/// 已存在时先试着连接，连得上说明真有实例在跑；连不上说明是上次异常退出留下的死文件，删掉重绑。
/// 拿不到所有权就一律拒绝启动（fail-closed）：宁可不起，也不要出现两个倒计时。
fn claim_single_instance() -> bool {
    use std::os::unix::net::{UnixListener, UnixStream};

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates
            .push(PathBuf::from(home).join("Library/Application Support/FocusBar/focusbar.sock"));
    }
    candidates.push(std::env::temp_dir().join("focusbar.sock"));

    for sock in candidates {
        if let Some(dir) = sock.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                continue;
            }
        }
        match UnixListener::bind(&sock) {
            Ok(listener) => {
                // 故意泄漏：进程活多久，socket 就占多久
                Box::leak(Box::new(listener));
                let _ = SOCK_PATH.set(sock);
                return true;
            }
            Err(_) => {
                if UnixStream::connect(&sock).is_ok() {
                    // 已经有实例在跑
                    return false;
                }
                // 死文件：删掉重绑一次
                if std::fs::remove_file(&sock).is_err() {
                    continue;
                }
                if let Ok(listener) = UnixListener::bind(&sock) {
                    Box::leak(Box::new(listener));
                    let _ = SOCK_PATH.set(sock);
                    return true;
                }
            }
        }
    }
    eprintln!("[focusbar] 无法独占实例锁，拒绝启动（避免出现两个倒计时）");
    false
}

fn cleanup() {
    if let Some(sock) = SOCK_PATH.get() {
        let _ = std::fs::remove_file(sock);
    }
}

fn main() {
    if !claim_single_instance() {
        println!("[focusbar] 已有实例在运行（或无法独占实例锁），本进程退出");
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            // 不占 Dock、不抢焦点，纯菜单栏进程
            app.set_activation_policy(ActivationPolicy::Accessory);

            let (f, b) = durations();
            // 初始文案按实际配置算，别硬编码 30:00（用 *_SECS 覆盖时才不会闪一下错值）
            let initial_title = Timer::new().menu_title();
            let initial_status = Timer::new().status_line();

            let toggle = MenuItem::with_id(app, "toggle", "开始", true, None::<&str>)?;
            let status = MenuItem::with_id(app, "status", initial_status, false, None::<&str>)?;
            let focus = MenuItem::with_id(
                app,
                "focus",
                format!("专注 {}", human(f)),
                true,
                None::<&str>,
            )?;
            let brk = MenuItem::with_id(
                app,
                "break",
                format!("休息 {}", human(b)),
                true,
                None::<&str>,
            )?;
            let reset = MenuItem::with_id(app, "reset", "重置本轮", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

            let menu = Menu::with_items(
                app,
                &[
                    &toggle,
                    &status,
                    &PredefinedMenuItem::separator(app)?,
                    &focus,
                    &brk,
                    &reset,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            // 有意不设图标：菜单栏只显示文字倒计时（给了图标反而会被 template 渲染成一块色斑）
            let _tray = TrayIconBuilder::with_id("focusbar")
                .title(initial_title)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let ui = app.state::<Ui>();
                    match event.id().as_ref() {
                        "toggle" => {
                            let mut state = ui.state.lock().unwrap();
                            if state.timer.running() {
                                state.timer.pause();
                                println!("[focusbar] 手动暂停");
                            } else {
                                state.timer.start();
                                println!("[focusbar] 手动开始");
                            }
                        }
                        "reset" => {
                            ui.state.lock().unwrap().timer.reset();
                            println!("[focusbar] 重置本轮");
                        }
                        "focus" => {
                            ui.state.lock().unwrap().timer.set_phase(Phase::Focus);
                            println!("[focusbar] 切到专注");
                        }
                        "break" => {
                            ui.state.lock().unwrap().timer.set_phase(Phase::Break);
                            println!("[focusbar] 切到休息");
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                    refresh(app);
                })
                .build(app)?;

            app.manage(Ui {
                state: Mutex::new(State {
                    timer: Timer::new(),
                    last_title: String::new(),
                    last_status: String::new(),
                    last_toggle: String::new(),
                }),
                toggle,
                status,
            });

            // tick 线程：每秒推进一次，并兜住 panic —— 否则线程一死，计时就静默停摆
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(TICK_MS));
                let mut finished: Option<(Phase, Phase)> = None;
                let ticked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let ui = handle.state::<Ui>();
                    let mut state = ui.state.lock().unwrap_or_else(|e| e.into_inner());
                    let done = state.timer.phase;
                    if state.timer.tick() {
                        finished = Some((done, state.timer.phase));
                    }
                }));
                if ticked.is_err() {
                    eprintln!("[focusbar] tick 出现 panic，已跳过本轮");
                }
                if let Some((done, next)) = finished {
                    notify(done, next);
                }
                refresh(&handle);
            });

            refresh(app.handle());
            println!("[focusbar] 启动：专注 {} / 休息 {}", human(f), human(b));
            if std::env::var("FOCUSBAR_AUTOSTART").is_ok() {
                app.state::<Ui>().state.lock().unwrap().timer.start();
                println!("[focusbar] 启动即开始计时");
                refresh(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building FocusBar")
        .run(|_, event| {
            if let RunEvent::Exit = event {
                cleanup();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// arm() 会多给 200ms，第一秒的 tick 不能算出「比整段还长」的剩余（历史上会闪一下 30:01）
    #[test]
    fn first_tick_never_exceeds_phase_length() {
        let mut t = Timer::new();
        t.start();
        t.tick();
        assert!(
            t.remaining <= Phase::Focus.secs(),
            "剩余 {} 超过了整段 {}",
            t.remaining,
            Phase::Focus.secs()
        );
    }

    /// 到点后：切到下一阶段、计数 +1、自动暂停（不自动接续）
    #[test]
    fn deadline_switches_phase_and_auto_pauses() {
        let mut t = Timer::new();
        t.start();
        t.deadline = Some(SystemTime::now() - Duration::from_secs(1));
        assert!(t.tick(), "到点应返回 true（需要提示音）");
        assert_eq!(t.phase, Phase::Break);
        assert_eq!(t.remaining, Phase::Break.secs());
        assert_eq!(t.done_focus, 1);
        assert!(!t.running(), "结束后必须自动暂停");
    }

    /// 暂停：remaining 冻结在快照值，且 deadline 清空
    #[test]
    fn pause_freezes_remaining() {
        let mut t = Timer::new();
        t.start();
        t.remaining = 42;
        t.deadline = Some(SystemTime::now() + Duration::from_secs(42));
        t.pause();
        assert!(!t.running());
        assert!(
            t.remaining <= 42 && t.remaining >= 41,
            "剩余被冻结成 {}",
            t.remaining
        );
    }

    /// 重置本轮：恢复整段，并保持原来的运行/暂停状态
    #[test]
    fn reset_restores_full_phase_and_keeps_running_state() {
        let mut t = Timer::new();
        t.start();
        t.tick();
        t.reset();
        assert_eq!(t.remaining, Phase::Focus.secs());
        assert!(t.running(), "本来在计时，重置后应继续计时");

        let mut t2 = Timer::new();
        t2.remaining = 5;
        t2.reset();
        assert_eq!(t2.remaining, Phase::Focus.secs());
        assert!(!t2.running(), "本来停着，重置后应仍然停着");
    }

    /// 切阶段也保持运行状态
    #[test]
    fn set_phase_keeps_running_state() {
        let mut t = Timer::new();
        t.start();
        t.set_phase(Phase::Break);
        assert_eq!(t.phase, Phase::Break);
        assert_eq!(t.remaining, Phase::Break.secs());
        assert!(t.running());
    }

    /// 菜单栏文本：暂停时带 ⏸，计时中不带
    #[test]
    fn menu_title_marks_paused_state() {
        let mut t = Timer::new();
        assert!(t.menu_title().ends_with('⏸'));
        t.start();
        assert!(!t.menu_title().ends_with('⏸'));
    }
}
