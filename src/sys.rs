//! 两个程序共用的 Win32 小工具：进程间通信、开机自启、进程信息。

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::ProcessStatus::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const AGENT_CLASS: &str = "RazerLite.Agent";
pub const AGENT_EXE: &str = "razer-lite.exe";
pub const SETTINGS_EXE: &str = "razer-lite-settings.exe";
pub const SETTINGS_TITLE: &str = "Razer Lite 设置";

/// 自定义窗口消息
pub const WM_RL_RELOAD: u32 = WM_APP + 1;
pub const WM_RL_TRAY: u32 = WM_APP + 2;
pub const WM_RL_FLUSH: u32 = WM_APP + 3;
pub const WM_RL_DEVICE: u32 = WM_APP + 4;
pub const WM_RL_BALLOON: u32 = WM_APP + 5;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn find_agent() -> HWND {
    let c = wide(AGENT_CLASS);
    unsafe { FindWindowW(c.as_ptr(), null()) }
}

pub fn agent_running() -> bool {
    !find_agent().is_null()
}

/// 通知后台重新加载配置
pub fn notify_reload() -> bool {
    let h = find_agent();
    !h.is_null() && unsafe { PostMessageW(h, WM_RL_RELOAD, 0, 0) } != 0
}

pub fn stop_agent() -> bool {
    let h = find_agent();
    !h.is_null() && unsafe { PostMessageW(h, WM_CLOSE, 0, 0) } != 0
}

pub fn start_agent() -> bool {
    Command::new(exe_dir().join(AGENT_EXE)).spawn().is_ok()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemInfo {
    pub pid: u32,
    /// 工作集（任务管理器“内存”列接近这个值）
    pub working_set: usize,
    /// 专用提交大小
    pub private: usize,
}

pub fn process_memory(pid: u32) -> Option<MemInfo> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb) != 0;
        CloseHandle(h);
        ok.then(|| MemInfo { pid, working_set: c.WorkingSetSize, private: c.PrivateUsage })
    }
}

pub fn agent_memory() -> Option<MemInfo> {
    let h = find_agent();
    if h.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    process_memory(pid)
}

/// 进程 exe 文件名（小写）
pub fn process_exe_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(full.rsplit('\\').next().unwrap_or(&full).to_lowercase())
    }
}

/// 前台窗口所属程序的 exe 名（小写）
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let w = GetForegroundWindow();
        if w.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(w, &mut pid);
        process_exe_name(pid)
    }
}

/// 列出有可见窗口的程序（给“添加程序”用），去重排序
pub fn running_apps() -> Vec<String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase();
                if name.ends_with(".exe") && !out.contains(&name) {
                    out.push(name);
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    const SKIP: &[&str] = &[
        "svchost.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "smss.exe",
        "system", "registry", "fontdrvhost.exe", "dwm.exe", "conhost.exe", "sihost.exe", "ctfmon.exe",
        "runtimebroker.exe", "dllhost.exe", "taskhostw.exe", "searchhost.exe", "startmenuexperiencehost.exe",
        "textinputhost.exe", "shellexperiencehost.exe", "securityhealthservice.exe", "spoolsv.exe",
        "wmiprvse.exe", "audiodg.exe", "msmpeng.exe", "nissrv.exe", "lsaiso.exe", "memcompression",
    ];
    out.retain(|n| !SKIP.contains(&n.as_str()));
    out.sort();
    out
}

pub fn is_process_running(exe_lower: &str) -> bool {
    running_apps_raw().iter().any(|n| n == exe_lower)
}

fn running_apps_raw() -> Vec<String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// 雷蛇官方软件是否还在运行
pub fn synapse_running() -> bool {
    let procs = running_apps_raw();
    procs.iter().any(|n| n == "razerappengine.exe" || n == "razer synapse 3.exe" || n == "razer synapse service.exe")
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// 以管理员身份运行 exe（弹 UAC），等待结束并返回退出码
pub fn run_elevated(exe: &std::path::Path, args: &str) -> Result<u32, String> {
    let file = wide(&exe.to_string_lossy());
    let params = wide(args);
    let verb = wide("runas");
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return Err("已取消管理员授权".into());
        }
        if info.hProcess.is_null() {
            return Err("无法启动进程".into());
        }
        WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code)
    }
}

// ---------------- 开机自启 ----------------

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "RazerLite";
pub const TASK_NAME: &str = "RazerLite";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoStart {
    Off,
    /// 当前用户登录时启动（注册表 Run，无需管理员）
    User,
    /// 计划任务，以最高权限启动（管理员窗口里也能映射）
    Admin,
}

fn agent_path() -> PathBuf {
    exe_dir().join(AGENT_EXE)
}

fn run_key_value() -> Option<String> {
    let sub = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        );
        if r != ERROR_SUCCESS {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn set_run_key(on: bool) -> Result<(), String> {
    let sub = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    unsafe {
        if on {
            let v = wide(&format!("\"{}\"", agent_path().display()));
            let r = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                v.as_ptr() as *const _,
                (v.len() * 2) as u32,
            );
            if r != ERROR_SUCCESS {
                return Err(format!("写入注册表失败（{r}）"));
            }
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr());
        }
    }
    Ok(())
}

fn schtasks(args: &[&str]) -> Option<std::process::Output> {
    Command::new("schtasks.exe").args(args).creation_flags(0x0800_0000).output().ok()
}

pub fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME]).map(|o| o.status.success()).unwrap_or(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 创建计划任务（需在管理员进程中调用）
pub fn task_install_direct() -> Result<(), String> {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Razer Lite 鼠标侧键映射（后台）</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{cmd}</Command></Exec></Actions>
</Task>"#,
        user = xml_escape(&user),
        cmd = xml_escape(&agent_path().to_string_lossy()),
    );
    let tmp = std::env::temp_dir().join("razerlite-task.xml");
    let mut bytes = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    let out = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &tmp.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&tmp);
    match out {
        Some(o) if o.status.success() => Ok(()),
        Some(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        None => Err("无法运行 schtasks".into()),
    }
}

pub fn task_remove_direct() -> Result<(), String> {
    match schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]) {
        Some(o) if o.status.success() => Ok(()),
        _ => Err("删除计划任务失败".into()),
    }
}

pub fn autostart_status() -> AutoStart {
    if task_exists() {
        AutoStart::Admin
    } else if run_key_value().is_some() {
        AutoStart::User
    } else {
        AutoStart::Off
    }
}

fn elevated_agent(args: &str) -> Result<(), String> {
    if is_admin() {
        return match args {
            "--task-install" => task_install_direct(),
            _ => task_remove_direct(),
        };
    }
    match run_elevated(&agent_path(), args)? {
        0 => Ok(()),
        c => Err(format!("计划任务操作失败（退出码 {c}）")),
    }
}

pub fn set_autostart(mode: AutoStart) -> Result<(), String> {
    let has_task = task_exists();
    match mode {
        AutoStart::Off => {
            set_run_key(false)?;
            if has_task {
                elevated_agent("--task-remove")?;
            }
        }
        AutoStart::User => {
            if has_task {
                elevated_agent("--task-remove")?;
            }
            set_run_key(true)?;
        }
        AutoStart::Admin => {
            elevated_agent("--task-install")?;
            set_run_key(false)?;
            // 立即以管理员身份重启后台，不必等下次登录
            if stop_agent() {
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            schtasks(&["/Run", "/TN", TASK_NAME]);
        }
    }
    Ok(())
}
