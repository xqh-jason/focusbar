//! FocusBar — 极简菜单栏专注计时器。
//!
//! 只做一件事：菜单栏显示倒计时，30 分钟专注 / 5 分钟休息，一轮结束自动暂停，
//! 需要人手点「开始」进入下一轮（和 WebPomodoro 的用法一致）。
//!
//! 刻意不创建任何窗口：不创建 window 就不会拉起 WebKit 的 WebContent / GPU 进程，
//! 物理内存能压到 25MB 上下，而原生 App + WKWebView 的方案实测在 165MB。
//! 另外每秒只在「显示文本真的变了」时才调 AppKit 改菜单栏，避免无谓重绘吃 CPU。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{ActivationPolicy, AppHandle, Manager};

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

#[derive(Clone, Copy, PartialEq, Eq)]
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

    /// 把剩余秒数折算成结束时刻（多给 200ms，显示不会提前跳到 0）
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

    fn reset(&mut self) {
        self.deadline = None;
        self.remaining = self.phase.secs();
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
        // 向上取整：还剩 0.4 秒时仍显示 00:01
        self.remaining = (left.as_millis() as u64).div_ceil(1000);
        false
    }

    fn clock(&self) -> String {
        format!("{:02}:{:02}", self.remaining / 60, self.remaining % 60)
    }

    fn menu_title(&self) -> String {
        if self.running() {
            self.clock()
        } else {
            format!("{} ⏸", self.clock())
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
            self.clock(),
            self.done_focus
        )
    }
}

struct Ui {
    timer: Mutex<Timer>,
    toggle: MenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    /// 上一次真正写进 AppKit 的文本，用来跳过无变化的重绘
    last: Mutex<(String, String, String)>,
}

/// 起一个外部命令并回收它，避免留下僵尸进程（afplay / osascript 都是短命进程）
fn spawn_quiet(cmd: &mut Command) {
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(_) => {}
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

fn refresh(app: &AppHandle) {
    let ui = app.state::<Ui>();
    let (title, status, toggle) = {
        let t = ui.timer.lock().unwrap();
        (
            t.menu_title(),
            t.status_line(),
            (if t.running() { "暂停" } else { "开始" }).to_owned(),
        )
    };
    let mut last = ui.last.lock().unwrap();
    if last.0 != title {
        if let Some(tray) = app.tray_by_id("focusbar") {
            let _ = tray.set_title(Some(&title));
        }
        last.0 = title;
    }
    if last.1 != status {
        let _ = ui.status.set_text(&status);
        last.1 = status;
    }
    if last.2 != toggle {
        let _ = ui.toggle.set_text(&toggle);
        last.2 = toggle;
    }
}

/// 单实例保护：菜单栏应用起两个实例，顶部就会出现两个倒计时。
/// 用 Unix socket 占位（不引入额外依赖）：绑定成功=拿到所有权；已存在时先试着连接，
/// 连得上说明真有实例在跑，连不上说明是上次异常退出留下的死文件，删掉重绑。
fn claim_single_instance() -> bool {
    use std::os::unix::net::{UnixListener, UnixStream};
    let Ok(home) = std::env::var("HOME") else {
        return true;
    };
    let dir = std::path::PathBuf::from(home).join("Library/Application Support/FocusBar");
    let _ = std::fs::create_dir_all(&dir);
    let sock = dir.join("focusbar.sock");
    match UnixListener::bind(&sock) {
        Ok(listener) => {
            // 故意泄漏：进程活多久，socket 就占多久
            Box::leak(Box::new(listener));
            true
        }
        Err(_) => {
            if UnixStream::connect(&sock).is_ok() {
                false
            } else {
                let _ = std::fs::remove_file(&sock);
                match UnixListener::bind(&sock) {
                    Ok(listener) => {
                        Box::leak(Box::new(listener));
                        true
                    }
                    Err(_) => true,
                }
            }
        }
    }
}

fn main() {
    if !claim_single_instance() {
        println!("[focusbar] 已有实例在运行，本进程退出");
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            // 不占 Dock、不抢焦点，纯菜单栏进程
            let _ = app.set_activation_policy(ActivationPolicy::Accessory);

            let (f, b) = durations();
            let toggle = MenuItem::with_id(app, "toggle", "开始", true, None::<&str>)?;
            let status =
                MenuItem::with_id(app, "status", "已暂停 · 专注 · 30:00", false, None::<&str>)?;
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

            let _tray = TrayIconBuilder::with_id("focusbar")
                .icon_as_template(true)
                .title("30:00 ⏸")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let ui = app.state::<Ui>();
                    match event.id().as_ref() {
                        "toggle" => {
                            let mut t = ui.timer.lock().unwrap();
                            if t.running() {
                                t.pause();
                                println!("[focusbar] 手动暂停");
                            } else {
                                t.start();
                                println!("[focusbar] 手动开始");
                            }
                        }
                        "reset" => {
                            ui.timer.lock().unwrap().reset();
                            println!("[focusbar] 重置本轮");
                        }
                        "focus" => {
                            ui.timer.lock().unwrap().set_phase(Phase::Focus);
                            println!("[focusbar] 切到专注");
                        }
                        "break" => {
                            ui.timer.lock().unwrap().set_phase(Phase::Break);
                            println!("[focusbar] 切到休息");
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                    refresh(app);
                })
                .build(app)?;

            app.manage(Ui {
                timer: Mutex::new(Timer::new()),
                toggle,
                status,
                last: Mutex::new((String::new(), String::new(), String::new())),
            });

            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(TICK_MS));
                let mut finished: Option<(Phase, Phase)> = None;
                {
                    let ui = handle.state::<Ui>();
                    let mut t = ui.timer.lock().unwrap();
                    let done = t.phase;
                    if t.tick() {
                        finished = Some((done, t.phase));
                    }
                }
                if let Some((done, next)) = finished {
                    notify(done, next);
                }
                refresh(&handle);
            });

            refresh(app.handle());
            println!("[focusbar] 启动：专注 {} / 休息 {}", human(f), human(b));
            if std::env::var("FOCUSBAR_AUTOSTART").is_ok() {
                app.state::<Ui>().timer.lock().unwrap().start();
                println!("[focusbar] 启动即开始计时");
                refresh(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building FocusBar")
        .run(|_, _| {});
}
