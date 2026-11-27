//! Razer Lite 设置界面。只在需要配置时打开，关闭即退出，不常驻内存。

#![windows_subsystem = "windows"]

mod firmware;
mod raster;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ vec2, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
};
use rlite::config::{Action, Button, Config, MouseButton, Profile};
use rlite::device::Device;
use rlite::sys::{self, AutoStart, MemInfo};
use rlite::{icon, keys};

// ---------------------------------------------------------------- 配色

const ACCENT: Color32 = Color32::from_rgb(0x44, 0xD6, 0x2C);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x2A, 0x6E, 0x22);
const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x15);
const SIDEBAR: Color32 = Color32::from_rgb(0x15, 0x17, 0x1B);
const CARD: Color32 = Color32::from_rgb(0x1B, 0x1E, 0x23);
const CARD_HI: Color32 = Color32::from_rgb(0x23, 0x27, 0x2D);
const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x35);
const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xED);
const TEXT_DIM: Color32 = Color32::from_rgb(0x8E, 0x94, 0x9E);
const WARN: Color32 = Color32::from_rgb(0xF5, 0xA5, 0x24);
const DANGER: Color32 = Color32::from_rgb(0xEF, 0x53, 0x50);

fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

// ---------------------------------------------------------------- 字体：内存映射系统字体，只有用到的字形页才进内存

fn map_font(path: &str) -> Option<&'static [u8]> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Memory::*;
    unsafe {
        let w = sys::wide(path);
        let f = CreateFileW(
            w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if f == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut size: i64 = 0;
        GetFileSizeEx(f, &mut size);
        let m = CreateFileMappingW(f, std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
        CloseHandle(f);
        if m.is_null() {
            return None;
        }
        let v = MapViewOfFile(m, FILE_MAP_READ, 0, 0, 0);
        CloseHandle(m);
        if v.Value.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(v.Value as *const u8, size as usize))
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let font = |name: &str| map_font(&format!("{windir}\\Fonts\\{name}"));
    let regular = font("msyh.ttc").or_else(|| font("simhei.ttf")).or_else(|| font("simsun.ttc"));
    let heavy = font("msyhbd.ttc").or(regular);
    if let Some(r) = regular {
        fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_static(r)));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
        fonts.families.get_mut(&FontFamily::Monospace).unwrap().push("cjk".into());
    }
    let mut bold_family = vec![];
    if let Some(b) = heavy {
        fonts.font_data.insert("cjk-bold".into(), Arc::new(FontData::from_static(b)));
        bold_family.push("cjk-bold".into());
    }
    bold_family.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(bold(), bold_family);
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        use egui::TextStyle::*;
        s.text_styles = [
            (Small, FontId::proportional(12.0)),
            (Body, FontId::proportional(14.0)),
            (Button, FontId::proportional(14.0)),
            (Heading, FontId::new(22.0, bold())),
            (Monospace, FontId::monospace(13.0)),
        ]
        .into();
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.button_padding = vec2(14.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.combo_width = 220.0;
        s.spacing.slider_width = 260.0;

        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x18);
        v.faint_bg_color = CARD_HI;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.hyperlink_color = ACCENT;
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0_f32, BORDER);
        v.slider_trailing_fill = true;
        let r = CornerRadius::same(8);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        v.widgets.inactive.bg_fill = CARD_HI;
        v.widgets.inactive.weak_bg_fill = CARD_HI;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2C, 0x31, 0x38);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2C, 0x31, 0x38);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_DIM);
        v.widgets.active.bg_fill = ACCENT_DIM;
        v.widgets.active.weak_bg_fill = ACCENT_DIM;
        v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        v.widgets.open.weak_bg_fill = CARD_HI;
    });
}

// ---------------------------------------------------------------- 小组件

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn section_title(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.label(RichText::new(title).family(bold()).size(15.0));
    if !sub.is_empty() {
        ui.label(RichText::new(sub).size(12.5).color(TEXT_DIM));
    }
    ui.add_space(6.0);
}

/// 开关
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = lerp_color(Color32::from_rgb(0x3A, 0x3F, 0x47), ACCENT, t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), bg);
    let cx = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    p.circle_filled(Pos2::new(cx, rect.center().y), 8.0, Color32::WHITE);
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// 带说明的开关行
fn toggle_row(ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 60.0);
            ui.label(RichText::new(title).size(14.0));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(12.0).color(TEXT_DIM));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = toggle(ui, on).changed();
        });
    });
    changed
}

/// 分段选择器
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(Color32::from_rgb(0x13, 0x15, 0x19))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, label) in options {
                    let sel = *value == *v;
                    let text = RichText::new(*label).color(if sel { Color32::BLACK } else { TEXT_DIM });
                    let b = egui::Button::new(text)
                        .fill(if sel { ACCENT } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(7))
                        .min_size(vec2(64.0, 28.0));
                    if ui.add(b).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::BLACK).family(bold()))
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .min_size(vec2(96.0, 32.0)),
    )
}

fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
}

// ---------------------------------------------------------------- 设备状态

#[derive(Default)]
struct DevInfo {
    pid: u16,
    fw: (u8, u8),
    name: String,
    firmware: String,
    serial: String,
    battery: Option<u8>,
    charging: bool,
    stages: Vec<u16>,
    active: u8,
    polling: u16,
    idle: u16,
    mode: Option<u8>,
    /// 用户正在编辑的值
    edit_stages: Vec<u16>,
    edit_active: u8,
    edit_polling: u16,
    edit_idle_min: u16,
}

impl DevInfo {
    fn dirty(&self) -> bool {
        self.edit_stages != self.stages
            || self.edit_active != self.active
            || self.edit_polling != self.polling
            || self.edit_idle_min * 60 != self.idle
    }
    fn reset_edits(&mut self) {
        self.edit_stages = self.stages.clone();
        self.edit_active = self.active;
        self.edit_polling = self.polling;
        self.edit_idle_min = (self.idle / 60).clamp(1, 15);
    }
}

// ---------------------------------------------------------------- App

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Buttons,
    Performance,
    General,
    About,
}

#[derive(PartialEq, Clone, Copy)]
enum ActionKind {
    Inherit,
    Default,
    Disabled,
    Keys,
    Text,
    Mouse,
    Run,
    DpiClutch,
    DpiCycle,
    Toggle,
}

impl ActionKind {
    fn of(a: &Action) -> Self {
        match a {
            Action::Inherit => Self::Inherit,
            Action::Default => Self::Default,
            Action::Disabled => Self::Disabled,
            Action::Keys { .. } => Self::Keys,
            Action::Text { .. } => Self::Text,
            Action::Mouse { .. } => Self::Mouse,
            Action::Run { .. } => Self::Run,
            Action::DpiClutch { .. } => Self::DpiClutch,
            Action::DpiCycle { .. } => Self::DpiCycle,
            Action::ToggleMapping => Self::Toggle,
        }
    }
    fn make(self) -> Action {
        match self {
            Self::Inherit => Action::Inherit,
            Self::Default => Action::Default,
            Self::Disabled => Action::Disabled,
            Self::Keys => Action::Keys { combo: String::new() },
            Self::Text => Action::Text { text: String::new() },
            Self::Mouse => Action::Mouse { button: MouseButton::Left },
            Self::Run => Action::Run { target: String::new(), args: String::new() },
            Self::DpiClutch => Action::DpiClutch { dpi: 400 },
            Self::DpiCycle => Action::DpiCycle { step: 1 },
            Self::Toggle => Action::ToggleMapping,
        }
    }
    const LIST: [ActionKind; 10] = [
        Self::Inherit,
        Self::Default,
        Self::Keys,
        Self::Mouse,
        Self::Text,
        Self::Run,
        Self::DpiClutch,
        Self::DpiCycle,
        Self::Toggle,
        Self::Disabled,
    ];
}

struct Toast {
    text: String,
    ok: bool,
    at: Instant,
}

struct App {
    cfg: Config,
    saved: Config,
    load_error: Option<String>,
    page: Page,
    profile: usize,
    button: Button,
    /// 正在录制快捷键：Some(false)=单击动作, Some(true)=长按动作
    recording: Option<bool>,
    dev: Option<Device>,
    info: DevInfo,
    dev_error: Option<String>,
    last_refresh: Instant,
    autostart: AutoStart,
    agent_mem: Option<MemInfo>,
    synapse: bool,
    running_apps: Vec<String>,
    new_app: String,
    rename_buf: String,
    toast: Option<Toast>,
    logo: Option<TextureHandle>,
    fw: FwUi,
}

