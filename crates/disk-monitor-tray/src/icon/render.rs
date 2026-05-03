use std::path::Path;

use anyhow::{anyhow, Context, Result};
use disk_monitor_core::Mount;
use freetype::face::LoadFlag;
use freetype::{Face, Library};
use image::ImageReader;
use tiny_skia::{BlendMode, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

const PER_MOUNT_GAP: u32 = 4;
const DONUT_PADDING: u32 = 2;
// Regular weight con `freetype` (mismo motor que PIL/freetype del tray Python).
// Versiones previas con `fontdue` o `ab_glyph` (sin bytecode interpreter)
// rasterizaban borroso a 10–12 px aunque hicieran hinting básico.
const DEFAULT_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
];

const COLOR_FREE: [u8; 4] = [0x66, 0xb3, 0xff, 0xff];
const COLOR_OK: [u8; 4] = [0x99, 0xff, 0x99, 0xff];
const COLOR_WARN1: [u8; 4] = [0xff, 0xdb, 0x4d, 0xff];
const COLOR_WARN2: [u8; 4] = [0xff, 0xcc, 0x99, 0xff];
const COLOR_HIGH: [u8; 4] = [0xff, 0x66, 0x66, 0xff];
const COLOR_TEXT: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

pub struct RenderedIcon {
    pub width: i32,
    pub height: i32,
    /// Bytes in `ARGB32` (network byte order: A, R, G, B per pixel),
    /// the layout that StatusNotifierItem expects for `IconPixmap`.
    pub argb: Vec<u8>,
}

// freetype::Library y freetype::Face contienen punteros C crudos y por eso no
// implementan Send. Pero ksni::TrayService::spawn exige Send en el state.
// El acceso aquí es secuencial (solo desde la callback de update en el thread
// de ksni), nunca concurrente, así que es seguro afirmar Send a mano.
struct FtState {
    _library: Library,
    face: Face,
}
unsafe impl Send for FtState {}
unsafe impl Sync for FtState {}

pub struct IconRenderer {
    height: u32,
    base_icon: Option<Pixmap>,
    ft: FtState,
}

/// Layout for a single mount block. `text_w` is computed independently per
/// mount so a short label (`/`) does not leave the same gap as a long one
/// (`seagat`). Mount labels are static within a session, so giving each
/// block its own width does not cause donut positions to jitter frame-to-frame.
struct BlockLayout {
    donut_size: u32,
    icon_w: u32,
    text_w: u32,
    connected: bool,
}

impl IconRenderer {
    pub fn new(height: u32, base_icon_path: &Path) -> Result<Self> {
        let base_icon = load_base_icon(base_icon_path, height).ok();
        let (ft_library, face) = load_face().context("loading DejaVu Sans Mono font")?;
        Ok(Self {
            height,
            base_icon,
            ft: FtState {
                _library: ft_library,
                face,
            },
        })
    }

    pub fn render(&self, mounts: &[Mount], connected: bool) -> RenderedIcon {
        let pixmap = self.render_pixmap(mounts, connected);
        RenderedIcon {
            width: pixmap.width() as i32,
            height: pixmap.height() as i32,
            argb: rgba_premul_to_argb_straight(pixmap.data()),
        }
    }

