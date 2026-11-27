//! 极简 CPU 光栅化器：把 egui 输出的三角形网格画进 0RGB 帧缓冲。
//! 不加载任何 GPU 驱动（OpenGL / D3D），设置窗口因此只占十几 MB 内存。

use std::collections::HashMap;

use egui::epaint::textures::TextureFilter;
use egui::epaint::{ClippedPrimitive, ImageData, ImageDelta, Primitive, Vertex};
use egui::{TextureId, TexturesDelta};

struct Tex {
    w: usize,
    h: usize,
    /// 预乘 alpha 的 sRGBA
    px: Vec<[u8; 4]>,
    linear: bool,
}

#[derive(Default)]
pub struct Painter {
    textures: HashMap<TextureId, Tex>,
}

impl Painter {
    pub fn update_textures(&mut self, delta: &TexturesDelta) {
        for (id, d) in &delta.set {
            self.set(*id, d);
        }
    }

    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    fn set(&mut self, id: TextureId, d: &ImageDelta) {
        let ImageData::Color(img) = &d.image;
        let [w, h] = img.size;
        match d.pos {
            None => {
                self.textures.insert(
                    id,
                    Tex {
                        w,
                        h,
                        px: img.pixels.iter().map(|c| c.to_array()).collect(),
                        linear: d.options.magnification == TextureFilter::Linear,
                    },
                );
            }
            Some([x0, y0]) => {
                if let Some(t) = self.textures.get_mut(&id) {
                    for y in 0..h {
                        for x in 0..w {
                            let (tx, ty) = (x0 + x, y0 + y);
                            if tx < t.w && ty < t.h {
                                t.px[ty * t.w + tx] = img.pixels[y * w + x].to_array();
                            }
                        }
                    }
                }
            }
        }
    }

    /// 绘制一帧。buf 为 width×height 的 0x00RRGGBB 像素。
    pub fn paint(&self, buf: &mut [u32], width: usize, height: usize, ppp: f32, prims: &[ClippedPrimitive], clear: [u8; 3]) {
        let c = (clear[0] as u32) << 16 | (clear[1] as u32) << 8 | clear[2] as u32;
        buf.fill(c);
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else { continue };
            let cr = p.clip_rect;
            let clip = (
                ((cr.min.x * ppp).round().max(0.0) as i32),
                ((cr.min.y * ppp).round().max(0.0) as i32),
                ((cr.max.x * ppp).round().min(width as f32) as i32),
                ((cr.max.y * ppp).round().min(height as f32) as i32),
            );
            if clip.0 >= clip.2 || clip.1 >= clip.3 {
                continue;
            }
            for t in mesh.indices.chunks_exact(3) {
                let v = [&mesh.vertices[t[0] as usize], &mesh.vertices[t[1] as usize], &mesh.vertices[t[2] as usize]];
                triangle(buf, width, clip, v, tex, ppp);
            }
        }
    }
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

