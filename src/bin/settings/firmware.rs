//! 固件更新：检查官方固件、下载官方更新程序、校验雷蛇签名并运行。
//!
//! 只在设置窗口里点按钮时才执行。网络组件 wininet.dll 用 LoadLibrary 按需加载，
//! 平时不进内存；后台常驻程序完全不涉及这部分代码。
//! 刷写固件由雷蛇官方更新程序完成（协议不公开，自己实现有把鼠标刷坏的风险）。

use std::collections::HashSet;
use std::ffi::c_void;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rlite::sys::wide;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

const NO_WINDOW: u32 = 0x0800_0000;

/// 雷蛇官方提供的独立固件更新程序（来自 mysupport.razer.com）
pub struct Known {
    pub version: &'static str,
    pub date: &'static str,
    pub url: &'static str,
    pub page: &'static str,
}

pub const GENERAL_PAGE: &str = "https://mysupport.razer.com/app/answers/detail/a_id/4166";

pub fn known(pid: u16) -> Option<Known> {
    match pid {
        0x00B8 => Some(Known {
            version: "2.00.02_r1",
            date: "2025-08-20",
            url: "https://rzr.to/cexrmi",
            page: "https://mysupport.razer.com/app/answers/detail/a_id/13747",
        }),
        _ => None,
    }
}

/// "2.00.02_r1" → (2, 0, 2, 1)
pub fn parse_version(s: &str) -> Option<(u32, u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let (main, rev) = match s.split_once('_') {
        Some((m, r)) => (m, r.trim_start_matches(['r', 'R']).parse().unwrap_or(0)),
        None => (s, 0),
    };
    let mut it = main.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c, rev))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// 官方版本更高
    Available,
    /// 前两位相同，鼠标不报告第三位，无法确定
    Maybe,
    UpToDate,
}

pub fn verdict(device: (u8, u8), official: &str, installed: Option<&str>) -> Verdict {
    let Some(o) = parse_version(official) else { return Verdict::Maybe };
    let d = (device.0 as u32, device.1 as u32);
    if (o.0, o.1) > d {
        return Verdict::Available;
    }
    if (o.0, o.1) < d {
        return Verdict::UpToDate;
    }
    match installed.and_then(parse_version) {
        Some(i) if i >= o => Verdict::UpToDate,
        _ if o.2 == 0 && o.3 <= 1 => Verdict::UpToDate,
        _ => Verdict::Maybe,
    }
}

// ---------------------------------------------------------------- WinINet（按需加载）

type HInet = *mut c_void;
type FnOpen = unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> HInet;
type FnOpenUrl = unsafe extern "system" fn(HInet, *const u16, *const u16, u32, u32, usize) -> HInet;
type FnRead = unsafe extern "system" fn(HInet, *mut c_void, u32, *mut u32) -> BOOL;
type FnClose = unsafe extern "system" fn(HInet) -> BOOL;
type FnQueryInfo = unsafe extern "system" fn(HInet, u32, *mut c_void, *mut u32, *mut u32) -> BOOL;
type FnQueryOpt = unsafe extern "system" fn(HInet, u32, *mut c_void, *mut u32) -> BOOL;
type FnSetOpt = unsafe extern "system" fn(HInet, u32, *const c_void, u32) -> BOOL;

struct Net {
    lib: HMODULE,
    session: HInet,
    open_url: FnOpenUrl,
    read: FnRead,
    close: FnClose,
    query_info: FnQueryInfo,
    query_opt: FnQueryOpt,
}

struct Resp<'a> {
    net: &'a Net,
    h: HInet,
}

impl Drop for Resp<'_> {
    fn drop(&mut self) {
        unsafe { (self.net.close)(self.h) };
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.session);
            FreeLibrary(self.lib);
        }
    }
}

macro_rules! sym {
    ($lib:expr, $name:literal, $t:ty) => {{
        let p = GetProcAddress($lib, concat!($name, "\0").as_ptr());
        match p {
            Some(f) => std::mem::transmute::<_, $t>(f),
            None => {
                FreeLibrary($lib);
                return Err(concat!("缺少函数 ", $name).into());
            }
        }
    }};
}

impl Net {
    fn new() -> Result<Net, String> {
        unsafe {
            let lib = LoadLibraryW(wide("wininet.dll").as_ptr());
            if lib.is_null() {
                return Err("无法加载 wininet.dll".into());
            }
            let open: FnOpen = sym!(lib, "InternetOpenW", FnOpen);
            let set_opt: FnSetOpt = sym!(lib, "InternetSetOptionW", FnSetOpt);
            let open_url: FnOpenUrl = sym!(lib, "InternetOpenUrlW", FnOpenUrl);
            let read: FnRead = sym!(lib, "InternetReadFile", FnRead);
            let close: FnClose = sym!(lib, "InternetCloseHandle", FnClose);
            let query_info: FnQueryInfo = sym!(lib, "HttpQueryInfoW", FnQueryInfo);
            let query_opt: FnQueryOpt = sym!(lib, "InternetQueryOptionW", FnQueryOpt);
            // INTERNET_OPEN_TYPE_PRECONFIG：沿用系统代理设置
            let agent = wide(&format!("RazerLite/{}", env!("CARGO_PKG_VERSION")));
            let session = open(agent.as_ptr(), 0, std::ptr::null(), std::ptr::null(), 0);
            if session.is_null() {
                FreeLibrary(lib);
                return Err("无法初始化网络".into());
            }
            for opt in [2u32, 6, 5] {
                // 连接 / 接收 / 发送超时 20 秒
                let ms: u32 = 20_000;
                set_opt(session, opt, &ms as *const u32 as *const c_void, 4);
            }
            Ok(Net { lib, session, open_url, read, close, query_info, query_opt })
        }
    }

