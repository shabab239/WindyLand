//! Drawing layer: Direct2D into a 32-bit DIB that is pushed to a layered
//! window with per-pixel alpha (fully transparent pixels are click-through).
//!
//! All coordinates are in DIPs; `scale` maps them to physical pixels.

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub use crate::store::Edge;

// ---------------------------------------------------------------- geometry

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn r(&self) -> f32 {
        self.x + self.w
    }
    pub fn b(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.r() && py >= self.y && py < self.b()
    }
    pub fn inset(&self, d: f32) -> Rect {
        self.inset_xy(d, d)
    }
    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    /// Slice `w` off the left; `self` keeps the rest minus `gap`.
    pub fn cut_left(&mut self, w: f32, gap: f32) -> Rect {
        let out = Rect::new(self.x, self.y, w.min(self.w), self.h);
        self.x += w + gap;
        self.w = (self.w - w - gap).max(0.0);
        out
    }
    pub fn cut_right(&mut self, w: f32, gap: f32) -> Rect {
        let out = Rect::new(self.r() - w.min(self.w), self.y, w.min(self.w), self.h);
        self.w = (self.w - w - gap).max(0.0);
        out
    }
    pub fn cut_top(&mut self, h: f32, gap: f32) -> Rect {
        let out = Rect::new(self.x, self.y, self.w, h.min(self.h));
        self.y += h + gap;
        self.h = (self.h - h - gap).max(0.0);
        out
    }
    pub fn cut_bottom(&mut self, h: f32, gap: f32) -> Rect {
        let out = Rect::new(self.x, self.b() - h.min(self.h), self.w, h.min(self.h));
        self.h = (self.h - h - gap).max(0.0);
        out
    }
    /// A `w`×`h` rect centred inside this one.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect::new(self.cx() - w / 2.0, self.cy() - h / 2.0, w, h)
    }
    /// Split horizontally into `n` equal columns with `gap` between.
    /// Split into `n` equal rows separated by `gap`.
    pub fn rows(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let h = (self.h - gap * (n - 1) as f32) / n as f32;
        (0..n).map(|i| Rect::new(self.x, self.y + i as f32 * (h + gap), self.w, h)).collect()
    }

    pub fn columns(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let cw = (self.w - gap * (n as f32 - 1.0)) / n as f32;
        (0..n).map(|i| Rect::new(self.x + i as f32 * (cw + gap), self.y, cw, self.h)).collect()
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.r(), bottom: self.b() }
    }
}

