//! Screenshot to the Shelf: freeze the screen, drag a rectangle (or click
//! for the whole monitor), and the picture is saved as a PNG in
//! Pictures\Screenshots, put on the Shelf ready to drag into a chat, and
//! copied to the clipboard. GDI window over every monitor, like the colour
//! picker; one at a time.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::modules::tools::picker::{capture_without, show_app, Snap};

/// A captured picture: top-down BGRA, fully opaque.
#[derive(Clone)]
pub struct Shot {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// Outcome of the last capture: Some(Some(shot)) taken, Some(None) cancelled.
pub static RESULT: Mutex<Option<Option<Shot>>> = Mutex::new(None);

struct Overlay {
    hwnd: HWND,
    app: isize,
    snap: Snap,
    dim: HDC,
    dim_bmp: HBITMAP,
    dim_old: HGDIOBJ,
    start: Option<(i32, i32)>,
    cur: (i32, i32),
    font: HFONT,
}

thread_local! {
    static OV: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

pub fn active() -> bool {
    OV.with(|o| o.borrow().is_some())
}

/// Crop `r` (client pixels) out of the frozen screenshot.
pub fn crop(snap: &Snap, r: RECT) -> Option<Shot> {
    let l = r.left.clamp(0, snap.w);
    let t = r.top.clamp(0, snap.h);
    let rr = r.right.clamp(0, snap.w);
    let b = r.bottom.clamp(0, snap.h);
    let (w, h) = (rr - l, b - t);
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in t..b {
        let i = ((y * snap.w + l) * 4) as usize;
        out.extend_from_slice(&snap.px[i..i + (w * 4) as usize]);
    }
    for a in out.iter_mut().skip(3).step_by(4) {
        *a = 255;
    }
    Some(Shot { w: w as u32, h: h as u32, bgra: out })
}

fn norm(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0) + 1, bottom: a.1.max(b.1) + 1 }
}

/// The monitor under client point `p`, in client pixels.
fn monitor_rect(snap: &Snap, p: (i32, i32)) -> RECT {
    unsafe {
        let m = MonitorFromPoint(POINT { x: p.0 + snap.x, y: p.1 + snap.y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            let r = mi.rcMonitor;
            RECT { left: r.left - snap.x, top: r.top - snap.y, right: r.right - snap.x, bottom: r.bottom - snap.y }
        } else {
            RECT { left: 0, top: 0, right: snap.w, bottom: snap.h }
        }
    }
}

pub fn start(app: HWND) -> bool {
    if active() || crate::modules::tools::picker::active() {
        return true;
    }
    let Some(snap) = capture_without(app) else { return false };
    unsafe {
        static REG: std::sync::Once = std::sync::Once::new();
        let Ok(hinst) = GetModuleHandleW(None) else { return false };
        REG.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: hinst.into(),
                lpszClassName: w!("WindyLandScreenshot"),
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        // A darkened copy of the screenshot for outside the selection.
        let screen = GetDC(None);
        let dim = CreateCompatibleDC(screen);
        let dim_bmp = CreateCompatibleBitmap(screen, snap.w, snap.h);
        ReleaseDC(None, screen);
        let mut px = snap.px.clone();
        for (i, v) in px.iter_mut().enumerate() {
            if i % 4 != 3 {
                *v = (*v as u16 * 115 / 255) as u8;
            }
        }
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: snap.w, biHeight: -snap.h, biPlanes: 1, biBitCount: 32, ..Default::default() };
        SetDIBits(dim, dim_bmp, 0, snap.h as u32, px.as_ptr() as *const _, &bi, DIB_RGB_COLORS);
        let dim_old = SelectObject(dim, dim_bmp);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, w!("WindyLandScreenshot"), w!("WindyLand screenshot"), WS_POPUP, snap.x, snap.y, snap.w, snap.h, None, None, hinst, None) else {
            SelectObject(dim, dim_old);
            let _ = DeleteObject(dim_bmp);
            let _ = DeleteDC(dim);
            show_app(app);
            return false;
        };
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let font = CreateFontW(-(13.0 * scale) as i32, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let ov = Overlay { hwnd, app: app.0 as isize, cur: (p.x - snap.x, p.y - snap.y), snap, dim, dim_bmp, dim_old, start: None, font };
        OV.with(|o| *o.borrow_mut() = Some(ov));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
    }
    true
}