    fn get(&self, url: &str) -> Result<Resp<'_>, String> {
        // RELOAD | NO_CACHE_WRITE | NO_COOKIES | NO_UI
        let flags = 0x8000_0000u32 | 0x0400_0000 | 0x0008_0000 | 0x0000_0200;
        let u = wide(url);
        let h = unsafe { (self.open_url)(self.session, u.as_ptr(), std::ptr::null(), 0, flags, 0) };
        if h.is_null() {
            return Err(format!("连接失败（错误 {}），请检查网络", unsafe { GetLastError() }));
        }
        let r = Resp { net: self, h };
        let status = r.number(19);
        if status != Some(200) {
            return Err(format!("服务器返回 {}", status.unwrap_or(0)));
        }
        Ok(r)
    }
}

impl Resp<'_> {
    fn number(&self, query: u32) -> Option<u32> {
        let mut v: u32 = 0;
        let mut len = 4u32;
        let ok = unsafe { (self.net.query_info)(self.h, query | 0x2000_0000, &mut v as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) };
        (ok != 0).then_some(v)
    }

    fn final_url(&self) -> String {
        let mut buf = [0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        unsafe { (self.net.query_opt)(self.h, 34, buf.as_mut_ptr() as *mut c_void, &mut len) };
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..n])
    }

    fn read(&self, buf: &mut [u8]) -> Result<usize, String> {
        let mut got = 0u32;
        if unsafe { (self.net.read)(self.h, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut got) } == 0 {
            return Err(format!("下载中断（错误 {}）", unsafe { GetLastError() }));
        }
        Ok(got as usize)
    }
}

// ---------------------------------------------------------------- 检查 / 下载 / 校验

#[derive(Clone, Debug)]
pub struct Remote {
    pub version: String,
    pub file_name: String,
    pub url: String,
    pub size: u64,
}

fn file_name_of(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("FirmwareUpdater.exe").to_string()
}

fn version_of(file: &str) -> Option<String> {
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    let i = stem.rfind("_v").or_else(|| stem.rfind("_V"))?;
    let v = &stem[i + 2..];
    parse_version(v).map(|_| v.to_string())
}

/// 联网确认官方下载地址仍然有效，并从文件名读出版本号
pub fn check(k: &Known) -> Result<Remote, String> {
    let net = Net::new()?;
    let r = net.get(k.url)?;
    let url = r.final_url();
    let file_name = file_name_of(&url);
    if !file_name.to_ascii_lowercase().ends_with(".exe") {
        return Err("官方下载地址已失效，请到官方页面查看".into());
    }
    Ok(Remote {
        version: version_of(&file_name).unwrap_or_else(|| k.version.to_string()),
        size: r.number(5).unwrap_or(0) as u64,
        file_name,
        url,
    })
}

pub fn download_dir() -> PathBuf {
    std::env::temp_dir().join("RazerLite-fw")
}

/// 下载到临时目录；progress = (已下载, 总大小)
pub fn download(remote: &Remote, progress: Arc<(AtomicU64, AtomicU64)>) -> Result<PathBuf, String> {
    let dir = download_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&remote.file_name);
    let part = dir.join(format!("{}.part", remote.file_name));

    let net = Net::new()?;
    let r = net.get(&remote.url)?;
    let total = r.number(5).unwrap_or(remote.size as u32) as u64;
    progress.1.store(total, Ordering::Relaxed);
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress.0.store(done, Ordering::Relaxed);
    }
    drop(f);
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&part);
        return Err(format!("下载不完整（{done}/{total} 字节）"));
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// 校验 Authenticode 签名：必须有效且签名者是雷蛇
pub fn verify_signature(path: &Path) -> Result<(), String> {
    let p = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath '{p}'; $s.Status.ToString(); if ($s.SignerCertificate) {{ $s.SignerCertificate.Subject }}"
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("无法校验签名：{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let status = lines.next().unwrap_or("");
    let subject = lines.next().unwrap_or("");
    if status != "Valid" {
        return Err(format!("数字签名无效（{status}），已删除下载的文件"));
    }
    let org = subject.split(',').map(str::trim).find(|p| p.starts_with("O=")).unwrap_or("");
    if !org.contains("Razer") {
        return Err(format!("签名者不是雷蛇（{subject}），已删除下载的文件"));
    }
    Ok(())
}

// ---------------------------------------------------------------- 运行官方更新程序

fn snapshot() -> Vec<(u32, u32)> {
    let mut v = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return v;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                v.push((e.th32ProcessID, e.th32ParentProcessID));
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    v
}

pub struct Launched {
    handle: HANDLE,
    before: HashSet<u32>,
}

unsafe impl Send for Launched {}

/// 以管理员身份启动（弹 UAC）。返回 Err 表示用户取消或启动失败
pub fn launch(path: &Path) -> Result<Launched, String> {
    let before: HashSet<u32> = snapshot().into_iter().map(|p| p.0).collect();
    let file = wide(&path.to_string_lossy());
    let dir = wide(&path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let verb = wide("runas");
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpDirectory = dir.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return Err(if GetLastError() == ERROR_CANCELLED { "已取消管理员授权".into() } else { "无法启动更新程序".into() });
        }
        Ok(Launched { handle: info.hProcess, before })
    }
}

