//! Taskbar mascot: the mascot can also live in the empty
//! corner of the Windows taskbar, where Windows puts the Widgets weather.
//!
//! It is its own small layered, topmost window laid over the taskbar (the
//! taskbar itself takes no child windows on Windows 11). Clicking the
//! taskbar raises it above us, so a foreground-change hook and a slow timer
//! put the mascot back on top. Transparent pixels are click-through, so
//! only the mascot itself takes the mouse.
//!
//! Where it sits:
//! - Taskbar icons centred (Windows 11 default): the left corner, or just
//!   right of the Widgets weather when that is on.
//! - Icons on the left (or Windows 10): the corner holds Start, so it sits
//!   just left of the clock and tray icons instead.
//! - Taskbar on the left/right edge, auto-hidden, or a full-screen app: hidden.

use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::Gfx;

/// Window size in DIPs: the mascot box plus room for the hover hint.
pub const PET_W: f32 = 236.0;
pub const MASCOT_W: f32 = 58.0;

pub const TIMER_FRAME: usize = 1;
pub const TIMER_PLACE: usize = 2;

/// Where the window goes (screen pixels) and how it is laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    /// The hint goes left of the mascot (mascot at the right end).
    pub hint_left: bool,
}

pub struct Pet {
    pub hwnd: HWND,
    pub gfx: Gfx,
    pub place: Option<Place>,
    pub hover: bool,
    pub hover_since: f64,
    pub shown: bool,
    hook: HWINEVENTHOOK,
}

/// Bounds of the Widgets (weather) button, found by a background thread
/// through UI Automation; None when Widgets is off.
static WIDGETS: Mutex<Option<RECT>> = Mutex::new(None);

/// Work out the window's place. Pure, so it can be tested with made-up
/// rects. `tb` is the taskbar, `mon` its monitor, `start` the Start button,
/// `tray` the clock/tray area, `widgets` the weather button.
pub fn compute(tb: RECT, mon: RECT, start: Option<RECT>, tray: Option<RECT>, widgets: Option<RECT>, dpi: u32) -> Option<Place> {
    let s = dpi.max(96) as f32 / 96.0;
    let (tw, th) = (tb.right - tb.left, tb.bottom - tb.top);
    if tw <= th || th <= 0 {
        return None; // vertical taskbar (Windows 10): not supported
    }
    // The part of the taskbar on screen (auto-hide slides it away).
    let top = tb.top.max(mon.top);
    let bottom = tb.bottom.min(mon.bottom);
    let h = bottom - top;
    if h < 8 || (h as f32) < th as f32 * 0.6 {
        return None;
    }
    let w = (PET_W * s).round() as i32;
    let mw = (MASCOT_W * s).round() as i32;
    let gap = (6.0 * s).round() as i32;
    let centred = start.map_or(false, |st| st.left - tb.left > (120.0 * s) as i32);
    if centred {
        let mut x = tb.left + gap;
        if let Some(wd) = widgets.filter(|wd| wd.right > wd.left && wd.left < tb.left + (420.0 * s) as i32 && wd.top < tb.bottom && wd.bottom > tb.top) {
            x = wd.right + gap;
        }
        // Keep clear of the centred icons.
        if let Some(st) = start {
            if x + mw + gap > st.left {
                return None;
            }
        }
        Some(Place { x, y: top, w, h, scale: s, hint_left: false })
    } else {
        let tray = tray?;
        let right = tray.left - gap;
        if right - mw <= tb.left + (200.0 * s) as i32 {
            return None;
        }
        Some(Place { x: right - w, y: top, w, h, scale: s, hint_left: true })
    }
}

fn rect_of(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { (!h.0.is_null() && GetWindowRect(h, &mut r).is_ok()).then_some(r) }
}

/// The live layout of the primary taskbar.
pub fn locate() -> Option<Place> {
    unsafe {
        let tb = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        if !IsWindowVisible(tb).as_bool() {
            return None;
        }
        let tbr = rect_of(tb)?;
        let mon = MonitorFromWindow(tb, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return None;
        }
        let child = |class: PCWSTR| FindWindowExW(tb, None, class, None).ok().and_then(rect_of);
        let start = child(w!("Start"));
        let tray = child(w!("TrayNotifyWnd"));
        let widgets = *WIDGETS.lock().unwrap();
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(tb);
        compute(tbr, mi.rcMonitor, start, tray, widgets, dpi)
    }
}

/// Look for the Widgets button every few seconds on a background thread
/// (UI Automation calls into Explorer; never on the UI thread).
fn start_widgets_watch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(|| unsafe {
            use windows::Win32::System::Com::*;
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return };
            loop {
                let found = (|| -> Option<RECT> {
                    let tb = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
                    let root = uia.ElementFromHandle(tb).ok()?;
                    let cond = uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from("WidgetsButton"))).ok()?;
                    let el = root.FindFirst(TreeScope_Descendants, &cond).ok()?;
                    let r = el.CurrentBoundingRectangle().ok()?;
                    (r.right > r.left && r.bottom > r.top).then_some(r)
                })();
                *WIDGETS.lock().unwrap() = found;
                std::thread::sleep(std::time::Duration::from_secs(4));
            }
        });
    });
}

extern "system" fn pet_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(Some(r)) = crate::app::with_app(|a| a.pet_msg(hwnd, msg, wp, lp)) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Another window came to the front (often the taskbar itself): get back
/// on top of it.
extern "system" fn on_foreground(_h: HWINEVENTHOOK, _e: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    crate::app::with_app(|a| a.pet_raise());
}

