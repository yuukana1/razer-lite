//! 配置文件：%APPDATA%\RazerLite\config.toml

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    DoubleClick,
    WheelUp,
    WheelDown,
}

impl MouseButton {
    pub const ALL: [MouseButton; 8] = [
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::DoubleClick,
        Self::Back,
        Self::Forward,
        Self::WheelUp,
        Self::WheelDown,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "左键",
            Self::Right => "右键",
            Self::Middle => "中键",
            Self::Back => "后退键（XButton1）",
            Self::Forward => "前进键（XButton2）",
            Self::DoubleClick => "左键双击",
            Self::WheelUp => "滚轮向上一格",
            Self::WheelDown => "滚轮向下一格",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 跟随全局方案（仅用于程序专属方案）
    Inherit,
    /// 保持按键原本功能
    Default,
    /// 屏蔽按键
    Disabled,
    /// 快捷键 / 组合键，如 "ctrl+c"
    Keys { combo: String },
    /// 输入一段文字
    Text { text: String },
    /// 模拟鼠标操作
    Mouse { button: MouseButton },
    /// 打开程序、文件、文件夹或网址
    Run {
        target: String,
        #[serde(default)]
        args: String,
    },
    /// 按住时临时切换到指定 DPI，松开恢复（狙击键）
    DpiClutch { dpi: u16 },
    /// 切换 DPI 档位：step = 1 下一档，-1 上一档
    DpiCycle { step: i8 },
    /// 暂停 / 恢复全部映射
    ToggleMapping,
}

impl Action {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Action::Inherit => "跟随全局",
            Action::Default => "原始功能",
            Action::Disabled => "禁用",
            Action::Keys { .. } => "快捷键",
            Action::Text { .. } => "输入文字",
            Action::Mouse { .. } => "鼠标操作",
            Action::Run { .. } => "打开程序 / 网址",
            Action::DpiClutch { .. } => "DPI 离合（按住降速）",
            Action::DpiCycle { .. } => "切换 DPI 档位",
            Action::ToggleMapping => "暂停 / 恢复映射",
        }
    }

    /// 简短描述，用于界面显示
    pub fn summary(&self) -> String {
        match self {
            Action::Keys { combo } => match crate::keys::preset_name(combo) {
                Some(n) => format!("{n}（{}）", crate::keys::pretty(combo)),
                None => crate::keys::pretty(combo),
            },
            Action::Text { text } => format!("输入「{}」", text.chars().take(12).collect::<String>()),
            Action::Mouse { button } => button.label().to_string(),
            Action::Run { target, .. } => {
                let name = target.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(target);
                format!("打开 {name}")
            }
            Action::DpiClutch { dpi } => format!("按住 {dpi} DPI"),
            Action::DpiCycle { step } => if *step > 0 { "下一档 DPI".into() } else { "上一档 DPI".into() },
            a => a.kind_label().to_string(),
        }
    }
}

fn default_hold_ms() -> u32 {
    400
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Binding {
    /// 单击（或无长按动作时按住）执行的动作
    pub tap: Action,
    /// 长按动作（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<Action>,
    #[serde(default = "default_hold_ms")]
    pub hold_ms: u32,
}

impl Binding {
    pub fn new(tap: Action) -> Self {
        Binding { tap, hold: None, hold_ms: default_hold_ms() }
    }
}

/// 可自定义的鼠标按键
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    SideBack = 0,
    SideFront = 1,
    Middle = 2,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::SideFront, Button::SideBack, Button::Middle];
    pub fn label(self) -> &'static str {
        match self {
            Button::SideFront => "前侧键",
            Button::SideBack => "后侧键",
            Button::Middle => "滚轮单击",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Button::SideFront => "靠近指尖的侧键，系统默认是「前进」",
            Button::SideBack => "靠近手腕的侧键，系统默认是「后退」",
            Button::Middle => "按下滚轮，系统默认是「中键」",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    /// 生效的程序（exe 文件名，不区分大小写）。全局方案为空。
    #[serde(default)]
    pub apps: Vec<String>,
    pub side_front: Binding,
    pub side_back: Binding,
    pub middle: Binding,
}

