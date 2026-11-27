//! Razer Lite 后台程序：鼠标按键映射 + 托盘 + 电量提醒。
//! 纯 Win32 实现，没有 GUI 框架，常驻内存极小。

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use rlite::config::{Action, Binding, Button, Config, MouseButton};
use rlite::{device, icon, keys, sys};
use sys::*;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static MAIN_HWND: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn hwnd() -> HWND {
    MAIN_HWND.load(Ordering::Relaxed)
}

const TIMER_HOLD: usize = 100; // 100..103：每个按键的长按计时器
const TIMER_TRIM: usize = 200;

const MENU_SETTINGS: usize = 1;
const MENU_PAUSE: usize = 3;
const MENU_RELOAD: usize = 4;
const MENU_EXIT: usize = 5;

// ---------------------------------------------------------------- 状态

#[derive(Default)]
enum BtnState {
    #[default]
    Idle,
    /// 本次按下交给系统原样处理
    Pass,
    /// 已执行“按下”，等待松开
    Pressed(Action),
    /// 设置了长按动作，等待判断单击还是长按
    Pending(Binding),
}

enum Cmd {
    Press(Button, Action),
    Release(Button, Action),
    Click(Button, Action),
}

enum DevCmd {
    Poll,
    Clutch(Option<u16>),
    Cycle(i8),
    Reconfigure { poll_minutes: u32, fix_mode: bool },
}

#[derive(Clone, Copy, Default)]
struct DevStatus {
    connected: bool,
    battery: u8,
    charging: bool,
    dpi: u16,
    pid: u16,
}

struct Agent {
    cfg: Config,
    states: [BtnState; 3],
    queue: VecDeque<Cmd>,
    dev_tx: Sender<DevCmd>,
    status: DevStatus,
    low_alerted: bool,
    tray_added: bool,
    icon_on: HICON,
    icon_off: HICON,
    taskbar_created: u32,
}

thread_local! {
    static AGENT: RefCell<Option<Agent>> = const { RefCell::new(None) };
}