// ---------------------------------------------------------------- 固件更新状态

enum FwMsg {
    Checked(Result<firmware::Remote, String>),
    Downloaded(Result<PathBuf, String>),
    Finished,
    Drivers(Vec<String>),
    Cleaned(Result<(), String>),
}

#[derive(PartialEq, Clone, Copy)]
enum FwState {
    Idle,
    Checking,
    Downloading,
    Running,
    Ask,
    Cleaning,
}

struct FwUi {
    state: FwState,
    remote: Option<firmware::Remote>,
    error: Option<String>,
    /// (已下载字节, 总字节)
    progress: Arc<(AtomicU64, AtomicU64)>,
    tx: Sender<FwMsg>,
    rx: Receiver<FwMsg>,
    agent_was_running: bool,
    /// 驱动库里的雷蛇驱动包；None = 还没检查
    drivers: Option<Vec<String>>,
    drivers_checking: bool,
}

impl FwUi {
    fn new() -> Self {
        let (tx, rx) = channel();
        FwUi {
            state: FwState::Idle,
            remote: None,
            error: None,
            progress: Arc::new((AtomicU64::new(0), AtomicU64::new(0))),
            tx,
            rx,
            agent_was_running: false,
            drivers: None,
            drivers_checking: false,
        }
    }

    fn busy(&self) -> bool {
        !matches!(self.state, FwState::Idle | FwState::Ask) || self.drivers_checking
    }
}

fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (op, u) = (sys::wide("open"), sys::wide(url));
    unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        setup_fonts(ctx);
        setup_style(ctx);
        let (cfg, load_error) = match Config::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        let logo = icon::render(64, true);
        let logo = ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], &logo),
            egui::TextureOptions::LINEAR,
        );
        let mut app = App {
            saved: cfg.clone(),
            cfg,
            load_error,
            page: Page::Buttons,
            profile: 0,
            button: Button::SideFront,
            recording: None,
            dev: None,
            info: DevInfo::default(),
            dev_error: None,
            last_refresh: Instant::now(),
            autostart: sys::autostart_status(),
            agent_mem: sys::agent_memory(),
            synapse: sys::synapse_running(),
            running_apps: vec![],
            new_app: String::new(),
            rename_buf: String::new(),
            toast: None,
            logo: Some(logo),
            fw: FwUi::new(),
        };
        app.connect();
        // 命令行参数 --page xxx：直接打开指定页面
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--page") {
            app.page = match args.get(i + 1).map(String::as_str) {
                Some("perf") => Page::Performance,
                Some("general") => Page::General,
                Some("about") => Page::About,
                _ => Page::Buttons,
            };
        }
        app
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some(Toast { text: text.into(), ok, at: Instant::now() });
    }

    // ------------------------------------------------ 设备

    fn connect(&mut self) {
        self.dev = None;
        match Device::open() {
            Ok(d) => {
                self.dev = Some(d);
                self.dev_error = None;
                self.read_device(true);
            }
            Err(e) => self.dev_error = Some(e.to_string()),
        }
    }

    fn read_device(&mut self, full: bool) {
        let Some(d) = &self.dev else { return };
        let i = &mut self.info;
        match d.battery() {
            Ok(b) => i.battery = Some(b),
            Err(e) => {
                self.dev_error = Some(e.to_string());
                if let rlite::device::Error::Io(_) = e {
                    self.dev = None;
                }
                return;
            }
        }
        i.charging = d.charging().unwrap_or(false);
        let was_clean = !i.dirty();
        if let Ok((active, st)) = d.dpi_stages() {
            i.active = active;
            i.stages = st.iter().map(|s| s.0).collect();
        }
        if full {
            i.name = d.name().to_string();
            i.pid = d.pid;
            i.fw = d.firmware_raw().unwrap_or((0, 0));
            i.firmware = d.firmware().unwrap_or_default();
            i.serial = d.serial().unwrap_or_default();
            i.polling = d.polling_rate().unwrap_or(0);
            i.idle = d.idle_time().unwrap_or(300);
            i.mode = d.device_mode().ok();
        }
        if full || was_clean {
            i.reset_edits();
        }
        self.dev_error = None;
    }

    fn apply_device(&mut self) {
        let Some(d) = &self.dev else { return };
        let i = &self.info;
        let stages: Vec<(u16, u16)> = i.edit_stages.iter().map(|&v| (v, v)).collect();
        let r = (|| {
            if i.edit_stages != i.stages || i.edit_active != i.active {
                d.set_dpi_stages(i.edit_active, &stages)?;
                let v = i.edit_stages[(i.edit_active as usize).saturating_sub(1).min(stages.len() - 1)];
                d.set_dpi(v, v, true)?;
            }
            if i.edit_polling != i.polling && i.edit_polling > 0 {
                d.set_polling_rate(i.edit_polling)?;
            }
            if i.edit_idle_min * 60 != i.idle {
                d.set_idle_time(i.edit_idle_min * 60)?;
            }
            Ok::<(), rlite::device::Error>(())
        })();
        match r {
            Ok(_) => {
                self.read_device(true);
                self.toast("已写入鼠标板载内存", true);
            }
            Err(e) => self.toast(format!("写入失败：{e}"), false),
        }
    }

    // ------------------------------------------------ 配置

    fn save(&mut self) {
        match self.cfg.save() {
            Ok(_) => {
                self.saved = self.cfg.clone();
                if sys::notify_reload() {
                    self.toast("已保存，立即生效", true);
                } else {
                    self.toast("已保存。后台服务未运行，启动后生效", true);
                }
            }
            Err(e) => self.toast(format!("保存失败：{e}"), false),
        }
    }

    // ------------------------------------------------ 页面：按键映射

    fn page_buttons(&mut self, ui: &mut egui::Ui) {
        self.profile_bar(ui);
        ui.add_space(12.0);
        let avail = ui.available_width();
        let left_w = 360.0_f32.min(avail * 0.45);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(left_w);
                card(ui, |ui| self.mouse_diagram(ui));
            });
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                card(ui, |ui| self.binding_editor(ui));
            });
        });
    }

    fn profile_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("方案").color(TEXT_DIM));
            let mut remove = None;
            for (idx, p) in self.cfg.profiles.iter().enumerate() {
                let sel = idx == self.profile;
                let label = if idx == 0 { "全局（所有程序）".to_string() } else { p.name.clone() };
                let text = RichText::new(label).color(if sel { Color32::BLACK } else { TEXT });
                let b = egui::Button::new(text)
                    .fill(if sel { ACCENT } else { CARD_HI })
                    .stroke(Stroke::new(1.0_f32, if sel { ACCENT } else { BORDER }))
                    .corner_radius(CornerRadius::same(15));
                let r = ui.add(b);
                if r.clicked() {
                    self.profile = idx;
                    self.rename_buf = p.name.clone();
                    self.recording = None;
                }
                if idx > 0 {
                    r.context_menu(|ui| {
                        if ui.button("删除这个方案").clicked() {
                            remove = Some(idx);
                            ui.close();
                        }
                    });
                }
            }
            if let Some(i) = remove {
                self.cfg.profiles.remove(i);
                self.profile = 0;
            }
            if ui
                .add(egui::Button::new(RichText::new("＋ 程序专属方案").color(ACCENT)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0_f32, ACCENT_DIM)).corner_radius(CornerRadius::same(15)))
                .on_hover_text("为指定程序（例如游戏）单独设置按键；没设置的按键跟随全局方案")
                .clicked()
            {
                let n = self.cfg.profiles.len();
                self.cfg.profiles.push(Profile::new_app_profile(format!("方案 {n}")));
                self.profile = n;
                self.rename_buf = format!("方案 {n}");
                self.running_apps = sys::running_apps();
            }
        });

        if self.profile > 0 {
            ui.add_space(8.0);
            card(ui, |ui| self.app_profile_editor(ui));
        }
    }

    fn app_profile_editor(&mut self, ui: &mut egui::Ui) {
        let idx = self.profile;
        ui.horizontal(|ui| {
            ui.label("方案名称");
            if ui.add(egui::TextEdit::singleline(&mut self.rename_buf).desired_width(180.0)).changed() {
                self.cfg.profiles[idx].name = self.rename_buf.trim().to_string();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("删除方案").color(DANGER))).clicked() {
                    self.cfg.profiles.remove(idx);
                    self.profile = 0;
                }
            });
        });
        if self.profile != idx {
            return;
        }
        ui.add_space(4.0);
        ui.label(RichText::new("当这些程序在前台时使用本方案：").size(12.5).color(TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            let mut remove = None;
            for (i, a) in self.cfg.profiles[idx].apps.iter().enumerate() {
                egui::Frame::new().fill(CARD_HI).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(a);
                        if ui.add(egui::Button::new("×").frame(false).small()).clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                self.cfg.profiles[idx].apps.remove(i);
            }
            if self.cfg.profiles[idx].apps.is_empty() {
                ui.label(RichText::new("（还没有添加程序）").color(WARN).size(12.5));
            }
        });
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.new_app).hint_text("输入 exe 名，如 cs2.exe").desired_width(200.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("添加").clicked() || enter {
                let mut n = self.new_app.trim().to_lowercase();
                if !n.is_empty() {
                    if !n.ends_with(".exe") {
                        n.push_str(".exe");
                    }
                    if !self.cfg.profiles[idx].apps.contains(&n) {
                        self.cfg.profiles[idx].apps.push(n);
                    }
                }
                self.new_app.clear();
            }
            let mut pick = None;
            egui::ComboBox::from_id_salt("running")
                .selected_text("从正在运行的程序中选择")
                .width(220.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    if self.running_apps.is_empty() {
                        self.running_apps = sys::running_apps();
                    }
                    for a in &self.running_apps {
                        if ui.selectable_label(false, a).clicked() {
                            pick = Some(a.clone());
                        }
                    }
                });
            if let Some(a) = pick {
                if !self.cfg.profiles[idx].apps.contains(&a) {
                    self.cfg.profiles[idx].apps.push(a);
                }
            }
            if ui.small_button("刷新").clicked() {
                self.running_apps = sys::running_apps();
            }
        });
    }

    /// 左侧：鼠标示意图 + 三个可点的按键
    fn mouse_diagram(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "选择按键", "点击要修改的按键");
        let w = ui.available_width();
        let h = 300.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let p = ui.painter_at(rect);

        // 鼠标本体（右侧）
        let mx = rect.right() - 80.0;
        let body = Rect::from_center_size(Pos2::new(mx, rect.top() + 150.0), vec2(104.0, 250.0));
        p.rect_filled(body, CornerRadius { nw: 52, ne: 52, sw: 48, se: 48 }, Color32::from_rgb(0x22, 0x25, 0x2B));
        p.rect_stroke(body, CornerRadius { nw: 52, ne: 52, sw: 48, se: 48 }, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        // 左右键分割线
        p.line_segment([Pos2::new(mx, body.top() + 2.0), Pos2::new(mx, body.top() + 100.0)], Stroke::new(1.5_f32, Color32::from_rgb(0x12, 0x14, 0x18)));
        p.line_segment(
            [Pos2::new(body.left() + 6.0, body.top() + 104.0), Pos2::new(body.right() - 6.0, body.top() + 104.0)],
            Stroke::new(1.0_f32, Color32::from_rgb(0x2E, 0x32, 0x39)),
        );
        // 雷蛇 logo 位置的小点缀
        p.circle_filled(Pos2::new(mx, body.bottom() - 60.0), 5.0, ACCENT_DIM);

        let hot = |b: Button| -> Rect {
            match b {
                Button::Middle => Rect::from_center_size(Pos2::new(mx, body.top() + 42.0), vec2(14.0, 30.0)),
                Button::SideFront => Rect::from_min_size(Pos2::new(body.left() - 5.0, body.top() + 112.0), vec2(8.0, 30.0)),
                Button::SideBack => Rect::from_min_size(Pos2::new(body.left() - 5.0, body.top() + 148.0), vec2(8.0, 30.0)),
            }
        };
        let label_y = |b: Button| match b {
            Button::Middle => rect.top() + 40.0,
            Button::SideFront => rect.top() + 125.0,
            Button::SideBack => rect.top() + 210.0,
        };

        let profile = &self.cfg.profiles[self.profile];
        for b in [Button::Middle, Button::SideFront, Button::SideBack] {
            let sel = self.button == b;
            let hr = hot(b);
            let col = if sel { ACCENT } else { Color32::from_rgb(0x55, 0x5B, 0x65) };
            p.rect_filled(hr, CornerRadius::same(4), col);

            let card_rect = Rect::from_min_size(Pos2::new(rect.left(), label_y(b) - 30.0), vec2((mx - 52.0 - rect.left() - 34.0).max(120.0), 60.0));
            let resp = ui.interact(card_rect.union(hr.expand(6.0)), ui.id().with(("btn", b as u8)), Sense::click());
            let hovered = resp.hovered();
            p.rect_filled(card_rect, CornerRadius::same(10), if sel { Color32::from_rgb(0x1E, 0x2A, 0x1E) } else if hovered { CARD_HI } else { Color32::from_rgb(0x17, 0x1A, 0x1E) });
            p.rect_stroke(card_rect, CornerRadius::same(10), Stroke::new(1.0_f32, if sel { ACCENT } else { BORDER }), StrokeKind::Inside);
            // 连接线
            let a = Pos2::new(card_rect.right(), card_rect.center().y);
            let bpt = Pos2::new(hr.center().x, hr.center().y);
            let mid = Pos2::new(a.x + 14.0, a.y);
            p.line_segment([a, mid], Stroke::new(1.0_f32, col));
            p.line_segment([mid, bpt], Stroke::new(1.0_f32, col));
            p.circle_filled(bpt, 3.0, col);

            let bd = profile.binding(b);
            let short = |a: &Action| match a {
                Action::Keys { combo } => keys::preset_name(combo).map(str::to_string).unwrap_or_else(|| keys::pretty(combo)),
                a => a.summary(),
            };
            let (summary, inherit) = if bd.tap == Action::Inherit {
                (short(&self.cfg.profiles[0].binding(b).tap), true)
            } else {
                (short(&bd.tap), false)
            };
            p.text(Pos2::new(card_rect.left() + 12.0, card_rect.top() + 18.0), Align2::LEFT_CENTER, b.label(), FontId::new(13.0, bold()), if sel { ACCENT } else { TEXT_DIM });
            let mut s = summary;
            if inherit {
                s = format!("{s} · 跟随全局");
            }
            if bd.hold.is_some() {
                s += " · 长按另设";
            }
            let max_chars = ((card_rect.width() - 24.0) / 13.0) as usize;
            if s.chars().count() > max_chars {
                s = s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…";
            }
            p.text(Pos2::new(card_rect.left() + 12.0, card_rect.top() + 41.0), Align2::LEFT_CENTER, s, FontId::proportional(14.0), TEXT);
            if resp.clicked() {
                self.button = b;
                self.recording = None;
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("DPI 键由鼠标硬件处理（循环切换档位），可在「灵敏度」页设置每一档的数值。").size(12.0).color(TEXT_DIM));
    }

    /// 右侧：当前按键的动作编辑
    fn binding_editor(&mut self, ui: &mut egui::Ui) {
        let b = self.button;
        let is_global = self.profile == 0;
        ui.horizontal(|ui| {
            ui.label(RichText::new(b.label()).family(bold()).size(18.0));
            pill(ui, b.hint(), TEXT_DIM);
        });
        ui.add_space(10.0);

        let mut bd = self.cfg.profiles[self.profile].binding(b).clone();
        ui.label(RichText::new(if bd.hold.is_some() { "单击时" } else { "按下时" }).family(bold()));
        let rec = self.recording;
        let mut start_rec = None;
        action_editor(ui, &mut bd.tap, is_global, false, rec == Some(false), &mut start_rec, "tap");

        if bd.tap != Action::Inherit {
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            let mut has_hold = bd.hold.is_some();
            if toggle_row(ui, &mut has_hold, "长按执行另一个动作", "短按执行上面的动作，按住超过设定时间执行下面的动作") {
                bd.hold = if has_hold { Some(Action::Keys { combo: String::new() }) } else { None };
            }
            if let Some(hold) = bd.hold.as_mut() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("长按判定时间");
                    ui.add(egui::Slider::new(&mut bd.hold_ms, 150..=1500).step_by(50.0).suffix(" 毫秒"));
                });
                ui.add_space(6.0);
                ui.label(RichText::new("长按时").family(bold()));
                action_editor(ui, hold, false, true, rec == Some(true), &mut start_rec, "hold");
            }
        }
        if let Some(r) = start_rec {
            self.recording = r;
        }
        // 录制快捷键
        if let Some(for_hold) = self.recording {
            if let Some(combo) = capture_combo(ui.ctx()) {
                let target = if for_hold { bd.hold.as_mut() } else { Some(&mut bd.tap) };
                if let Some(Action::Keys { combo: c }) = target {
                    if combo != "esc" || c.is_empty() {
                        *c = combo;
                    }
                }
                self.recording = None;
            }
        }
        *self.cfg.profiles[self.profile].binding_mut(b) = bd;
    }

    // ------------------------------------------------ 页面：灵敏度

    fn page_performance(&mut self, ui: &mut egui::Ui) {
        if self.dev.is_none() {
            card(ui, |ui| {
                ui.label(RichText::new("没有连接到鼠标").family(bold()).size(16.0));
                ui.label(RichText::new(self.dev_error.clone().unwrap_or_default()).color(TEXT_DIM));
                ui.label(RichText::new("请确认接收器已插好、鼠标已开机，动一下鼠标唤醒后再试。").color(TEXT_DIM));
                ui.add_space(8.0);
                if primary_button(ui, "重新检测").clicked() {
                    self.connect();
                }
            });
            return;
        }
        let i = &mut self.info;
        card(ui, |ui| {
            section_title(ui, "DPI 档位", "按鼠标底部 / 顶部的 DPI 键会在这些档位之间循环。点圆点设为当前档位。");
            let n = i.edit_stages.len();
            let mut remove = None;
            for k in 0..n {
                ui.horizontal(|ui| {
                    let active = i.edit_active as usize == k + 1;
                    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    let p = ui.painter();
                    p.circle_stroke(r.center(), 8.0, Stroke::new(1.5_f32, if active { ACCENT } else { TEXT_DIM }));
                    if active {
                        p.circle_filled(r.center(), 4.5, ACCENT);
                    }
                    if resp.on_hover_text("设为当前档位").clicked() {
                        i.edit_active = k as u8 + 1;
                    }
                    ui.label(RichText::new(format!("档位 {}", k + 1)).color(if active { ACCENT } else { TEXT }));
                    let v = &mut i.edit_stages[k];
                    ui.add(egui::Slider::new(v, 100..=30000).logarithmic(true).step_by(50.0).show_value(false));
                    ui.add(egui::DragValue::new(v).range(100..=30000).speed(10.0).suffix(" DPI"));
                    for q in [400u16, 800, 1600, 3200] {
                        if ui.small_button(q.to_string()).clicked() {
                            *v = q;
                        }
                    }
                    if n > 1 && ui.add(egui::Button::new(RichText::new("×").color(TEXT_DIM)).frame(false)).on_hover_text("删除档位").clicked() {
                        remove = Some(k);
                    }
                });
            }
            if let Some(k) = remove {
                i.edit_stages.remove(k);
                let n = i.edit_stages.len() as u8;
                if i.edit_active > n {
                    i.edit_active = n;
                }
            }
            if i.edit_stages.len() < 5 && ui.button("＋ 添加档位").clicked() {
                let last = *i.edit_stages.last().unwrap_or(&800);
                i.edit_stages.push((last.saturating_mul(2)).min(30000));
            }
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "回报率", "鼠标每秒向电脑报告位置的次数。越高越跟手，耗电也越多。");
            segmented(ui, &mut i.edit_polling, &[(125, "125 Hz"), (500, "500 Hz"), (1000, "1000 Hz")]);
            ui.add_space(14.0);
            section_title(ui, "自动休眠", "无操作多久后鼠标进入休眠以省电。");
            ui.add(egui::Slider::new(&mut i.edit_idle_min, 1..=15).suffix(" 分钟"));
        });
        ui.add_space(12.0);
        let dirty = i.dirty();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(dirty, |ui| {
                if primary_button(ui, "写入鼠标").clicked() {
                    self.apply_device();
                }
                if ui.button("撤销修改").clicked() {
                    self.info.reset_edits();
                }
            });
            if ui.button("从鼠标重新读取").clicked() {
                self.read_device(true);
                self.info.reset_edits();
            }
            ui.label(RichText::new("设置保存在鼠标板载内存里，换电脑、不开本软件也有效").size(12.0).color(TEXT_DIM));
        });
    }

    // ------------------------------------------------ 页面：通用设置

    fn page_general(&mut self, ui: &mut egui::Ui) {
        if self.synapse {
            egui::Frame::new()
                .fill(WARN.gamma_multiply(0.12))
                .stroke(Stroke::new(1.0_f32, WARN.gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("雷蛇官方软件（Synapse）仍在运行").family(bold()).color(WARN));
                    ui.label("两边同时改侧键会重复触发（比如粘贴两次）。确认 Razer Lite 好用后，可以在「设置 → 应用 → 已安装的应用」里卸载 Razer Synapse 和 Razer App Engine。");
                });
            ui.add_space(12.0);
        }

        card(ui, |ui| {
            section_title(ui, "后台服务", "负责按键映射和电量提醒的常驻小程序（razer-lite.exe）。");
            ui.horizontal(|ui| {
                match self.agent_mem {
                    Some(m) => {
                        pill(ui, "● 运行中", ACCENT);
                        ui.label(RichText::new(format!(
                            "内存 {:.1} MB（专用 {:.1} MB）",
                            m.working_set as f64 / 1048576.0,
                            m.private as f64 / 1048576.0
                        ))
                        .color(TEXT_DIM));
                    }
                    None => pill(ui, "○ 未运行", WARN),
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.agent_mem.is_some() {
                        if ui.button("停止").clicked() {
                            sys::stop_agent();
                            std::thread::sleep(Duration::from_millis(300));
                            self.agent_mem = sys::agent_memory();
                        }
                        if ui.button("重启").clicked() {
                            sys::stop_agent();
                            std::thread::sleep(Duration::from_millis(400));
                            sys::start_agent();
                            std::thread::sleep(Duration::from_millis(400));
                            self.agent_mem = sys::agent_memory();
                        }
                    } else if primary_button(ui, "启动").clicked() {
                        sys::start_agent();
                        std::thread::sleep(Duration::from_millis(500));
                        self.agent_mem = sys::agent_memory();
                    }
                });
            });
            ui.add_space(12.0);
            ui.label(RichText::new("开机自启").family(bold()));
            let mut mode = self.autostart;
            if segmented(ui, &mut mode, &[(AutoStart::Off, "关闭"), (AutoStart::User, "开机自启"), (AutoStart::Admin, "开机自启（管理员）")]) {
                match sys::set_autostart(mode) {
                    Ok(_) => {
                        self.toast("开机自启已更新", true);
                    }
                    Err(e) => self.toast(e, false),
                }
                self.autostart = sys::autostart_status();
            }
            ui.label(
                RichText::new(match self.autostart {
                    AutoStart::Off => "不随系统启动，需要时手动运行 razer-lite.exe。",
                    AutoStart::User => "登录 Windows 后自动在后台运行。注意：在以管理员身份运行的窗口里侧键映射不生效（系统安全限制）。",
                    AutoStart::Admin => "通过计划任务以管理员权限启动，管理员窗口里也能用。设置时会弹一次 UAC 确认。",
                })
                .size(12.0)
                .color(TEXT_DIM),
            );
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "常规", "");
            toggle_row(ui, &mut self.cfg.enabled, "启用按键映射", "关闭后所有按键恢复系统默认功能");
            ui.add_space(4.0);
            toggle_row(ui, &mut self.cfg.tray_icon, "显示托盘图标", "隐藏后可以直接运行 razer-lite-settings.exe 打开设置");
            ui.add_space(4.0);
            toggle_row(
                ui,
                &mut self.cfg.fix_device_mode,
                "自动修复设备模式",
                "雷蛇软件退出后鼠标可能停在「驱动模式」导致侧键无反应，后台启动时自动切回普通模式",
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label("低电量提醒");
                ui.add(egui::Slider::new(&mut self.cfg.low_battery_alert, 0..=50).suffix(" %"));
                ui.label(RichText::new(if self.cfg.low_battery_alert == 0 { "已关闭" } else { "低于该电量时弹出通知" }).size(12.0).color(TEXT_DIM));
            });
            ui.horizontal(|ui| {
                ui.label("电量检查间隔");
                ui.add(egui::Slider::new(&mut self.cfg.battery_poll_minutes, 1..=60).suffix(" 分钟"));
            });
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            section_title(ui, "工具", "");
            ui.horizontal(|ui| {
                if ui.button("打开配置文件夹").clicked() {
                    let _ = std::fs::create_dir_all(rlite::config::dir());
                    let _ = std::process::Command::new("explorer.exe").arg(rlite::config::dir()).spawn();
                }
                let mode = self.info.mode;
                if self.dev.is_some() && ui.button("把鼠标切回普通模式").on_hover_text("相当于官方软件退出时做的复位").clicked() {
                    if let Some(d) = &self.dev {
                        match d.set_device_mode(0) {
                            Ok(_) => {
                                self.info.mode = Some(0);
                                self.toast("已切回普通模式", true);
                            }
                            Err(e) => self.toast(e.to_string(), false),
                        }
                    }
                }
                if let Some(m) = mode {
                    ui.label(RichText::new(format!("当前：{}", if m == 0 { "普通模式" } else if m == 3 { "驱动模式（官方软件接管中）" } else { "其他" })).size(12.0).color(TEXT_DIM));
                }
            });
        });
    }

    fn page_about(&mut self, ui: &mut egui::Ui) {
        if self.dev.is_some() {
            card(ui, |ui| {
                section_title(ui, "设备信息", "");
                egui::Grid::new("devinfo").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    let i = &self.info;
                    for (k, v) in [
                        ("型号", i.name.clone()),
                        ("固件", i.firmware.clone()),
                        ("序列号", i.serial.clone()),
                        ("当前 DPI", i.stages.get((i.active as usize).saturating_sub(1)).map(|v| v.to_string()).unwrap_or_default()),
                    ] {
                        ui.label(RichText::new(k).color(TEXT_DIM));
                        ui.label(v);
                        ui.end_row();
                    }
                });
            });
        }
        if self.info.pid != 0 {
            if self.dev.is_some() {
                ui.add_space(12.0);
            }
            card(ui, |ui| self.firmware_card(ui));
            ui.add_space(12.0);
        }
        card(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(t) = &self.logo {
                    ui.add(egui::Image::new((t.id(), vec2(48.0, 48.0))));
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("Razer Lite").family(bold()).size(20.0));
                    ui.label(RichText::new(format!("版本 {}", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                });
            });
            ui.add_space(10.0);
            ui.label("一个完全离线的雷蛇鼠标轻量工具，用来替代官方软件里最常用的功能：侧键映射、DPI 档位、回报率、休眠时间和电量提醒。");
            ui.add_space(6.0);
            ui.label(RichText::new("· 不需要账号、不依赖任何雷蛇组件，直接通过 Windows 自带的 HID 接口与鼠标通信").color(TEXT_DIM));
            ui.label(RichText::new("· 平时完全离线，只有手动检查固件更新时才访问雷蛇官方下载地址").color(TEXT_DIM));
            ui.label(RichText::new("· 后台程序只有一个钩子和一个托盘图标，常驻内存约 1 MB").color(TEXT_DIM));
            ui.label(RichText::new("· 本设置界面关闭后进程完全退出，不占内存").color(TEXT_DIM));
            ui.label(RichText::new("· 通信协议参考开源项目 OpenRazer").color(TEXT_DIM));
            ui.add_space(10.0);
            ui.label(RichText::new(format!("配置文件：{}", rlite::config::path().display())).size(12.0).color(TEXT_DIM));
        });
    }

    // ------------------------------------------------ 固件更新

    fn firmware_card(&mut self, ui: &mut egui::Ui) {
        let busy = self.fw.busy();
        let known = firmware::known(self.info.pid);
        ui.horizontal(|ui| {
            ui.label(RichText::new("固件更新").family(bold()).size(15.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if known.is_some() && ui.add_enabled(!busy && self.dev.is_some(), egui::Button::new("检查更新")).clicked() {
                    self.fw_check();
                }
                if self.fw.state == FwState::Checking {
                    ui.add(egui::Spinner::new().size(16.0));
                }
            });
        });
        ui.label(RichText::new("只有点按钮时才联网。固件由雷蛇官方更新程序刷写，下载后先校验雷蛇的数字签名再运行。").size(12.0).color(TEXT_DIM));
        ui.add_space(8.0);

        let Some(k) = known else {
            ui.label("这个型号没有内置的固件信息，可以到雷蛇官方固件页面查找。");
            if ui.button("打开雷蛇官方固件页面").clicked() {
                open_url(firmware::GENERAL_PAGE);
            }
            self.driver_residue(ui);
            return;
        };

        let official = self.fw.remote.as_ref().map(|r| r.version.clone()).unwrap_or_else(|| k.version.to_string());
        let installed = self.cfg.firmware_installed.get(&self.info.serial).cloned();
        let verdict = firmware::verdict(self.info.fw, &official, installed.as_deref());
        egui::Grid::new("fw").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("鼠标当前固件").color(TEXT_DIM));
            let mut cur = format!("v{}.{:02}", self.info.fw.0, self.info.fw.1);
            if let Some(v) = &installed {
                cur += &format!("（已用官方工具更新到 v{v}）");
            }
            ui.label(cur);
            ui.end_row();
            ui.label(RichText::new("官方最新").color(TEXT_DIM));
            ui.horizontal(|ui| {
                ui.label(format!("v{official}"));
                let note = match &self.fw.remote {
                    Some(r) if r.size > 0 => format!("已联网确认 · 更新程序 {:.1} MB", r.size as f64 / 1048576.0),
                    Some(_) => "已联网确认".to_string(),
                    None => format!("内置记录（{} 发布），点「检查更新」联网确认", k.date),
                };
                ui.label(RichText::new(note).size(12.0).color(TEXT_DIM));
            });
            ui.end_row();
            ui.label(RichText::new("状态").color(TEXT_DIM));
            match verdict {
                firmware::Verdict::Available => pill(ui, "有新固件", WARN),
                firmware::Verdict::Maybe => pill(ui, "可能需要更新", WARN),
                firmware::Verdict::UpToDate => pill(ui, "已是最新", ACCENT),
            }
            ui.end_row();
        });
        if verdict == firmware::Verdict::Maybe {
            ui.label(
                RichText::new("鼠标只报告前两位版本号，看不出是否已经装了第三位的小版本。如果没用官方工具更新过，建议更新一次；更新后这里会记住。")
                    .size(12.0)
                    .color(TEXT_DIM),
            );
        }
        if let Some(e) = &self.fw.error {
            ui.label(RichText::new(e).color(DANGER));
        }
        ui.add_space(8.0);

        match self.fw.state {
            FwState::Downloading => {
                let (got, total) = (self.fw.progress.0.load(Ordering::Relaxed), self.fw.progress.1.load(Ordering::Relaxed));
                let frac = if total > 0 { got as f32 / total as f32 } else { 0.0 };
                let text = if total > 0 && got >= total {
                    "正在校验数字签名…".to_string()
                } else {
                    format!("正在下载官方更新程序 {:.1} / {:.1} MB", got as f64 / 1048576.0, total as f64 / 1048576.0)
                };
                ui.add(egui::ProgressBar::new(frac).text(text).fill(ACCENT_DIM));
            }
            FwState::Running => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(RichText::new("官方更新程序运行中").family(bold()));
                });
                ui.label("按更新程序的提示操作：先插好接收器更新接收器，再用数据线连接鼠标更新鼠标。完成后关闭更新程序，这里会自动恢复后台服务。");
                ui.label(RichText::new("更新期间按键映射暂停。不要拔接收器或数据线，笔记本请插电。").size(12.0).color(WARN));
            }
            FwState::Ask => {
                ui.label(RichText::new("更新程序已关闭。固件更新成功了吗？").family(bold()));
                ui.horizontal(|ui| {
                    if primary_button(ui, "成功了").clicked() {
                        self.fw_record(official.clone());
                        self.fw.state = FwState::Idle;
                    }
                    if ui.button("没有更新 / 失败了").clicked() {
                        self.fw.state = FwState::Idle;
                    }
                });
            }
            FwState::Cleaning => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label("正在清理雷蛇驱动…");
                });
            }
            FwState::Idle | FwState::Checking => {
                ui.horizontal(|ui| {
                    let label = "下载并运行官方更新程序";
                    let enabled = !busy && self.dev.is_some();
                    let r = if verdict != firmware::Verdict::UpToDate {
                        ui.add_enabled_ui(enabled, |ui| primary_button(ui, label)).inner
                    } else {
                        ui.add_enabled(enabled, egui::Button::new(label))
                    };
                    if r.clicked() {
                        self.fw_update();
                    }
                    if ui.button("打开官方页面").on_hover_text(k.page).clicked() {
                        open_url(k.page);
                    }
                });
                ui.label(
                    RichText::new("官方页面有浏览器验证，程序没法自动发现比内置记录更新的版本；想确认的话点「打开官方页面」看一眼。")
                        .size(11.5)
                        .color(TEXT_DIM),
                );
            }
        }
        self.driver_residue(ui);
    }

    fn driver_residue(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);
        if self.fw.drivers.is_none() && !self.fw.drivers_checking {
            self.fw_check_drivers();
        }
        match &self.fw.drivers {
            None => {
                ui.label(RichText::new("正在检查雷蛇驱动残留…").size(12.5).color(TEXT_DIM));
            }
            Some(v) if v.is_empty() => {
                ui.label(RichText::new("没有雷蛇驱动残留，鼠标用的是 Windows 自带驱动").size(12.5).color(TEXT_DIM));
            }
            Some(v) => {
                let text = format!("检测到 {} 个雷蛇驱动包（{}），通常是官方更新程序装的。", v.len(), v.join("、"));
                let can = !self.fw.busy();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).size(12.5).color(WARN));
                    if ui.add_enabled(can, egui::Button::new("清理（需要管理员）")).clicked() {
                        self.fw_clean_drivers();
                    }
                });
            }
        }
    }

    fn fw_check(&mut self) {
        let pid = self.info.pid;
        self.fw.state = FwState::Checking;
        self.fw.error = None;
        let tx = self.fw.tx.clone();
        std::thread::spawn(move || {
            let r = firmware::known(pid).ok_or_else(|| "没有这个型号的固件信息".to_string()).and_then(|k| firmware::check(&k));
            let _ = tx.send(FwMsg::Checked(r));
        });
    }

    fn fw_update(&mut self) {
        let pid = self.info.pid;
        self.fw.error = None;
        self.fw.state = FwState::Downloading;
        self.fw.progress.0.store(0, Ordering::Relaxed);
        self.fw.progress.1.store(0, Ordering::Relaxed);
        let (tx, progress, remote) = (self.fw.tx.clone(), self.fw.progress.clone(), self.fw.remote.clone());
        std::thread::spawn(move || {
            let r = (|| {
                let remote = match remote {
                    Some(r) => r,
                    None => firmware::check(&firmware::known(pid).ok_or("没有这个型号的固件信息")?)?,
                };
                let path = firmware::download(&remote, progress)?;
                if let Err(e) = firmware::verify_signature(&path) {
                    let _ = std::fs::remove_dir_all(firmware::download_dir());
                    return Err(e);
                }
                Ok(path)
            })();
            let _ = tx.send(FwMsg::Downloaded(r));
        });
    }

    /// 暂停后台、释放设备句柄，避免和官方工具抢设备
    fn release_for_tool(&mut self) {
        self.fw.agent_was_running = sys::agent_running();
        if self.fw.agent_was_running {
            sys::stop_agent();
            std::thread::sleep(Duration::from_millis(500));
        }
        self.dev = None;
    }

    fn resume_after_tool(&mut self) {
        if self.fw.agent_was_running && !sys::agent_running() {
            sys::start_agent();
        }
        std::thread::sleep(Duration::from_millis(800));
        self.connect();
        self.agent_mem = sys::agent_memory();
    }

    fn fw_check_drivers(&mut self) {
        self.fw.drivers_checking = true;
        let tx = self.fw.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(FwMsg::Drivers(firmware::razer_driver_packages()));
        });
    }

    fn fw_clean_drivers(&mut self) {
        let Some(v) = self.fw.drivers.clone() else { return };
        self.release_for_tool();
        self.fw.state = FwState::Cleaning;
        let tx = self.fw.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(FwMsg::Cleaned(firmware::remove_driver_packages(&v)));
        });
    }

    fn fw_record(&mut self, ver: String) {
        let serial = self.info.serial.clone();
        if serial.is_empty() {
            return;
        }
        let mut disk = Config::load().unwrap_or_else(|_| self.saved.clone());
        disk.firmware_installed.insert(serial, ver);
        match disk.save() {
            Ok(_) => {
                self.saved.firmware_installed = disk.firmware_installed.clone();
                self.cfg.firmware_installed = disk.firmware_installed;
                sys::notify_reload();
                self.toast("已记录固件版本", true);
            }
            Err(e) => self.toast(format!("记录失败：{e}"), false),
        }
    }

    fn on_fw(&mut self, m: FwMsg) {
        match m {
            FwMsg::Checked(r) => {
                self.fw.state = FwState::Idle;
                match r {
                    Ok(remote) => {
                        self.toast(format!("官方最新固件 v{}", remote.version), true);
                        self.fw.remote = Some(remote);
                    }
                    Err(e) => self.fw.error = Some(format!("检查失败：{e}")),
                }
            }
            FwMsg::Downloaded(Err(e)) => {
                self.fw.state = FwState::Idle;
                self.fw.error = Some(e);
            }
            FwMsg::Downloaded(Ok(path)) => {
                self.release_for_tool();
                match firmware::launch(&path) {
                    Ok(l) => {
                        self.fw.state = FwState::Running;
                        let tx = self.fw.tx.clone();
                        std::thread::spawn(move || {
                            firmware::wait(l);
                            let _ = tx.send(FwMsg::Finished);
                        });
                    }
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(firmware::download_dir());
                        self.resume_after_tool();
                        self.fw.state = FwState::Idle;
                        self.fw.error = Some(e);
                    }
                }
            }
            FwMsg::Finished => {
                let _ = std::fs::remove_dir_all(firmware::download_dir());
                self.resume_after_tool();
                self.fw.state = FwState::Ask;
                self.fw_check_drivers();
            }
            FwMsg::Drivers(v) => {
                self.fw.drivers_checking = false;
                self.fw.drivers = Some(v);
            }
            FwMsg::Cleaned(r) => {
                self.resume_after_tool();
                self.fw.state = FwState::Idle;
                match r {
                    Ok(_) => self.toast("已清理雷蛇驱动", true),
                    Err(e) => self.toast(e, false),
                }
                self.fw_check_drivers();
            }
        }
    }

    // ------------------------------------------------ 侧栏

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if let Some(t) = &self.logo {
                ui.add(egui::Image::new((t.id(), vec2(30.0, 30.0))));
            }
            ui.label(RichText::new("Razer Lite").family(bold()).size(18.0));
        });
        ui.add_space(22.0);
        for (p, label, sub) in [
            (Page::Buttons, "按键映射", "侧键 / 滚轮"),
            (Page::Performance, "灵敏度", "DPI · 回报率 · 休眠"),
            (Page::General, "通用设置", "自启 · 托盘 · 电量"),
            (Page::About, "关于", "设备信息 · 固件更新"),
        ] {
            let sel = self.page == p;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if sub.is_empty() { 40.0 } else { 52.0 }), Sense::click());
            let painter = ui.painter();
            if sel || resp.hovered() {
                painter.rect_filled(rect.shrink2(vec2(4.0, 2.0)), CornerRadius::same(10), if sel { CARD_HI } else { CARD });
            }
            if sel {
                painter.rect_filled(Rect::from_min_size(rect.left_top() + vec2(4.0, 12.0), vec2(3.0, rect.height() - 24.0)), CornerRadius::same(2), ACCENT);
            }
            let tx = rect.left() + 20.0;
            if sub.is_empty() {
                painter.text(Pos2::new(tx, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(14.5, bold()), if sel { TEXT } else { TEXT_DIM });
            } else {
                painter.text(Pos2::new(tx, rect.top() + 17.0), Align2::LEFT_CENTER, label, FontId::new(14.5, bold()), if sel { TEXT } else { TEXT_DIM });
                painter.text(Pos2::new(tx, rect.top() + 36.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.5), TEXT_DIM);
            }
            if resp.clicked() {
                self.page = p;
                self.recording = None;
                if p == Page::General {
                    self.agent_mem = sys::agent_memory();
                    self.synapse = sys::synapse_running();
                    self.autostart = sys::autostart_status();
                }
            }
        }

        // 底部设备卡片
        let card_h = if self.dev.is_some() || self.dev_error.as_deref().is_some_and(|e| e.contains("休眠")) { 98.0 } else { 74.0 };
        ui.add_space((ui.available_height() - card_h - 16.0).max(10.0));
        {
            egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(12)).stroke(Stroke::new(1.0_f32, BORDER)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if self.dev.is_some() {
                    let short = self.info.name.trim_start_matches("Razer ").to_string();
                    ui.label(RichText::new(short).family(bold()).size(13.0));
                    if let Some(b) = self.info.battery {
                        let col = if b <= self.cfg.low_battery_alert.max(10) { DANGER } else if b < 30 { WARN } else { ACCENT };
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width() - 48.0, 8.0), Sense::hover());
                            ui.painter().rect_filled(r, CornerRadius::same(4), Color32::from_rgb(0x2E, 0x32, 0x39));
                            let mut f = r;
                            f.set_width(r.width() * b as f32 / 100.0);
                            ui.painter().rect_filled(f, CornerRadius::same(4), col);
                            ui.label(RichText::new(format!("{b}%")).size(12.5).color(col));
                        });
                        let dpi = self.info.stages.get((self.info.active as usize).saturating_sub(1)).copied().unwrap_or(0);
                        ui.label(RichText::new(format!("{}{} DPI · {} Hz", if self.info.charging { "充电中 · " } else { "" }, dpi, self.info.polling)).size(11.5).color(TEXT_DIM));
                    }
                } else {
                    let asleep = self.dev_error.as_deref().is_some_and(|e| e.contains("休眠"));
                    ui.label(RichText::new(if asleep { "鼠标休眠中" } else { "未连接鼠标" }).color(TEXT_DIM));
                    if asleep {
                        ui.label(RichText::new("动一下鼠标，会自动重新连接").size(11.5).color(TEXT_DIM));
                    }
                    if ui.small_button("重新检测").clicked() {
                        self.connect();
                    }
                }
                });
            });
        }
    }
}

