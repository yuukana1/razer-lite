//! 编译时生成程序图标（与托盘图标同一套绘制代码）并嵌入版本信息。

#[path = "src/icon.rs"]
mod icon;

use std::path::PathBuf;

fn ico_bytes(sizes: &[u32]) -> Vec<u8> {
    let mut images = vec![];
    for &s in sizes {
        let rgba = icon::render(s, true);
        let mut d = vec![];
        // BITMAPINFOHEADER，高度为两倍（颜色 + 掩码）
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&(s as i32).to_le_bytes());
        d.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(&[0u8; 24]);
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                d.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        let mask_row = ((s + 31) / 32 * 4) as usize;
        d.extend(std::iter::repeat(0u8).take(mask_row * s as usize));
        images.push((s, d));
    }
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, d) in &images {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += d.len() as u32;
    }
    for (_, d) in &images {
        out.extend_from_slice(d);
    }
    out
}

fn rc(ico: &str, desc: &str, name: &str) -> String {
    let v = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = v.split('.').collect();
    let comma = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    format!(
        r#"#pragma code_page(65001)
1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404b0"
    BEGIN
      VALUE "FileDescription", "{desc}"
      VALUE "ProductName", "Razer Lite"
      VALUE "FileVersion", "{v}"
      VALUE "ProductVersion", "{v}"
      VALUE "OriginalFilename", "{name}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#
    )
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("app.ico");
    std::fs::write(&ico, ico_bytes(&[16, 20, 24, 32, 40, 48, 64, 128])).unwrap();
    let ico_s = ico.display().to_string().replace(std::path::MAIN_SEPARATOR, "/");
    for (bin, desc) in [("razer-lite", "Razer Lite 后台（按键映射）"), ("razer-lite-settings", "Razer Lite 设置")] {
        let rc_path = out.join(format!("{bin}.rc"));
        std::fs::write(&rc_path, rc(&ico_s, desc, &format!("{bin}.exe"))).unwrap();
        embed_resource::compile_for(&rc_path, [bin], embed_resource::NONE).manifest_optional().unwrap();
    }
}