/// 借用全局状态；如果正被借用（理论上不会发生）则返回 None，保证钩子永不 panic
fn with<R>(f: impl FnOnce(&mut Agent) -> R) -> Option<R> {
    AGENT.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// ---------------------------------------------------------------- 低级鼠标钩子

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wp as u32;
        if matches!(msg, WM_XBUTTONDOWN | WM_XBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP) {
            let info = &*(lp as *const MSLLHOOKSTRUCT);
            // 忽略模拟出来的事件（包括我们自己发的），避免死循环
            if info.flags & LLMHF_INJECTED == 0 {
                let button = match msg {
                    WM_MBUTTONDOWN | WM_MBUTTONUP => Some(Button::Middle),
                    _ => match (info.mouseData >> 16) as u16 {
                        1 => Some(Button::SideBack),
                        2 => Some(Button::SideFront),
                        _ => None,
                    },
                };
                if let Some(b) = button {
                    let down = matches!(msg, WM_XBUTTONDOWN | WM_MBUTTONDOWN);
                    if with(|a| a.on_button(b, down)).unwrap_or(false) {
                        return 1; // 吞掉原始事件
                    }
                }
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

fn is_toggle(b: &Binding) -> bool {
    b.tap == Action::ToggleMapping || b.hold == Some(Action::ToggleMapping)
}

impl Agent {
    /// 返回 true 表示吞掉原始事件
    fn on_button(&mut self, b: Button, down: bool) -> bool {
        let i = b as usize;
        let prev = std::mem::take(&mut self.states[i]);
        if down {
            if let BtnState::Pressed(a) = prev {
                self.queue.push_back(Cmd::Release(b, a));
            }
            let exe = if self.cfg.has_app_profiles() { foreground_exe() } else { None };
            let bd = self.cfg.resolve(b, exe.as_deref()).clone();
            if !self.cfg.enabled && !is_toggle(&bd) {
                self.states[i] = BtnState::Pass;
                return false;
            }
            match bd.hold {
                None if bd.tap == Action::Default => {
                    self.states[i] = BtnState::Pass;
                    return false;
                }
                None => {
                    self.queue.push_back(Cmd::Press(b, bd.tap.clone()));
                    self.states[i] = BtnState::Pressed(bd.tap);
                }
                Some(_) => {
                    unsafe { SetTimer(hwnd(), TIMER_HOLD + i, bd.hold_ms.clamp(100, 3000), None) };
                    self.states[i] = BtnState::Pending(bd);
                }
            }
            self.schedule_flush();
            true
        } else {
            let swallow = match prev {
                BtnState::Idle | BtnState::Pass => false,
                BtnState::Pressed(a) => {
                    self.queue.push_back(Cmd::Release(b, a));
                    true
                }
                BtnState::Pending(bd) => {
                    unsafe { KillTimer(hwnd(), TIMER_HOLD + i) };
                    self.queue.push_back(Cmd::Click(b, bd.tap));
                    true
                }
            };
            self.schedule_flush();
            swallow
        }
    }

    fn on_hold_timer(&mut self, i: usize) {
        unsafe { KillTimer(hwnd(), TIMER_HOLD + i) };
        let b = [Button::SideBack, Button::SideFront, Button::Middle][i];
        if let BtnState::Pending(bd) = std::mem::take(&mut self.states[i]) {
            let hold = bd.hold.unwrap_or(Action::Disabled);
            self.queue.push_back(Cmd::Press(b, hold.clone()));
            self.states[i] = BtnState::Pressed(hold);
        }
    }

    fn schedule_flush(&self) {
        if !self.queue.is_empty() {
            unsafe { PostMessageW(hwnd(), WM_RL_FLUSH, 0, 0) };
        }
    }

    /// 释放所有按住的状态（暂停、重载、退出时调用）
    fn release_all(&mut self) {
        for i in 0..3 {
            let b = [Button::SideBack, Button::SideFront, Button::Middle][i];
            unsafe { KillTimer(hwnd(), TIMER_HOLD + i) };
            if let BtnState::Pressed(a) = std::mem::take(&mut self.states[i]) {
                self.queue.push_back(Cmd::Release(b, a));
            }
        }
        self.schedule_flush();
    }
}

// ---------------------------------------------------------------- 动作执行

fn flush() {
    while let Some(Some(cmd)) = with(|a| a.queue.pop_front()) {
        match cmd {
            Cmd::Press(b, a) => press(b, &a),
            Cmd::Release(b, a) => release(b, &a),
            Cmd::Click(b, a) => {
                press(b, &a);
                release(b, &a);
            }
        }
    }
}

fn key_input(vk: u16, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if keys::is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(flags: u32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send(inputs: &[INPUT]) {
    if !inputs.is_empty() {
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    }
}

fn original_button(b: Button, down: bool) {
    let (flags, data) = match (b, down) {
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::SideBack, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON1 as i32),
        (Button::SideFront, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON2 as i32),
    };
    send(&[mouse_input(flags, data)]);
}

fn mouse_action(m: MouseButton, down: bool) {
    let x = |d: bool, n: u16| mouse_input(if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, n as i32);
    match (m, down) {
        (MouseButton::Left, true) => send(&[mouse_input(MOUSEEVENTF_LEFTDOWN, 0)]),
        (MouseButton::Left, false) => send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0)]),
        (MouseButton::Right, true) => send(&[mouse_input(MOUSEEVENTF_RIGHTDOWN, 0)]),
        (MouseButton::Right, false) => send(&[mouse_input(MOUSEEVENTF_RIGHTUP, 0)]),
        (MouseButton::Middle, true) => send(&[mouse_input(MOUSEEVENTF_MIDDLEDOWN, 0)]),
        (MouseButton::Middle, false) => send(&[mouse_input(MOUSEEVENTF_MIDDLEUP, 0)]),
        (MouseButton::Back, d) => send(&[x(d, XBUTTON1)]),
        (MouseButton::Forward, d) => send(&[x(d, XBUTTON2)]),
        (MouseButton::DoubleClick, true) => send(&[
            mouse_input(MOUSEEVENTF_LEFTDOWN, 0),
            mouse_input(MOUSEEVENTF_LEFTUP, 0),
            mouse_input(MOUSEEVENTF_LEFTDOWN, 0),
            mouse_input(MOUSEEVENTF_LEFTUP, 0),
        ]),
        (MouseButton::WheelUp, true) => send(&[mouse_input(MOUSEEVENTF_WHEEL, 120)]),
        (MouseButton::WheelDown, true) => send(&[mouse_input(MOUSEEVENTF_WHEEL, -120)]),
        _ => {}
    }
}

fn press(b: Button, a: &Action) {
    match a {
        Action::Default => original_button(b, true),
        Action::Keys { combo } => {
            if let Ok(v) = keys::parse_combo(combo) {
                send(&v.iter().map(|&k| key_input(k, false)).collect::<Vec<_>>());
            }
        }
        Action::Text { text } => {
            let mut v = vec![];
            for ch in text.replace("\r\n", "\n").chars() {
                if ch == '\n' {
                    v.push(key_input(0x0D, false));
                    v.push(key_input(0x0D, true));
                    continue;
                }
                let mut buf = [0u16; 2];
                for &u in ch.encode_utf16(&mut buf).iter() {
                    v.push(unicode_input(u, false));
                    v.push(unicode_input(u, true));
                }
            }
            send(&v);
        }
        Action::Mouse { button } => mouse_action(*button, true),
        Action::Run { target, args } => {
            let (t, p) = (target.clone(), args.clone());
            std::thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn(move || unsafe {
                    let (t, p, op) = (wide(&t), wide(&p), wide("open"));
                    ShellExecuteW(null_mut(), op.as_ptr(), t.as_ptr(), if p.len() > 1 { p.as_ptr() } else { null() }, null(), SW_SHOWNORMAL);
                })
                .ok();
        }
        Action::DpiClutch { dpi } => {
            with(|a| a.dev_tx.send(DevCmd::Clutch(Some(*dpi))).ok());
        }
        Action::DpiCycle { step } => {
            with(|a| a.dev_tx.send(DevCmd::Cycle(*step)).ok());
        }
        Action::ToggleMapping => {
            let enabled = with(|a| {
                a.cfg.enabled = !a.cfg.enabled;
                a.cfg.enabled
            });
            if let Some(false) = enabled {
                with(|a| a.release_all());
            }
            tray_update();
            if let Some(e) = enabled {
                balloon("Razer Lite", if e { "按键映射已恢复" } else { "按键映射已暂停" }, false);
            }
        }
        Action::Inherit | Action::Disabled => {}
    }
}

fn release(b: Button, a: &Action) {
    match a {
        Action::Default => original_button(b, false),
        Action::Keys { combo } => {
            if let Ok(v) = keys::parse_combo(combo) {
                send(&v.iter().rev().map(|&k| key_input(k, true)).collect::<Vec<_>>());
            }
        }
        Action::Mouse { button } => mouse_action(*button, false),
        Action::DpiClutch { .. } => {
            with(|a| a.dev_tx.send(DevCmd::Clutch(None)).ok());
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- 设备线程

fn pack_status(s: &DevStatus) -> (WPARAM, LPARAM) {
    let w = s.battery as usize | (s.charging as usize) << 8 | (s.connected as usize) << 9;
    let l = s.dpi as isize | (s.pid as isize) << 16;
    (w, l)
}

fn unpack_status(w: WPARAM, l: LPARAM) -> DevStatus {
    DevStatus {
        battery: (w & 0xFF) as u8,
        charging: w & 0x100 != 0,
        connected: w & 0x200 != 0,
        dpi: (l & 0xFFFF) as u16,
        pid: ((l >> 16) & 0xFFFF) as u16,
    }
}

fn device_worker(rx: Receiver<DevCmd>, mut poll_minutes: u32, mut fix_mode: bool) {
    let mut dev: Option<device::Device> = None;
    let mut mode_checked = false;
    let mut clutch_restore: Option<(u16, u16)> = None;
    let mut wait = Duration::from_millis(500);
    loop {
        let cmd = match rx.recv_timeout(wait) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => DevCmd::Poll,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        wait = Duration::from_secs(poll_minutes.clamp(1, 240) as u64 * 60);
        for attempt in 0..2 {
            if dev.is_none() {
                dev = device::Device::open().ok();
                mode_checked = false;
            }
            let Some(d) = dev.as_ref() else { break };
            let r: device::Result<()> = (|| {
                if fix_mode && !mode_checked {
                    mode_checked = true;
                    if !synapse_running() && d.device_mode()? != 0 {
                        d.set_device_mode(0)?;
                    }
                }
                match &cmd {
                    DevCmd::Poll | DevCmd::Reconfigure { .. } => {}
                    DevCmd::Clutch(Some(v)) => {
                        if clutch_restore.is_none() {
                            clutch_restore = Some(d.dpi()?);
                        }
                        d.set_dpi(*v, *v, false)?;
                    }
                    DevCmd::Clutch(None) => {
                        if let Some((x, y)) = clutch_restore.take() {
                            d.set_dpi(x, y, false)?;
                        }
                    }
                    DevCmd::Cycle(step) => {
                        let (active, stages) = d.dpi_stages()?;
                        let n = stages.len() as i32;
                        if n > 0 {
                            let next = ((active as i32 - 1 + *step as i32).rem_euclid(n)) + 1;
                            d.set_dpi_stages(next as u8, &stages)?;
                        }
                    }
                }
                Ok(())
            })();
            if let Err(device::Error::Io(_)) = r {
                dev = None; // 设备断开（如拔掉接收器、睡眠唤醒），重新打开再试一次
                if attempt == 0 {
                    continue;
                }
            }
            break;
        }
        if let DevCmd::Reconfigure { poll_minutes: p, fix_mode: f } = cmd {
            poll_minutes = p;
            fix_mode = f;
            mode_checked = false;
        }
        let mut st = DevStatus::default();
        if let Some(d) = dev.as_ref() {
            st.pid = d.pid;
            if let Ok(b) = d.battery() {
                st.connected = true;
                st.battery = b;
                st.charging = d.charging().unwrap_or(false);
                st.dpi = d.dpi().map(|x| x.0).unwrap_or(0);
            }
        }
        let (w, l) = pack_status(&st);
        unsafe { PostMessageW(hwnd(), WM_RL_DEVICE, w, l) };
    }
}

// ---------------------------------------------------------------- 托盘

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn tray_data() -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd();
    nid.uID = 1;
    nid
}

fn tooltip() -> String {
    with(|a| {
        let s = &a.status;
        let mut t = String::from("Razer Lite");
        if s.connected {
            t += &format!("\n{} · 电量 {}%{}", short_name(s.pid), s.battery, if s.charging { "（充电中）" } else { "" });
            if s.dpi > 0 {
                t += &format!("\nDPI {}", s.dpi);
            }
        } else {
            t += "\n未连接鼠标";
        }
        if !a.cfg.enabled {
            t += "\n映射已暂停";
        }
        t
    })
    .unwrap_or_default()
}

fn short_name(pid: u16) -> &'static str {
    device::model_name(pid).trim_start_matches("Razer ")
}

fn tray_update() {
    let Some((show, added, enabled, on, off)) =
        with(|a| (a.cfg.tray_icon, a.tray_added, a.cfg.enabled, a.icon_on, a.icon_off))
    else {
        return;
    };
    let mut nid = tray_data();
    unsafe {
        if !show {
            if added {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                with(|a| a.tray_added = false);
            }
            return;
        }
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_RL_TRAY;
        nid.hIcon = if enabled { on } else { off };
        copy_wide(&mut nid.szTip, &tooltip());
        if !added || Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
            Shell_NotifyIconW(NIM_ADD, &nid);
            with(|a| a.tray_added = true);
        }
    }
}

fn balloon(title: &str, text: &str, warn: bool) {
    if !with(|a| a.tray_added).unwrap_or(false) {
        return;
    }
    let mut nid = tray_data();
    nid.uFlags = NIF_INFO;
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    nid.dwInfoFlags = if warn { NIIF_WARNING } else { NIIF_INFO };
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn open_settings() {
    unsafe {
        let t = wide(SETTINGS_TITLE);
        let w = FindWindowW(null(), t.as_ptr());
        if !w.is_null() {
            ShowWindow(w, SW_RESTORE);
            SetForegroundWindow(w);
            return;
        }
    }
    if std::process::Command::new(exe_dir().join(SETTINGS_EXE)).spawn().is_err() {
        balloon("Razer Lite", "找不到 razer-lite-settings.exe，请把它和 razer-lite.exe 放在同一目录", true);
    }
}

fn show_menu() {
    let Some((st, enabled)) = with(|a| (a.status, a.cfg.enabled)) else { return };
    unsafe {
        let m = CreatePopupMenu();
        let item = |id: usize, flags: u32, text: &str| {
            let w = wide(text);
            AppendMenuW(m, MF_STRING | flags, id, w.as_ptr());
        };
        item(MENU_SETTINGS, 0, "打开设置");
        AppendMenuW(m, MF_SEPARATOR, 0, null());
        let info = if st.connected {
            format!("{}  ·  电量 {}%{}", short_name(st.pid), st.battery, if st.charging { " ⚡" } else { "" })
        } else {
            "未连接鼠标".into()
        };
        item(0, MF_GRAYED, &info);
        item(MENU_PAUSE, if enabled { 0 } else { MF_CHECKED }, "暂停按键映射");
        item(MENU_RELOAD, 0, "重新加载配置");
        AppendMenuW(m, MF_SEPARATOR, 0, null());
        item(MENU_EXIT, 0, "退出");
        SetMenuDefaultItem(m, MENU_SETTINGS as u32, 0);

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd());
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd(), null());
        PostMessageW(hwnd(), WM_NULL, 0, 0);
        DestroyMenu(m);
        match cmd as usize {
            MENU_SETTINGS => open_settings(),
            MENU_PAUSE => press(Button::Middle, &Action::ToggleMapping),
            MENU_RELOAD => reload(true),
            MENU_EXIT => {
                DestroyWindow(hwnd());
            }
            _ => {}
        }
    }
    trim_later();
}

