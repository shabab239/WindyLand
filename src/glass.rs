//! The Liquid Glass theme's backdrop: what is behind the notch, captured
//! small and blurred. The notch itself is left out of captures while that
//! theme is on (`SetWindowDisplayAffinity`), so it can see through itself;
//! the same flag keeps it out of screenshots and screen sharing then.
//!
//! Cheap by design: one GDI capture, scaled down 4x by the GPU-less
//! HALFTONE stretch, then a three-pass box blur on the small image; at most
//! ten times a second while the notch animates, about once a second at rest.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};

/// Captured pixels: (width, height, premultiplied BGRA, average luminance).
pub type Backdrop = (u32, u32, Vec<u8>, f32);

/// Shrink factor of the capture (and so of the blur's cost).
const DOWN: i32 = 4;

/// Keep the notch out of screen captures (Liquid Glass on) or not.
pub fn exclude_from_capture(hwnd: HWND, on: bool) {
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}

/// Capture the screen rectangle `r` (pixels), scaled down and blurred.
pub fn capture(r: RECT) -> Option<Backdrop> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w < 8 || h < 8 {
        return None;
    }
    let (sw, sh) = ((w / DOWN).max(2), (h / DOWN).max(2));
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sw,
                biHeight: -sh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return None;
        };
        let old = SelectObject(mem, dib);
        SetStretchBltMode(mem, HALFTONE);
        let ok = StretchBlt(mem, 0, 0, sw, sh, screen, r.left, r.top, w, h, SRCCOPY | CAPTUREBLT).as_bool();
        let mut px = vec![0u8; (sw * sh * 4) as usize];
        if ok && !bits.is_null() {
            std::ptr::copy_nonoverlapping(bits as *const u8, px.as_mut_ptr(), px.len());
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return None;
        }
        Some(finish(sw as u32, sh as u32, px))
    }
}

/// Opaque, blurred, with its average luminance.
pub fn finish(w: u32, h: u32, mut px: Vec<u8>) -> Backdrop {
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    for _ in 0..3 {
        box_blur(&mut px, w as usize, h as usize, 2);
    }
    let n = (w * h) as f32;
    let lum = px.chunks_exact(4).map(|p| 0.0722 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.2126 * p[2] as f32).sum::<f32>() / n / 255.0;
    (w, h, px, lum)
}

/// One horizontal and one vertical box blur pass of radius `r`.
fn box_blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let mut tmp = px.to_vec();
    let k = (2 * r + 1) as u32;
    for y in 0..h {
        for c in 0..3 {
            let row = |x: isize| px[(y * w + x.clamp(0, w as isize - 1) as usize) * 4 + c] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(row).sum();
            for x in 0..w {
                tmp[(y * w + x) * 4 + c] = (sum / k) as u8;
                sum += row(x as isize + r as isize + 1);
                sum -= row(x as isize - r as isize);
            }
        }
    }
    for x in 0..w {
        for c in 0..3 {
            let col = |y: isize| tmp[(y.clamp(0, h as isize - 1) as usize * w + x) * 4 + c] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(col).sum();
            for y in 0..h {
                px[(y * w + x) * 4 + c] = (sum / k) as u8;
                sum += col(y as isize + r as isize + 1);
                sum -= col(y as isize - r as isize);
            }
        }
    }
}

/// A made-up backdrop for renders and tests: "light", "dark" or a colourful
/// one (anything else), like wallpapers and windows behind the notch.
pub fn sample(kind: &str, w: u32, h: u32) -> Backdrop {
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            // Soft blobs, like a photo or a busy page behind the notch.
            let blob = |cx: f32, cy: f32, s: f32| (-((fx - cx).powi(2) + (fy - cy).powi(2)) / s).exp();
            let (r, g, b) = match kind {
                "light" => {
                    let v = 0.93 - 0.08 * blob(0.3, 0.4, 0.03);
                    (v + 0.05 * blob(0.7, 0.6, 0.02), v, v - 0.06 * blob(0.7, 0.6, 0.02))
                }
                "dark" => {
                    let v = 0.10 + 0.08 * blob(0.6, 0.3, 0.05);
                    (v, v + 0.02, v + 0.05)
                }
                _ => (
                    0.35 + 0.6 * blob(0.15, 0.3, 0.06) + 0.3 * blob(0.85, 0.7, 0.05),
                    0.25 + 0.5 * blob(0.55, 0.8, 0.05),
                    0.55 + 0.4 * blob(0.4, 0.2, 0.04) - 0.3 * blob(0.85, 0.7, 0.05),
                ),
            };
            let i = ((y * w + x) * 4) as usize;
            px[i] = (b.clamp(0.0, 1.0) * 255.0) as u8;
            px[i + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
            px[i + 2] = (r.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    finish(w, h, px)
}