    /// Render and encode the icon as a PNG (straight RGBA, decoders happy).
    pub fn render_png(&self, mounts: &[Mount], connected: bool) -> Result<Vec<u8>> {
        let pixmap = self.render_pixmap(mounts, connected);
        let straight = unpremultiply_to_rgba(pixmap.data());
        let img = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), straight)
            .ok_or_else(|| anyhow!("failed to wrap pixmap as RgbaImage"))?;
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
        Ok(buf)
    }

    fn render_pixmap(&self, mounts: &[Mount], connected: bool) -> Pixmap {
        let h = self.height;
        let donut_size = h.saturating_sub(DONUT_PADDING * 2).max(8);
        let icon_w = self.base_icon.as_ref().map(|p| p.width()).unwrap_or(0);
        let px = text_size(h);

        // Per-mount layout: each block is sized to its own label so `/` does
        // not get the same width as `seagat`. Stable across frames because
        // mount points themselves don't change within a session.
        let blocks: Vec<(String, BlockLayout)> = mounts
            .iter()
            .map(|m| {
                let label = short_label(&m.mount_point);
                let text_w = self.measure_text(&label, px);
                (
                    label,
                    BlockLayout {
                        donut_size,
                        icon_w,
                        text_w,
                        connected,
                    },
                )
            })
            .collect();

        let total_w: u32 = blocks
            .iter()
            .map(|(_, l)| l.icon_w + 2 + l.text_w + 2 + l.donut_size)
            .sum::<u32>()
            + PER_MOUNT_GAP * (blocks.len().saturating_sub(1)) as u32;

        let mut pixmap = Pixmap::new(total_w.max(h), h).expect("non-zero pixmap");

        let mut x_cursor = 0u32;
        for (i, (mount, (label, layout))) in mounts.iter().zip(blocks.iter()).enumerate() {
            if i > 0 {
                x_cursor += PER_MOUNT_GAP;
            }
            self.draw_mount_block(&mut pixmap, x_cursor, mount, label, layout);
            x_cursor += layout.icon_w + 2 + layout.text_w + 2 + layout.donut_size;
        }

        if mounts.is_empty() {
            self.draw_empty(&mut pixmap, donut_size);
        }

        pixmap
    }

    fn draw_mount_block(
        &self,
        pixmap: &mut Pixmap,
        x: u32,
        mount: &Mount,
        label: &str,
        layout: &BlockLayout,
    ) {
        if let Some(ref icon) = self.base_icon {
            let icon_y = (self.height as i32 - icon.height() as i32) / 2;
            pixmap.draw_pixmap(
                x as i32,
                icon_y,
                icon.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        let neutral_color = if layout.connected {
            COLOR_TEXT
        } else {
            [0xaa, 0xaa, 0xaa, 0xff]
        };
        let px = text_size(self.height);
        let text_x = x + layout.icon_w + 2;
        // El label es el mountpoint: una sola métrica (ocupación). El wedge
        // del donut ya transmite el código de colores; mantener neutro evita
        // duplicar la señal y dejar el icono ruidoso.
        self.draw_text(pixmap, text_x as f32, label, px, neutral_color);

        let donut_x = (x + layout.icon_w + 2 + layout.text_w + 2) as f32;
        let used_pct = mount.usage.used_percent();
        draw_donut(
            pixmap,
            donut_x,
            DONUT_PADDING as f32,
            layout.donut_size,
            used_pct,
            layout.connected,
        );

        // Porcentaje numérico dentro del donut (ver consejos.md: la cifra va
        // en color neutro porque el wedge del anillo ya hace el code de color).
        let pct_text = (used_pct.round() as u32).to_string();
        let pct_size = 8.0;
        let pct_w = self.measure_text(&pct_text, pct_size) as f32;
        let pct_x = donut_x + layout.donut_size as f32 / 2.0 - pct_w / 2.0;
        self.draw_text(pixmap, pct_x, &pct_text, pct_size, neutral_color);
    }

    fn draw_empty(&self, pixmap: &mut Pixmap, donut_size: u32) {
        self.draw_text(pixmap, 0.0, "no disks", text_size(self.height), COLOR_TEXT);
        draw_donut(pixmap, 0.0, DONUT_PADDING as f32, donut_size, 0.0, false);
    }

    fn measure_text(&self, text: &str, px: f32) -> u32 {
        let px_size = px.round() as u32;
        if self.ft.face.set_pixel_sizes(0, px_size).is_err() {
            return 0;
        }
        let mut width: i64 = 0;
        for ch in text.chars() {
            if self
                .ft
                .face
                .load_char(ch as usize, LoadFlag::DEFAULT)
                .is_err()
            {
                continue;
            }
            // advance.x viene en 26.6 fixed-point; >>6 para pasar a pixeles.
            width += self.ft.face.glyph().advance().x >> 6;
        }
        width.max(0) as u32
    }

    fn draw_text(&self, pixmap: &mut Pixmap, x: f32, text: &str, px: f32, color: [u8; 4]) {
        let px_size = px.round() as u32;
        if self.ft.face.set_pixel_sizes(0, px_size).is_err() {
            return;
        }
        // FT::ascender/descender vienen en font units; convertir vía size.metrics().
        // metrics.ascender está en 26.6 fixed-point.
        let ascent_px = (self.ft.face.size_metrics().map(|m| m.ascender).unwrap_or(0) >> 6) as f32;
        let baseline_y = (((self.height as f32 - px_size as f32) / 2.0) + ascent_px).round() as i32;
        let mut pen_x = x.round() as i32;
        for ch in text.chars() {
            if self
                .ft
                .face
                .load_char(ch as usize, LoadFlag::RENDER | LoadFlag::TARGET_NORMAL)
                .is_err()
            {
                continue;
            }
            let glyph = self.ft.face.glyph();
            let bmp = glyph.bitmap();
            let buffer = bmp.buffer();
            let bw = bmp.width();
            let bh = bmp.rows();
            let pitch = bmp.pitch();
            let glyph_left = pen_x + glyph.bitmap_left();
            let glyph_top = baseline_y - glyph.bitmap_top();
            for gy in 0..bh {
                let row_start = (gy * pitch) as isize;
                for gx in 0..bw {
                    let idx = (row_start + gx as isize) as usize;
                    let coverage = buffer[idx];
                    if coverage == 0 {
                        continue;
                    }
                    let px_x = glyph_left + gx;
                    let px_y = glyph_top + gy;
                    if px_x < 0 || px_y < 0 {
                        continue;
                    }
                    blend_pixel(
                        pixmap,
                        px_x as u32,
                        px_y as u32,
                        color,
                        coverage as f32 / 255.0,
                    );
                }
            }
            pen_x += (glyph.advance().x >> 6) as i32;
        }
    }
}

fn text_size(h: u32) -> f32 {
    // Redondear a píxel entero: freetype hintea limpio solo a tamaños enteros.
    (h as f32 * 0.45).round().clamp(8.0, 16.0)
}

/// Short, panel-friendly version of a mount point used as the label next to
/// the donut. Truncates to 6 chars so the icon does not dominate the panel.
pub fn short_label(mount_point: &str) -> String {
    let trimmed = mount_point.trim_end_matches('/');
    let leaf = if trimmed.is_empty() {
        "/"
    } else {
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    };
    truncate_chars(leaf, 6)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        chars.into_iter().take(max_chars).collect()
    }
}