// ------------------------------------------------------------------ colour

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn hex(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

impl Color {
    pub const fn with_a(self, a: f32) -> Color {
        Color { r: self.r, g: self.g, b: self.b, a }
    }
    pub fn mul_a(self, a: f32) -> Color {
        Color { a: self.a * a, ..self }
    }
    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
}

/// Palette. Change these to re-theme the whole notch.
pub mod col {
    use super::{hex, Color};
    pub const NOTCH: Color = hex(0x000000);
    pub const TEXT: Color = hex(0xF5F5F7);
    pub const DIM: Color = hex(0x9A9AA0);
    pub const FAINT: Color = hex(0x5A5A60);
    pub const BTN: Color = hex(0xFFFFFF).with_a(0.09);
    pub const BTN_HOVER: Color = hex(0xFFFFFF).with_a(0.17);
    pub const ACCENT: Color = hex(0xFF9F0A);
    pub const GREEN: Color = hex(0x30D158);
    pub const BLUE: Color = hex(0x0A84FF);
    pub const CYAN: Color = hex(0x64D2FF);
    pub const RED: Color = hex(0xFF453A);
    pub const YELLOW: Color = hex(0xFFD60A);
    pub const PURPLE: Color = hex(0xBF5AF2);
    pub const PINK: Color = hex(0xFF375F);
}

/// Segoe MDL2 / Fluent icon glyphs used across the app.
pub mod glyph {
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const PREV: &str = "\u{E892}";
    pub const NEXT: &str = "\u{E893}";
    pub const MUSIC: &str = "\u{E8D6}";
    pub const LYRICS: &str = "\u{E8E4}";
    pub const TIMER: &str = "\u{E916}";
    pub const TODO: &str = "\u{E9D5}";
    pub const SHELF: &str = "\u{E7B8}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const FILE: &str = "\u{E7C3}";
    pub const LINK: &str = "\u{E71B}";
    pub const ADD: &str = "\u{E710}";
    pub const DELETE: &str = "\u{E74D}";
    pub const CLOSE: &str = "\u{E711}";
    pub const CHECK: &str = "\u{E73E}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const BACK: &str = "\u{E72B}";
    pub const RESET: &str = "\u{E72C}";
    pub const SKIP: &str = "\u{E893}";
    pub const ALARM: &str = "\u{E823}";
    pub const BATTERY: &str = "\u{E83F}";
    pub const CHARGING: &str = "\u{E83E}";
    pub const WIFI: &str = "\u{E701}";
    pub const WIFI_OFF: &str = "\u{EB5E}";
    pub const NO_NET: &str = "\u{E871}";
    pub const BLUETOOTH: &str = "\u{E702}";
    pub const HEADPHONE: &str = "\u{E7F6}";
    pub const VOLUME: &str = "\u{E767}";
    pub const MUTE: &str = "\u{E74F}";
    pub const KEYBOARD: &str = "\u{E765}";
    pub const CAPS: &str = "\u{E752}";
    pub const DROP: &str = "\u{EB42}";
    pub const COPY: &str = "\u{E8C8}";
    pub const PASTE: &str = "\u{E77F}";
    pub const PIN: &str = "\u{E718}";
    pub const MIC: &str = "\u{E720}";
    pub const CAMERA: &str = "\u{E722}";
    pub const IMAGE: &str = "\u{EB9F}";
    pub const BELL: &str = "\u{EA8F}";
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Semi,
    Bold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Family {
    Text,
    Icon,
    Mono,
}

// -------------------------------------------------------------------- Gfx

pub struct Gfx {
    factory: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    formats: HashMap<(u32, Weight, Align, Family, bool), IDWriteTextFormat>,
    icon_family: HSTRING,
    mem_dc: HDC,
    dib: HBITMAP,
    old_bmp: HGDIOBJ,
    bits: *mut u8,
    /// Last EndDraw failure (None = last frame OK).
    pub last_err: Option<HRESULT>,
    pub pw: i32,
    pub ph: i32,
    pub scale: f32,
    /// Global opacity multiplier applied to every draw call.
    pub alpha: f32,
    /// Bumped whenever the render target is recreated; bitmaps made with an
    /// older generation must be rebuilt (see `music.rs`).
    pub gen: u32,
    layers: Vec<ID2D1Layer>,
}

fn d2c(c: Color, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * a }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn ident() -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Gfx {
    pub fn new(pw: i32, ph: i32) -> Result<Gfx> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let rt = Self::make_rt(&factory)?;
            let brush = rt.CreateSolidColorBrush(&d2c(col::TEXT, 1.0), None)?;

            // Win11 ships "Segoe Fluent Icons"; Win10 has "Segoe MDL2 Assets".
            let mut icon_family = HSTRING::from("Segoe MDL2 Assets");
            let mut coll: Option<IDWriteFontCollection> = None;
            if dw.GetSystemFontCollection(&mut coll, false).is_ok() {
                if let Some(coll) = coll {
                    let mut idx = 0u32;
                    let mut exists = BOOL(0);
                    let name = HSTRING::from("Segoe Fluent Icons");
                    if coll.FindFamilyName(&name, &mut idx, &mut exists).is_ok() && exists.as_bool() {
                        icon_family = name;
                    }
                }
            }

            let mem_dc = CreateCompatibleDC(None);
            let mut g = Gfx {
                factory,
                dw,
                rt,
                brush,
                formats: HashMap::new(),
                icon_family,
                mem_dc,
                dib: HBITMAP::default(),
                old_bmp: HGDIOBJ::default(),
                bits: std::ptr::null_mut(),
                last_err: None,
                pw: 0,
                ph: 0,
                scale: 1.0,
                alpha: 1.0,
                gen: 0,
                layers: Vec::new(),
            };
            g.resize(pw, ph)?;
            Ok(g)
        }
    }

    unsafe fn make_rt(factory: &ID2D1Factory) -> Result<ID2D1DCRenderTarget> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = factory.CreateDCRenderTarget(&props)?;
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        Ok(rt)
    }

    pub fn resize(&mut self, pw: i32, ph: i32) -> Result<()> {
        if pw == self.pw && ph == self.ph {
            return Ok(());
        }
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.mem_dc, self.old_bmp);
                let _ = DeleteObject(self.dib);
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: pw,
                    biHeight: -ph,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            self.dib = CreateDIBSection(self.mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.old_bmp = SelectObject(self.mem_dc, self.dib);
            self.bits = bits as *mut u8;
            self.pw = pw;
            self.ph = ph;
            let rc = RECT { left: 0, top: 0, right: pw, bottom: ph };
            self.rt.BindDC(self.mem_dc, &rc)?;
        }
        Ok(())
    }

    pub fn begin(&mut self, scale: f32) {
        self.scale = scale;
        self.alpha = 1.0;
        unsafe {
            self.rt.BeginDraw();
            self.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let m = Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: 0.0, M32: 0.0 };
            self.rt.SetTransform(&m);
        }
    }

    /// Finish the frame and push it to the layered window at screen (x, y).
    pub fn end_and_present(&mut self, hwnd: HWND, x: i32, y: i32) {
        unsafe {
            let res = self.rt.EndDraw(None, None);
            self.last_err = res.as_ref().err().map(|e| e.code());
            if let Err(e) = res {
                if e.code() == D2DERR_RECREATE_TARGET {
                    if let Ok(rt) = Self::make_rt(&self.factory) {
                        self.rt = rt;
                        if let Ok(b) = self.rt.CreateSolidColorBrush(&d2c(col::TEXT, 1.0), None) {
                            self.brush = b;
                        }
                        self.layers.clear();
                        self.gen += 1;
                        let (pw, ph) = (self.pw, self.ph);
                        self.pw = 0;
                        let _ = self.resize(pw, ph);
                    }
                }
                return;
            }
            let screen = GetDC(None);
            let pos = POINT { x, y };
            let size = SIZE { cx: self.pw, cy: self.ph };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                screen,
                Some(&pos),
                Some(&size),
                self.mem_dc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
        }
    }

    /// Number of non-transparent pixels in the last frame (self-test aid).
    pub fn coverage(&self) -> usize {
        if self.bits.is_null() {
            return 0;
        }
        let n = (self.pw * self.ph) as usize;
        let px = unsafe { std::slice::from_raw_parts(self.bits, n * 4) };
        px.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    /// Bounding box (x0, y0, x1, y1) of non-transparent pixels (self-test aid).
    pub fn bbox(&self) -> Option<(i32, i32, i32, i32)> {
        if self.bits.is_null() {
            return None;
        }
        let px = unsafe { std::slice::from_raw_parts(self.bits, (self.pw * self.ph * 4) as usize) };
        let mut b: Option<(i32, i32, i32, i32)> = None;
        for y in 0..self.ph {
            for x in 0..self.pw {
                if px[((y * self.pw + x) * 4 + 3) as usize] > 0 {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                    });
                }
            }
        }
        b
    }

    /// Copy of the last frame as premultiplied BGRA (docs image export).
    pub fn pixels(&self) -> Vec<u8> {
        if self.bits.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(self.bits, (self.pw * self.ph * 4) as usize).to_vec() }
    }

    /// Number of bright (text/icon-like) pixels in the last frame.
    pub fn bright(&self) -> usize {
        if self.bits.is_null() {
            return 0;
        }
        let n = (self.pw * self.ph) as usize;
        let px = unsafe { std::slice::from_raw_parts(self.bits, n * 4) };
        px.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 300).count()
    }

    fn set(&self, c: Color) {
        unsafe { self.brush.SetColor(&d2c(c, self.alpha)) }
    }

    // ------------------------------------------------------------ shapes

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        self.set(c);
        unsafe { self.rt.FillRectangle(&r.d2d(), &self.brush) }
    }

    pub fn fill_rr(&mut self, r: Rect, rad: f32, c: Color) {
        self.set(c);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: rad, radiusY: rad };
        unsafe { self.rt.FillRoundedRectangle(&rr, &self.brush) }
    }

    pub fn stroke_rr(&mut self, r: Rect, rad: f32, c: Color, w: f32) {
        self.set(c);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: rad, radiusY: rad };
        unsafe { self.rt.DrawRoundedRectangle(&rr, &self.brush, w, None) }
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color) {
        self.set(c);
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: rx, radiusY: ry };
        unsafe { self.rt.FillEllipse(&e, &self.brush) }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        self.ellipse(cx, cy, r, r, c);
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color, w: f32) {
        self.set(c);
        let e = D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r };
        unsafe { self.rt.DrawEllipse(&e, &self.brush, w, None) }
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color, w: f32) {
        self.set(c);
        unsafe { self.rt.DrawLine(pt(x1, y1), pt(x2, y2), &self.brush, w, None) }
    }

    fn path(&self, build: impl FnOnce(&ID2D1GeometrySink)) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            build(&sink);
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// Stroke a circular arc. Angles in radians, 0 = 12 o'clock, clockwise.
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, sweep: f32, c: Color, w: f32) {
        if sweep.abs() < 0.001 {
            return;
        }
        let sweep = sweep.clamp(-6.2831, 6.2831);
        let p = |a: f32| pt(cx + r * a.sin(), cy - r * a.cos());
        let start = p(a0);
        let end = p(a0 + sweep);
        let geo = self.path(|s| unsafe {
            s.BeginFigure(start, D2D1_FIGURE_BEGIN_HOLLOW);
            s.AddArc(&D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: if sweep > 0.0 { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                arcSize: if sweep.abs() > std::f32::consts::PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            s.EndFigure(D2D1_FIGURE_END_OPEN);
        });
        if let Some(geo) = geo {
            self.set(c);
            unsafe {
                let style = self
                    .factory
                    .CreateStrokeStyle(
                        &D2D1_STROKE_STYLE_PROPERTIES {
                            startCap: D2D1_CAP_STYLE_ROUND,
                            endCap: D2D1_CAP_STYLE_ROUND,
                            ..Default::default()
                        },
                        None,
                    )
                    .ok();
                self.rt.DrawGeometry(&geo, &self.brush, w, style.as_ref());
            }
        }
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)], c: Color) {
        if pts.len() < 3 {
            return;
        }
        let geo = self.path(|s| unsafe {
            s.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
            for p in &pts[1..] {
                s.AddLine(pt(p.0, p.1));
            }
            s.EndFigure(D2D1_FIGURE_END_CLOSED);
        });
        if let Some(geo) = geo {
            self.set(c);
            unsafe { self.rt.FillGeometry(&geo, &self.brush, None) }
        }
    }

    /// The notch silhouette: flat side flush with the screen `edge`, rounded
    /// corners on the inner side, and small concave "ears" flaring into the edge.
    /// Built in edge-local coordinates (u along the edge, v away from it).
    pub fn notch_geometry(&self, r: Rect, rad: f32, ear: f32, edge: Edge) -> Option<ID2D1PathGeometry> {
        let (len, dep) = if edge.vertical() { (r.h, r.w) } else { (r.w, r.h) };
        let rad = rad.min(dep / 2.0).min(len / 2.0).max(0.0);
        let map = |u: f32, v: f32| match edge {
            Edge::Top => pt(r.x + u, r.y + v),
            Edge::Bottom => pt(r.x + u, r.b() - v),
            Edge::Left => pt(r.x + v, r.y + u),
            Edge::Right => pt(r.r() - v, r.y + u),
        };
        // Mirrored mappings reverse the arc direction.
        let sweep = if matches!(edge, Edge::Bottom | Edge::Left) {
            D2D1_SWEEP_DIRECTION_CLOCKWISE
        } else {
            D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
        };
        let arc = |p: D2D_POINT_2F| D2D1_ARC_SEGMENT {
            point: p,
            size: D2D_SIZE_F { width: rad, height: rad },
            rotationAngle: 0.0,
            sweepDirection: sweep,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        self.path(|s| unsafe {
            s.BeginFigure(map(-ear, 0.0), D2D1_FIGURE_BEGIN_FILLED);
            s.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: map(0.0, 0.0), point2: map(0.0, ear) });
            s.AddLine(map(0.0, dep - rad));
            s.AddArc(&arc(map(rad, dep)));
            s.AddLine(map(len - rad, dep));
            s.AddArc(&arc(map(len, dep - rad)));
            s.AddLine(map(len, ear));
            s.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: map(len, 0.0), point2: map(len + ear, 0.0) });
            s.EndFigure(D2D1_FIGURE_END_CLOSED);
        })
    }

    pub fn fill_geometry(&mut self, geo: &ID2D1PathGeometry, c: Color) {
        self.set(c);
        unsafe { self.rt.FillGeometry(geo, &self.brush, None) }
    }

    /// Clip subsequent drawing to `geo` and fade it by `opacity`.
    pub fn push_layer(&mut self, geo: Option<&ID2D1PathGeometry>, opacity: f32) {
        unsafe {
            let layer = match self.rt.CreateLayer(None) {
                Ok(l) => l,
                Err(_) => return,
            };
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: D2D_RECT_F { left: -1e6, top: -1e6, right: 1e6, bottom: 1e6 },
                geometricMask: ManuallyDrop::new(geo.map(|g| g.cast::<ID2D1Geometry>().unwrap())),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: ident(),
                opacity: opacity.clamp(0.0, 1.0),
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            self.rt.PushLayer(&params, &layer);
            let mut p = params;
            ManuallyDrop::drop(&mut p.geometricMask);
            self.layers.push(layer);
        }
    }

    pub fn pop_layer(&mut self) {
        if self.layers.pop().is_some() {
            unsafe { self.rt.PopLayer() }
        }
    }

    pub fn push_clip(&mut self, r: Rect) {
        unsafe { self.rt.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn pop_clip(&mut self) {
        unsafe { self.rt.PopAxisAlignedClip() }
    }

    // -------------------------------------------------------------- text

    fn format(&mut self, size: f32, weight: Weight, align: Align, fam: Family, wrap: bool) -> Option<IDWriteTextFormat> {
        let key = ((size * 4.0) as u32, weight, align, fam, wrap);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        unsafe {
            let family = match fam {
                Family::Text => HSTRING::from("Segoe UI"),
                Family::Icon => self.icon_family.clone(),
                Family::Mono => HSTRING::from("Segoe UI"),
            };
            let w = match weight {
                Weight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
                Weight::Semi => DWRITE_FONT_WEIGHT_SEMI_BOLD,
                Weight::Bold => DWRITE_FONT_WEIGHT_BOLD,
            };
            let f = self
                .dw
                .CreateTextFormat(&family, None, w, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
                .ok()?;
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            if wrap {
                let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            } else {
                let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&f) {
                    let trim = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = f.SetTrimming(&trim, &sign);
                }
            }
            self.formats.insert(key, f.clone());
            Some(f)
        }
    }

    fn draw_str(&mut self, s: &str, r: Rect, f: &IDWriteTextFormat, c: Color) {
        self.set(c);
        let w = wide(s);
        unsafe {
            self.rt.DrawText(
                &w,
                f,
                &r.d2d(),
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        }
    }

    /// Single line, vertically centred in `r`, trimmed with an ellipsis.
    pub fn text(&mut self, s: &str, r: Rect, size: f32, c: Color, weight: Weight, align: Align) {
        if let Some(f) = self.format(size, weight, align, Family::Text, false) {
            self.draw_str(s, r, &f, c);
        }
    }

    /// Wrapped paragraph starting at the top of `r`.
    pub fn text_wrap(&mut self, s: &str, r: Rect, size: f32, c: Color, weight: Weight, align: Align) {
        if let Some(f) = self.format(size, weight, align, Family::Text, true) {
            self.draw_str(s, r, &f, c);
        }
    }

    /// Tabular-ish digits for timers (Segoe UI digits are already equal-width).
    pub fn digits(&mut self, s: &str, r: Rect, size: f32, c: Color, align: Align) {
        if let Some(f) = self.format(size, Weight::Semi, align, Family::Mono, false) {
            self.draw_str(s, r, &f, c);
        }
    }

    pub fn icon(&mut self, g: &str, r: Rect, size: f32, c: Color) {
        if let Some(f) = self.format(size, Weight::Regular, Align::Center, Family::Icon, false) {
            self.draw_str(g, r, &f, c);
        }
    }

    pub fn measure(&mut self, s: &str, size: f32, weight: Weight) -> f32 {
        let Some(f) = self.format(size, weight, Align::Left, Family::Text, false) else {
            return 0.0;
        };
        unsafe {
            let w = wide(s);
            let Ok(layout) = self.dw.CreateTextLayout(&w, &f, 10000.0, 100.0) else {
                return 0.0;
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                m.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        }
    }

    // ------------------------------------------------------------ bitmaps

    /// Create a bitmap from premultiplied BGRA pixels.
    pub fn make_bitmap(&mut self, w: u32, h: u32, bgra: &[u8]) -> Option<ID2D1Bitmap> {
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            self.rt
                .CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(bgra.as_ptr() as _), w * 4, &props)
                .ok()
        }
    }

    /// Draw a bitmap stretched into a rounded rectangle.
    pub fn image_rr(&mut self, bmp: &ID2D1Bitmap, r: Rect, rad: f32) {
        unsafe {
            let sz = bmp.GetSize();
            if sz.width <= 0.0 || sz.height <= 0.0 {
                return;
            }
            let Ok(brush) = self.rt.CreateBitmapBrush(bmp, None, None) else { return };
            let sx = r.w / sz.width;
            let sy = r.h / sz.height;
            let m = Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: r.x, M32: r.y };
            brush.SetTransform(&m);
            brush.SetOpacity(self.alpha);
            let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: rad, radiusY: rad };
            self.rt.FillRoundedRectangle(&rr, &brush);
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            if !self.dib.is_invalid() {
                SelectObject(self.mem_dc, self.old_bmp);
                let _ = DeleteObject(self.dib);
            }
            let _ = DeleteDC(self.mem_dc);
        }
    }
}