/// 等待更新程序及其派生的所有子进程结束（自解压壳会再启动真正的更新程序）
pub fn wait(l: Launched) {
    let root = unsafe { GetProcessId(l.handle) };
    let mut tracked: HashSet<u32> = HashSet::from([root]);
    let start = Instant::now();
    loop {
        let snap = snapshot();
        for &(pid, ppid) in &snap {
            if !l.before.contains(&pid) && tracked.contains(&ppid) {
                tracked.insert(pid);
            }
        }
        let alive = snap.iter().any(|(pid, _)| tracked.contains(pid));
        if !alive || start.elapsed() > Duration::from_secs(3600) {
            break;
        }
        std::thread::sleep(Duration::from_millis(800));
    }
    unsafe { CloseHandle(l.handle) };
}

// ---------------------------------------------------------------- 雷蛇驱动残留

/// 列出驱动库里的雷蛇驱动包（oemNN.inf）。pnputil 的输出会本地化，只依赖 ASCII 的包名和厂商名
pub fn razer_driver_packages() -> Vec<String> {
    let Ok(out) = Command::new("pnputil.exe").arg("/enum-drivers").creation_flags(NO_WINDOW).output() else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    let mut v = vec![];
    for block in text.split("\n\n") {
        if !block.contains("Razer") {
            continue;
        }
        if let Some(inf) = block.split_whitespace().find(|w| {
            let w = w.to_ascii_lowercase();
            w.starts_with("oem") && w.ends_with(".inf") && w[3..w.len() - 4].chars().all(|c| c.is_ascii_digit())
        }) {
            v.push(inf.to_ascii_lowercase());
        }
    }
    v
}

/// 以管理员身份删除这些驱动包，设备自动换回 Windows 自带驱动
pub fn remove_driver_packages(infs: &[String]) -> Result<(), String> {
    if infs.is_empty() {
        return Ok(());
    }
    let mut cmd: Vec<String> = infs.iter().map(|i| format!("pnputil /delete-driver {i} /uninstall /force")).collect();
    cmd.push("pnputil /scan-devices".into());
    let args = format!("/c {}", cmd.join(" & "));
    let file = wide("cmd.exe");
    let params = wide(&args);
    let verb = wide("runas");
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return Err("已取消管理员授权".into());
        }
        WaitForSingleObject(info.hProcess, 120_000);
        CloseHandle(info.hProcess);
    }
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟自解压壳：父进程启动子进程后立即退出，wait 必须等到子进程结束
    #[test]
    fn wait_tracks_children() {
        let before: HashSet<u32> = snapshot().into_iter().map(|p| p.0).collect();
        let child = Command::new("cmd.exe").args(["/c", "start", "/b", "ping", "-n", "4", "127.0.0.1"]).creation_flags(NO_WINDOW).spawn().unwrap();
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, child.id()) };
        assert!(!h.is_null());
        let t = Instant::now();
        wait(Launched { handle: h, before });
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(2500), "returned too early: {e:?}");
        assert!(e < Duration::from_secs(10), "returned too late: {e:?}");
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("2.00.02_r1"), Some((2, 0, 2, 1)));
        assert_eq!(parse_version("v1.14.00_r2"), Some((1, 14, 0, 2)));
        assert_eq!(version_of("ViperV3HyperSpeed_00B8_FirmwareUpdater_v2.00.02_r1.exe").as_deref(), Some("2.00.02_r1"));
        assert_eq!(verdict((2, 0), "2.00.02_r1", None), Verdict::Maybe);
        assert_eq!(verdict((2, 0), "2.00.02_r1", Some("2.00.02_r1")), Verdict::UpToDate);
        assert_eq!(verdict((1, 9), "2.00.02_r1", None), Verdict::Available);
        assert_eq!(verdict((2, 1), "2.00.02_r1", None), Verdict::UpToDate);
    }
}