fn load_base_icon(path: &Path, target_h: u32) -> Result<Pixmap> {
    let img = ImageReader::open(path)
        .with_context(|| format!("opening icon {}", path.display()))?
        .decode()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let scale = target_h as f32 / h as f32;
    let new_w = ((w as f32) * scale).round().max(1.0) as u32;
    let new_h = target_h;
    let resized =
        image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
    let mut pixmap = Pixmap::new(new_w, new_h).context("alloc pixmap")?;
    let dst = pixmap.data_mut();
    for (chunk, out) in resized.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let a = chunk[3] as u32;
        out[0] = (chunk[0] as u32 * a / 255) as u8;
        out[1] = (chunk[1] as u32 * a / 255) as u8;
        out[2] = (chunk[2] as u32 * a / 255) as u8;
        out[3] = a as u8;
    }
    Ok(pixmap)
}

fn load_face() -> Result<(Library, Face)> {
    let library = Library::init().context("initializing freetype library")?;
    for path in DEFAULT_FONT_PATHS {
        if std::path::Path::new(path).exists() {
            let face = library
                .new_face(path, 0)
                .map_err(|e| anyhow!("loading font {}: {}", path, e))?;
            return Ok((library, face));
        }
    }
    anyhow::bail!(
        "DejaVu Sans Mono font not found in any of: {:?}; install fonts-dejavu-core",
        DEFAULT_FONT_PATHS
    );
}

fn used_color(pct: f32) -> [u8; 4] {
    if pct >= 90.0 {
        COLOR_HIGH
    } else if pct >= 80.0 {
        COLOR_WARN2
    } else if pct >= 70.0 {
        COLOR_WARN1
    } else {
        COLOR_OK
    }
}

fn draw_donut(pixmap: &mut Pixmap, x: f32, y: f32, size: u32, used_pct: f32, connected: bool) {
    let cx = x + size as f32 / 2.0;
    let cy = y + size as f32 / 2.0;
    let r_outer = size as f32 / 2.0;
    let r_inner = r_outer * 0.78;

    let free_color = if connected {
        COLOR_FREE
    } else {
        [0x80, 0x80, 0x80, 0xff]
    };
    fill_disk(pixmap, cx, cy, r_outer, free_color);

    if used_pct > 0.5 {
        let color = if connected {
            used_color(used_pct)
        } else {
            [0x60, 0x60, 0x60, 0xff]
        };
        let sweep = (used_pct.clamp(0.0, 100.0) / 100.0) * 360.0;
        fill_pie(pixmap, cx, cy, r_outer, -90.0, -90.0 + sweep, color);
    }

    clear_disk(pixmap, cx, cy, r_inner);
}

fn fill_disk(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
    let path = match PathBuilder::from_circle(cx, cy, r) {
        Some(p) => p,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::EvenOdd,
        Transform::identity(),
        None,
    );
}