fn reload(notify: bool) {
    match Config::load() {
        Ok(cfg) => {
            with(|a| {
                a.release_all();
                let _ = a.dev_tx.send(DevCmd::Reconfigure {
                    poll_minutes: cfg.battery_poll_minutes,
                    fix_mode: cfg.fix_device_mode,
                });
                a.cfg = cfg;
                a.low_alerted = false;
            });
            tray_update();
            if notify {
                balloon("Razer Lite", "配置已重新加载", false);
            }
        }
        Err(e) => balloon("配置未生效", &e, true),
    }
    trim_later();
}

fn trim_later() {
    unsafe { SetTimer(hwnd(), TIMER_TRIM, 3000, None) };
}

/// 把工作集交还给系统：常驻内存降到最低。被换出的页面需要时会从备用列表快速取回。
fn trim_memory() {
    unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

// ---------------------------------------------------------------- 窗口过程

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_RL_FLUSH => {
            flush();
            0
        }
        WM_TIMER => {
            match wp {
                TIMER_TRIM => {
                    KillTimer(h, TIMER_TRIM);
                    trim_memory();
                }
                i if (TIMER_HOLD..TIMER_HOLD + 3).contains(&i) => {
                    with(|a| a.on_hold_timer(i - TIMER_HOLD));
                    flush();
                }
                _ => {}
            }
            0
        }
        WM_RL_RELOAD => {
            reload(false);
            0
        }
        WM_RL_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => open_settings(),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(),
                _ => {}
            }
            0
        }
        WM_RL_DEVICE => {
            let st = unpack_status(wp, lp);
            let alert = with(|a| {
                a.status = st;
                let th = a.cfg.low_battery_alert;
                if !st.connected || st.charging || st.battery > th.saturating_add(5) {
                    a.low_alerted = false;
                    return None;
                }
                if th > 0 && st.battery <= th && !a.low_alerted {
                    a.low_alerted = true;
                    return Some(st.battery);
                }
                None
            })
            .flatten();
            tray_update();
            if let Some(b) = alert {
                balloon("鼠标电量低", &format!("{} 剩余电量 {}%，记得换电池或充电", short_name(st.pid), b), true);
            }
            trim_later();
            0
        }
        WM_POWERBROADCAST => {
            if wp as u32 == PBT_APMRESUMEAUTOMATIC {
                with(|a| a.dev_tx.send(DevCmd::Reconfigure {
                    poll_minutes: a.cfg.battery_poll_minutes,
                    fix_mode: a.cfg.fix_device_mode,
                }));
            }
            1
        }
        WM_ENDSESSION | WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            with(|a| a.release_all());
            flush();
            let nid = tray_data();
            Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
            0
        }
        _ => {
            if with(|a| a.taskbar_created == msg).unwrap_or(false) {
                // 资源管理器重启后重新添加托盘图标
                with(|a| a.tray_added = false);
                tray_update();
                return 0;
            }
            DefWindowProcW(h, msg, wp, lp)
        }
    }
}

