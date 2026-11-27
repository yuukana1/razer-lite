//! 雷蛇鼠标 HID 协议（90 字节 feature report），参考 OpenRazer 的公开实现。
//! 只用 Windows 自带的 hid.dll / cfgmgr32.dll，不依赖任何雷蛇组件。

use std::ffi::c_void;
use std::ptr::null;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;

pub const RAZER_VID: u16 = 0x1532;
const REPORT_LEN: usize = 90;

/// 已知型号（PID → 名称）。未列出的雷蛇鼠标也会尝试连接。
pub fn model_name(pid: u16) -> &'static str {
    match pid {
        0x00B8 => "Razer Viper V3 HyperSpeed",
        0x00C0 | 0x00C1 => "Razer Viper V3 Pro",
        0x00AB | 0x00AA => "Razer Basilisk V3 Pro",
        0x00B6 | 0x00B7 => "Razer DeathAdder V3 Pro",
        0x00B4 | 0x00B3 => "Razer Viper Mini SE / Hyperspeed",
        0x0083 | 0x0085 => "Razer Basilisk X HyperSpeed",
        _ => "Razer 鼠标",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy = 1,
    Ok = 2,
    Failed = 3,
    Timeout = 4,
    NotSupported = 5,
}

#[derive(Debug)]
pub enum Error {
    NotFound,
    Io(&'static str),
    Status(u8),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::NotFound => write!(f, "未找到雷蛇鼠标"),
            Error::Io(s) => write!(f, "HID 通信失败：{s}"),
            Error::Status(5) => write!(f, "设备不支持该命令"),
            Error::Status(1) | Error::Status(4) => write!(f, "接收器已连接，但鼠标没有响应（可能在休眠），动一下鼠标再试"),
            Error::Status(s) => write!(f, "设备返回错误状态 {s}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Device {
    handle: HANDLE,
    pub pid: u16,
    pub path: String,
    tid: u8,
}

unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 列出系统里所有 HID 接口路径（只保留雷蛇 VID）。
fn razer_hid_paths() -> Vec<String> {
    unsafe {
        let mut len = 0u32;
        if CM_Get_Device_Interface_List_SizeW(
            &mut len,
            &GUID_DEVINTERFACE_HID,
            null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        ) != CR_SUCCESS
            || len == 0
        {
            return vec![];
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(
            &GUID_DEVINTERFACE_HID,
            null(),
            buf.as_mut_ptr(),
            len,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        ) != CR_SUCCESS
        {
            return vec![];
        }
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .filter(|p| p.to_ascii_lowercase().contains("vid_1532"))
            .collect()
    }
}

impl Device {
    /// 找到可以收发 90 字节 feature report 的雷蛇控制接口并握手。
    pub fn open() -> Result<Device> {
        let mut candidates = vec![];
        for path in razer_hid_paths() {
            unsafe {
                let w = wide(&path);
                // 访问权限 0：鼠标接口被系统独占读取，但 feature report 仍可收发
                let h = CreateFileW(
                    w.as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if h == INVALID_HANDLE_VALUE {
                    continue;
                }
                let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
                attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
                let mut ok = HidD_GetAttributes(h, &mut attr) != 0 && attr.VendorID == RAZER_VID;
                if ok {
                    let mut pp: PHIDP_PREPARSED_DATA = 0;
                    ok = false;
                    if HidD_GetPreparsedData(h, &mut pp) != 0 {
                        let mut caps: HIDP_CAPS = std::mem::zeroed();
                        if HidP_GetCaps(pp, &mut caps) == HIDP_STATUS_SUCCESS {
                            ok = caps.FeatureReportByteLength as usize == REPORT_LEN + 1;
                        }
                        HidD_FreePreparsedData(pp);
                    }
                }
                if ok {
                    candidates.push(Device { handle: h, pid: attr.ProductID, path: path.clone(), tid: 0x1F });
                } else {
                    CloseHandle(h);
                }
            }
        }
        // 逐个接口、逐个 transaction id 尝试，读取固件版本作为握手
        let mut err = Error::NotFound;
        for mut dev in candidates {
            for tid in [0x1F, 0x3F, 0xFF] {
                dev.tid = tid;
                match dev.firmware_raw() {
                    Ok(_) => return Ok(dev),
                    // 接收器回了话，但鼠标没响应：多半是无线鼠标在休眠
                    Err(Error::Status(s @ (1 | 4))) => err = Error::Status(s),
                    Err(_) => {}
                }
            }
        }
        Err(err)
    }

    fn transfer(&self, class: u8, id: u8, size: u8, args: &[u8]) -> Result<[u8; REPORT_LEN]> {
        let mut rep = [0u8; REPORT_LEN];
        rep[1] = self.tid;
        rep[5] = size;
        rep[6] = class;
        rep[7] = id;
        rep[8..8 + args.len()].copy_from_slice(args);
        rep[88] = rep[2..88].iter().fold(0, |a, b| a ^ b);

        let mut buf = [0u8; REPORT_LEN + 1];
        buf[1..].copy_from_slice(&rep);
        let mut last = Status::Busy as u8;
        // 发一次命令后轮询回复；无线鼠标的回复要经过接收器转发，通常 5~40 毫秒
        for _send in 0..3 {
            unsafe {
                if HidD_SetFeature(self.handle, buf.as_ptr() as *const c_void, buf.len() as u32) == 0 {
                    return Err(Error::Io("SetFeature"));
                }
            }
            let start = Instant::now();
            sleep(Duration::from_millis(2));
            loop {
                let mut resp = [0u8; REPORT_LEN + 1];
                if unsafe { HidD_GetFeature(self.handle, resp.as_mut_ptr() as *mut c_void, resp.len() as u32) } == 0 {
                    return Err(Error::Io("GetFeature"));
                }
                let r: [u8; REPORT_LEN] = resp[1..].try_into().unwrap();
                // 只认本条命令的回复，其他（旧回复、别的进程的命令）继续等
                if r[6] == class && r[7] == id {
                    match r[0] {
                        2 => return Ok(r),
                        0 | 1 => {}
                        3 | 5 => return Err(Error::Status(r[0])),
                        s => {
                            last = s;
                            break; // 超时：重发
                        }
                    }
                }
                if start.elapsed() > Duration::from_millis(120) {
                    last = Status::Timeout as u8;
                    break;
                }
                sleep(Duration::from_millis(4));
            }
        }
        Err(Error::Status(last))
    }

    pub fn name(&self) -> &'static str {
        model_name(self.pid)
    }

    /// 固件版本 (主版本, 次版本)
    pub fn firmware_raw(&self) -> Result<(u8, u8)> {
        let r = self.transfer(0x00, 0x81, 0x02, &[])?;
        Ok((r[8], r[9]))
    }

    pub fn firmware(&self) -> Result<String> {
        let (a, b) = self.firmware_raw()?;
        Ok(format!("v{a}.{b:02}"))
    }

    pub fn serial(&self) -> Result<String> {
        let r = self.transfer(0x00, 0x82, 0x16, &[])?;
        Ok(r[8..8 + 22].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect())
    }

    /// 电量百分比
    pub fn battery(&self) -> Result<u8> {
        let r = self.transfer(0x07, 0x80, 0x02, &[])?;
        Ok(((r[9] as u32 * 100 + 127) / 255) as u8)
    }

    pub fn charging(&self) -> Result<bool> {
        let r = self.transfer(0x07, 0x84, 0x02, &[])?;
        Ok(r[9] != 0)
    }

    /// 当前 DPI (x, y)
    pub fn dpi(&self) -> Result<(u16, u16)> {
        let r = self.transfer(0x04, 0x85, 0x07, &[0x00])?;
        Ok((u16::from_be_bytes([r[9], r[10]]), u16::from_be_bytes([r[11], r[12]])))
    }

    /// 设置当前 DPI。store=false 时不写入板载存储（用于 DPI 离合等临时切换，避免反复擦写）
    pub fn set_dpi(&self, x: u16, y: u16, store: bool) -> Result<()> {
        let [xh, xl] = x.to_be_bytes();
        let [yh, yl] = y.to_be_bytes();
        self.transfer(0x04, 0x05, 0x07, &[store as u8, xh, xl, yh, yl, 0, 0])?;
        Ok(())
    }

    /// 设备模式：0 = 普通模式（鼠标自己处理按键），3 = 驱动模式（由雷蛇软件接管）
    pub fn device_mode(&self) -> Result<u8> {
        let r = self.transfer(0x00, 0x84, 0x02, &[])?;
        Ok(r[8])
    }

    pub fn set_device_mode(&self, mode: u8) -> Result<()> {
        self.transfer(0x00, 0x04, 0x02, &[mode, 0x00])?;
        Ok(())
    }

    /// DPI 档位：(当前档位 从 1 开始, 每档 (x, y))
    pub fn dpi_stages(&self) -> Result<(u8, Vec<(u16, u16)>)> {
        let r = self.transfer(0x04, 0x86, 0x26, &[0x01])?;
        let a = &r[8..];
        let active = a[1];
        let count = a[2].min(5) as usize;
        let mut v = vec![];
        for i in 0..count {
            let o = 3 + i * 7;
            v.push((u16::from_be_bytes([a[o + 1], a[o + 2]]), u16::from_be_bytes([a[o + 3], a[o + 4]])));
        }
        Ok((active, v))
    }

    pub fn set_dpi_stages(&self, active: u8, stages: &[(u16, u16)]) -> Result<()> {
        let n = stages.len().clamp(1, 5);
        let mut a = vec![0u8; 3 + n * 7];
        a[0] = 0x01;
        a[1] = active.clamp(1, n as u8);
        a[2] = n as u8;
        for (i, &(x, y)) in stages.iter().take(n).enumerate() {
            let o = 3 + i * 7;
            a[o] = i as u8 + 1;
            a[o + 1..o + 3].copy_from_slice(&x.to_be_bytes());
            a[o + 3..o + 5].copy_from_slice(&y.to_be_bytes());
        }
        self.transfer(0x04, 0x06, 0x26, &a)?;
        Ok(())
    }

    /// 回报率 Hz
    pub fn polling_rate(&self) -> Result<u16> {
        let r = self.transfer(0x00, 0x85, 0x01, &[])?;
        Ok(match r[8] {
            0x01 => 1000,
            0x02 => 500,
            0x08 => 125,
            _ => 0,
        })
    }

    pub fn set_polling_rate(&self, hz: u16) -> Result<()> {
        let code = match hz {
            1000 => 0x01,
            500 => 0x02,
            _ => 0x08,
        };
        self.transfer(0x00, 0x05, 0x01, &[code])?;
        Ok(())
    }

    /// 无操作多少秒后休眠
    pub fn idle_time(&self) -> Result<u16> {
        let r = self.transfer(0x07, 0x83, 0x02, &[])?;
        Ok(u16::from_be_bytes([r[8], r[9]]))
    }

    pub fn set_idle_time(&self, secs: u16) -> Result<()> {
        let secs = secs.clamp(60, 900);
        self.transfer(0x07, 0x03, 0x02, &secs.to_be_bytes())?;
        Ok(())
    }

    /// 调试用：发任意命令，返回原始回复
    pub fn raw(&self, class: u8, id: u8, size: u8, args: &[u8]) -> Result<[u8; REPORT_LEN]> {
        self.transfer(class, id, size, args)
    }
}