#[inline]
fn fetch(t: &Tex, x: i32, y: i32) -> [f32; 4] {
    let x = x.clamp(0, t.w as i32 - 1) as usize;
    let y = y.clamp(0, t.h as i32 - 1) as usize;
    let p = t.px[y * t.w + x];
    [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
}

#[inline]
fn sample(t: &Tex, u: f32, v: f32) -> [f32; 4] {
    let x = u * t.w as f32 - 0.5;
    let y = v * t.h as f32 - 0.5;
    if !t.linear {
        return fetch(t, (x + 0.5).floor() as i32, (y + 0.5).floor() as i32);
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let a = fetch(t, x0, y0);
    let b = fetch(t, x0 + 1, y0);
    let c = fetch(t, x0, y0 + 1);
    let d = fetch(t, x0 + 1, y0 + 1);
    let mut o = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        o[i] = top + (bot - top) * fy;
    }
    o
}

/// 顶左填充规则：像素中心恰好落在边上时只归属一个三角形，避免半透明区域重复叠加
#[inline]
fn top_left(ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let (dx, dy) = (bx - ax, by - ay);
    (dy == 0.0 && dx > 0.0) || dy < 0.0
}

fn triangle(buf: &mut [u32], width: usize, clip: (i32, i32, i32, i32), v: [&Vertex; 3], tex: &Tex, ppp: f32) {
    let mut p = [(0f32, 0f32); 3];
    for i in 0..3 {
        p[i] = (v[i].pos.x * ppp, v[i].pos.y * ppp);
    }
    let mut order = [0usize, 1, 2];
    let mut area = edge(p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1);
    if area.abs() < 1e-6 {
        return;
    }
    if area < 0.0 {
        order = [0, 2, 1];
        area = -area;
    }
    let (p0, p1, p2) = (p[order[0]], p[order[1]], p[order[2]]);
    let (v0, v1, v2) = (v[order[0]], v[order[1]], v[order[2]]);

    let minx = (p0.0.min(p1.0).min(p2.0).floor() as i32).max(clip.0);
    let maxx = (p0.0.max(p1.0).max(p2.0).ceil() as i32).min(clip.2);
    let miny = (p0.1.min(p1.1).min(p2.1).floor() as i32).max(clip.1);
    let maxy = (p0.1.max(p1.1).max(p2.1).ceil() as i32).min(clip.3);
    if minx >= maxx || miny >= maxy {
        return;
    }

    let col = |v: &Vertex| {
        let a = v.color.to_array();
        [a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32]
    };
    let (c0, c1, c2) = (col(v0), col(v1), col(v2));
    let same_color = v0.color == v1.color && v1.color == v2.color;
    let same_uv = v0.uv == v1.uv && v1.uv == v2.uv;
    let const_tex = if same_uv { sample(tex, v0.uv.x, v0.uv.y) } else { [0.0; 4] };

    let tl0 = top_left(p1.0, p1.1, p2.0, p2.1);
    let tl1 = top_left(p2.0, p2.1, p0.0, p0.1);
    let tl2 = top_left(p0.0, p0.1, p1.0, p1.1);
    let inv = 1.0 / area;

    // 纯色、无纹理变化的快速路径预先算好源颜色
    let solid = if same_color && same_uv {
        let mut s = [0f32; 4];
        for i in 0..4 {
            s[i] = c0[i] * const_tex[i] / 255.0;
        }
        Some(s)
    } else {
        None
    };
    if let Some(s) = solid {
        if s[0] <= 0.0 && s[1] <= 0.0 && s[2] <= 0.0 && s[3] <= 0.0 {
            return;
        }
    }

    for y in miny..maxy {
        let py = y as f32 + 0.5;
        let row = y as usize * width;
        for x in minx..maxx {
            let px = x as f32 + 0.5;
            let w0 = edge(p1.0, p1.1, p2.0, p2.1, px, py);
            let w1 = edge(p2.0, p2.1, p0.0, p0.1, px, py);
            let w2 = edge(p0.0, p0.1, p1.0, p1.1, px, py);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            if (w0 == 0.0 && !tl0) || (w1 == 0.0 && !tl1) || (w2 == 0.0 && !tl2) {
                continue;
            }
            let src = match solid {
                Some(s) => s,
                None => {
                    let (b0, b1, b2) = (w0 * inv, w1 * inv, w2 * inv);
                    let c = if same_color {
                        c0
                    } else {
                        [
                            c0[0] * b0 + c1[0] * b1 + c2[0] * b2,
                            c0[1] * b0 + c1[1] * b1 + c2[1] * b2,
                            c0[2] * b0 + c1[2] * b1 + c2[2] * b2,
                            c0[3] * b0 + c1[3] * b1 + c2[3] * b2,
                        ]
                    };
                    let t = if same_uv {
                        const_tex
                    } else {
                        let u = v0.uv.x * b0 + v1.uv.x * b1 + v2.uv.x * b2;
                        let v = v0.uv.y * b0 + v1.uv.y * b1 + v2.uv.y * b2;
                        sample(tex, u, v)
                    };
                    [c[0] * t[0] / 255.0, c[1] * t[1] / 255.0, c[2] * t[2] / 255.0, c[3] * t[3] / 255.0]
                }
            };
            if src[3] <= 0.0 && src[0] <= 0.0 && src[1] <= 0.0 && src[2] <= 0.0 {
                continue;
            }
            let d = &mut buf[row + x as usize];
            let k = 1.0 - (src[3] / 255.0).min(1.0);
            let dr = ((*d >> 16) & 0xFF) as f32;
            let dg = ((*d >> 8) & 0xFF) as f32;
            let db = (*d & 0xFF) as f32;
            let r = (src[0] + dr * k).min(255.0) as u32;
            let g = (src[1] + dg * k).min(255.0) as u32;
            let b = (src[2] + db * k).min(255.0) as u32;
            *d = r << 16 | g << 8 | b;
        }
    }
}