// ---------------------------------------------------------------- 动作编辑器

fn action_editor(
    ui: &mut egui::Ui,
    a: &mut Action,
    is_global: bool,
    is_hold: bool,
    recording: bool,
    start_rec: &mut Option<Option<bool>>,
    id: &str,
) {
    let mut kind = ActionKind::of(a);
    let label_of = |k: ActionKind| match k {
        ActionKind::Default if is_hold => "原始功能（模拟原按键）",
        _ => k.make().kind_label(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("动作").color(TEXT_DIM));
        egui::ComboBox::from_id_salt(("kind", id)).selected_text(label_of(kind)).width(240.0).show_ui(ui, |ui| {
            for k in ActionKind::LIST {
                if k == ActionKind::Inherit && (is_global || is_hold) {
                    continue;
                }
                ui.selectable_value(&mut kind, k, label_of(k));
            }
        });
    });
    if kind != ActionKind::of(a) {
        *a = kind.make();
    }
    ui.add_space(4.0);
    match a {
        Action::Keys { combo } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("常用").color(TEXT_DIM));
                let current = keys::preset_name(combo).unwrap_or("自定义");
                egui::ComboBox::from_id_salt(("preset", id)).selected_text(current).width(240.0).height(360.0).show_ui(ui, |ui| {
                    let mut group = "";
                    for (g, name, c) in keys::PRESETS {
                        if *g != group {
                            group = g;
                            ui.label(RichText::new(*g).size(11.5).color(ACCENT));
                        }
                        let sel = keys::preset_name(combo) == Some(name);
                        if ui.selectable_label(sel, format!("{name}    {}", keys::pretty(c))).clicked() {
                            *combo = c.to_string();
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("按键").color(TEXT_DIM));
                ui.add(egui::TextEdit::singleline(combo).hint_text("如 ctrl+shift+t").desired_width(160.0).font(egui::TextStyle::Monospace));
                if recording {
                    let t = ui.ctx().input(|i| i.time);
                    let c = if (t * 2.0) as i64 % 2 == 0 { ACCENT } else { ACCENT_DIM };
                    if ui.add(egui::Button::new(RichText::new("● 请按下组合键…（Esc 取消）").color(c))).clicked() {
                        *start_rec = Some(None);
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                } else if ui.button("● 录制").on_hover_text("点一下后在键盘上按下想要的组合键").clicked() {
                    *start_rec = Some(Some(is_hold));
                }
            });
            match keys::parse_combo(combo) {
                Ok(v) => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, k) in v.iter().enumerate() {
                            if i > 0 {
                                ui.label(RichText::new("+").color(TEXT_DIM));
                            }
                            egui::Frame::new()
                                .fill(CARD_HI)
                                .stroke(Stroke::new(1.0_f32, BORDER))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(8, 3))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(keys::key_label(*k)).family(bold()).size(13.0));
                                });
                        }
                    });
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(if combo.is_empty() { TEXT_DIM } else { DANGER }).size(12.5));
                }
            }
            ui.label(
                RichText::new("支持 ctrl / shift / alt / win、a-z、0-9、f1-f24、方向键、媒体键等，用 + 连接。Win 组合键请手动输入或选常用。")
                    .size(11.5)
                    .color(TEXT_DIM),
            );
        }
        Action::Text { text } => {
            ui.add(egui::TextEdit::multiline(text).hint_text("按下按键时自动输入这段文字（支持中文、换行）").desired_rows(3).desired_width(f32::INFINITY));
        }
        Action::Mouse { button } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("模拟").color(TEXT_DIM));
                egui::ComboBox::from_id_salt(("mouse", id)).selected_text(button.label()).width(240.0).show_ui(ui, |ui| {
                    for m in MouseButton::ALL {
                        ui.selectable_value(button, m, m.label());
                    }
                });
            });
        }
        Action::Run { target, args } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("目标").color(TEXT_DIM));
                ui.add(egui::TextEdit::singleline(target).hint_text("程序路径、文件夹或网址，如 notepad.exe / https://…").desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("参数").color(TEXT_DIM));
                ui.add(egui::TextEdit::singleline(args).hint_text("可选").desired_width(f32::INFINITY));
            });
        }
        Action::DpiClutch { dpi } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("按住时 DPI").color(TEXT_DIM));
                ui.add(egui::Slider::new(dpi, 100..=6400).step_by(50.0).suffix(" DPI"));
            });
            ui.label(RichText::new("按住按键临时切到这个 DPI（适合狙击时精细瞄准），松开恢复，不写入板载内存。").size(12.0).color(TEXT_DIM));
        }
        Action::DpiCycle { step } => {
            segmented(ui, step, &[(1, "下一档"), (-1, "上一档")]);
        }
        Action::Default => {
            ui.label(RichText::new(if is_hold { "模拟这个按键原本的功能。" } else { "不做修改，保持系统默认（侧键为前进 / 后退，滚轮为中键）。" }).size(12.5).color(TEXT_DIM));
        }
        Action::Disabled => {
            ui.label(RichText::new("按下时什么都不做。").size(12.5).color(TEXT_DIM));
        }
        Action::Inherit => {
            ui.label(RichText::new("使用全局方案里这个按键的设置。").size(12.5).color(TEXT_DIM));
        }
        Action::ToggleMapping => {
            ui.label(RichText::new("一键暂停 / 恢复所有按键映射（例如进游戏前临时关闭）。").size(12.5).color(TEXT_DIM));
        }
    }
}

