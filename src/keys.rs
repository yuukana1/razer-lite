//! 按键名 ↔ 虚拟键码

const TABLE: &[(&str, u16, &str)] = &[
    // (配置里的名字, VK, 显示名)
    ("ctrl", 0xA2, "Ctrl"),
    ("shift", 0xA0, "Shift"),
    ("alt", 0xA4, "Alt"),
    ("win", 0x5B, "Win"),
    ("rctrl", 0xA3, "右Ctrl"),
    ("rshift", 0xA1, "右Shift"),
    ("ralt", 0xA5, "右Alt"),
    ("enter", 0x0D, "Enter"),
    ("tab", 0x09, "Tab"),
    ("esc", 0x1B, "Esc"),
    ("space", 0x20, "Space"),
    ("backspace", 0x08, "Backspace"),
    ("delete", 0x2E, "Delete"),
    ("insert", 0x2D, "Insert"),
    ("home", 0x24, "Home"),
    ("end", 0x23, "End"),
    ("pageup", 0x21, "PgUp"),
    ("pagedown", 0x22, "PgDn"),
    ("left", 0x25, "←"),
    ("up", 0x26, "↑"),
    ("right", 0x27, "→"),
    ("down", 0x28, "↓"),
    ("capslock", 0x14, "CapsLock"),
    ("printscreen", 0x2C, "PrtSc"),
    ("pause", 0x13, "Pause"),
    ("menu", 0x5D, "菜单键"),
    ("volumeup", 0xAF, "音量+"),
    ("volumedown", 0xAE, "音量−"),
    ("mute", 0xAD, "静音"),
    ("playpause", 0xB3, "播放/暂停"),
    ("nexttrack", 0xB0, "下一首"),
    ("prevtrack", 0xB1, "上一首"),
    ("stop", 0xB2, "停止播放"),
    ("browserback", 0xA6, "浏览器后退"),
    ("browserforward", 0xA7, "浏览器前进"),
    ("browserrefresh", 0xA8, "浏览器刷新"),
    ("browserhome", 0xAC, "浏览器主页"),
    ("minus", 0xBD, "-"),
    ("equal", 0xBB, "="),
    ("comma", 0xBC, ","),
    ("period", 0xBE, "."),
    ("slash", 0xBF, "/"),
    ("backslash", 0xDC, "\\"),
    ("semicolon", 0xBA, ";"),
    ("quote", 0xDE, "'"),
    ("backquote", 0xC0, "`"),
    ("lbracket", 0xDB, "["),
    ("rbracket", 0xDD, "]"),
    ("num0", 0x60, "Num0"),
    ("num1", 0x61, "Num1"),
    ("num2", 0x62, "Num2"),
    ("num3", 0x63, "Num3"),
    ("num4", 0x64, "Num4"),
    ("num5", 0x65, "Num5"),
    ("num6", 0x66, "Num6"),
    ("num7", 0x67, "Num7"),
    ("num8", 0x68, "Num8"),
    ("num9", 0x69, "Num9"),
    ("numadd", 0x6B, "Num+"),
    ("numsub", 0x6D, "Num−"),
    ("nummul", 0x6A, "Num*"),
    ("numdiv", 0x6F, "Num/"),
];

const ALIASES: &[(&str, &str)] = &[
    ("control", "ctrl"),
    ("lctrl", "ctrl"),
    ("lshift", "shift"),
    ("lalt", "alt"),
    ("lwin", "win"),
    ("super", "win"),
    ("cmd", "win"),
    ("return", "enter"),
    ("escape", "esc"),
    ("del", "delete"),
    ("ins", "insert"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("prtsc", "printscreen"),
    ("-", "minus"),
    ("=", "equal"),
    (",", "comma"),
    (".", "period"),
    ("/", "slash"),
    ("\\", "backslash"),
    (";", "semicolon"),
    ("'", "quote"),
    ("`", "backquote"),
    ("[", "lbracket"),
    ("]", "rbracket"),
];

/// 解析单个按键名
pub fn parse_key(name: &str) -> Option<u16> {
    let n = name.trim().to_ascii_lowercase();
    let n = ALIASES.iter().find(|(a, _)| *a == n).map(|(_, b)| b.to_string()).unwrap_or(n);
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        if c.is_ascii_lowercase() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(num) = n.strip_prefix('f').and_then(|r| r.parse::<u16>().ok()) {
        if (1..=24).contains(&num) {
            return Some(0x70 + num - 1);
        }
    }
    TABLE.iter().find(|(k, _, _)| *k == n).map(|(_, vk, _)| *vk)
}

/// 解析组合键 "ctrl+shift+t"
pub fn parse_combo(s: &str) -> Result<Vec<u16>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("还没有设置按键".into());
    }
    // 允许 "ctrl++" 表示 ctrl + '='（加号键）
    let mut parts: Vec<&str> = s.split('+').collect();
    if s.ends_with("++") {
        parts.retain(|p| !p.is_empty());
        parts.push("=");
    }
    let mut out = vec![];
    for p in parts {
        if p.trim().is_empty() {
            return Err(format!("组合键格式不对：{s}"));
        }
        let vk = parse_key(p).ok_or_else(|| format!("不认识的按键：{}", p.trim()))?;
        if !out.contains(&vk) {
            out.push(vk);
        }
    }
    Ok(out)
}

