//! 程序图标：代码绘制（无需图片资源）。返回 RGBA（非预乘 alpha）。

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn cov(d: f32, aa: f32) -> f32 {
    (0.5 - d / aa).clamp(0.0, 1.0)
}

/// 绘制 size×size 图标。active=false 时为灰色（映射暂停）。
pub fn render(size: u32, active: bool) -> Vec<u8> {
    let s = size as f32;
    let aa = 1.0 / s; // 一个像素在单位坐标中的宽度
    let accent = if active { [0x44, 0xD6, 0x2C] } else { [0x8A, 0x8F, 0x98] };
    let dark = [0x14, 0x16, 0x1A];
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // 3×3 超采样
            let mut acc = [0f32; 4];
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 3.0) / s;
                    let py = (y as f32 + (sy as f32 + 0.5) / 3.0) / s;
                    // 背景：圆角方块
                    let bg = cov(sd_round_rect(px, py, 0.5, 0.5, 0.5, 0.5, 0.24), aa);
                    // 鼠标外形
                    let body = cov(sd_round_rect(px, py, 0.5, 0.53, 0.2, 0.31, 0.2), aa);
                    // 左右键分割线 + 滚轮
                    let split: f32 = if (px - 0.5).abs() < 0.035 && py > 0.2 && py < 0.46 { 1.0 } else { 0.0 };
                    let wheel = cov(sd_round_rect(px, py, 0.5, 0.33, 0.045, 0.075, 0.045), aa);
                    let mut c = [accent[0] as f32, accent[1] as f32, accent[2] as f32];
                    let mut a: f32 = bg;
                    if body > 0.0 {
                        for i in 0..3 {
                            c[i] = c[i] * (1.0 - body) + dark[i] as f32 * body;
                        }
                    }
                    let line = split.max(wheel) * body;
                    if line > 0.0 {
                        for i in 0..3 {
                            c[i] = c[i] * (1.0 - line) + accent[i] as f32 * line;
                        }
                    }
                    a = a.max(0.0);
                    for i in 0..3 {
                        acc[i] += c[i] * a;
                    }
                    acc[3] += a;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            let a = acc[3] / 9.0;
            if a > 0.0 {
                for k in 0..3 {
                    out[i + k] = (acc[k] / 9.0 / a).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}