fn egui_key_name(k: egui::Key) -> Option<String> {
    let n = k.name();
    let mapped = match n {
        "Escape" => "esc",
        "Equals" | "Plus" => "equal",
        "OpenBracket" => "lbracket",
        "CloseBracket" => "rbracket",
        "Backtick" => "backquote",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        other => other,
    }
    .to_ascii_lowercase();
    keys::parse_key(&mapped).map(|vk| keys::key_name(vk))
}

/// 读取本帧的按键事件，返回组合键字符串
fn capture_combo(ctx: &egui::Context) -> Option<String> {
    let events = ctx.input(|i| i.events.clone());
    for e in events {
        let (mods, key) = match e {
            egui::Event::Copy => return Some("ctrl+c".into()),
            egui::Event::Cut => return Some("ctrl+x".into()),
            egui::Event::Paste(_) => return Some("ctrl+v".into()),
            egui::Event::Key { key, pressed: true, modifiers, .. } => (modifiers, key),
            _ => continue,
        };
        let name = egui_key_name(key)?;
        let mut parts = vec![];
        if mods.ctrl {
            parts.push("ctrl".to_string());
        }
        if mods.shift {
            parts.push("shift".to_string());
        }
        if mods.alt {
            parts.push("alt".to_string());
        }
        if name == "esc" && parts.is_empty() {
            return Some("esc".into());
        }
        parts.push(name);
        return Some(parts.join("+"));
    }
    None
}