fn clear_disk(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    let path = match PathBuilder::from_circle(cx, cy, r) {
        Some(p) => p,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 0);
    paint.blend_mode = BlendMode::Clear;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::EvenOdd,
        Transform::identity(),
        None,
    );
}

fn fill_pie(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    start_deg: f32,
    end_deg: f32,
    color: [u8; 4],
) {
    let segments = ((end_deg - start_deg).abs() / 5.0).ceil().max(2.0) as u32;
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy);
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let angle = (start_deg + t * (end_deg - start_deg)).to_radians();
        pb.line_to(cx + r * angle.cos(), cy + r * angle.sin());
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, color: [u8; 4], coverage: f32) {
    if x >= pixmap.width() || y >= pixmap.height() {
        return;
    }
    let stride = pixmap.width() as usize * 4;
    let idx = (y as usize) * stride + (x as usize) * 4;
    let data = pixmap.data_mut();
    let src_a = (coverage.clamp(0.0, 1.0) * color[3] as f32) as u32;
    if src_a == 0 {
        return;
    }
    let inv_a = 255 - src_a;
    let blend = |s: u8, d: u8| -> u8 { ((s as u32 * src_a + d as u32 * inv_a) / 255) as u8 };
    data[idx] = blend(color[0], data[idx]);
    data[idx + 1] = blend(color[1], data[idx + 1]);
    data[idx + 2] = blend(color[2], data[idx + 2]);
    data[idx + 3] = (data[idx + 3] as u32 + src_a).min(255) as u8;
}

/// Convert tiny-skia's premultiplied RGBA into straight RGBA bytes
/// (so the result encodes as a normal PNG that any image viewer renders).
fn unpremultiply_to_rgba(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (chunk, slot) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = chunk[3];
        if a == 0 {
            slot.copy_from_slice(&[0, 0, 0, 0]);
        } else {
            let unpremul = |c: u8| -> u8 {
                let v = (c as u32 * 255 + a as u32 / 2) / a as u32;
                v.min(255) as u8
            };
            slot[0] = unpremul(chunk[0]);
            slot[1] = unpremul(chunk[1]);
            slot[2] = unpremul(chunk[2]);
            slot[3] = a;
        }
    }
    out
}

/// Convert tiny-skia's premultiplied RGBA to the ARGB32 network-byte-order
/// layout that StatusNotifierItem expects (alpha as straight, not premultiplied).
fn rgba_premul_to_argb_straight(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (chunk, slot) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = chunk[3];
        slot[0] = a;
        if a == 0 {
            slot[1] = 0;
            slot[2] = 0;
            slot[3] = 0;
        } else {
            let unpremul = |c: u8| -> u8 {
                let v = (c as u32 * 255 + a as u32 / 2) / a as u32;
                v.min(255) as u8
            };
            slot[1] = unpremul(chunk[0]);
            slot[2] = unpremul(chunk[1]);
            slot[3] = unpremul(chunk[2]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_color_thresholds() {
        assert_eq!(used_color(0.0), COLOR_OK);
        assert_eq!(used_color(69.9), COLOR_OK);
        assert_eq!(used_color(70.0), COLOR_WARN1);
        assert_eq!(used_color(80.0), COLOR_WARN2);
        assert_eq!(used_color(90.0), COLOR_HIGH);
        assert_eq!(used_color(100.0), COLOR_HIGH);
    }

    #[test]
    fn opaque_pixels_passthrough_color() {
        // Premul == straight when alpha == 255.
        let rgba = vec![0x11, 0x22, 0x33, 0xff];
        let argb = rgba_premul_to_argb_straight(&rgba);
        assert_eq!(argb, vec![0xff, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn fully_transparent_pixels_become_zero() {
        let rgba = vec![0x33, 0x33, 0x33, 0x00];
        let argb = rgba_premul_to_argb_straight(&rgba);
        assert_eq!(argb, vec![0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn half_alpha_pixel_is_unpremultiplied() {
        let rgba = vec![0x00, 0x80, 0x00, 0x80];
        let argb = rgba_premul_to_argb_straight(&rgba);
        assert_eq!(argb[0], 0x80);
        assert!(
            argb[2] >= 0xfe,
            "green should round up to ~255, got {:#x}",
            argb[2]
        );
    }

    #[test]
    fn short_label_uses_leaf_path() {
        assert_eq!(short_label("/"), "/");
        assert_eq!(short_label("/home"), "home");
        assert_eq!(short_label("/boot/efi"), "efi");
        assert_eq!(short_label("/media/wallabot/seagate2T"), "seagat");
    }
}