impl Pet {
    pub fn create() -> Option<Pet> {
        start_widgets_watch();
        unsafe {
            static REG: std::sync::Once = std::sync::Once::new();
            let hinst = GetModuleHandleW(None).ok()?;
            REG.call_once(|| {
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(pet_proc),
                    hInstance: hinst.into(),
                    lpszClassName: w!("WindyLandTaskbarPet"),
                    hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassExW(&wc);
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("WindyLandTaskbarPet"),
                w!("WindyLand mascot"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                hinst,
                None,
            )
            .ok()?;
            let gfx = Gfx::new(10, 10).ok()?;
            let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
            SetTimer(hwnd, TIMER_PLACE, 500, None);
            Some(Pet { hwnd, gfx, place: None, hover: false, hover_since: 0.0, shown: false, hook })
        }
    }

    pub fn destroy(self) {
        unsafe {
            let _ = UnhookWinEvent(self.hook);
            let _ = KillTimer(self.hwnd, TIMER_FRAME);
            let _ = KillTimer(self.hwnd, TIMER_PLACE);
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// Show at `place` (or hide when None); keep it above the taskbar.
    pub fn set_place(&mut self, place: Option<Place>) {
        unsafe {
            match place {
                Some(p) => {
                    if self.place != Some(p) || !self.shown {
                        let _ = self.gfx.resize(p.w, p.h);
                        let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, p.x, p.y, p.w, p.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                        SetTimer(self.hwnd, TIMER_FRAME, 40, None);
                    }
                    self.shown = true;
                }
                None => {
                    if self.shown {
                        let _ = ShowWindow(self.hwnd, SW_HIDE);
                        let _ = KillTimer(self.hwnd, TIMER_FRAME);
                    }
                    self.shown = false;
                    self.hover = false;
                }
            }
        }
        self.place = place;
    }

    pub fn raise(&self) {
        if self.shown {
            unsafe {
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER);
            }
        }
    }
}

/// Self-test: the same placement rules as the unit tests.
pub fn self_check() -> bool {
    let r = |l, t, rr, b| RECT { left: l, top: t, right: rr, bottom: b };
    let (mon, tb) = (r(0, 0, 1920, 1080), r(0, 1032, 1920, 1080));
    let start = Some(r(717, 1032, 762, 1080));
    let corner = compute(tb, mon, start, None, None, 96).map(|p| (p.x, p.y, p.hint_left)) == Some((6, 1032, false));
    let weather = compute(tb, mon, start, None, Some(r(0, 1032, 168, 1080)), 96).map(|p| p.x) == Some(174);
    let tray = compute(tb, mon, Some(r(0, 1032, 48, 1080)), Some(r(1518, 1032, 1920, 1080)), None, 96).map(|p| (p.x + p.w, p.hint_left)) == Some((1512, true));
    let hidden = compute(r(0, 1078, 1920, 1126), mon, start, None, None, 96).is_none() && compute(r(0, 0, 62, 1040), mon, None, None, None, 96).is_none();
    corner && weather && tray && hidden
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: rr, bottom: b }
    }
    const MON: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    const TB: RECT = RECT { left: 0, top: 1032, right: 1920, bottom: 1080 };

    #[test]
    fn centred_icons_use_the_left_corner() {
        // His taskbar: Start at 717, tray from 1518, Widgets off.
        let p = compute(TB, MON, Some(r(717, 1032, 762, 1080)), Some(r(1518, 1032, 1920, 1080)), None, 96).unwrap();
        assert_eq!((p.x, p.y, p.h, p.hint_left), (6, 1032, 48, false));
    }

    #[test]
    fn sits_right_of_the_weather() {
        let p = compute(TB, MON, Some(r(717, 1032, 762, 1080)), None, Some(r(0, 1032, 168, 1080)), 96).unwrap();
        assert_eq!(p.x, 174);
    }

    #[test]
    fn left_aligned_icons_put_it_beside_the_tray() {
        let p = compute(TB, MON, Some(r(0, 1032, 48, 1080)), Some(r(1518, 1032, 1920, 1080)), None, 96).unwrap();
        assert!(p.hint_left);
        assert_eq!(p.x + p.w, 1512);
    }

    #[test]
    fn hidden_when_auto_hidden_vertical_or_cramped() {
        // Auto-hide: only 2 px on screen.
        assert!(compute(r(0, 1078, 1920, 1126), MON, Some(r(717, 1078, 762, 1126)), None, None, 96).is_none());
        // Taskbar on the left edge (Windows 10).
        assert!(compute(r(0, 0, 62, 1040), MON, None, None, None, 96).is_none());
        // Centred icons so wide they reach the corner.
        assert!(compute(TB, MON, Some(r(130, 1032, 175, 1080)), None, Some(r(0, 1032, 100, 1080)), 96).is_none());
    }

    #[test]
    fn scales_with_dpi() {
        let tb = r(0, 2088, 3840, 2160);
        let mon = r(0, 0, 3840, 2160);
        let p = compute(tb, mon, Some(r(1700, 2088, 1772, 2160)), None, None, 144).unwrap();
        assert_eq!((p.x, p.h, p.w), (9, 72, 354));
        assert_eq!(p.scale, 1.5);
    }

    #[test]
    fn top_taskbar() {
        let p = compute(r(0, 0, 1920, 48), MON, Some(r(717, 0, 762, 48)), None, None, 96).unwrap();
        assert_eq!(p.y, 0);
    }
}