// ---------------------------------------------------------------- 入口

unsafe fn make_icon(size: i32, active: bool) -> HICON {
    let rgba = icon::render(size as u32, active);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size;
    bmi.bmiHeader.biHeight = -size;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = null_mut();
    let dc = GetDC(null_mut());
    let color = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    ReleaseDC(null_mut(), dc);
    if color.is_null() || bits.is_null() {
        return null_mut();
    }
    let px = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
    for (d, s) in px.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
    }
    let mask_bits = vec![0u8; ((size as usize + 15) / 16 * 2) * size as usize];
    let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr() as *const c_void);
    let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let h = CreateIconIndirect(&ii);
    DeleteObject(color);
    DeleteObject(mask);
    h
}

fn attach_console() {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn diag() {
    match device::Device::open() {
        Err(e) => println!("{e}"),
        Ok(d) => {
            println!("型号     {} (PID {:04X})", d.name(), d.pid);
            println!("固件     {}", d.firmware().unwrap_or_default());
            println!("电量     {:?}  充电 {:?}", d.battery(), d.charging());
            println!("DPI      {:?}", d.dpi());
            println!("DPI 档位 {:?}", d.dpi_stages());
            println!("回报率   {:?}", d.polling_rate());
            println!("休眠     {:?} 秒", d.idle_time());
            println!("设备模式 {:?}（0=普通 3=驱动）", d.device_mode());
        }
    }
    println!("官方软件运行中：{}", synapse_running());
    println!("后台运行中：{}  内存：{:?}", agent_running(), agent_memory());
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "--task-install" => std::process::exit(task_install_direct().is_err() as i32),
        "--task-remove" => std::process::exit(task_remove_direct().is_err() as i32),
        "--stop" => {
            stop_agent();
            return;
        }
        "--reload" => {
            notify_reload();
            return;
        }
        "--autostart" => {
            attach_console();
            let mode = match std::env::args().nth(2).as_deref() {
                Some("user") => AutoStart::User,
                Some("admin") => AutoStart::Admin,
                _ => AutoStart::Off,
            };
            match set_autostart(mode) {
                Ok(_) => println!("开机自启：{:?}", autostart_status()),
                Err(e) => println!("设置失败：{e}"),
            }
            return;
        }
        "--diag" => {
            attach_console();
            diag();
            return;
        }
        "--mode-normal" => {
            attach_console();
            match device::Device::open().and_then(|d| d.set_device_mode(0)) {
                Ok(_) => println!("已切回普通模式"),
                Err(e) => println!("{e}"),
            }
            return;
        }
        _ => {}
    }

    unsafe {
        let mutex_name = wide("Local\\RazerLite.Agent.Mutex");
        let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return; // 已在运行
        }
        SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);

        let cfg = Config::load_or_default();
        let hinst = GetModuleHandleW(null());
        let class = wide(AGENT_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        MAIN_HWND.store(h, Ordering::Relaxed);

        // 允许非管理员的设置程序给（可能以管理员运行的）后台发消息
        let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        for m in [WM_RL_RELOAD, WM_CLOSE, taskbar_created] {
            ChangeWindowMessageFilterEx(h, m, MSGFLT_ALLOW, null_mut());
        }

        let (tx, rx) = channel();
        let (pm, fm) = (cfg.battery_poll_minutes, cfg.fix_device_mode);
        std::thread::Builder::new()
            .name("device".into())
            .stack_size(128 * 1024)
            .spawn(move || device_worker(rx, pm, fm))
            .ok();

        let sz = GetSystemMetrics(SM_CXSMICON).max(16);
        AGENT.with(|a| {
            *a.borrow_mut() = Some(Agent {
                cfg,
                states: Default::default(),
                queue: VecDeque::new(),
                dev_tx: tx,
                status: DevStatus::default(),
                low_alerted: false,
                tray_added: false,
                icon_on: make_icon(sz, true),
                icon_off: make_icon(sz, false),
                taskbar_created,
            })
        });
        tray_update();

        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0);
        if hook.is_null() {
            balloon("Razer Lite", "安装鼠标钩子失败，按键映射不可用", true);
        }
        trim_later();

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if !hook.is_null() {
            UnhookWindowsHookEx(hook);
        }
    }
}