pub fn key_label(vk: u16) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => TABLE.iter().find(|(_, v, _)| *v == vk).map(|(_, _, l)| l.to_string()).unwrap_or(format!("0x{vk:02X}")),
    }
}

pub fn key_name(vk: u16) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => (vk as u8 as char).to_ascii_lowercase().to_string(),
        0x70..=0x87 => format!("f{}", vk - 0x6F),
        _ => TABLE.iter().find(|(_, v, _)| *v == vk).map(|(n, _, _)| n.to_string()).unwrap_or_default(),
    }
}

/// 用于显示："ctrl+shift+t" → "Ctrl + Shift + T"
pub fn pretty(combo: &str) -> String {
    match parse_combo(combo) {
        Ok(v) => v.iter().map(|&k| key_label(k)).collect::<Vec<_>>().join(" + "),
        Err(_) => combo.to_string(),
    }
}

pub fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0xA0..=0xA5 | 0x5B | 0x5C | 0x10 | 0x11 | 0x12)
}

/// 需要 KEYEVENTF_EXTENDEDKEY 的键
pub fn is_extended(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D | 0xA3 | 0xA5 | 0x6F | 0x2C | 0xA6..=0xB7)
}

/// 常用预设：(分组, 名称, 组合键)
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("编辑", "复制", "ctrl+c"),
    ("编辑", "粘贴", "ctrl+v"),
    ("编辑", "剪切", "ctrl+x"),
    ("编辑", "撤销", "ctrl+z"),
    ("编辑", "重做", "ctrl+y"),
    ("编辑", "全选", "ctrl+a"),
    ("编辑", "保存", "ctrl+s"),
    ("编辑", "查找", "ctrl+f"),
    ("浏览", "后退", "browserback"),
    ("浏览", "前进", "browserforward"),
    ("浏览", "刷新", "f5"),
    ("浏览", "新建标签页", "ctrl+t"),
    ("浏览", "关闭标签页", "ctrl+w"),
    ("浏览", "恢复关闭的标签页", "ctrl+shift+t"),
    ("浏览", "下一个标签页", "ctrl+tab"),
    ("浏览", "上一个标签页", "ctrl+shift+tab"),
    ("系统", "显示桌面", "win+d"),
    ("系统", "任务视图", "win+tab"),
    ("系统", "截图", "win+shift+s"),
    ("系统", "锁屏", "win+l"),
    ("系统", "文件资源管理器", "win+e"),
    ("系统", "剪贴板历史", "win+v"),
    ("系统", "关闭窗口", "alt+f4"),
    ("系统", "切换输入法", "win+space"),
    ("系统", "下一个虚拟桌面", "ctrl+win+right"),
    ("系统", "上一个虚拟桌面", "ctrl+win+left"),
    ("媒体", "播放 / 暂停", "playpause"),
    ("媒体", "下一首", "nexttrack"),
    ("媒体", "上一首", "prevtrack"),
    ("媒体", "音量 +", "volumeup"),
    ("媒体", "音量 −", "volumedown"),
    ("媒体", "静音", "mute"),
];

pub fn preset_name(combo: &str) -> Option<&'static str> {
    let norm = |s: &str| parse_combo(s).ok();
    let v = norm(combo)?;
    PRESETS.iter().find(|(_, _, c)| norm(c).as_ref() == Some(&v)).map(|(_, n, _)| *n)
}