// ---------------------------------------------------------------- 主循环

impl App {
    fn ui(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.fw.rx.try_recv() {
            self.on_fw(m);
        }
        if self.fw.busy() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        // 定时刷新电量 / 当前档位
        if self.last_refresh.elapsed() > Duration::from_secs(15) {
            self.last_refresh = Instant::now();
            if self.dev.is_some() {
                self.read_device(false);
            } else if !matches!(self.fw.state, FwState::Running | FwState::Cleaning | FwState::Downloading) {
                // 鼠标休眠或拔掉接收器后自动重连
                self.connect();
            }
            self.agent_mem = sys::agent_memory();
        }
        ctx.request_repaint_after(Duration::from_secs(15));

        egui::SidePanel::left("nav")
            .exact_width(220.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin::symmetric(12, 0)))
            .show(ctx, |ui| self.sidebar(ui));

        let dirty = self.cfg != self.saved;
        if dirty {
            egui::TopBottomPanel::bottom("savebar")
                .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin::symmetric(24, 12)).stroke(Stroke::new(1.0_f32, BORDER)))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(WARN));
                        ui.label("有未保存的修改");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if primary_button(ui, "保存并应用").clicked() {
                                self.save();
                            }
                            if ui.button("放弃修改").clicked() {
                                self.cfg = self.saved.clone();
                                self.profile = self.profile.min(self.cfg.profiles.len() - 1);
                                self.recording = None;
                            }
                        });
                    });
                });
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(Margin { left: 28, right: 28, top: 22, bottom: 0 })).show(ctx, |ui| {
            let (title, sub) = match self.page {
                Page::Buttons => ("按键映射", "给侧键和滚轮单击指定功能，保存后立即生效。可以为不同程序设置不同方案。"),
                Page::Performance => ("灵敏度", "直接读写鼠标的硬件设置。"),
                Page::General => ("通用设置", ""),
                Page::About => ("关于", ""),
            };
            ui.heading(title);
            if !sub.is_empty() {
                ui.label(RichText::new(sub).color(TEXT_DIM));
            }
            if let Some(e) = &self.load_error {
                ui.label(RichText::new(format!("{e}（已使用默认配置，保存后会覆盖原文件）")).color(DANGER));
            }
            ui.add_space(14.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match self.page {
                    Page::Buttons => self.page_buttons(ui),
                    Page::Performance => self.page_performance(ui),
                    Page::General => self.page_general(ui),
                    Page::About => self.page_about(ui),
                }
                ui.add_space(24.0);
            });
        });

        // 提示条
        if let Some(t) = &self.toast {
            let age = t.at.elapsed().as_secs_f32();
            if age > 3.5 {
                self.toast = None;
            } else {
                let (text, ok) = (t.text.clone(), t.ok);
                egui::Area::new("toast".into()).anchor(Align2::CENTER_BOTTOM, vec2(110.0, -70.0)).order(egui::Order::Foreground).show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x22, 0x26, 0x2C))
                        .stroke(Stroke::new(1.0_f32, if ok { ACCENT } else { DANGER }))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("{}  {text}", if ok { "✓" } else { "×" })).color(if ok { ACCENT } else { DANGER }));
                        });
                });
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
    }
}