/// Build a 32×32 tray icon procedurally: a black pill with two white eyes.
pub fn make_tray_icon() -> HICON {
    unsafe {
        let n = 32i32;
        let mut px = vec![0u8; (n * n * 4) as usize];
        let set = |px: &mut Vec<u8>, x: i32, y: i32, v: u8, a: u8| {
            let i = ((y * n + x) * 4) as usize;
            px[i] = v;
            px[i + 1] = v;
            px[i + 2] = v;
            px[i + 3] = a;
        };
        for y in 0..n {
            for x in 0..n {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                // rounded rect 2..30 x 7..25, radius 9
                let (l, t, r, b, rad) = (2.0, 7.0, 30.0, 25.0, 9.0f32);
                let cx = fx.clamp(l + rad, r - rad);
                let cy = fy.clamp(t + rad, b - rad);
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let inside = fx >= l && fx <= r && fy >= t && fy <= b && d <= rad;
                if inside {
                    set(&mut px, x, y, 20, 255);
                }
                for ex in [11.5f32, 20.5] {
                    let d = (((fx - ex) / 2.4).powi(2) + ((fy - 16.0) / 3.6).powi(2)).sqrt();
                    if d <= 1.0 {
                        set(&mut px, x, y, 255, 255);
                    }
                }
            }
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n,
                biHeight: -n,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            ReleaseDC(None, dc);
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(n, n, 1, 1, None);
        ReleaseDC(None, dc);
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}