impl Profile {
    pub fn binding(&self, b: Button) -> &Binding {
        match b {
            Button::SideBack => &self.side_back,
            Button::SideFront => &self.side_front,
            Button::Middle => &self.middle,
        }
    }
    pub fn binding_mut(&mut self, b: Button) -> &mut Binding {
        match b {
            Button::SideBack => &mut self.side_back,
            Button::SideFront => &mut self.side_front,
            Button::Middle => &mut self.middle,
        }
    }
    pub fn new_app_profile(name: String) -> Self {
        Profile {
            name,
            apps: vec![],
            side_front: Binding::new(Action::Inherit),
            side_back: Binding::new(Action::Inherit),
            middle: Binding::new(Action::Inherit),
        }
    }
}

fn yes() -> bool {
    true
}
fn d_low_battery() -> u8 {
    15
}
fn d_poll() -> u32 {
    10
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    /// 总开关
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 显示托盘图标
    #[serde(default = "yes")]
    pub tray_icon: bool,
    /// 电量低于该百分比时弹出提醒，0 = 关闭
    #[serde(default = "d_low_battery")]
    pub low_battery_alert: u8,
    /// 电量刷新间隔（分钟）
    #[serde(default = "d_poll")]
    pub battery_poll_minutes: u32,
    /// 启动时如果鼠标停留在雷蛇驱动模式，自动切回普通模式
    #[serde(default = "yes")]
    pub fix_device_mode: bool,
    /// 通过官方更新程序装过的固件版本（序列号 → 版本），用于判断是否已是最新
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub firmware_installed: BTreeMap<String, String>,
    /// 第一个是全局方案，后面是程序专属方案
    pub profiles: Vec<Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            tray_icon: true,
            low_battery_alert: 15,
            battery_poll_minutes: 10,
            fix_device_mode: true,
            firmware_installed: BTreeMap::new(),
            profiles: vec![Profile {
                name: "全局".into(),
                apps: vec![],
                side_front: Binding::new(Action::Keys { combo: "ctrl+v".into() }),
                side_back: Binding::new(Action::Keys { combo: "ctrl+c".into() }),
                middle: Binding::new(Action::Default),
            }],
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("RazerLite")
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

const HEADER: &str = "# Razer Lite 配置文件。建议用设置界面修改；手动修改后在托盘菜单点「重新加载配置」。\n\n";

impl Config {
    /// 读取配置；文件不存在时写入默认配置。解析失败返回错误信息。
    pub fn load() -> Result<Config, String> {
        let p = path();
        match std::fs::read_to_string(&p) {
            Ok(s) => {
                let mut c: Config = toml::from_str(&s).map_err(|e| format!("配置文件格式错误：{e}"))?;
                c.normalize();
                Ok(c)
            }
            Err(_) => {
                let c = Config::default();
                let _ = c.save();
                Ok(c)
            }
        }
    }

    pub fn load_or_default() -> Config {
        Config::load().unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let _ = std::fs::create_dir_all(dir());
        let s = toml::to_string(self).map_err(|e| e.to_string())?;
        let tmp = path().with_extension("toml.tmp");
        std::fs::write(&tmp, format!("{HEADER}{s}")).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path()).map_err(|e| e.to_string())
    }

    fn normalize(&mut self) {
        if self.profiles.is_empty() {
            self.profiles = Config::default().profiles;
        }
        // 全局方案不能“跟随全局”
        let g = &mut self.profiles[0];
        g.apps.clear();
        for b in Button::ALL {
            let bd = g.binding_mut(b);
            if bd.tap == Action::Inherit {
                bd.tap = Action::Default;
            }
        }
        for p in &mut self.profiles {
            for a in &mut p.apps {
                *a = a.trim().to_lowercase();
            }
            p.apps.retain(|a| !a.is_empty());
        }
    }

    /// 根据前台程序找到某个按键的绑定
    pub fn resolve(&self, button: Button, exe: Option<&str>) -> &Binding {
        let global = self.profiles[0].binding(button);
        if let Some(exe) = exe {
            for p in &self.profiles[1..] {
                if p.apps.iter().any(|a| a == exe) {
                    let b = p.binding(button);
                    return if b.tap == Action::Inherit { global } else { b };
                }
            }
        }
        global
    }

    pub fn has_app_profiles(&self) -> bool {
        self.profiles.len() > 1
    }
}