// ---------------------------------------------------------------- 窗口 + 软件渲染

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct Runner {
    ctx: egui::Context,
    window: Option<Rc<Window>>,
    _sb: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<Surface>,
    state: Option<egui_winit::State>,
    app: Option<App>,
    painter: raster::Painter,
    next_repaint: Option<Instant>,
    shown: bool,
}

impl Runner {
    fn paint(&mut self) {
        let (Some(w), Some(surface), Some(state), Some(app)) =
            (self.window.clone(), self.surface.as_mut(), self.state.as_mut(), self.app.as_mut())
        else {
            return;
        };
        let raw = state.take_egui_input(&w);
        let out = self.ctx.run(raw, |ctx| app.ui(ctx));
        state.handle_platform_output(&w, out.platform_output);

        let size = w.inner_size();
        let (Some(wd), Some(ht)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        self.painter.update_textures(&out.textures_delta);
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        if surface.resize(wd, ht).is_ok() {
            if let Ok(mut buf) = surface.buffer_mut() {
                self.painter.paint(&mut buf, size.width as usize, size.height as usize, out.pixels_per_point, &prims, [BG.r(), BG.g(), BG.b()]);
                let _ = buf.present();
            }
        }
        self.painter.free_textures(&out.textures_delta);
        if !self.shown {
            self.shown = true;
            w.set_visible(true);
            w.focus_window();
        }
        self.next_repaint = None;
        if let Some(vo) = out.viewport_output.get(&egui::ViewportId::ROOT) {
            if vo.repaint_delay.is_zero() {
                w.request_redraw();
            } else if vo.repaint_delay < Duration::from_secs(3600) {
                self.next_repaint = Some(Instant::now() + vo.repaint_delay);
            }
        }
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let icon = winit::window::Icon::from_rgba(icon::render(64, true), 64, 64).ok();
        let attrs = Window::default_attributes()
            .with_title(sys::SETTINGS_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(1040.0, 700.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(900.0, 600.0))
            .with_window_icon(icon)
            .with_visible(false);
        let w = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(_) => {
                el.exit();
                return;
            }
        };
        let sb = softbuffer::Context::new(w.clone()).expect("softbuffer");
        let surface = softbuffer::Surface::new(&sb, w.clone()).expect("surface");
        let state = egui_winit::State::new(self.ctx.clone(), egui::ViewportId::ROOT, &w, Some(w.scale_factor() as f32), None, Some(8192));
        self.app = Some(App::new(&self.ctx));
        self.state = Some(state);
        self.surface = Some(surface);
        self._sb = Some(sb);
        self.window = Some(w.clone());
        // 隐藏窗口收不到重绘消息，先主动画第一帧再显示，避免白屏闪烁
        self.paint();
        w.request_redraw();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(w), Some(state)) = (self.window.clone(), self.state.as_mut()) else { return };
        let resp = state.on_window_event(&w, &event);
        match event {
            WindowEvent::CloseRequested => {
                // 官方更新程序运行期间后台是暂停的，不能直接关掉窗口，否则后台不会自动恢复
                if let Some(app) = self.app.as_mut() {
                    if matches!(app.fw.state, FwState::Running | FwState::Cleaning | FwState::Downloading) {
                        app.toast("固件更新进行中，请先完成并关闭官方更新程序", false);
                        w.request_redraw();
                        return;
                    }
                }
                el.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.paint();
                return;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => w.request_redraw(),
            _ => {}
        }
        if resp.repaint {
            w.request_redraw();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.next_repaint {
            Some(t) if Instant::now() >= t => {
                self.next_repaint = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn main() {
    // 只允许一个设置窗口
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let t = sys::wide(sys::SETTINGS_TITLE);
        let w = FindWindowW(std::ptr::null(), t.as_ptr());
        if !w.is_null() {
            ShowWindow(w, SW_RESTORE);
            SetForegroundWindow(w);
            return;
        }
    }
    let Ok(el) = EventLoop::new() else { return };
    let mut runner = Runner {
        ctx: egui::Context::default(),
        window: None,
        _sb: None,
        surface: None,
        state: None,
        app: None,
        painter: raster::Painter::default(),
        next_repaint: None,
        shown: false,
    };
    let _ = el.run_app(&mut runner);
}