fn finish(shot: Option<Shot>) {
    let Some(ov) = OV.with(|o| o.borrow_mut().take()) else { return };
    unsafe {
        let _ = ReleaseCapture();
        let _ = DestroyWindow(ov.hwnd);
        SelectObject(ov.dim, ov.dim_old);
        let _ = DeleteObject(ov.dim_bmp);
        let _ = DeleteDC(ov.dim);
        let _ = DeleteObject(ov.font);
        show_app(HWND(ov.app as *mut _));
        *RESULT.lock().unwrap() = Some(shot);
        let _ = PostMessageW(HWND(ov.app as *mut _), crate::app::WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

/// Self-test: drag from (x0, y0) to (x1, y1) in an open capture window.
pub fn test_drag(x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
    let Some(hwnd) = OV.with(|o| o.borrow().as_ref().map(|v| v.hwnd)) else { return false };
    let lp = |x: i32, y: i32| LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize));
    unsafe {
        SendMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), lp(x0, y0));
        SendMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), lp(x0, y0));
        SendMessageW(hwnd, WM_MOUSEMOVE, WPARAM(1), lp(x1, y1));
        SendMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), lp(x1, y1));
    }
    true
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe fn paint(ov: &Overlay) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(ov.hwnd, &mut ps);
    let rc = ps.rcPaint;
    let (rw, rh) = (rc.right - rc.left, rc.bottom - rc.top);
    if rw > 0 && rh > 0 {
        let bb = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, rw, rh);
        let old = SelectObject(bb, bmp);
        let _ = SetViewportOrgEx(bb, -rc.left, -rc.top, None);
        let _ = BitBlt(bb, rc.left, rc.top, rw, rh, ov.dim, rc.left, rc.top, SRCCOPY);
        let scale = GetDpiForWindow(ov.hwnd) as f32 / 96.0;
        let white = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let dark = CreateSolidBrush(COLORREF(0x00241E1C));
        SetBkMode(bb, TRANSPARENT);
        SetTextColor(bb, COLORREF(0x00F7F5F5));
        let oldf = SelectObject(bb, ov.font);
        let pad = (8.0 * scale) as i32;
        let label = |text: &str, x: i32, y: i32| {
            let mut t = wide(text);
            let mut m = RECT::default();
            DrawTextW(bb, &mut t, &mut m, DT_CALCRECT | DT_SINGLELINE);
            let (w, h) = (m.right - m.left + 2 * pad, m.bottom - m.top + pad);
            let x = x.clamp(0, (ov.snap.w - w).max(0));
            let y = y.clamp(0, (ov.snap.h - h).max(0));
            let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
            FillRect(bb, &r, dark);
            let mut tr = r;
            DrawTextW(bb, &mut t, &mut tr, DT_CENTER | DT_SINGLELINE | DT_VCENTER);
        };
        match ov.start {
            Some(a) => {
                let s = norm(a, ov.cur);
                let _ = BitBlt(bb, s.left, s.top, s.right - s.left, s.bottom - s.top, ov.snap.mem(), s.left, s.top, SRCCOPY);
                FrameRect(bb, &RECT { left: s.left - 1, top: s.top - 1, right: s.right + 1, bottom: s.bottom + 1 }, white);
                let gap = (28.0 * scale) as i32;
                let y = if s.top > gap + 4 { s.top - gap } else { s.bottom + 6 };
                label(&format!("{} \u{00D7} {}", s.right - s.left, s.bottom - s.top), s.left, y);
            }
            None => {
                let gap = (18.0 * scale) as i32;
                label("Drag to capture \u{00B7} click for the whole screen \u{00B7} Esc cancels", ov.cur.0 + gap, ov.cur.1 + gap);
            }
        }
        SelectObject(bb, oldf);
        let _ = DeleteObject(white);
        let _ = DeleteObject(dark);
        let _ = SetViewportOrgEx(bb, 0, 0, None);
        let _ = BitBlt(hdc, rc.left, rc.top, rw, rh, bb, 0, 0, SRCCOPY);
        SelectObject(bb, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(bb);
    }
    let _ = EndPaint(ov.hwnd, &ps);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let xy = || (((lp.0 & 0xffff) as u16 as i16) as i32, (((lp.0 >> 16) & 0xffff) as u16 as i16) as i32);
    match msg {
        WM_PAINT => {
            let done = OV.with(|o| {
                if let Some(ov) = o.borrow().as_ref() {
                    paint(ov);
                    true
                } else {
                    false
                }
            });
            if !done {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            OV.with(|o| {
                if let Some(ov) = o.borrow_mut().as_mut() {
                    ov.cur = xy();
                    let _ = InvalidateRect(ov.hwnd, None, false);
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let p = xy();
            OV.with(|o| {
                if let Some(ov) = o.borrow_mut().as_mut() {
                    ov.start = Some(p);
                    ov.cur = p;
                }
            });
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let p = xy();
            let shot = OV.with(|o| {
                let o = o.borrow();
                let ov = o.as_ref()?;
                let a = ov.start?;
                let mut r = norm(a, p);
                // A click (no real drag) takes the whole monitor.
                if r.right - r.left < 6 || r.bottom - r.top < 6 {
                    r = monitor_rect(&ov.snap, p);
                }
                crop(&ov.snap, r)
            });
            if shot.is_some() {
                finish(shot);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            finish(None);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            if vk == VK_ESCAPE {
                finish(None);
            }
            LRESULT(0)
        }
        WM_ACTIVATEAPP if wp.0 == 0 => {
            finish(None);
            LRESULT(0)
        }
        WM_DPICHANGED => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ------------------------------------------------------------ saving

/// Pictures\Screenshots (where Windows keeps its own), or the data folder
/// while testing.
pub fn folder() -> PathBuf {
    if std::env::var("WINDYLAND_DATA_DIR").is_ok() {
        return crate::store::dir().join("Screenshots");
    }
    use windows::Win32::UI::Shell::*;
    let pics = unsafe { SHGetKnownFolderPath(&FOLDERID_Pictures, KNOWN_FOLDER_FLAG(0), None) }.ok().map(|p| {
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        PathBuf::from(s)
    });
    pics.unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Pictures")).join("Screenshots")
}

/// "Screenshot 2026-11-02 162005.png", numbered if taken twice a second.
pub fn new_path(dir: &Path) -> PathBuf {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let base = format!("Screenshot {:04}-{:02}-{:02} {:02}{:02}{:02}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond);
    let mut p = dir.join(format!("{base}.png"));
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({k}).png"));
        k += 1;
    }
    p
}

pub fn save_png(path: &Path, s: &Shot) -> bool {
    unsafe {
        let run = || -> Result<()> {
            let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = f.CreateStream()?;
            let name = HSTRING::from(path.as_os_str());
            stream.InitializeFromFilename(&name, GENERIC_WRITE.0)?;
            let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
            enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let mut frame = None;
            let mut props = None;
            enc.CreateNewFrame(&mut frame, &mut props)?;
            let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
            frame.Initialize(props.as_ref())?;
            frame.SetSize(s.w, s.h)?;
            let mut fmt = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut fmt)?;
            frame.WritePixels(s.h, s.w * 4, &s.bgra)?;
            frame.Commit()?;
            enc.Commit()
        };
        run().is_ok()
    }
}

/// The picture as a 32-bit bottom-up CF_DIB (header + rows).
pub fn dib32(s: &Shot) -> Vec<u8> {
    let mut d = Vec::with_capacity(40 + s.bgra.len());
    let p32 = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
    p32(&mut d, 40);
    p32(&mut d, s.w);
    p32(&mut d, s.h);
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    p32(&mut d, 0);
    p32(&mut d, s.bgra.len() as u32);
    for _ in 0..4 {
        p32(&mut d, 0);
    }
    let row = (s.w * 4) as usize;
    for y in (0..s.h as usize).rev() {
        d.extend_from_slice(&s.bgra[y * row..(y + 1) * row]);
    }
    d
}
