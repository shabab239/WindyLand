//! The notch window: layout, animation, input routing, toasts and the mascot.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::*;
use crate::modules::{self, Ctx, Module, Toast};
use crate::mascot::{self, Mood};
use crate::store::{ClipMode, Edge, Mascot, NotchStyle, Settings, Sound};
use crate::time::Now;
use crate::ui::{Hit, Ui};
use crate::{dragdrop, sound, tray};

pub const WM_APP_WAKE: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 2;
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Logical canvas (DIPs). The window is this big; transparent parts are click-through.
const CANVAS_W: f32 = 780.0;
const CANVAS_H: f32 = 360.0;
const TAB_H: f32 = 30.0;
const PAD: f32 = 16.0;
/// Tab bar hits use this module index.
const APP_HIT: usize = usize::MAX;
/// Toast action hits use this module index; the id is the action slot.
const TOAST: usize = usize::MAX - 1;
/// Settings panel hits use this module index.
const SETTINGS: usize = usize::MAX - 2;
/// Tab bar id of the gear button (other tab ids are module indexes).
const GEAR: u32 = 1000;
/// Size of the settings panel content (DIPs).
const SETTINGS_SIZE: (f32, f32) = (640.0, 286.0);
/// Welcome tour panel content size, its hits' module index and action ids.
const ONBOARD_SIZE: (f32, f32) = (600.0, 214.0);
const ONBOARD: usize = usize::MAX - 3;
const O_NEXT: u32 = 1;
const O_BACK: u32 = 2;
const O_SKIP: u32 = 3;
const O_STYLE: u32 = 10; // + index into NotchStyle::ALL
const O_EDGE: u32 = 20; // + index into Edge::ALL
const ONBOARD_STEPS: u8 = 3;

// Settings panel action ids.
const S_STYLE: u32 = 10; // + index into NotchStyle::ALL
const S_EDGE: u32 = 20; // + index into Edge::ALL
const S_CENTER: u32 = 30;
const S_MASCOT: u32 = 40; // 0 = clock only, 1.. = Mascot::ALL[i - 1]
const S_STARTUP: u32 = 60;
const S_VOLUME: u32 = 61;
const S_UPDATES: u32 = 62;
const S_DATA: u32 = 63;
const S_GITHUB: u32 = 64;
const S_QUIT: u32 = 65;
const S_CLIP_ON: u32 = 66;
const S_PRIVACY: u32 = 67;
const S_CLIP_MODE: u32 = 70; // + index into ClipMode::ALL
const S_SOUND: u32 = 80; // + index into Sound::ALL
const S_PAGE: u32 = 90; // + page index (General, Privacy & alerts)
const S_NOTIF_ON: u32 = 100;
const S_NOTIF_TEXT: u32 = 101;
const S_NOTIF_BANNERS: u32 = 102;
const S_NOTIF_UNMUTE: u32 = 103;
/// Hit id of a toast's body (clicking the toast itself, not a button).
const TOAST_BODY: u32 = 999;

const TIMER_TICK: usize = 1;
const TIMER_FRAME: usize = 2;
/// `frame_interval` value meaning "draw on every display refresh".
pub const VSYNC: u32 = 1;

/// How the main loop should schedule the next frame.
pub enum Pacing {
    /// Nothing is moving: sleep until a message arrives.
    Idle,
    /// A transition is running: draw, then wait for the next display refresh.
    Refresh,
    /// A slow ambient animation: draw after this many seconds.
    After(f64),
}

/// Sleep until just after the display's next refresh, so transitions draw
/// once per refresh at any monitor rate. The refresh phase and period come
/// from the compositor's timing info. (Neither DwmFlush nor DXGI's
/// WaitForVBlank is used: measured on a 144 Hz desktop, DwmFlush waited
/// ~250 ms whenever nothing else on screen changed, and WaitForVBlank
/// returned at once.) Falls back to ~120 fps if the timing isn't available.
pub fn wait_refresh(_hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let mut secs = 1.0 / 120.0;
    unsafe {
        let mut ti = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        let (mut now, mut freq) = (0i64, 0i64);
        if DwmGetCompositionTimingInfo(None, &mut ti).is_ok() && QueryPerformanceCounter(&mut now).is_ok() && QueryPerformanceFrequency(&mut freq).is_ok() {
            let (period, vblank) = (ti.qpcRefreshPeriod as i64, ti.qpcVBlank as i64);
            // Sane refresh rates only (24..500 Hz).
            if freq > 0 && period > freq / 500 && period < freq / 24 {
                let next = vblank + ((now - vblank).div_euclid(period) + 1) * period;
                secs = (next - now) as f64 / freq as f64 + 0.0005;
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Collapsed,
    Toast,
    Expanded,
}

pub enum Deferred {
    TrayMenu,
    Quit,
    DragOut(Vec<String>),
}

pub struct App {
    pub hwnd: HWND,
    gfx: Gfx,
    pub modules: Vec<Box<dyn Module>>,
    pub settings: Settings,
    hits: Vec<Hit>,
    tab: usize,
    expanded: bool,
    hover: bool,
    /// When the mouse entered the window (for the per-style hover delay).
    hover_since: f64,
    last_leave: f64,
    mouse: (f32, f32),
    press: Option<(Hit, (f32, f32))>,
    pub drag_files: bool,
    pub dragging_out: bool,
    leave_at: Option<f64>,
    keyboard: Option<usize>,
    queue: VecDeque<Toast>,
    toast: Option<(Toast, f64)>,
    cur: (f32, f32, f32),
    /// Spring velocity of `cur`.
    vel: (f32, f32, f32),
    /// Tab (and settings view) drawn last frame, and when it last changed,
    /// for the cross-fade when switching.
    shown: (usize, bool),
    switch_t: f64,
    /// The big settings panel is showing instead of a module.
    settings_open: bool,
    /// "Start with Windows" (registry), read when the settings panel opens.
    startup_cache: bool,
    /// When "Check for updates" was clicked in the settings panel.
    updates_clicked: f64,
    /// Last header layout: (title shown, nothing overlapped). For the self-test.
    header_fit: (bool, bool),
    /// What was showing before the current tab switch, and which way it went.
    prev_shown: (usize, bool),
    switch_dir: f32,
    /// Tab highlight pill offset in the tab bar, and when it was last drawn.
    pill_off: f32,
    pill_last: f64,
    /// Welcome tour step while it is showing (first run, or from the tray).
    onboarding: Option<u8>,
    /// Settings page: 0 General, 1 Privacy & alerts.
    settings_page: u8,
    /// The current toast's picture as a bitmap: (image address, gfx generation).
    toast_bmp: Option<(usize, u32, windows::Win32::Graphics::Direct2D::ID2D1Bitmap)>,
    /// Current transition's (start, last frame, frames, worst gap) for the
    /// debug log.
    perf: Option<(f64, f64, u32, f64)>,
    start: Instant,
    /// Virtual clock for the scripted demo video (`--render-demo`).
    virt: Option<f64>,
    last_frame: f64,
    frame_interval: u32,
    scale: f32,
    origin: (i32, i32),
    /// Centre of the notch along its edge, in canvas DIPs.
    anchor: f32,
    /// Right-button press point (screen px) and whether it became a drag.
    rdrag: Option<POINT>,
    dragging: bool,
    hidden: bool,
    mood: Mood,
    mood_since: f64,
    last_activity: f64,
    /// Set after clicking a toast button: hovering must not reopen the panel
    /// until the mouse has left the notch once.
    hover_block: bool,
    /// Hit under the mouse with a tooltip: (module, id, since, suppressed by a click).
    tip: Option<(usize, u32, f64, bool)>,
    pub deferred: Vec<Deferred>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Handle of the single-instance mutex (also the installer's AppMutex).
pub static SINGLE_INSTANCE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Release the single-instance mutex early, so an installer started by the
/// updater doesn't see WindyLand as still running while we shut down.
pub fn release_single_instance() {
    let h = SINGLE_INSTANCE.swap(0, std::sync::atomic::Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(HANDLE(h as *mut _));
            let _ = CloseHandle(HANDLE(h as *mut _));
        }
    }
}

/// Leak the app at exit: dropping D2D/COM/WinRT objects from a thread-local
/// destructor during process teardown is unsafe and corrupts the exit code.
pub fn shutdown() {
    APP.with(|a| {
        if let Ok(mut g) = a.try_borrow_mut() {
            std::mem::forget(g.take());
        }
    });
}

/// Run `f` against the app if it exists and is not already borrowed.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

/// Append a line to debug.log in the data folder when WINDYLAND_DEBUG=1.
pub fn dlog(msg: impl AsRef<str>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var("WINDYLAND_DEBUG").map_or(false, |v| v == "1")) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::store::dir().join("debug.log")) {
            let _ = writeln!(f, "{:.3} {}", crate::time::unix_now(), msg.as_ref());
        }
    }
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

impl App {
    pub fn create(visible: bool) -> Result<()> {
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let class = w!("WindyLandNotch");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!("WindyLand"),
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                hinst,
                None,
            )?;

            let gfx = Gfx::new(10, 10)?;
            let modules = modules::all(hwnd);
            let tab = modules.iter().position(|m| m.has_tab()).unwrap_or(0);
            let mut app = App {
                hwnd,
                gfx,
                modules,
                settings: Settings::load(),
                hits: Vec::new(),
                tab,
                expanded: false,
                hover: false,
                hover_since: 0.0,
                last_leave: -1.0,
                mouse: (-1e4, -1e4),
                press: None,
                drag_files: false,
                dragging_out: false,
                leave_at: None,
                keyboard: None,
                queue: VecDeque::new(),
                toast: None,
                cur: (200.0, 32.0, 12.0),
                vel: (0.0, 0.0, 0.0),
                shown: (tab, false),
                switch_t: -10.0,
                settings_open: false,
                startup_cache: false,
                updates_clicked: -10.0,
                header_fit: (true, true),
                prev_shown: (tab, false),
                switch_dir: 1.0,
                pill_off: 0.0,
                pill_last: -10.0,
                onboarding: None,
                settings_page: 0,
                toast_bmp: None,
                perf: None,
                start: Instant::now(),
                virt: None,
                last_frame: 0.0,
                frame_interval: 0,
                scale: 1.0,
                origin: (0, 0),
                anchor: CANVAS_W / 2.0,
                rdrag: None,
                dragging: false,
                hidden: false,
                mood: Mood::Clock,
                mood_since: 0.0,
                last_activity: 0.0,
                hover_block: false,
                tip: None,
                deferred: Vec::new(),
            };
            app.place();
            sound::set(app.settings.sound);
            if !app.settings.onboarded {
                app.start_onboarding();
            }
            if visible {
                use windows::Win32::UI::Accessibility::SetWinEventHook;
                for (lo, hi) in [(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND), (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND)] {
                    let _ = SetWinEventHook(lo, hi, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
                }
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                SetTimer(hwnd, TIMER_TICK, 1000, None);
                tray::add(hwnd);
                dragdrop::register(hwnd);
            }
            app.tick();
            app.request_frame();
            APP.with(|a| *a.borrow_mut() = Some(app));
            with_app(|a| a.frame());
        }
        Ok(())
    }

    fn t(&self) -> f64 {
        self.virt.unwrap_or_else(|| self.start.elapsed().as_secs_f64())
    }

    /// Size and position the window on the chosen edge/monitor. The window is
    /// clamped inside the monitor's work area; the notch itself can sit
    /// anywhere along the edge (`anchor` is its centre inside the canvas).
    fn place(&mut self) {
        unsafe {
            let mon = match self.settings.monitor {
                Some((x, y)) => MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
            };
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            self.scale = dx as f32 / 96.0;
            let pw = (CANVAS_W * self.scale).ceil() as i32;
            let ph = (CANVAS_H * self.scale).ceil() as i32;
            let r = mi.rcWork;
            let pos = self.settings.pos.clamp(0.0, 1.0);
            let (x, y, anchor_px) = match self.settings.edge {
                Edge::Top | Edge::Bottom => {
                    let c = r.left + ((r.right - r.left) as f32 * pos) as i32;
                    let x = (c - pw / 2).clamp(r.left, (r.right - pw).max(r.left));
                    let y = if self.settings.edge == Edge::Top { r.top } else { r.bottom - ph };
                    (x, y, c - x)
                }
                Edge::Left | Edge::Right => {
                    let c = r.top + ((r.bottom - r.top) as f32 * pos) as i32;
                    let y = (c - ph / 2).clamp(r.top, (r.bottom - ph).max(r.top));
                    let x = if self.settings.edge == Edge::Left { r.left } else { r.right - pw };
                    (x, y, c - y)
                }
            };
            self.origin = (x, y);
            self.anchor = anchor_px as f32 / self.scale;
            let _ = self.gfx.resize(pw, ph);
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                self.origin.0,
                self.origin.1,
                pw,
                ph,
                SWP_NOACTIVATE,
            );
        }
    }

    // ------------------------------------------------------------- ctx glue

    /// Call into module `i` with a fresh `Ctx`, then apply what it asked for.
    pub fn call<R>(&mut self, i: usize, f: impl FnOnce(&mut dyn Module, &mut Ctx) -> R) -> R {
        let mut toasts = Vec::new();
        let mut open_tab = None;
        let mut keyboard = None;
        let r = {
            let mut ctx = Ctx {
                now: Now::get(),
                hwnd: self.hwnd,
                hidden: self.hidden,
                settings: &mut self.settings,
                module: i,
                toasts: &mut toasts,
                open_tab: &mut open_tab,
                keyboard: &mut keyboard,
            };
            f(self.modules[i].as_mut(), &mut ctx)
        };
        for t in toasts {
            self.push_toast(t);
        }
        if let Some(m) = open_tab {
            self.tab = m;
            self.expanded = true;
            self.leave_at = Some(self.t() + 2.5);
        }
        match keyboard {
            Some(true) => self.grab_keyboard(i),
            Some(false) => self.release_keyboard(),
            None => {}
        }
        self.request_frame();
        r
    }

    /// Queue a toast. Its chime plays when it actually appears on screen
    /// (see `frame`), never while the notch is hidden behind a full-screen app.
    fn push_toast(&mut self, t: Toast) {
        let now = self.t();
        let hidden = self.hidden;
        if let Some((cur, end)) = &mut self.toast {
            if cur.key == t.key && !hidden {
                if t.chime {
                    sound::chime();
                }
                *end = now + t.secs as f64;
                *cur = t;
                return;
            }
        }
        if let Some(q) = self.queue.iter_mut().find(|q| q.key == t.key) {
            *q = t;
        } else {
            self.queue.push_back(t);
        }
    }

    fn grab_keyboard(&mut self, i: usize) {
        self.keyboard = Some(i);
        self.expanded = true;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
        }
    }

    fn release_keyboard(&mut self) {
        if self.keyboard.take().is_some() {
            unsafe {
                let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
            }
            if !self.hover {
                self.leave_at = Some(self.t() + 0.4);
            }
        }
    }

    // ------------------------------------------------------------ periodic

    pub fn tick(&mut self) {
        for i in 0..self.modules.len() {
            self.call(i, |m, c| m.tick(c));
        }
        self.check_fullscreen();
        self.update_mood();
        self.request_frame();
    }

    /// Hide while a game, video or presentation fills the notch's screen.
    /// Windows' own "busy" state (QUNS_BUSY) is not used: it also reports
    /// busy while the desktop itself has focus and for some ordinary
    /// windows, which made the notch vanish until the Start menu was opened.
    pub fn check_fullscreen(&mut self) {
        let busy = unsafe {
            matches!(SHQueryUserNotificationState(), Ok(QUNS_RUNNING_D3D_FULL_SCREEN) | Ok(QUNS_PRESENTATION_MODE)) || self.foreground_fullscreen()
        };
        if busy != self.hidden {
            dlog(format!("fullscreen {busy}"));
            self.hidden = busy;
            unsafe {
                let _ = ShowWindow(self.hwnd, if busy { SW_HIDE } else { SW_SHOWNOACTIVATE });
            }
            if !busy {
                self.raise();
            }
            self.request_frame();
        }
    }

    /// True when the foreground window really is full screen on our monitor.
    fn foreground_fullscreen(&self) -> bool {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() || fg == self.hwnd || IsIconic(fg).as_bool() || !IsWindowVisible(fg).as_bool() {
                return false;
            }
            let mut buf = [0u16; 128];
            let n = GetClassNameW(fg, &mut buf) as usize;
            let class = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
            let mut wr = RECT::default();
            if GetWindowRect(fg, &mut wr).is_err() {
                return false;
            }
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            is_fullscreen(wr, mi.rcMonitor, IsZoomed(fg).as_bool(), &class)
        }
    }

    /// Put the notch back on top of other always-on-top windows.
    fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn system_idle_secs() -> f64 {
        unsafe {
            let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            if GetLastInputInfo(&mut lii).as_bool() {
                GetTickCount().wrapping_sub(lii.dwTime) as f64 / 1000.0
            } else {
                0.0
            }
        }
    }

    fn live_module(&self) -> Option<usize> {
        let mut best: Option<(u8, usize)> = None;
        for (i, m) in self.modules.iter().enumerate() {
            if let Some(p) = m.live() {
                if best.map_or(true, |(bp, _)| p > bp) {
                    best = Some((p, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Mascot state machine: clock -> eyes (notch idle) -> doze (user away)
    /// -> wave (user back) -> eyes.
    fn update_mood(&mut self) {
        let t = self.t();
        let busy = self.expanded || self.toast.is_some() || self.live_module().is_some();
        if busy {
            self.last_activity = t;
        }
        let idle = Self::system_idle_secs();
        let next = if !self.settings.personality || busy || self.settings.style != NotchStyle::Full {
            Mood::Clock
        } else {
            match self.mood {
                Mood::Doze if idle < 2.0 => Mood::Wave,
                Mood::Wave if t - self.mood_since > 3.0 => Mood::Eyes,
                Mood::Wave => Mood::Wave,
                _ if idle > 120.0 => Mood::Doze,
                _ if t - self.last_activity > 45.0 => Mood::Eyes,
                Mood::Clock => Mood::Clock,
                m => m,
            }
        };
        if next != self.mood {
            self.mood = next;
            self.mood_since = t;
        }
    }

    // ---------------------------------------------------------- animation

    pub fn request_frame(&mut self) {
        self.set_frame_timer(VSYNC);
    }

    /// Choose the frame rate: 0 = none, `VSYNC` = every display refresh,
    /// otherwise every `ms`. The main loop (main.rs) does the pacing; the
    /// Windows timer only keeps things moving inside modal loops (menus,
    /// drag and drop), where the main loop doesn't run.
    fn set_frame_timer(&mut self, ms: u32) {
        if self.frame_interval != ms {
            self.frame_interval = ms;
            unsafe {
                if ms == 0 {
                    let _ = KillTimer(self.hwnd, TIMER_FRAME);
                } else {
                    SetTimer(self.hwnd, TIMER_FRAME, if ms == VSYNC { 16 } else { ms }, None);
                }
            }
        }
    }

    /// When the main loop should draw next.
    pub fn pacing(&self) -> Pacing {
        match self.frame_interval {
            0 => Pacing::Idle,
            VSYNC => Pacing::Refresh,
            ms => Pacing::After((self.last_frame + ms as f64 / 1000.0 - self.t()).max(0.0)),
        }
    }

    /// The fallback timer fired: draw only if the main loop hasn't (it is
    /// blocked in a modal loop), so frames are never doubled.
    fn timer_frame(&mut self) {
        let due = if self.frame_interval == VSYNC { 0.03 } else { self.frame_interval as f64 / 1000.0 * 1.5 };
        if self.t() - self.last_frame >= due {
            self.frame();
        }
    }

    fn mode(&self) -> Mode {
        if self.expanded || self.keyboard.is_some() || self.drag_files || self.onboarding.is_some() {
            Mode::Expanded
        } else if self.toast.is_some() {
            Mode::Toast
        } else {
            Mode::Collapsed
        }
    }

    /// Target size in edge-local terms: (along the edge, away from it, radius).
    fn target(&mut self) -> (f32, f32, f32) {
        let vertical = self.settings.edge.vertical();
        // Panels keep their upright (w, h); on side edges "along" is the height.
        let panel = |w: f32, h: f32, rad: f32| if vertical { (h, w, rad) } else { (w, h, rad) };
        match self.mode() {
            Mode::Expanded => {
                // Every tab shares one panel size so the tab bar never moves;
                // only an extra-wide view (Music lyrics) grows sideways.
                let (w, h) = if self.onboarding.is_some() {
                    ONBOARD_SIZE
                } else if self.settings_open {
                    SETTINGS_SIZE
                } else {
                    (self.modules[self.tab].size().0.max(modules::PANEL.0), modules::PANEL.1)
                };
                let w = (w + 2.0 * PAD).clamp(380.0, CANVAS_W - 40.0);
                let h = (h + TAB_H + PAD + 8.0).clamp(80.0, CANVAS_H - 20.0);
                panel(w, h, 26.0)
            }
            Mode::Toast => {
                let has_actions = self.toast.as_ref().map_or(false, |t| !t.0.actions.is_empty());
                let h = if has_actions { 104.0 } else { 66.0 };
                panel(380.0, h, 24.0)
            }
            Mode::Collapsed => {
                let live = self.live_module().is_some();
                match (self.settings.style, vertical) {
                    (NotchStyle::Full, false) if live => (300.0, 34.0, 13.0),
                    (NotchStyle::Full, false) => (200.0, 32.0, 12.0),
                    (NotchStyle::Full, true) if live => (130.0, 40.0, 14.0),
                    (NotchStyle::Full, true) => (100.0, 40.0, 14.0),
                    (NotchStyle::Compact, false) if live => (190.0, 22.0, 9.0),
                    (NotchStyle::Compact, false) => (96.0, 20.0, 8.0),
                    (NotchStyle::Compact, true) if live => (64.0, 26.0, 10.0),
                    (NotchStyle::Compact, true) => (60.0, 26.0, 10.0),
                    // No black notch at all; the bar is drawn separately.
                    (NotchStyle::Pill | NotchStyle::Hidden, _) => (if live { 120.0 } else { 84.0 }, 0.0, 0.0),
                }
            }
        }
    }

    /// Screen rect (canvas DIPs) of a notch `along` x `depth` on the current
    /// edge, centred on `anchor` but kept inside the canvas.
    fn edge_rect(&self, along: f32, depth: f32) -> Rect {
        let edge = self.settings.edge;
        let span = if edge.vertical() { CANVAS_H } else { CANVAS_W };
        let c = self.anchor.clamp(along / 2.0, (span - along / 2.0).max(along / 2.0));
        match edge {
            Edge::Top => Rect::new(c - along / 2.0, 0.0, along, depth),
            Edge::Bottom => Rect::new(c - along / 2.0, CANVAS_H - depth, along, depth),
            Edge::Left => Rect::new(0.0, c - along / 2.0, depth, along),
            Edge::Right => Rect::new(CANVAS_W - depth, c - along / 2.0, depth, along),
        }
    }

    /// Inner content rect with `near` padding on the screen-edge side, `far`
    /// on the opposite side and `side` padding left/right.
    fn content_rect(&self, goal: Rect, near: f32, far: f32, side: f32) -> Rect {
        match self.settings.edge {
            Edge::Top => Rect::new(goal.x + side, goal.y + near, goal.w - 2.0 * side, goal.h - near - far),
            Edge::Bottom => Rect::new(goal.x + side, goal.y + far, goal.w - 2.0 * side, goal.h - near - far),
            Edge::Left | Edge::Right => {
                let v = (near + far) / 2.0;
                Rect::new(goal.x + side, goal.y + v, goal.w - 2.0 * side, goal.h - 2.0 * v)
            }
        }
    }

    pub fn frame(&mut self) {
        let t = self.t();
        let dt = (t - self.last_frame).clamp(0.0, 0.1) as f32;
        self.last_frame = t;

        // Open once the mouse has rested on the collapsed notch long enough.
        if self.hover && self.can_hover_open() && t - self.hover_since >= self.settings.style.hover_delay() {
            self.open_from_hover();
        }
        // Collapse after the mouse has left for a moment.
        if let Some(at) = self.leave_at {
            if t >= at && !self.hover && self.keyboard.is_none() && !self.drag_files && self.onboarding.is_none() {
                self.expanded = false;
                self.settings_open = false;
                self.leave_at = None;
            }
        }
        // Toast lifecycle (paused while the panel is open, and while the
        // notch is hidden so nothing expires or chimes unseen).
        if self.mode() != Mode::Expanded && !self.hidden {
            if let Some((_, end)) = &self.toast {
                if t >= *end && !self.hover {
                    self.toast = None;
                }
            }
            if self.toast.is_none() {
                if let Some(next) = self.queue.pop_front() {
                    let end = t + next.secs as f64;
                    if next.chime {
                        sound::chime();
                    }
                    self.toast = Some((next, end));
                    // Another always-on-top window may have covered us.
                    self.raise();
                }
            }
        }

        if !self.expanded && self.keyboard.is_none() && !self.drag_files {
            self.settings_open = false;
        }
        // Switching tabs (or into settings) cross-fades the content.
        let shown = (self.tab, self.settings_open);
        if shown != self.shown {
            if self.mode() == Mode::Expanded {
                self.switch_t = t;
                self.prev_shown = self.shown;
                // Slide toward the side of the newly picked tab.
                let slot = |s: (usize, bool)| if s.1 { usize::MAX } else { s.0 };
                self.switch_dir = if slot(shown) >= slot(self.shown) { 1.0 } else { -1.0 };
            }
            self.shown = shown;
        }

        let target = self.target();
        let dist = self.step_anim(target, dt);

        self.render(target, dist);

        let ambient = self.modules.iter().any(|m| m.animating())
            || matches!(self.mood, Mood::Eyes | Mood::Doze | Mood::Wave)
            || self.toast.is_some()
            || self.leave_at.is_some()
            || self.hover;
        // Tab switch: slide, highlight glide and glow all finish within 0.6 s.
        let moving = dist > 0.0 || t - self.switch_t < 0.6;
        self.perf_note(t, moving);
        if moving {
            self.set_frame_timer(VSYNC);
        } else if ambient {
            // Precise timer (see main.rs): 60 fps for live content in the
            // open panel (lyrics, progress, stopwatch), 30 fps otherwise.
            let live = self.mode() == Mode::Expanded && self.modules.iter().any(|m| m.animating());
            self.set_frame_timer(if live { 16 } else { 33 });
        } else {
            self.set_frame_timer(0);
        }
    }

    /// With WINDYLAND_DEBUG=1, log each transition's frame rate and worst gap
    /// to debug.log, so a "looks choppy" report comes with numbers.
    fn perf_note(&mut self, t: f64, moving: bool) {
        if moving {
            let p = self.perf.get_or_insert((t, t, 0, 0.0));
            p.3 = p.3.max(t - p.1);
            p.1 = t;
            p.2 += 1;
        } else if let Some((s, e, n, worst)) = self.perf.take() {
            if n > 2 {
                dlog(format!("anim {n} frames in {:.0} ms: {:.0} fps, worst gap {:.1} ms", (e - s) * 1000.0, (n - 1) as f64 / (e - s).max(1e-6), worst * 1000.0));
            }
        }
    }

    /// Move the notch size toward `target` with a lightly under-damped spring
    /// (a small, soft overshoot like a phone's island), in fixed sub-steps so
    /// it behaves the same at any frame rate. Returns the remaining distance
    /// (0 once settled).
    fn step_anim(&mut self, target: (f32, f32, f32), dt: f32) -> f32 {
        const W: f32 = 20.0; // stiffness (rad/s)
        const Z: f32 = 0.74; // damping ratio
        let n = ((dt * 240.0).ceil() as usize).clamp(1, 32);
        let h = dt / n as f32;
        let tg = [target.0, target.1, target.2];
        let mut x = [self.cur.0, self.cur.1, self.cur.2];
        let mut v = [self.vel.0, self.vel.1, self.vel.2];
        for _ in 0..n {
            for i in 0..3 {
                let a = W * W * (tg[i] - x[i]) - 2.0 * Z * W * v[i];
                v[i] += a * h;
                x[i] += v[i] * h;
            }
        }
        for xi in x.iter_mut() {
            *xi = xi.max(0.0);
        }
        self.cur = (x[0], x[1], x[2]);
        self.vel = (v[0], v[1], v[2]);
        let dist = (x[0] - tg[0]).abs() + (x[1] - tg[1]).abs();
        let speed = v[0].abs() + v[1].abs();
        if dist < 0.3 && speed < 6.0 {
            self.cur = target;
            self.vel = (0.0, 0.0, 0.0);
            return 0.0;
        }
        dist.max(0.31)
    }

    /// Progress (0..1) of the tab-switch transition.
    fn switch_progress(&self, t: f64) -> f32 {
        ((t - self.switch_t) / 0.32).clamp(0.0, 1.0) as f32
    }

    /// Opacity of the incoming content during a tab switch (0..1, eased).
    fn switch_alpha(&self, t: f64) -> f32 {
        let k = ((self.switch_progress(t) - 0.15) / 0.85).clamp(0.0, 1.0);
        k * k * (3.0 - 2.0 * k)
    }

    fn render(&mut self, target: (f32, f32, f32), dist: f32) {
        let t = self.t();
        let (along, depth, rad) = self.cur;
        let notch = self.edge_rect(along, depth);
        let goal = self.edge_rect(target.0, target.1);
        let edge = self.settings.edge;
        let mode = self.mode();
        // While switching tabs the panel stays solid and only the content
        // cross-fades; otherwise content fades in as the notch opens.
        let switching = mode == Mode::Expanded && t - self.switch_t < 0.4;
        let content_alpha = if switching { 1.0 } else { (1.0 - dist / 50.0).clamp(0.0, 1.0) };

        self.hits.clear();
        self.gfx.begin(self.scale);
        let ear = (depth / 4.0).min(9.0);
        let barless = matches!(self.settings.style, NotchStyle::Pill | NotchStyle::Hidden);
        if depth >= 0.5 {
            if let Some(geo) = self.gfx.notch_geometry(notch, rad, ear, edge) {
                self.gfx.fill_geometry(&geo, col::NOTCH);
                self.gfx.push_layer(Some(&geo), content_alpha);
                match mode {
                    Mode::Expanded if self.onboarding.is_some() => self.draw_onboarding(goal, t),
                    Mode::Expanded => self.draw_expanded(notch, goal, t),
                    Mode::Toast => self.draw_toast(goal, t),
                    Mode::Collapsed if !barless => self.draw_collapsed(goal, t),
                    Mode::Collapsed => {}
                }
                self.gfx.pop_layer();
                // Keep the whole silhouette clickable even while content is fading.
                self.hits.insert(0, Hit { rect: notch, module: APP_HIT, id: u32::MAX, tip: "" });
            }
        }
        if barless {
            self.draw_bar(along, depth);
        }
        if mode != Mode::Expanded {
            let dots = self.indicators();
            if !dots.is_empty() {
                self.draw_dots(&dots, notch);
            }
        }
        if dist <= 0.0 {
            self.draw_tooltip(t);
        }
        self.gfx.end_and_present(self.hwnd, self.origin.0, self.origin.1);
    }

    /// Tooltip for the hit under the mouse after it has rested there for a
    /// moment. Drawn outside the notch (on the side away from the screen
    /// edge) and kept inside the canvas.
    fn draw_tooltip(&mut self, t: f64) {
        const DELAY: f64 = 0.6;
        let hit = if self.press.is_none() && !self.dragging { self.hit_at(self.mouse.0, self.mouse.1) } else { None };
        let Some(h) = hit.filter(|h| !h.tip.is_empty()) else {
            self.tip = None;
            return;
        };
        match self.tip {
            Some((m, id, _, _)) if m == h.module && id == h.id => {}
            _ => self.tip = Some((h.module, h.id, t, false)),
        }
        let Some((_, _, since, suppressed)) = self.tip else { return };
        if suppressed || t - since < DELAY {
            return;
        }
        let size = 11.5;
        let w = self.gfx.measure(h.tip, size, Weight::Regular) + 16.0;
        let ht = 22.0;
        let r = h.rect;
        let gap = 6.0;
        let (mut x, mut y) = match self.settings.edge {
            Edge::Top => (r.cx() - w / 2.0, r.b() + gap),
            Edge::Bottom => (r.cx() - w / 2.0, r.y - gap - ht),
            Edge::Left => (r.r() + gap, r.cy() - ht / 2.0),
            Edge::Right => (r.x - gap - w, r.cy() - ht / 2.0),
        };
        // Flip to the other side if it would leave the canvas.
        if y + ht > CANVAS_H {
            y = r.y - gap - ht;
        }
        if y < 0.0 {
            y = r.b() + gap;
        }
        x = x.clamp(2.0, (CANVAS_W - w - 2.0).max(2.0));
        y = y.clamp(2.0, (CANVAS_H - ht - 2.0).max(2.0));
        let fade = ((t - since - DELAY) / 0.12).clamp(0.0, 1.0) as f32;
        let tr = Rect::new(x, y, w, ht);
        self.gfx.alpha = fade;
        self.gfx.fill_rr(tr, 7.0, hex(0x2A2A2E));
        self.gfx.stroke_rr(tr, 7.0, col::TEXT.with_a(0.14), 1.0);
        self.gfx.text(h.tip, tr, size, col::TEXT, Weight::Regular, Align::Center);
        self.gfx.alpha = 1.0;
    }

    /// The open panel: tab bar (laid out in the animated `notch` rect so it
    /// glides with the shape) and the module or settings content (laid out
    /// in the final `goal` rect so it doesn't reflow while animating).
    fn draw_expanded(&mut self, notch: Rect, goal: Rect, t: f64) {
        let r = self.content_rect(goal, 6.0, PAD, PAD);
        let r = Rect::new(r.x, r.y + TAB_H + 8.0, r.w, r.h - TAB_H - 8.0);
        let mut bar = self.content_rect(notch, 6.0, PAD, PAD).cut_top(TAB_H, 8.0);
        let now = Now::get();
        let mouse = self.mouse;
        let slot = TAB_H + 4.0;
        let tabs: Vec<usize> = (0..self.modules.len()).filter(|&i| self.modules[i].has_tab()).collect();
        // The highlight pill glides to the selected tab (or the gear).
        let sel_off = if self.settings_open {
            tabs.len() as f32 * slot + 4.0
        } else {
            tabs.iter().position(|&i| i == self.tab).unwrap_or(0) as f32 * slot
        };
        let gap = t - self.pill_last;
        self.pill_last = t;
        if gap > 0.15 {
            self.pill_off = sel_off;
        } else {
            let k = 1.0 - (-(gap as f32) * 16.0).exp();
            self.pill_off += (sel_off - self.pill_off) * k;
            if (self.pill_off - sel_off).abs() < 0.3 {
                self.pill_off = sel_off;
            }
        }
        // Just-selected tab: a glow in its colour and a little pop.
        let gp = ((t - self.switch_t) / 0.55).clamp(0.0, 1.0) as f32;
        let accent = if self.settings_open { col::TEXT } else { self.modules[self.tab].accent() };
        let indicators = self.indicators();
        {
            let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: APP_HIT, mouse, t };
            let pill = Rect::new(bar.x + self.pill_off, bar.y, TAB_H, TAB_H);
            if gp < 1.0 {
                let fade = (1.0 - gp) * (1.0 - gp);
                let (cx, cy) = (bar.x + sel_off + TAB_H / 2.0, bar.cy());
                for k in 0..5 {
                    let rad = TAB_H * 0.5 + 1.0 + k as f32 * 3.0 + gp * 6.0;
                    ui.circle(cx, cy, rad, accent.with_a(0.16 * fade * (1.0 - k as f32 / 5.0)));
                }
            }
            let pill_c = col::TEXT.with_a(0.16).lerp(accent.with_a(0.42), (1.0 - gp) * 0.8);
            ui.fill_rr(pill, TAB_H / 2.0, pill_c);
            for &i in &tabs {
                let tr = bar.cut_left(TAB_H, 4.0);
                let m = &self.modules[i];
                let sel = i == self.tab && !self.settings_open;
                let fg = if sel && gp < 1.0 {
                    m.accent().lerp(col::TEXT, gp)
                } else if sel {
                    col::TEXT
                } else {
                    col::DIM
                };
                // The icon stays still (no size change or hop): only its colour
                // fades from the tab's accent to white.
                ui.icon_btn(tr, m.icon(), i as u32, 14.0, fg, None, m.title());
            }
            // Settings gear, set slightly apart from the tabs.
            bar.cut_left(4.0, 0.0);
            let gr = bar.cut_left(TAB_H, 4.0);
            let fg = if self.settings_open { col::TEXT } else { col::DIM };
            ui.icon_btn(gr, glyph::SETTINGS, GEAR, 14.0, fg, None, if self.settings_open { "Close settings" } else { "Settings" });
            // Title of the current tab + date/time on the right. With many
            // tabs on a narrow panel, drop the date first, then the title
            // (tab tooltips still name them), so nothing overlaps.
            let time = now.hhmm();
            let tw = ui.measure(&time, 14.0, Weight::Semi);
            let tr = bar.cut_right(tw + 2.0, 8.0);
            ui.text(&time, tr, 14.0, col::TEXT, Weight::Semi, Align::Right);
            // Mic / camera in-use dots sit just left of the clock.
            for c in indicators.iter().rev() {
                let d = bar.cut_right(10.0, 2.0);
                ui.circle(d.cx(), d.cy(), 3.5, *c);
            }
            let title = if self.settings_open { "Settings" } else { self.modules[self.tab].title() };
            let date = now.short_date();
            let title_w = ui.measure(title, 13.0, Weight::Semi) + 10.0;
            let date_w = ui.measure(&date, 12.0, Weight::Regular) + 8.0;
            if bar.w >= title_w + date_w {
                let dr = bar.cut_right(date_w, 0.0);
                ui.text(&date, dr, 12.0, col::DIM, Weight::Regular, Align::Right);
            }
            let show_title = bar.w >= title_w;
            if show_title {
                bar.x += 6.0;
                bar.w -= 6.0;
                ui.text(title, bar, 13.0, col::TEXT, Weight::Semi, Align::Left);
            }
            self.header_fit = (show_title, bar.w >= 0.0);
        }

        // Content: on a tab switch the old content slides out and fades
        // while the new slides in from the side you moved toward.
        let p = self.switch_progress(t);
        let base = self.gfx.alpha;
        self.gfx.push_clip(r.inset(-2.0));
        let old = self.prev_shown;
        let sliding = p < 1.0 && !self.settings_open && !old.1 && old.0 != self.tab && old.0 < self.modules.len();
        let travel = 40.0 * self.switch_dir;
        let e = 1.0 - (1.0 - p).powi(3);
        if sliding {
            let oa = (1.0 - p / 0.55).clamp(0.0, 1.0);
            if oa > 0.0 {
                let n = self.hits.len();
                self.gfx.alpha = base * oa;
                let rr = shift(self.module_rect(old.0, r), -travel * e);
                let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: old.0, mouse: (-1e4, -1e4), t };
                self.modules[old.0].draw(&mut ui, rr);
                // The outgoing content is display-only.
                self.hits.truncate(n);
            }
        }
        self.gfx.alpha = base * self.switch_alpha(t);
        if self.settings_open {
            self.draw_settings(r, t);
        } else {
            let i = self.tab;
            let dx = if sliding { travel * (1.0 - e) } else { 0.0 };
            let rr = shift(self.module_rect(i, r), dx);
            let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: i, mouse, t };
            self.modules[i].draw(&mut ui, rr);
        }
        self.gfx.pop_clip();
        self.gfx.alpha = base;
    }

    /// Mic / camera dots just beside the closed notch (or toast), on the
    /// screen edge, like a phone's privacy dots. A dark rim keeps them
    /// visible on white windows.
    fn draw_dots(&mut self, dots: &[Color], notch: Rect) {
        let edge = self.settings.edge;
        let n = dots.len() as f32;
        for (k, c) in dots.iter().enumerate() {
            let off = 10.0 + k as f32 * 12.0;
            let (mut x, mut y) = match edge {
                Edge::Top => (notch.r() + off, 7.0),
                Edge::Bottom => (notch.r() + off, CANVAS_H - 7.0),
                Edge::Left => (7.0, notch.b() + off),
                Edge::Right => (CANVAS_W - 7.0, notch.b() + off),
            };
            // Near the canvas end: put them on the other side instead.
            if x > CANVAS_W - 8.0 {
                x = notch.x - 10.0 - (n - 1.0 - k as f32) * 12.0;
            }
            if y > CANVAS_H - 8.0 && edge.vertical() {
                y = notch.y - 10.0 - (n - 1.0 - k as f32) * 12.0;
            }
            self.gfx.circle(x, y, 5.4, col::NOTCH.with_a(0.55));
            self.gfx.circle(x, y, 4.0, *c);
        }
    }

    /// Where module `i` draws inside the panel content rect `r`: its natural
    /// size, centred.
    fn module_rect(&self, i: usize, r: Rect) -> Rect {
        let (nw, nh) = self.modules[i].size();
        let (w, h) = (nw.min(r.w), nh.min(r.h));
        Rect::new(r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0, w, h)
    }

    /// Privacy dots (mic / camera in use) from every module.
    fn indicators(&self) -> Vec<Color> {
        if !self.settings.privacy_dots {
            return Vec::new();
        }
        self.modules.iter().flat_map(|m| m.indicator()).collect()
    }


    /// The settings panel: look and position on the left, mascot picker
    /// below, general switches and links on the right.
    fn draw_settings(&mut self, r: Rect, t: f64) {
        let mouse = self.mouse;
        let st = self.settings.clone();
        let startup = self.startup_cache;
        let checking = t - self.updates_clicked < 4.0;
        let personality = st.personality;
        let mascot_kind = st.mascot;
        let accent = col::ACCENT;
        let page = self.settings_page;
        let muted = crate::modules::notifs::muted(&st.extra).len();
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: SETTINGS, mouse, t };
        let mut left = Rect::new(r.x, r.y, 372.0_f32.min(r.w * 0.6), r.h);
        let mut right = Rect::new(left.r() + 20.0, r.y, r.r() - left.r() - 20.0, r.h);
        let label = |ui: &mut Ui, rr: Rect, s: &str| ui.text(s, rr, 11.5, col::DIM, Weight::Semi, Align::Left);

        label(&mut ui, left.cut_top(16.0, 4.0), "Notch style");
        let row = left.cut_top(26.0, 10.0);
        for (i, (cell, s)) in row.columns(4, 6.0).into_iter().zip(NotchStyle::ALL).enumerate() {
            let name = match s {
                NotchStyle::Full => "Full",
                NotchStyle::Compact => "Compact",
                NotchStyle::Pill => "Pill",
                NotchStyle::Hidden => "Hidden",
            };
            ui.chip(cell, name, S_STYLE + i as u32, st.style == s, accent);
        }

        label(&mut ui, left.cut_top(16.0, 4.0), "Position (or right-drag the notch)");
        let mut row = left.cut_top(26.0, 10.0);
        let center = row.cut_right(70.0, 6.0);
        for (i, (cell, e)) in row.columns(4, 6.0).into_iter().zip(Edge::ALL).enumerate() {
            ui.chip(cell, e.label(), S_EDGE + i as u32, st.edge == e, accent);
        }
        ui.button(center, "Center", S_CENTER);

        label(&mut ui, left.cut_top(16.0, 4.0), "Mascot (Full style, when idle)");
        let grid = left.cut_top(94.0, 10.0);
        let rows = [Rect::new(grid.x, grid.y, grid.w, 44.0), Rect::new(grid.x, grid.y + 50.0, grid.w, 44.0)];
        // Tile 0 is "Clock" (no mascot), then every mascot.
        let mut k = 0usize;
        for row in rows {
            for cell in row.columns(5, 6.0) {
                if k > Mascot::ALL.len() {
                    break;
                }
                let selected = if k == 0 { !personality } else { personality && mascot_kind == Mascot::ALL[k - 1] };
                let hov = ui.hovered(cell);
                let bg = if selected {
                    accent.with_a(0.22)
                } else if hov {
                    col::BTN_HOVER
                } else {
                    col::BTN
                };
                ui.fill_rr(cell, 10.0, bg);
                if selected {
                    ui.stroke_rr(cell, 10.0, accent.with_a(0.8), 1.2);
                }
                let mut c = cell;
                let name_r = c.cut_bottom(14.0, 0.0);
                let pic = c.inset_xy(4.0, 3.0);
                if k == 0 {
                    ui.text(&Now::get().hhmm(), pic, 12.0, col::TEXT, Weight::Semi, Align::Center);
                    ui.text("Clock", name_r, 9.5, col::DIM, Weight::Regular, Align::Center);
                } else {
                    let m = Mascot::ALL[k - 1];
                    let mood = if hov { Mood::Wave } else { Mood::Eyes };
                    let look = gaze(pic.cx(), pic.cy(), mouse, 60.0);
                    ui.push_clip(cell);
                    mascot::draw(&mut ui, pic, m, mood, t, look);
                    ui.pop_clip();
                    ui.text(m.label(), name_r, 9.5, if selected { col::TEXT } else { col::DIM }, Weight::Regular, Align::Center);
                }
                ui.hit_tip(cell, S_MASCOT + k as u32, "");
                k += 1;
            }
        }

        label(&mut ui, left.cut_top(16.0, 4.0), "Sound for reminders (click to hear it)");
        let row = left.cut_top(24.0, 0.0);
        for (i, (cell, s)) in row.columns(Sound::ALL.len(), 6.0).into_iter().zip(Sound::ALL).enumerate() {
            ui.chip(cell, s.label(), S_SOUND + i as u32, st.sound == s, accent);
        }
        let foot = left.cut_bottom(14.0, 0.0);
        let v = format!("WindyLand {}  ·  right-click the notch for settings", crate::modules::update::VERSION);
        ui.text(&v, foot, 10.0, col::FAINT, Weight::Regular, Align::Left);

        // The right column has two pages.
        let seg = right.cut_top(26.0, 12.0);
        for (i, (cell, name)) in seg.columns(2, 6.0).into_iter().zip(["General", "Privacy & alerts"]).enumerate() {
            ui.chip(cell, name, S_PAGE + i as u32, page == i as u8, accent);
        }
        let half = |rr: &mut Rect, gap: f32| {
            let row = rr.cut_top(26.0, gap);
            let c = row.columns(2, 8.0);
            (c[0], c[1])
        };
        if page == 0 {
            ui.toggle(right.cut_top(26.0, 2.0), "Start with Windows", startup, S_STARTUP, accent);
            ui.toggle(right.cut_top(26.0, 12.0), "Volume overlay", st.volume_overlay, S_VOLUME, accent);
            let (a, b) = half(&mut right, 6.0);
            ui.button(a, if checking { "Checking..." } else { "Check for updates" }, S_UPDATES);
            ui.button(b, "Open data folder", S_DATA);
            let (a, b) = half(&mut right, 6.0);
            ui.button(a, "GitHub page", S_GITHUB);
            ui.button_c(b, "Quit WindyLand", S_QUIT, col::RED.with_a(0.18), col::RED);
        } else {
            label(&mut ui, right.cut_top(16.0, 2.0), "Windows notifications");
            ui.toggle(right.cut_top(26.0, 0.0), "Show them in the notch", st.notif_on, S_NOTIF_ON, accent);
            ui.toggle(right.cut_top(26.0, 4.0), "Include message text", st.notif_text, S_NOTIF_TEXT, accent);
            let (a, b) = half(&mut right, 10.0);
            ui.button(a, "Windows banners", S_NOTIF_BANNERS);
            let unmute = if muted == 0 { "No muted apps".to_string() } else { format!("Unmute {muted} app{}", if muted == 1 { "" } else { "s" }) };
            ui.button(b, &unmute, S_NOTIF_UNMUTE);
            ui.toggle(right.cut_top(26.0, 8.0), "Mic & camera dots", st.privacy_dots, S_PRIVACY, accent);
            label(&mut ui, right.cut_top(16.0, 2.0), "Clipboard history");
            ui.toggle(right.cut_top(26.0, 2.0), "Record what I copy", st.clip_on, S_CLIP_ON, accent);
            let row = right.cut_top(24.0, 0.0);
            for (i, (cell, m)) in row.columns(3, 6.0).into_iter().zip(ClipMode::ALL).enumerate() {
                ui.chip(cell, clip_mode_short(m), S_CLIP_MODE + i as u32, st.clip_mode == m, accent);
            }
        }
    }

    /// Apply a click in the settings panel.
    fn settings_action(&mut self, id: u32) {
        match id {
            i if (S_STYLE..S_STYLE + NotchStyle::ALL.len() as u32).contains(&i) => {
                self.settings.style = NotchStyle::ALL[(i - S_STYLE) as usize];
            }
            i if (S_EDGE..S_EDGE + Edge::ALL.len() as u32).contains(&i) => {
                self.set_edge(Edge::ALL[(i - S_EDGE) as usize], false);
            }
            S_CENTER => {
                let e = self.settings.edge;
                self.set_edge(e, true);
            }
            S_MASCOT => self.settings.personality = false,
            i if (S_MASCOT + 1..=S_MASCOT + Mascot::ALL.len() as u32).contains(&i) => {
                self.settings.personality = true;
                self.settings.mascot = Mascot::ALL[(i - S_MASCOT - 1) as usize];
            }
            S_STARTUP => {
                tray::set_startup(!tray::startup_enabled());
                self.startup_cache = tray::startup_enabled();
            }
            S_VOLUME => self.settings.volume_overlay = !self.settings.volume_overlay,
            S_PRIVACY => self.settings.privacy_dots = !self.settings.privacy_dots,
            i if (S_SOUND..S_SOUND + Sound::ALL.len() as u32).contains(&i) => {
                let s = Sound::ALL[(i - S_SOUND) as usize];
                self.settings.sound = s;
                sound::set(s);
                sound::play(s);
            }
            S_CLIP_ON => self.settings.clip_on = !self.settings.clip_on,
            i if (S_PAGE..S_PAGE + 2).contains(&i) => self.settings_page = (i - S_PAGE) as u8,
            S_NOTIF_ON => self.settings.notif_on = !self.settings.notif_on,
            S_NOTIF_TEXT => self.settings.notif_text = !self.settings.notif_text,
            S_NOTIF_BANNERS => crate::modules::notifs::open_uri(w!("ms-settings:notifications")),
            S_NOTIF_UNMUTE => {
                self.settings.extra.remove(crate::modules::notifs::MUTED_KEY);
            }
            i if (S_CLIP_MODE..S_CLIP_MODE + ClipMode::ALL.len() as u32).contains(&i) => {
                self.settings.clip_mode = ClipMode::ALL[(i - S_CLIP_MODE) as usize];
            }
            S_UPDATES => {
                self.updates_clicked = self.t();
                if let Some(i) = self.modules.iter().position(|m| m.title() == "Updates") {
                    self.call(i, |m, c| m.action(crate::modules::update::A_CHECK_NOW, (0.0, 0.0), c));
                }
            }
            S_DATA => unsafe {
                let d = HSTRING::from(crate::store::dir().display().to_string());
                ShellExecuteW(None, w!("open"), &d, None, None, SW_SHOWNORMAL);
            },
            S_GITHUB => unsafe {
                let u = HSTRING::from(format!("https://github.com/{}", crate::modules::update::REPO));
                ShellExecuteW(None, w!("open"), &u, None, None, SW_SHOWNORMAL);
            },
            S_QUIT => self.deferred.push(Deferred::Quit),
            _ => {}
        }
        self.settings.save();
        // Modules that mirror settings (clipboard, privacy dots) pick it up now.
        self.tick();
        self.request_frame();
    }

    /// Show the welcome tour (first run, or "Welcome tour" in the tray menu).
    pub fn start_onboarding(&mut self) {
        self.onboarding = Some(0);
        self.settings_open = false;
        self.expanded = true;
        self.toast = None;
        self.request_frame();
    }

    fn finish_onboarding(&mut self) {
        self.onboarding = None;
        self.settings.onboarded = true;
        self.settings.save();
        self.expanded = false;
        self.leave_at = None;
        self.push_toast(
            Toast::new("welcome", glyph::SETTINGS, col::ACCENT, "You're all set", "Change the style or position anytime: right-click the notch")
                .secs(7.0),
        );
        self.request_frame();
    }

    fn onboarding_action(&mut self, id: u32) {
        let Some(step) = self.onboarding else { return };
        match id {
            O_NEXT if step + 1 >= ONBOARD_STEPS => self.finish_onboarding(),
            O_NEXT => self.onboarding = Some(step + 1),
            O_BACK => self.onboarding = Some(step.saturating_sub(1)),
            O_SKIP => self.finish_onboarding(),
            i if (O_STYLE..O_STYLE + NotchStyle::ALL.len() as u32).contains(&i) => {
                self.settings.style = NotchStyle::ALL[(i - O_STYLE) as usize];
                self.settings.save();
            }
            i if (O_EDGE..O_EDGE + Edge::ALL.len() as u32).contains(&i) => {
                self.set_edge(Edge::ALL[(i - O_EDGE) as usize], true);
            }
            _ => {}
        }
        self.request_frame();
    }

    /// The welcome tour: 1. how the closed notch looks, 2. where it sits,
    /// 3. how to use it. Every choice applies straight away.
    fn draw_onboarding(&mut self, goal: Rect, t: f64) {
        let Some(step) = self.onboarding else { return };
        let mouse = self.mouse;
        let st = self.settings.clone();
        let accent = col::ACCENT;
        let mut r = self.content_rect(goal, 10.0, PAD, PAD + 4.0);
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: ONBOARD, mouse, t };

        // Header: title, step dots, Skip.
        let mut head = r.cut_top(24.0, 4.0);
        ui.button_c(head.cut_right(60.0, 0.0), "Skip", O_SKIP, col::BTN, col::DIM);
        let dots = head.cut_right(54.0, 8.0);
        for k in 0..ONBOARD_STEPS {
            let on = k == step;
            let c = if on { accent } else { col::TEXT.with_a(0.25) };
            let w = if on { 16.0 } else { 6.0 };
            let x = dots.x + k as f32 * 12.0 + if k > step { 10.0 } else { 0.0 };
            ui.fill_rr(Rect::new(x, dots.cy() - 3.0, w, 6.0), 3.0, c);
        }
        ui.text("Welcome to WindyLand", head, 15.0, col::TEXT, Weight::Semi, Align::Left);
        let (question, hint) = match step {
            0 => ("First, how should the notch look when it's closed?", "Pick one. You can change it anytime in Settings."),
            1 => ("Where should it sit?", "Or right-drag the notch to any edge, on any screen."),
            _ => ("Three things to know", "That's it. Enjoy!"),
        };
        ui.text(question, r.cut_top(20.0, 2.0), 12.5, col::DIM, Weight::Regular, Align::Left);
        let mut foot = r.cut_bottom(28.0, 0.0);
        let next = if step + 1 >= ONBOARD_STEPS { "Start using WindyLand" } else { "Next" };
        let nw = ui.measure(next, 12.0, Weight::Semi) + 40.0;
        ui.button_accent(foot.cut_right(nw, 8.0), next, O_NEXT, accent);
        if step > 0 {
            ui.button(foot.cut_right(70.0, 8.0), "Back", O_BACK);
        }
        ui.text(hint, foot, 11.0, col::FAINT, Weight::Regular, Align::Left);
        let body = r.inset_xy(0.0, 6.0);

        match step {
            0 => {
                // Four tiles, each with a tiny screen showing that style.
                for (i, (cell, s)) in body.columns(4, 10.0).into_iter().zip(NotchStyle::ALL).enumerate() {
                    let sel = st.style == s;
                    let hov = ui.hovered(cell);
                    let bg = if sel { accent.with_a(0.2) } else if hov { col::BTN_HOVER } else { col::BTN };
                    ui.fill_rr(cell, 12.0, bg);
                    if sel {
                        ui.stroke_rr(cell, 12.0, accent.with_a(0.85), 1.4);
                    }
                    let mut c = cell.inset(8.0);
                    let screen = c.cut_top(48.0, 6.0);
                    ui.fill_rr(screen, 6.0, hex(0x3A3A40));
                    let cx = screen.cx();
                    match s {
                        NotchStyle::Full => {
                            let n = Rect::new(cx - 30.0, screen.y, 60.0, 13.0);
                            ui.fill_rr(n, 5.0, col::NOTCH);
                            ui.text("12:30", n, 8.0, col::TEXT, Weight::Semi, Align::Center);
                        }
                        NotchStyle::Compact => ui.fill_rr(Rect::new(cx - 17.0, screen.y, 34.0, 8.0), 4.0, col::NOTCH),
                        NotchStyle::Pill => ui.fill_rr(Rect::new(cx - 15.0, screen.y + 3.0, 30.0, 3.0), 1.5, col::TEXT.with_a(0.8)),
                        NotchStyle::Hidden => {
                            for k in 0..5 {
                                ui.fill_rect(Rect::new(cx - 18.0 + k as f32 * 8.0, screen.y + 2.0, 4.0, 1.0), col::TEXT.with_a(0.25));
                            }
                        }
                    }
                    let (name, desc) = match s {
                        NotchStyle::Full => ("Full", "Clock, music, mascot"),
                        NotchStyle::Compact => ("Compact", "A smaller notch"),
                        NotchStyle::Pill => ("Pill", "Just a thin bar"),
                        NotchStyle::Hidden => ("Hidden", "Invisible until hovered"),
                    };
                    ui.text(name, c.cut_top(16.0, 0.0), 12.5, if sel { col::TEXT } else { col::DIM }, Weight::Semi, Align::Center);
                    ui.text(desc, c.cut_top(14.0, 0.0), 10.0, col::FAINT, Weight::Regular, Align::Center);
                    ui.hit_tip(cell, O_STYLE + i as u32, "");
                }
            }
            1 => {
                let mut b = body;
                let screen = b.cut_left(150.0, 20.0).centered(150.0, 90.0);
                ui.fill_rr(screen, 8.0, hex(0x3A3A40));
                let bar = match st.edge {
                    Edge::Top => Rect::new(screen.cx() - 22.0, screen.y, 44.0, 8.0),
                    Edge::Bottom => Rect::new(screen.cx() - 22.0, screen.b() - 8.0, 44.0, 8.0),
                    Edge::Left => Rect::new(screen.x, screen.cy() - 18.0, 8.0, 36.0),
                    Edge::Right => Rect::new(screen.r() - 8.0, screen.cy() - 18.0, 8.0, 36.0),
                };
                ui.fill_rr(bar, 4.0, col::NOTCH);
                ui.stroke_rr(bar, 4.0, accent, 1.0);
                let row = b.centered(b.w, 30.0);
                for (i, (cell, e)) in row.columns(4, 8.0).into_iter().zip(Edge::ALL).enumerate() {
                    ui.chip(cell, e.label(), O_EDGE + i as u32, st.edge == e, accent);
                }
            }
            _ => {
                let tips = [
                    (glyph::PLAY, "Hover to open", "The icons along the top switch between music, timer, to-do and more."),
                    (glyph::SETTINGS, "Right-click: Settings", "Style, position, mascot, clipboard history and more live there."),
                    (glyph::BACK, "Right-drag: move it", "It snaps to the nearest edge of whichever screen you drop it on."),
                ];
                for (cell, (g, title, sub)) in body.columns(3, 10.0).into_iter().zip(tips) {
                    ui.fill_rr(cell, 12.0, col::BTN);
                    let mut c = cell.inset(10.0);
                    let ic = c.cut_top(26.0, 6.0);
                    ui.circle(ic.x + 13.0, ic.cy(), 13.0, accent.with_a(0.18));
                    ui.icon(g, Rect::new(ic.x, ic.y, 26.0, 26.0), 12.0, accent);
                    ui.text(title, c.cut_top(18.0, 2.0), 12.5, col::TEXT, Weight::Semi, Align::Left);
                    ui.text_wrap(sub, c, 11.0, col::DIM, Weight::Regular, Align::Left);
                }
            }
        }
    }

    /// Open the notch on the settings panel (right-click, tray icon).
    pub fn open_settings(&mut self) {
        self.expanded = true;
        self.settings_open = true;
        self.startup_cache = tray::startup_enabled();
        // Opened from the tray the mouse is elsewhere: give it time to arrive.
        if !self.hover {
            self.leave_at = Some(self.t() + 6.0);
        }
        self.request_frame();
    }

    fn draw_toast(&mut self, goal: Rect, t: f64) {
        let Some((toast, _)) = self.toast.clone() else { return };
        let mouse = self.mouse;
        let mut r = self.content_rect(goal, 8.0, 10.0, 18.0);
        // App icon (e.g. of a Windows notification), cached as a bitmap.
        let bmp = toast.image.as_ref().and_then(|img| {
            let key = std::sync::Arc::as_ptr(img) as usize;
            match &self.toast_bmp {
                Some((k, g, b)) if *k == key && *g == self.gfx.gen => Some(b.clone()),
                _ => {
                    let b = self.gfx.make_bitmap(img.0, img.1, &img.2)?;
                    self.toast_bmp = Some((key, self.gfx.gen, b.clone()));
                    Some(b)
                }
            }
        });
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: TOAST, mouse, t };
        let mut top = r.cut_top(44.0, 6.0);
        if toast.click.is_some() {
            ui.hit_tip(top, TOAST_BODY, "");
        }
        let ic = top.cut_left(36.0, 12.0);
        match &bmp {
            Some(b) => ui.g.image_rr(b, ic.centered(34.0, 34.0), 9.0),
            None => {
                ui.circle(ic.cx(), ic.cy(), 18.0, toast.color.with_a(0.18));
                ui.icon(toast.icon, ic, 16.0, toast.color);
            }
        }
        if !toast.right.is_empty() {
            let rw = ui.measure(&toast.right, 16.0, Weight::Semi) + 4.0;
            let rr = top.cut_right(rw, 8.0);
            ui.text(&toast.right, rr, 16.0, toast.color, Weight::Semi, Align::Right);
        }
        let mut txt = top;
        if let Some(p) = toast.progress {
            let pr = txt.cut_bottom(8.0, 0.0);
            ui.progress(pr.inset_xy(0.0, 2.0), p, toast.color, None);
        }
        let tt = txt.cut_top(txt.h * 0.55, 0.0);
        ui.text(&toast.title, tt, 14.0, col::TEXT, Weight::Semi, Align::Left);
        ui.text(&toast.sub, txt, 12.0, col::DIM, Weight::Regular, Align::Left);
        if !toast.actions.is_empty() {
            let row = r.cut_top(28.0, 0.0);
            let n = toast.actions.len();
            for (k, (cell, (label, _))) in row.columns(n, 8.0).into_iter().zip(toast.actions.iter()).enumerate() {
                if k == 0 {
                    ui.button_accent(cell, label, k as u32, toast.color);
                } else {
                    ui.button(cell, label, k as u32);
                }
            }
        }
    }

    /// Pill and Hidden styles: an invisible hover strip along the edge, plus
    /// (Pill) a thin bar that fades out as the notch opens.
    fn draw_bar(&mut self, along: f32, depth: f32) {
        let pill = self.settings.style == NotchStyle::Pill;
        // Alpha of ~1% is invisible but still catches the mouse
        // (fully transparent layered-window pixels are click-through).
        let (za, zd) = if pill { (200.0, 14.0) } else { (280.0, 3.0) };
        let zone = self.edge_rect(za, zd);
        self.gfx.fill_rect(zone, col::NOTCH.with_a(0.012));
        self.hits.insert(0, Hit { rect: zone, module: APP_HIT, id: u32::MAX, tip: "" });
        let fade = (1.0 - depth / 18.0).clamp(0.0, 1.0);
        if !pill || fade <= 0.0 {
            return;
        }
        let color = match self.live_module() {
            Some(i) => self.modules[i].live_color(),
            None => col::TEXT.with_a(0.6),
        };
        // A 5 px bar, 4 px away from the edge.
        let slot = self.edge_rect(along, 9.0);
        let bar = match self.settings.edge {
            Edge::Top => Rect::new(slot.x, slot.y + 4.0, slot.w, 5.0),
            Edge::Bottom => Rect::new(slot.x, slot.y, slot.w, 5.0),
            Edge::Left => Rect::new(slot.x + 4.0, slot.y, 5.0, slot.h),
            Edge::Right => Rect::new(slot.x, slot.y, 5.0, slot.h),
        };
        self.gfx.alpha = fade;
        // Faint dark rim so the bar also shows on white title bars.
        self.gfx.fill_rr(bar.inset(-1.0), 3.5, col::NOTCH.with_a(0.35));
        self.gfx.fill_rr(bar, 2.5, color);
        self.gfx.alpha = 1.0;
    }

    fn draw_collapsed(&mut self, goal: Rect, t: f64) {
        let mouse = self.mouse;
        let compact = self.settings.style == NotchStyle::Compact;
        let vertical = self.settings.edge.vertical();
        if let Some(i) = self.live_module() {
            let (left, right) = if vertical {
                // Icon/art on top, time below (Compact: icon only).
                let s = if compact { 16.0 } else { 20.0 };
                let top = Rect::new(goal.cx() - s / 2.0, goal.y + 10.0, s, s);
                let bottom = if compact { Rect::default() } else { Rect::new(goal.x + 2.0, goal.b() - 30.0, goal.w - 4.0, 18.0) };
                (top, bottom)
            } else {
                let (wing, pad) = if compact { (68.0, 10.0) } else { (92.0, 12.0) };
                (
                    Rect::new(goal.x + pad, goal.y + 2.0, wing, goal.h - 4.0),
                    Rect::new(goal.r() - pad - wing, goal.y + 2.0, wing, goal.h - 4.0),
                )
            };
            let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: i, mouse, t };
            self.modules[i].draw_live(&mut ui, left, right);
            // Live wings are display-only; clicking anywhere expands.
            self.hits.retain(|h| h.module != i);
            return;
        }
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: APP_HIT, mouse, t };
        let area = if vertical { goal.inset_xy(2.0, 14.0) } else { goal.inset_xy(14.0, 2.0) };
        let mood_t = (t - self.mood_since) as f32;
        let fade = (mood_t / 0.6).clamp(0.0, 1.0);
        match self.mood {
            Mood::Clock => {
                let now = Now::get();
                let size = if compact { 11.0 } else { 14.0 };
                if vertical {
                    // Stacked hours over minutes.
                    let half = area.h.min(size * 2.6) / 2.0;
                    let top = Rect::new(area.x, area.cy() - half, area.w, half);
                    let bot = Rect::new(area.x, area.cy(), area.w, half);
                    ui.text(&format!("{:02}", now.hour), top, size, col::TEXT, Weight::Semi, Align::Center);
                    ui.text(&format!("{:02}", now.min), bot, size, col::DIM, Weight::Semi, Align::Center);
                } else {
                    ui.text(&now.hhmm(), area, size, col::TEXT, Weight::Semi, Align::Center);
                }
            }
            m => {
                ui.alpha = fade;
                draw_mascot(&mut ui, area, self.settings.mascot, m, t, self.origin, self.scale);
                ui.alpha = 1.0;
            }
        }
    }

    // --------------------------------------------------------------- input

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y)).copied()
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        if !self.hover {
            self.hover = true;
            unsafe {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            // A leave/enter blip (cursor jittering on a thin edge strip) keeps
            // the hover timer running instead of restarting it.
            if self.t() - self.last_leave > 0.2 {
                self.hover_since = self.t();
            }
            if self.can_hover_open() && self.settings.style.hover_delay() <= 0.0 {
                self.open_from_hover();
            }
        }
        self.leave_at = None;
        // Dragging something out of a module?
        if let Some((hit, p0)) = self.press {
            let moved = (x - p0.0).abs() + (y - p0.1).abs();
            if moved > 6.0 && hit.module < self.modules.len() {
                if let Some(paths) = self.modules[hit.module].drag_out(hit.id) {
                    self.press = None;
                    self.deferred.push(Deferred::DragOut(paths));
                }
            }
        }
        self.request_frame();
    }

    /// Hovering opens the panel unless that would cover something the user is
    /// about to click: a toast with buttons stays put while hovered, and after
    /// a toast button was clicked the panel waits until the mouse leaves once.
    fn can_hover_open(&self) -> bool {
        let toast_buttons = self.toast.as_ref().map_or(false, |(t, _)| !t.actions.is_empty())
            || (self.toast.is_none() && self.queue.front().map_or(false, |t| !t.actions.is_empty()));
        !self.expanded && !self.dragging && !self.hover_block && !toast_buttons
    }

    fn open_from_hover(&mut self) {
        self.expanded = true;
        self.raise();
        self.last_activity = self.t();
        if self.mood != Mood::Clock {
            self.mood = Mood::Clock;
            self.mood_since = self.t();
        }
        self.request_frame();
    }

    fn on_mouse_leave(&mut self) {
        dlog("leave");
        self.hover = false;
        self.hover_block = false;
        self.tip = None;
        self.last_leave = self.t();
        self.mouse = (-1e4, -1e4);
        self.press = None;
        // The big settings panel is easy to overshoot; give it more slack.
        self.leave_at = Some(self.t() + if self.settings_open { 0.9 } else { 0.35 });
        self.request_frame();
    }

    fn on_down(&mut self, x: f32, y: f32) {
        self.press = self.hit_at(x, y).map(|h| (h, (x, y)));
        // A click hides the tooltip until the mouse moves to another hit.
        if let Some(tip) = &mut self.tip {
            tip.3 = true;
        }
        dlog(format!(
            "down ({x:.0},{y:.0}) mode={} tab={} hits={} -> {:?}",
            self.mode() as u8,
            self.tab,
            self.hits.len(),
            self.press.map(|(h, _)| (h.module, h.id))
        ));
    }

    fn on_up(&mut self, x: f32, y: f32) {
        dlog(format!("up ({x:.0},{y:.0}) press={:?}", self.press.map(|(h, _)| (h.module, h.id))));
        let Some((hit, _)) = self.press.take() else { return };
        if !hit.rect.contains(x, y) {
            return;
        }
        let rel = ((x - hit.rect.x) / hit.rect.w.max(1.0), (y - hit.rect.y) / hit.rect.h.max(1.0));
        match hit.module {
            APP_HIT => {
                if hit.id == u32::MAX {
                    // Background click: expand, or drop keyboard focus.
                    self.expanded = true;
                    if let Some(k) = self.keyboard {
                        self.modules[k].blur();
                        self.release_keyboard();
                    }
                } else if hit.id == GEAR {
                    if self.settings_open {
                        self.settings_open = false;
                    } else {
                        self.open_settings();
                    }
                } else {
                    self.tab = hit.id as usize;
                    self.settings_open = false;
                    if let Some(k) = self.keyboard {
                        self.modules[k].blur();
                        self.release_keyboard();
                    }
                }
            }
            SETTINGS => self.settings_action(hit.id),
            ONBOARD => self.onboarding_action(hit.id),
            TOAST => {
                // The mouse is still on the notch: don't let hover turn the
                // collapsing toast straight into the full panel.
                self.hover_block = true;
                self.expanded = false;
                if let Some((toast, _)) = self.toast.take() {
                    if hit.id == TOAST_BODY {
                        if let Some(id) = toast.click {
                            self.call(toast.module, |m, c| m.action(id, (0.0, 0.0), c));
                        }
                    } else if let Some((_, id)) = toast.actions.get(hit.id as usize) {
                        let id = *id;
                        self.call(toast.module, |m, c| m.action(id, (0.0, 0.0), c));
                    }
                }
            }
            i if i < self.modules.len() => {
                self.call(i, |m, c| m.action(hit.id, rel, c));
            }
            _ => {}
        }
        self.request_frame();
    }

    fn on_wheel(&mut self, delta: f32) {
        if self.mode() == Mode::Expanded {
            self.modules[self.tab].scroll(delta);
            self.request_frame();
        }
    }

    fn on_key(&mut self, vk: u16) -> bool {
        let Some(k) = self.keyboard else { return false };
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        self.call(k, |m, c| m.key(vk, ctrl, c))
    }

    fn on_char(&mut self, ch: char) -> bool {
        let Some(k) = self.keyboard else { return false };
        self.call(k, |m, c| m.chr(ch, c))
    }

    fn on_blur(&mut self) {
        if let Some(k) = self.keyboard {
            self.modules[k].blur();
            self.release_keyboard();
            self.request_frame();
        }
    }

    // --------------------------------------------------- drag & drop (in)

    pub fn drag_enter(&mut self) {
        if self.dragging_out {
            return;
        }
        self.drag_files = true;
        if let Some(i) = self.modules.iter().position(|m| m.wants_drops()) {
            self.tab = i;
        }
        for m in self.modules.iter_mut() {
            m.drag_hover(true);
        }
        self.request_frame();
    }

    pub fn drag_leave(&mut self) {
        self.drag_files = false;
        for m in self.modules.iter_mut() {
            m.drag_hover(false);
        }
        self.leave_at = Some(self.t() + 0.5);
        self.request_frame();
    }

    pub fn drop_paths(&mut self, paths: Vec<String>) {
        self.drag_leave();
        if self.dragging_out || paths.is_empty() {
            return;
        }
        let order: Vec<usize> = std::iter::once(self.tab).chain(0..self.modules.len()).collect();
        for i in order {
            if self.modules[i].wants_drops() || i == self.tab {
                let p = paths.clone();
                if self.call(i, |m, c| m.drop_files(p, c)) {
                    self.tab = i;
                    self.expanded = true;
                    self.leave_at = Some(self.t() + 2.0);
                    break;
                }
            }
        }
    }

    // ------------------------------------------------------- move the notch

    /// Right-drag: snap to the nearest edge of whichever monitor the cursor
    /// is on, and follow the cursor along it.
    fn on_right_drag(&mut self) {
        let Some(p0) = self.rdrag else { return };
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        if !self.dragging {
            if (p.x - p0.x).abs() + (p.y - p0.y).abs() < 8 {
                return;
            }
            self.dragging = true;
            self.expanded = false;
            self.toast = None;
        }
        self.move_to_point(p);
    }

    pub fn move_to_point(&mut self, p: POINT) {
        unsafe {
            let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let r = mi.rcWork;
            let (w, h) = ((r.right - r.left).max(1) as f32, (r.bottom - r.top).max(1) as f32);
            let fx = ((p.x - r.left) as f32 / w).clamp(0.0, 1.0);
            let fy = ((p.y - r.top) as f32 / h).clamp(0.0, 1.0);
            // Nearest edge, measured in pixels.
            let d = [
                (Edge::Top, (p.y - r.top) as f32),
                (Edge::Bottom, (r.bottom - p.y) as f32),
                (Edge::Left, (p.x - r.left) as f32),
                (Edge::Right, (r.right - p.x) as f32),
            ];
            let edge = d.iter().min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|e| e.0).unwrap_or(Edge::Top);
            self.settings.edge = edge;
            self.settings.pos = if edge.vertical() { fy } else { fx };
            let mc = mi.rcMonitor;
            self.settings.monitor = Some(((mc.left + mc.right) / 2, (mc.top + mc.bottom) / 2));
        }
        self.place();
        self.request_frame();
    }

    fn end_drag(&mut self) {
        self.dragging = false;
        self.settings.save();
        dlog(format!("moved to {} at {:.3}", self.settings.edge.key(), self.settings.pos));
        self.request_frame();
    }

    /// Tray "Position" menu: jump to an edge (keeping the position along it).
    pub fn set_edge(&mut self, edge: Edge, center: bool) {
        self.settings.edge = edge;
        if center {
            self.settings.pos = 0.5;
        }
        self.settings.save();
        self.place();
        self.request_frame();
    }

    // ------------------------------------------------------------ messages

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let xy = || {
            let x = (lp.0 & 0xffff) as i16 as f32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
            (x / self.scale, y / self.scale)
        };
        match msg {
            WM_TIMER => {
                match wp.0 {
                    TIMER_TICK => self.tick(),
                    TIMER_FRAME => self.timer_frame(),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_APP_WAKE => {
                self.tick();
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                if self.rdrag.is_some() {
                    self.on_right_drag();
                    return Some(LRESULT(0));
                }
                let (x, y) = xy();
                self.on_mouse_move(x, y);
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN => {
                let mut p = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut p);
                    SetCapture(self.hwnd);
                }
                self.rdrag = Some(p);
                self.dragging = false;
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                if self.rdrag.take().is_some() && self.dragging {
                    self.end_drag();
                }
                None
            }
            WM_MOUSELEAVE => {
                self.on_mouse_leave();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (x, y) = xy();
                self.on_down(x, y);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = xy();
                self.on_up(x, y);
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                // Plain right-click opens the menu; right-drag moves the notch.
                let was_drag = self.dragging;
                self.rdrag = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                if was_drag {
                    self.end_drag();
                } else {
                    // Plain right-click on the notch opens the settings panel.
                    self.open_settings();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let d = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
                self.on_wheel(d);
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                if self.on_key(wp.0 as u16) {
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_CHAR => {
                if let Some(c) = char::from_u32(wp.0 as u32) {
                    if !c.is_control() && self.on_char(c) {
                        return Some(LRESULT(0));
                    }
                }
                None
            }
            WM_KILLFOCUS => {
                self.on_blur();
                None
            }
            WM_MOUSEACTIVATE => {
                if self.keyboard.is_some() {
                    None
                } else {
                    Some(LRESULT(MA_NOACTIVATE as isize))
                }
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                self.place();
                self.request_frame();
                None
            }
            WM_APP_TRAY => {
                let ev = (lp.0 & 0xffff) as u32;
                if ev == WM_LBUTTONUP {
                    self.open_settings();
                } else if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                    self.deferred.push(Deferred::TrayMenu);
                }
                Some(LRESULT(0))
            }
            WM_COMMAND => {
                tray::command(self, (wp.0 & 0xffff) as u32);
                Some(LRESULT(0))
            }
            m if m == tray::taskbar_created() => {
                tray::add(self.hwnd);
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- self-test

fn snap(a: &mut App, label: &str) -> String {
    let tgt = a.target();
    a.cur = tgt;
    a.render(tgt, 0.0);
    let ok = a.gfx.last_err.is_none();
    format!(
        "{:<4} {label:<34} size={:>3.0}x{:<3.0} opaque_px={:<7} bright_px={:<6} hits={}\n",
        if ok { "OK" } else { "FAIL" },
        tgt.0,
        tgt.1,
        a.gfx.coverage(),
        a.gfx.bright(),
        a.hits.len()
    )
}

fn find(a: &App, title: &str) -> Option<usize> {
    a.modules.iter().position(|m| m.title() == title)
}

/// Exercise every view and the main flows without showing the window.
pub fn selftest() -> String {
    // Let the media thread find the current session first.
    std::thread::sleep(std::time::Duration::from_secs(3));
    with_app(|a| {
        let mut out = String::new();
        // The self-test runs on the user's PC: no chimes.
        sound::mute(true);
        // A fresh data folder is a first run: the welcome tour must be up.
        let fresh_tour = a.onboarding == Some(0) && a.mode() == Mode::Expanded;
        out += &format!("{:<4} first run opens the welcome tour\n", if fresh_tour { "OK" } else { "FAIL" });
        a.onboarding = None;
        a.tick();
        a.settings.personality = true;
        a.settings.style = NotchStyle::Full;
        a.queue.clear();
        a.toast = None;

        a.expanded = false;
        a.mood = Mood::Clock;
        let live = a.live_module().map(|i| a.modules[i].title()).unwrap_or("none");
        out += &snap(a, &format!("collapsed (live: {live})"));
        for (m, n) in [(Mood::Eyes, "mascot eyes"), (Mood::Doze, "mascot doze"), (Mood::Wave, "mascot wave")] {
            a.mood = m;
            a.mood_since = a.t() - 5.0;
            if a.live_module().is_none() {
                out += &snap(a, n);
            } else {
                out += &format!("SKIP {n:<34} (a live activity is showing)\n");
            }
        }
        a.mood = Mood::Clock;
        for s in [NotchStyle::Compact, NotchStyle::Pill, NotchStyle::Hidden] {
            a.settings.style = s;
            out += &snap(a, &format!("collapsed style {}", s.key()));
        }
        a.settings.style = NotchStyle::Full;

        // Edges: the drawn shape must touch the chosen screen side, and
        // panels must stay upright (wider than tall) on every edge.
        for e in Edge::ALL {
            a.settings.edge = e;
            a.anchor = if e.vertical() { CANVAS_H / 2.0 } else { CANVAS_W / 2.0 };
            for (style, open) in [(NotchStyle::Full, false), (NotchStyle::Pill, false), (NotchStyle::Full, true)] {
                a.settings.style = style;
                a.expanded = open;
                let label = format!("edge {} {}{}", e.key(), style.key(), if open { " open" } else { "" });
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                let (pw, ph) = (a.gfx.pw, a.gfx.ph);
                let ok = match a.gfx.bbox() {
                    Some((x0, y0, x1, y1)) => {
                        let touches = match e {
                            Edge::Top => y0 == 0,
                            Edge::Bottom => y1 == ph - 1,
                            Edge::Left => x0 == 0,
                            Edge::Right => x1 == pw - 1,
                        };
                        let upright = !open || (x1 - x0) > (y1 - y0);
                        touches && upright && a.gfx.last_err.is_none()
                    }
                    None => false,
                };
                out += &format!("{:<4} {label:<34} bbox={:?}\n", if ok { "OK" } else { "FAIL" }, a.gfx.bbox());
            }
        }
        a.expanded = false;
        a.settings.style = NotchStyle::Full;
        // Right-drag logic: a point near the middle of the primary monitor's
        // left side must snap to the left edge at about half height.
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let r = mi.rcWork;
            a.move_to_point(POINT { x: r.left + 5, y: (r.top + r.bottom) / 2 });
            let ok = a.settings.edge == Edge::Left && (a.settings.pos - 0.5).abs() < 0.02;
            out += &format!("{:<4} drag snaps to nearest edge (left, pos {:.2})\n", if ok { "OK" } else { "FAIL" }, a.settings.pos);
            a.move_to_point(POINT { x: r.right - 300, y: r.bottom - 2 });
            let ok = a.settings.edge == Edge::Bottom && a.settings.pos > 0.7;
            out += &format!("{:<4} drag snaps to bottom near right corner (pos {:.2})\n", if ok { "OK" } else { "FAIL" }, a.settings.pos);
        }
        a.settings.edge = Edge::Top;
        a.settings.pos = 0.5;
        a.settings.monitor = None;
        a.place();

        for i in 0..a.modules.len() {
            if !a.modules[i].has_tab() {
                continue;
            }
            a.tab = i;
            a.expanded = true;
            let t = a.modules[i].title();
            out += &snap(a, &format!("tab: {t}"));
        }

        // To-do: add a task through the real editor + keyboard path.
        if let Some(i) = find(a, "TodoList") {
            a.tab = i;
            a.call(i, |m, c| m.action(1, (0.0, 0.0), c));
            out += &snap(a, "todo editor open");
            a.call(i, |m, c| {
                m.action(10, (0.0, 0.0), c); // +15m
                for ch in "Selftest task".chars() {
                    m.chr(ch, c);
                }
                m.key(0x0D, false, c);
            });
            let saved = crate::store::read("todo.txt");
            let ok = saved.lines().any(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                f.len() > 4 && f[4] == "Selftest task" && f[2].parse::<f64>().unwrap_or(0.0) > 0.0
            });
            out += &format!("{:<4} todo saved to disk with due time\n", if ok { "OK" } else { "FAIL" });
            out += &snap(a, "todo list with task");
        }
        // Water: drink + settings page.
        if let Some(i) = find(a, "Water Reminder") {
            a.tab = i;
            a.call(i, |m, c| m.action(1, (0.0, 0.0), c));
            let saved = crate::store::read("water.txt");
            out += &format!("{:<4} water drink logged\n", if saved.contains("log ") { "OK" } else { "FAIL" });
            a.call(i, |m, c| m.action(2, (0.0, 0.0), c));
            out += &snap(a, "water settings page");
            a.call(i, |m, c| m.action(3, (0.0, 0.0), c));
        }
        // Shelf: drop a real file.
        if let Some(i) = find(a, "Shelf") {
            a.tab = i;
            let p = std::env::current_dir().unwrap_or_default().join("Cargo.toml").display().to_string();
            a.drop_paths(vec![p]);
            let saved = crate::store::read("shelf.txt");
            out += &format!("{:<4} shelf item stored\n", if saved.contains("Cargo.toml") { "OK" } else { "FAIL" });
            a.tab = i;
            a.expanded = true;
            out += &snap(a, "shelf with item");
            let drag = a.modules[i].drag_out(1 << 24);
            out += &format!("{:<4} shelf drag-out provides path\n", if drag.map_or(false, |d| d.len() == 1) { "OK" } else { "FAIL" });
        }
        // Clipboard history: limits, de-duplication, pins that persist, clear.
        if let Some(i) = find(a, "Clipboard") {
            use crate::modules::clipboard::{self as cb, Content};
            let field = |s: &str, k: &str| -> String {
                s.split_whitespace().find_map(|w| w.strip_prefix(&format!("{k}="))).unwrap_or("").to_string()
            };
            a.call(i, |m, c| m.action(cb::A_DEMO, (0.0, 0.0), c));
            a.call(i, |m, c| m.action(cb::A_DEMO, (0.0, 0.0), c));
            let st = a.modules[i].debug_state();
            let ok = field(&st, "unpinned") == "30" && field(&st, "dupes") == "0" && field(&st, "images") == "1";
            out += &format!("{:<4} clipboard keeps the newest 30, no duplicates ({st})\n", if ok { "OK" } else { "FAIL" });
            out += &format!("{:<4} clipboard history is off by default\n", if field(&st, "on") == "false" && field(&st, "mode") == "0" { "OK" } else { "FAIL" });
            let img = cb::image_from_dib(cb::make_test_dib(64, 32));
            let ok = img.as_ref().map_or(false, |im| im.w == 64 && im.h == 32 && im.thumb.0 == 64 && im.thumb.1 == 32 && im.thumb.2.len() == 64 * 32 * 4);
            out += &format!("{:<4} copied images get a thumbnail ({:?})\n", if ok { "OK" } else { "FAIL" }, img.map(|i| (i.thumb.0, i.thumb.1)));
            let big = cb::image_from_dib(cb::make_test_dib(400, 100)).map(|i| (i.thumb.0, i.thumb.1));
            out += &format!("{:<4} big images shrink to fit the thumbnail {:?}\n", if big == Some((80, 20)) { "OK" } else { "FAIL" }, big);
            let top: u32 = field(&st, "top_id").parse().unwrap_or(0);
            a.call(i, |m, c| m.action((2 << 24) | top, (0.0, 0.0), c)); // pin
            let saved = crate::store::read("clipboard.txt");
            let fresh = cb::Clipboard::new(a.hwnd).debug_state();
            let ok = saved.lines().count() == 1 && field(&fresh, "pinned") == "1";
            out += &format!("{:<4} pinned clipboard entry survives a restart\n", if ok { "OK" } else { "FAIL" });
            a.call(i, |m, c| m.action(1, (0.0, 0.0), c)); // clear
            let st = a.modules[i].debug_state();
            out += &format!("{:<4} clear keeps pinned entries ({})\n", if field(&st, "items") == "1" && field(&st, "pinned") == "1" { "OK" } else { "FAIL" }, field(&st, "items"));
            a.tab = i;
            a.expanded = true;
            out += &snap(a, "clipboard with a pinned entry");
            a.call(i, |m, c| m.action((3 << 24) | top, (0.0, 0.0), c)); // delete
            let ok = a.modules[i].debug_state().contains("items=0") && crate::store::read("clipboard.txt").trim().is_empty();
            out += &format!("{:<4} deleting a pinned entry removes it from disk\n", if ok { "OK" } else { "FAIL" });
            a.expanded = false;

            // Live Windows clipboard (opt-in: it briefly replaces what you copied,
            // then puts it back).
            if std::env::var("WINDYLAND_TEST_CLIPBOARD").is_ok() {
                let hwnd = a.hwnd;
                let saved = cb::current(hwnd);
                a.settings.clip_on = true;
                a.settings.clip_mode = ClipMode::All;
                a.call(i, |m, c| m.tick(c));
                let wait = |a: &mut App| {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    a.call(i, |m, c| m.tick(c));
                };
                let tag = format!("WindyLand selftest {}", crate::time::unix_now() as u64);
                cb::set_clipboard(hwnd, &Content::Text(tag.clone()));
                wait(a);
                let st = a.modules[i].debug_state();
                out += &format!("{:<4} copying text is recorded instantly\n", if st.ends_with(&format!("top={tag}")) { "OK" } else { "FAIL" });
                let older: u32 = field(&st, "top_id").parse().unwrap_or(0);
                cb::set_private_for_test(hwnd, "secret-password-123");
                wait(a);
                let st = a.modules[i].debug_state();
                out += &format!("{:<4} password-manager copies are not recorded\n", if !st.contains("secret-password") { "OK" } else { "FAIL" });
                let file = std::env::current_dir().unwrap_or_default().join("Cargo.toml").display().to_string();
                cb::set_clipboard(hwnd, &Content::Files(vec![file]));
                wait(a);
                let st = a.modules[i].debug_state();
                out += &format!("{:<4} copying a file is recorded\n", if st.contains("top=Cargo.toml") { "OK" } else { "FAIL" });
                if let Some(im) = cb::image_from_dib(cb::make_test_dib(64, 32)) {
                    cb::set_clipboard(hwnd, &Content::Image(std::sync::Arc::new(im)));
                }
                wait(a);
                let st = a.modules[i].debug_state();
                out += &format!("{:<4} copying an image is recorded (Everything mode)\n", if st.contains("top=Image  64 x 32") { "OK" } else { "FAIL" });
                a.settings.clip_mode = ClipMode::Text;
                a.call(i, |m, c| m.tick(c));
                let file2 = std::env::current_dir().unwrap_or_default().join("README.md").display().to_string();
                cb::set_clipboard(hwnd, &Content::Files(vec![file2]));
                wait(a);
                let st = a.modules[i].debug_state();
                out += &format!("{:<4} Text-only mode skips copied files\n", if !st.contains("README") { "OK" } else { "FAIL" });
                a.settings.clip_mode = ClipMode::All;
                a.call(i, |m, c| m.tick(c));
                a.call(i, |m, c| m.action((1 << 24) | older, (0.0, 0.0), c)); // copy the text again
                let back = cb::current(hwnd);
                out += &format!("{:<4} clicking an entry puts it back on the clipboard\n", if back == Some(Content::Text(tag.clone())) { "OK" } else { "FAIL" });
                if let Some(s) = saved {
                    cb::set_clipboard(hwnd, &s);
                }
                wait(a);
                a.call(i, |m, c| m.action(1, (0.0, 0.0), c));
                a.settings.clip_on = false;
                a.settings.clip_mode = ClipMode::Text;
                a.call(i, |m, c| m.tick(c));
            }
        }
        // Mic / camera dots.
        if let Some(i) = find(a, "Privacy") {
            use crate::modules::privacy::*;
            let names = [
                app_name("C:#Program Files#Zoom#bin#Zoom.exe"),
                app_name("Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"),
                app_name("MSTeams_8wekyb3d8bbwe"),
            ];
            let ok = names == ["Zoom".to_string(), "Windows Sound Recorder".to_string(), "MS Teams".to_string()];
            out += &format!("{:<4} mic/camera app names from the registry keys {:?}\n", if ok { "OK" } else { "FAIL" }, names);
            a.queue.clear();
            a.toast = None;
            a.call(i, |m, c| m.tick(c));
            a.call(i, |m, c| m.action(A_DEMO, (0.0, 0.0), c));
            a.call(i, |m, c| m.tick(c));
            let dots = a.indicators();
            let toast = a.queue.iter().any(|t| t.key == "privacy" && t.title == "Microphone in use" && t.sub.contains("Demo Recorder"));
            out += &format!("{:<4} mic in use shows an orange dot and names the app\n", if dots == vec![MIC] && toast { "OK" } else { "FAIL" });
            a.queue.clear();
            a.expanded = false;
            a.settings.style = NotchStyle::Full;
            let mut shapes = Vec::new();
            for on in [true, false] {
                a.settings.privacy_dots = on;
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                shapes.push(a.gfx.coverage());
            }
            a.settings.privacy_dots = true;
            out += &format!("{:<4} dot is drawn beside the closed notch ({} px vs {} px without)\n", if shapes[0] > shapes[1] + 40 { "OK" } else { "FAIL" }, shapes[0], shapes[1]);
            a.call(i, |m, c| m.action(A_DEMO, (0.0, 0.0), c));
            a.call(i, |m, c| m.tick(c));
            out += &format!("{:<4} dot goes away when the mic is released\n", if a.indicators().is_empty() { "OK" } else { "FAIL" });
            a.queue.clear();
        }
        // Pomodoro: start -> live activity, skip -> toast.
        if let Some(i) = find(a, "Pomodoro") {
            a.call(i, |m, c| m.action(1, (0.0, 0.0), c));
            a.expanded = false;
            a.keyboard = None;
            out += &format!("{:<4} pomodoro live activity while running\n", if a.modules[i].live().is_some() { "OK" } else { "FAIL" });
            out += &snap(a, "collapsed with pomodoro");
            for s in [NotchStyle::Compact, NotchStyle::Pill] {
                a.settings.style = s;
                out += &snap(a, &format!("pomodoro live, style {}", s.key()));
            }
            a.settings.style = NotchStyle::Full;
            a.call(i, |m, c| m.action(3, (0.0, 0.0), c));
            a.toast = a.queue.pop_front().map(|t| (t, a.t() + 10.0));
            out += &format!("{:<4} pomodoro skip produced toast\n", if a.toast.is_some() { "OK" } else { "FAIL" });
            out += &snap(a, "toast with actions");
        }
        a.toast = None;
        // Timer: preset, start -> live, expire -> toast, restart; stopwatch laps.
        if let Some(i) = find(a, "Timer") {
            if let Some(p) = find(a, "Pomodoro") {
                a.call(p, |m, c| m.action(2, (0.0, 0.0), c)); // reset pomodoro so the timer is live
            }
            a.tab = i;
            a.expanded = true;
            a.call(i, |m, c| m.action(12, (0.0, 0.0), c)); // 5 min preset
            out += &snap(a, "tab: Timer (countdown)");
            a.call(i, |m, c| m.action(3, (0.0, 0.0), c)); // start
            a.expanded = false;
            let live = a.live_module().map(|k| a.modules[k].title()).unwrap_or("none");
            out += &format!("{:<4} timer running shows in notch (live: {live})\n", if live == "Timer" { "OK" } else { "FAIL" });
            out += &snap(a, "collapsed with timer");
            a.queue.clear();
            a.call(i, |m, c| m.action(crate::modules::timer::A_DEBUG_EXPIRE, (0.0, 0.0), c));
            a.call(i, |m, c| m.tick(c));
            let ok = a.queue.iter().any(|t| t.key == "timer" && t.actions.len() == 2);
            out += &format!("{:<4} finished timer pops toast with Restart/Dismiss\n", if ok { "OK" } else { "FAIL" });
            a.queue.clear();
            a.call(i, |m, c| m.action(20, (0.0, 0.0), c)); // Restart from toast
            out += &format!("{:<4} restart from toast runs again\n", if a.modules[i].live().is_some() { "OK" } else { "FAIL" });
            a.call(i, |m, c| m.action(4, (0.0, 0.0), c)); // reset
            a.call(i, |m, c| {
                m.action(2, (0.0, 0.0), c); // stopwatch view
                m.action(3, (0.0, 0.0), c); // start
            });
            std::thread::sleep(std::time::Duration::from_millis(60));
            a.call(i, |m, c| {
                m.action(7, (0.0, 0.0), c); // lap
                m.action(7, (0.0, 0.0), c); // lap
            });
            a.expanded = true;
            out += &snap(a, "tab: Timer (stopwatch)");
            let st = a.modules[i].debug_state();
            out += &format!("{:<4} stopwatch running with 2 laps\n", if st.contains("running=true laps=2") { "OK" } else { "FAIL" });
            a.call(i, |m, c| {
                m.action(4, (0.0, 0.0), c);
                m.action(1, (0.0, 0.0), c);
            });
            a.expanded = false;
        }
        // Hovering a toast that has buttons must not swap it for the panel,
        // and clicking a toast button must not reopen the panel under the
        // still-hovering mouse.
        if let Some(ti) = find(a, "Timer") {
            a.expanded = false;
            a.queue.clear();
            let mut tt = Toast::new("selftest", glyph::TIMER, col::PINK, "Selftest", "toast").action("Reset", 4).action("Reset", 4);
            tt.module = ti;
            a.toast = Some((tt, a.t() + 30.0));
            a.hover = true;
            a.hover_since = a.t() - 5.0;
            a.frame();
            let ok = !a.expanded && a.mode() == Mode::Toast;
            out += &format!("{:<4} hovering a toast with buttons keeps the toast\n", if ok { "OK" } else { "FAIL" });
            let tgt = a.target();
            a.cur = tgt;
            a.render(tgt, 0.0);
            let btn = a.hits.iter().find(|h| h.module == TOAST && h.id == 0).copied();
            let ok = match btn {
                Some(b) => {
                    a.on_down(b.rect.cx(), b.rect.cy());
                    a.on_up(b.rect.cx(), b.rect.cy());
                    a.hover_since = a.t() - 5.0;
                    a.frame();
                    a.toast.is_none() && !a.expanded
                }
                None => false,
            };
            out += &format!("{:<4} toast button click does not reopen the panel while hovered\n", if ok { "OK" } else { "FAIL" });
            a.on_mouse_leave();
            out += &format!("{:<4} hover opens again after the mouse left\n", if a.can_hover_open() { "OK" } else { "FAIL" });
            a.hover = false;
            a.leave_at = None;
            a.toast = None;
            a.queue.clear();
        }
        // Tooltips: resting on an icon-only tab shows its name.
        {
            a.expanded = true;
            a.tab = a.modules.iter().position(|m| m.has_tab()).unwrap_or(0);
            let tgt = a.target();
            a.cur = tgt;
            a.render(tgt, 0.0);
            let tab = a.hits.iter().find(|h| h.module == APP_HIT && h.id != u32::MAX && !h.tip.is_empty()).copied();
            let ok = match tab {
                Some(h) => {
                    let base = a.gfx.bright();
                    a.mouse = (h.rect.cx(), h.rect.cy());
                    a.render(tgt, 0.0);
                    if let Some(tip) = &mut a.tip {
                        tip.2 -= 2.0;
                    }
                    a.render(tgt, 0.0);
                    let shown = a.gfx.bright() > base + 20;
                    a.mouse = (-1e4, -1e4);
                    a.tip = None;
                    shown
                }
                None => false,
            };
            out += &format!("{:<4} tooltip shows on a resting tab icon\n", if ok { "OK" } else { "FAIL" });
            let mut untipped: Vec<u32> = Vec::new();
            for i in 0..a.modules.len() {
                if !a.modules[i].has_tab() {
                    continue;
                }
                a.tab = i;
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                untipped.extend(a.hits.iter().filter(|h| h.module == APP_HIT && h.id != u32::MAX && h.tip.is_empty()).map(|h| h.id));
            }
            untipped.dedup();
            let mut no_title = Vec::new();
            for i in 0..a.modules.len() {
                if !a.modules[i].has_tab() {
                    continue;
                }
                a.tab = i;
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                if !a.header_fit.0 || !a.header_fit.1 {
                    no_title.push(a.modules[i].title());
                }
            }
            out += &format!("{:<4} every tab's header fits with its title {:?}\n", if no_title.is_empty() { "OK" } else { "FAIL" }, no_title);
            out += &format!("{:<4} every tab icon has a tooltip {:?}\n", if untipped.is_empty() { "OK" } else { "FAIL" }, untipped);
            a.expanded = false;
        }
        // Mascots: every character renders in each mood, draws something
        // visible, and looks different from the others.
        {
            use std::hash::{Hash, Hasher};
            let mut hashes = Vec::new();
            for m in Mascot::ALL {
                let mut ok = true;
                let mut bright = 0;
                for (mood, mi) in [(Mood::Eyes, 0), (Mood::Doze, 1), (Mood::Wave, 2)] {
                    a.hits.clear();
                    a.gfx.begin(a.scale);
                    {
                        let mut ui = Ui { g: &mut a.gfx, hits: &mut a.hits, module: APP_HIT, mouse: (-1e4, -1e4), t: 1.0 };
                        ui.fill_rect(Rect::new(0.0, 0.0, 220.0, 48.0), col::NOTCH);
                        mascot::draw(&mut ui, Rect::new(14.0, 2.0, 172.0, 28.0), m, mood, 1.0, (1.0, 0.5));
                    }
                    a.gfx.end_and_present(a.hwnd, a.origin.0, a.origin.1);
                    ok &= a.gfx.last_err.is_none();
                    if mi == 0 {
                        bright = a.gfx.bright();
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        a.gfx.pixels().hash(&mut h);
                        hashes.push(h.finish());
                    }
                }
                let ok = ok && bright > 30;
                out += &format!("{:<4} mascot {:<8} renders (bright_px={bright})\n", if ok { "OK" } else { "FAIL" }, m.key());
            }
            let mut uniq = hashes.clone();
            uniq.sort();
            uniq.dedup();
            out += &format!("{:<4} every mascot looks different ({} of {})\n", if uniq.len() == hashes.len() { "OK" } else { "FAIL" }, uniq.len(), hashes.len());
        }
        // Sounds: every built-in sound is a valid, audible, distinct WAV;
        // the choice is saved and "Off" plays nothing.
        {
            let mut ok = true;
            let mut seen = Vec::new();
            let mut info = Vec::new();
            for s in Sound::ALL {
                if let Some(w) = sound::wav(s) {
                    let pk = sound::peak(w);
                    let secs = (w.len() - 44) as f32 / 2.0 / 44100.0;
                    ok &= &w[..4] == b"RIFF" && pk > 0.15 && pk < 1.0 && secs > 0.2 && secs < 2.0 && !seen.contains(w);
                    seen.push(w.clone());
                    info.push(format!("{} {secs:.2}s peak {pk:.2}", s.key()));
                }
            }
            out += &format!("{:<4} {} built-in sounds are valid and distinct ({})\n", if ok && seen.len() == 4 { "OK" } else { "FAIL" }, seen.len(), info.join(", "));
            sound::mute(false);
            sound::set(Sound::Off);
            let silent = !sound::chime();
            sound::mute(true);
            sound::set(a.settings.sound);
            out += &format!("{:<4} sound Off plays nothing\n", if silent { "OK" } else { "FAIL" });
        }
        // Full-screen detection: only a real full-screen window hides the
        // notch; the desktop, Start, taskbar and maximized windows never do.
        {
            let mon = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
            let cases = [
                ("F11 browser / video", RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, false, "Chrome_WidgetWin_1", true),
                ("borderless game", RECT { left: -1, top: -1, right: 1921, bottom: 1081 }, false, "UnityWndClass", true),
                ("desktop focused", RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, false, "WorkerW", false),
                ("desktop (Progman)", RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, false, "Progman", false),
                ("Start menu", RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, false, "Windows.UI.Core.CoreWindow", false),
                ("maximized window", RECT { left: -8, top: -8, right: 1928, bottom: 1040 }, true, "Chrome_WidgetWin_1", false),
                ("maximized, auto-hide taskbar", RECT { left: -8, top: -8, right: 1928, bottom: 1088 }, true, "SunAwtFrame", false),
                ("normal window", RECT { left: 100, top: 80, right: 1500, bottom: 900 }, false, "Notepad", false),
                ("full screen on other monitor", RECT { left: 1920, top: 0, right: 3840, bottom: 1080 }, false, "Chrome_WidgetWin_1", false),
            ];
            let wrong: Vec<&str> = cases.iter().filter(|c| is_fullscreen(c.1, mon, c.2, c.3) != c.4).map(|c| c.0).collect();
            out += &format!("{:<4} only real full-screen windows hide the notch {:?}\n", if wrong.is_empty() { "OK" } else { "FAIL" }, wrong);
            out += &format!("INFO full screen right now: {}\n", a.foreground_fullscreen());
            // While hidden, a reminder waits (no expiry, no chime) and shows
            // as soon as the notch is visible again.
            a.queue.clear();
            a.toast = None;
            a.expanded = false;
            a.hidden = true;
            a.push_toast(Toast::new("selftest-hidden", glyph::DROP, col::CYAN, "Time to Drink", "test").secs(0.01));
            a.frame();
            std::thread::sleep(std::time::Duration::from_millis(30));
            a.frame();
            let waited = a.toast.is_none() && a.queue.len() == 1;
            a.hidden = false;
            a.frame();
            let shown = a.toast.as_ref().map_or(false, |t| t.0.key == "selftest-hidden");
            out += &format!("{:<4} a reminder waits while the notch is hidden, then shows\n", if waited && shown { "OK" } else { "FAIL" });
            a.toast = None;
            a.queue.clear();
        }
        // Frame pacing, measured on this PC: a tab switch must draw at the
        // display's refresh rate with no long gaps, and each frame must be
        // cheap. Also reports what the old Windows-timer pacing delivered.
        {
            let probe = |ms: u32| -> f64 {
                unsafe {
                    let id = SetTimer(None, 0, ms, None);
                    let (s, mut last, mut gaps) = (std::time::Instant::now(), None, Vec::new());
                    let mut m = MSG::default();
                    while s.elapsed().as_secs_f64() < 0.5 {
                        if PeekMessageW(&mut m, None, WM_TIMER, WM_TIMER, PM_REMOVE).as_bool() {
                            if m.wParam.0 != id {
                                continue;
                            }
                            let n = s.elapsed().as_secs_f64();
                            if let Some(l) = last {
                                gaps.push(n - l);
                            }
                            last = Some(n);
                        } else {
                            std::thread::yield_now();
                        }
                    }
                    let _ = KillTimer(None, id);
                    if gaps.is_empty() { 0.0 } else { gaps.len() as f64 / gaps.iter().sum::<f64>() }
                }
            };
            let (old15, old33) = (probe(15), probe(33));
            a.queue.clear();
            a.toast = None;
            a.onboarding = None;
            a.hidden = false;
            a.settings_open = false;
            let tabs: Vec<usize> = (0..a.modules.len()).filter(|&i| a.modules[i].has_tab()).collect();
            a.tab = tabs[0];
            a.expanded = true;
            a.hover = true;
            a.leave_at = None;
            a.request_frame();
            let s0 = a.t();
            while matches!(a.pacing(), Pacing::Refresh) && a.t() - s0 < 2.0 {
                a.frame();
                wait_refresh(a.hwnd);
            }
            a.tab = tabs[tabs.len().min(4) - 1];
            a.request_frame();
            let (mut gaps, mut costs) = (Vec::new(), Vec::new());
            let mut last: Option<f64> = None;
            let mut refresh_all = true;
            let s1 = a.t();
            loop {
                let st = a.t();
                if let Some(l) = last {
                    gaps.push(st - l);
                }
                last = Some(st);
                a.frame();
                costs.push(a.t() - st);
                if !matches!(a.pacing(), Pacing::Refresh) {
                    break;
                }
                if a.t() - s1 > 2.0 {
                    refresh_all = false;
                    break;
                }
                wait_refresh(a.hwnd);
            }
            let ambient_after = matches!(a.pacing(), Pacing::After(_));
            let n = gaps.len().max(1) as f64;
            let fps = n / gaps.iter().sum::<f64>().max(1e-6);
            let worst = gaps.iter().cloned().fold(0.0, f64::max) * 1000.0;
            let avg_cost = costs.iter().sum::<f64>() / costs.len().max(1) as f64 * 1000.0;
            let max_cost = costs.iter().cloned().fold(0.0, f64::max) * 1000.0;
            let ok = refresh_all && ambient_after && fps >= 55.0 && worst < 40.0 && avg_cost < 4.0;
            out += &format!(
                "{:<4} tab switch frame rate {fps:.0} fps over {} frames, worst gap {worst:.1} ms, draw {avg_cost:.2} ms avg / {max_cost:.2} ms max\n",
                if ok { "OK" } else { "FAIL" },
                costs.len()
            );
            out += &format!("INFO old Windows-timer pacing here: 15 ms timer = {old15:.0} fps, 33 ms timer = {old33:.0} fps (old tab switches used the 33 ms one)\n");
            a.hover = false;
            a.expanded = false;
        }
        // Welcome tour: style is step one, choices apply and save, the panel
        // stays open without hover, and finishing says where to change it.
        {
            let click = |a: &mut App, id: u32| -> bool {
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                match a.hits.iter().find(|h| h.module == ONBOARD && h.id == id).copied() {
                    Some(h) => {
                        a.on_down(h.rect.cx(), h.rect.cy());
                        a.on_up(h.rect.cx(), h.rect.cy());
                        true
                    }
                    None => false,
                }
            };
            let before = (a.settings.style, a.settings.edge);
            a.queue.clear();
            a.toast = None;
            a.start_onboarding();
            a.hover = false;
            a.leave_at = Some(a.t() - 1.0);
            a.frame();
            out += &format!("{:<4} welcome tour stays open without the mouse\n", if a.mode() == Mode::Expanded && a.onboarding == Some(0) { "OK" } else { "FAIL" });
            for (k, name) in ["style", "position", "tips"].iter().enumerate() {
                a.onboarding = Some(k as u8);
                out += &snap(a, &format!("welcome tour step {} ({name})", k + 1));
            }
            a.onboarding = Some(0);
            let tiles = {
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                a.hits.iter().filter(|h| h.module == ONBOARD && (O_STYLE..O_STYLE + 4).contains(&h.id)).count()
            };
            let ok = tiles == 4 && click(a, O_STYLE + 2) && a.settings.style == NotchStyle::Pill && crate::store::read("settings.txt").contains("style=pill");
            out += &format!("{:<4} step 1 offers the 4 styles and picking Pill applies it\n", if ok { "OK" } else { "FAIL" });
            let ok = click(a, O_NEXT) && a.onboarding == Some(1) && click(a, O_EDGE + 1) && a.settings.edge == Edge::Bottom && a.mode() == Mode::Expanded;
            out += &format!("{:<4} step 2 moves the notch and the tour stays open\n", if ok { "OK" } else { "FAIL" });
            let ok = click(a, O_NEXT) && a.onboarding == Some(2) && click(a, O_NEXT) && a.onboarding.is_none();
            let saved = crate::store::read("settings.txt").contains("onboarded=1");
            let told = a.queue.iter().any(|t| t.key == "welcome" && t.sub.contains("right-click"));
            out += &format!("{:<4} finishing saves it and says how to change things later\n", if ok && saved && told { "OK" } else { "FAIL" });
            // Updating from a version without the tour (settings but no
            // "onboarded" key) shows it once; once finished it never returns.
            let saved_settings = crate::store::read("settings.txt");
            crate::store::write("settings.txt", "edge=top\nstyle=pill\nvolume_osd=1\n");
            let old_user = crate::store::Settings::load();
            crate::store::write("settings.txt", &saved_settings);
            let ok = !old_user.onboarded && old_user.style == NotchStyle::Pill && crate::store::Settings::load().onboarded;
            out += &format!("{:<4} people updating from a version without the tour get it once\n", if ok { "OK" } else { "FAIL" });
            // Tour text fits: style tiles, tip cards, hints beside the buttons.
            let mut tight = Vec::new();
            let tile_w = (ONBOARD_SIZE.0 - 8.0 - 30.0) / 4.0 - 16.0;
            for s in ["Clock, music, mascot", "A smaller notch", "Just a thin bar", "Invisible until hovered"] {
                if a.gfx.measure(s, 10.0, Weight::Regular) > tile_w {
                    tight.push(s);
                }
            }
            let card_w = (ONBOARD_SIZE.0 - 8.0 - 20.0) / 3.0 - 20.0;
            for s in ["Hover to open", "Right-click: Settings", "Right-drag: move it"] {
                if a.gfx.measure(s, 12.5, Weight::Semi) > card_w {
                    tight.push(s);
                }
            }
            let foot_w = ONBOARD_SIZE.0 - 8.0 - (a.gfx.measure("Start using WindyLand", 12.0, Weight::Semi) + 48.0) - 78.0;
            for s in ["Pick one. You can change it anytime in Settings.", "Or right-drag the notch to any edge, on any screen."] {
                if a.gfx.measure(s, 11.0, Weight::Regular) > foot_w {
                    tight.push(s);
                }
            }
            if a.gfx.measure("First, how should the notch look when it's closed?", 12.5, Weight::Regular) > ONBOARD_SIZE.0 {
                tight.push("question");
            }
            out += &format!("{:<4} welcome tour text fits {:?}\n", if tight.is_empty() { "OK" } else { "FAIL" }, tight);
            a.queue.clear();
            a.settings.style = before.0;
            a.set_edge(before.1, true);
            a.settings.edge = Edge::Top;
            a.settings.save();
            a.expanded = false;
        }
        // Settings panel: opens from the gear, fits, and its clicks apply and save.
        {
            a.toast = None;
            a.queue.clear();
            a.expanded = true;
            a.tab = a.modules.iter().position(|m| m.has_tab()).unwrap_or(0);
            let tgt = a.target();
            a.cur = tgt;
            a.render(tgt, 0.0);
            let gear = a.hits.iter().find(|h| h.module == APP_HIT && h.id == GEAR).copied();
            if let Some(g) = gear {
                a.on_down(g.rect.cx(), g.rect.cy());
                a.on_up(g.rect.cx(), g.rect.cy());
            }
            out += &format!("{:<4} gear opens the settings panel\n", if gear.is_some() && a.settings_open { "OK" } else { "FAIL" });
            a.switch_t = -10.0;
            out += &snap(a, "settings panel");
            let tiles = a.hits.iter().filter(|h| h.module == SETTINGS && (S_MASCOT..=S_MASCOT + Mascot::ALL.len() as u32).contains(&h.id)).count();
            out += &format!("{:<4} settings shows {tiles} mascot tiles (clock + {})\n", if tiles == Mascot::ALL.len() + 1 { "OK" } else { "FAIL" }, Mascot::ALL.len());
            // Every settings label fits its control (button text needs ~16 DIP padding).
            let right_w = SETTINGS_SIZE.0 - 372.0 - 20.0;
            let btn_w = (right_w - 8.0) / 2.0 - 16.0;
            let mut tight = Vec::new();
            for s in ["Check for updates", "Open data folder", "GitHub page", "Quit WindyLand", "Checking..."] {
                if a.gfx.measure(s, 12.0, Weight::Semi) > btn_w {
                    tight.push(s);
                }
            }
            let sound_w = (372.0 - 5.0 * 6.0) / 6.0 - 6.0;
            for s in Sound::ALL {
                if a.gfx.measure(s.label(), 11.5, Weight::Semi) > sound_w {
                    tight.push(s.label());
                }
            }
            let chip_w = (right_w - 12.0) / 3.0 - 8.0;
            for m in ClipMode::ALL {
                if a.gfx.measure(clip_mode_short(m), 11.5, Weight::Semi) > chip_w {
                    tight.push(clip_mode_short(m));
                }
            }
            if a.gfx.measure("WindyLand 0.0.00  ·  right-click the notch for settings", 10.0, Weight::Regular) > 372.0 {
                tight.push("footer");
            }
            for s in ["Windows banners", "Unmute 12 apps", "No muted apps"] {
                if a.gfx.measure(s, 12.0, Weight::Semi) > btn_w {
                    tight.push(s);
                }
            }
            for s in ["General", "Privacy & alerts"] {
                if a.gfx.measure(s, 11.5, Weight::Semi) > (right_w - 6.0) / 2.0 - 8.0 {
                    tight.push(s);
                }
            }
            for s in ["Start with Windows", "Volume overlay", "Mic & camera dots", "Record what I copy", "Show them in the notch", "Include message text"] {
                if a.gfx.measure(s, 12.5, Weight::Regular) > right_w - 16.0 - 42.0 {
                    tight.push(s);
                }
            }

            let tile_w = (372.0 - 4.0 * 6.0) / 5.0 - 4.0;
            if a.gfx.measure("Sound for reminders (click to hear it)", 11.5, Weight::Semi) > 372.0 {
                tight.push("sound label");
            }
            for m in Mascot::ALL {
                if a.gfx.measure(m.label(), 9.5, Weight::Regular) > tile_w {
                    tight.push(m.label());
                }
            }
            out += &format!("{:<4} settings labels fit their controls {:?}\n", if tight.is_empty() { "OK" } else { "FAIL" }, tight);
            let before = (a.settings.style, a.settings.mascot, a.settings.personality);
            let click = |a: &mut App, id: u32| -> bool {
                let tgt = a.target();
                a.cur = tgt;
                a.render(tgt, 0.0);
                match a.hits.iter().find(|h| h.module == SETTINGS && h.id == id).copied() {
                    Some(h) => {
                        a.on_down(h.rect.cx(), h.rect.cy());
                        a.on_up(h.rect.cx(), h.rect.cy());
                        true
                    }
                    None => false,
                }
            };
            let robot = S_MASCOT + 1 + Mascot::ALL.iter().position(|m| *m == Mascot::Robot).unwrap_or(0) as u32;
            let ok = click(a, robot) && a.settings.mascot == Mascot::Robot && a.settings.personality && crate::store::read("settings.txt").contains("mascot=robot");
            out += &format!("{:<4} picking a mascot applies and saves\n", if ok { "OK" } else { "FAIL" });
            let ok = click(a, S_MASCOT) && !a.settings.personality;
            out += &format!("{:<4} picking Clock turns the mascot off\n", if ok { "OK" } else { "FAIL" });
            let ok = click(a, S_STYLE + 1) && a.settings.style == NotchStyle::Compact && crate::store::read("settings.txt").contains("style=compact");
            out += &format!("{:<4} notch style chip applies and saves\n", if ok { "OK" } else { "FAIL" });
            a.settings.style = before.0;
            a.settings.mascot = before.1;
            a.settings.personality = before.2;
            a.settings.save();
            let ok = click(a, S_VOLUME) && click(a, S_VOLUME);
            out += &format!("{:<4} settings switches respond\n", if ok { "OK" } else { "FAIL" });
            let bell = S_SOUND + Sound::ALL.iter().position(|s| *s == Sound::Bell).unwrap_or(0) as u32;
            let ok = click(a, bell) && a.settings.sound == Sound::Bell && crate::store::read("settings.txt").contains("sound=bell");
            out += &format!("{:<4} picking a sound applies and saves\n", if ok { "OK" } else { "FAIL" });
            // Second page: privacy & alerts, and its switches.
            let ok = click(a, S_PAGE + 1) && a.settings_page == 1;
            out += &snap(a, "settings page 2 (privacy & alerts)");
            let page2 = [S_NOTIF_ON, S_NOTIF_TEXT, S_NOTIF_BANNERS, S_NOTIF_UNMUTE, S_PRIVACY, S_CLIP_ON, S_CLIP_MODE].iter().all(|id| a.hits.iter().any(|h| h.module == SETTINGS && h.id == *id));
            let gone = !a.hits.iter().any(|h| h.module == SETTINGS && h.id == S_QUIT);
            let (n0, t0) = (a.settings.notif_on, a.settings.notif_text);
            let toggled = click(a, S_NOTIF_TEXT) && a.settings.notif_text != t0 && crate::store::read("settings.txt").contains(&format!("notif_text={}", (!t0) as u8));
            click(a, S_NOTIF_TEXT);
            let fits = a.gfx.bbox().map_or(false, |b| (b.3 as f32) < CANVAS_H * a.scale);
            out += &format!(
                "{:<4} settings has a Privacy & alerts page with notification switches (notifications start {})\n",
                if ok && page2 && gone && toggled && fits && !n0 { "OK" } else { "FAIL" },
                if n0 { "ON (should be off)" } else { "off" }
            );
            click(a, S_PAGE);
            a.settings.sound = Sound::Chime;
            a.settings.save();
            a.settings_open = false;
            a.expanded = false;
        }
        // Resize animation: soft spring that settles quickly with a small
        // overshoot, and content that fades in after a tab switch.
        {
            let from = (476.0f32, 150.0f32, 26.0f32);
            let to = (from.0 + 280.0, from.1 + 66.0, from.2);
            a.cur = from;
            a.vel = (0.0, 0.0, 0.0);
            let (mut steps, mut over) = (0u32, 0.0f32);
            while steps < 180 {
                let d = a.step_anim(to, 1.0 / 60.0);
                over = over.max(a.cur.0 - to.0);
                steps += 1;
                if d == 0.0 {
                    break;
                }
            }
            let secs = steps as f32 / 60.0;
            let ok = secs <= 0.6 && over > 0.0 && over < 0.08 * 280.0;
            out += &format!("{:<4} resize spring settles in {secs:.2}s, overshoot {over:.1}px of 280\n", if ok { "OK" } else { "FAIL" });
            // Same result at a choppy 20 fps.
            a.cur = from;
            a.vel = (0.0, 0.0, 0.0);
            let mut s2 = 0u32;
            while s2 < 60 && a.step_anim(to, 0.05) != 0.0 {
                s2 += 1;
            }
            out += &format!("{:<4} resize spring is frame-rate independent ({:.2}s at 20 fps)\n", if (s2 as f32 * 0.05 - secs).abs() < 0.12 { "OK" } else { "FAIL" }, s2 as f32 * 0.05);
            let now = a.t();
            a.switch_t = now;
            let (a0, a1) = (a.switch_alpha(now), a.switch_alpha(now + 0.35));
            out += &format!("{:<4} tab switch fades content in ({a0:.2} -> {a1:.2})\n", if a0 < 0.05 && a1 > 0.99 { "OK" } else { "FAIL" });
            a.switch_t = -10.0;
            // Every tab opens to the same panel (only Music lyrics may widen).
            let mut sizes = Vec::new();
            for i in 0..a.modules.len() {
                if a.modules[i].has_tab() {
                    a.tab = i;
                    a.expanded = true;
                    let t = a.target();
                    sizes.push((a.modules[i].title(), t.0 as i32, t.1 as i32));
                }
            }
            let same = sizes.windows(2).all(|w| (w[0].1, w[0].2) == (w[1].1, w[1].2));
            out += &format!("{:<4} every tab opens to the same panel size {:?}\n", if same { "OK" } else { "FAIL" }, sizes.first().map(|s| (s.1, s.2)));

            // Switching tabs: old content slides out while the new slides in;
            // only the new content is clickable mid-way.
            let tabs: Vec<usize> = (0..a.modules.len()).filter(|&i| a.modules[i].has_tab()).collect();
            let (t0, t1) = (tabs[0], tabs[3.min(tabs.len() - 1)]);
            a.tab = t0;
            a.shown = (t0, false);
            a.settings_open = false;
            let tgt = a.target();
            a.cur = tgt;
            a.render(tgt, 0.0);
            a.tab = t1;
            a.frame();
            let mid = a.t() - 0.12;
            a.switch_t = mid;
            a.render(tgt, 0.0);
            let old_hits = a.hits.iter().filter(|h| h.module == t0).count();
            let new_hits = a.hits.iter().filter(|h| h.module == t1).count();
            let ok = a.prev_shown.0 == t0 && a.switch_dir > 0.0 && old_hits == 0 && new_hits > 0 && a.gfx.last_err.is_none();
            out += &format!("{:<4} tab switch slides old content out, new in (old hits {old_hits}, new {new_hits})\n", if ok { "OK" } else { "FAIL" });
            // The highlight pill glides to the new tab instead of jumping.
            let slot = TAB_H + 4.0;
            let target_off = tabs.iter().position(|&i| i == t1).unwrap_or(0) as f32 * slot;
            a.pill_off = 0.0;
            a.pill_last = a.t() - 0.016;
            a.render(tgt, 0.0);
            let first = a.pill_off;
            for _ in 0..40 {
                a.pill_last = a.t() - 0.016;
                a.render(tgt, 0.0);
            }
            let pill_ok = first > 0.0 && first < target_off && (a.pill_off - target_off).abs() < 0.5;
            out += &format!("{:<4} highlight pill glides to the new tab ({first:.1} -> {:.1} of {target_off:.0})\n", if pill_ok { "OK" } else { "FAIL" }, a.pill_off);
            a.switch_t = -10.0;
            a.expanded = false;
        }
        // Notifications: text rules, colours, muting, Do Not Disturb, and a
        // real Windows notification travelling into the notch.
        {
            use crate::modules::notifs::{self, Note};
            let ni = find(a, "Notifications").unwrap_or(0);
            let n = Note { app: "Messages".into(), title: "Nadia".into(), body: "See you\n at 7".into(), ..Default::default() };
            let ok = notifs::toast_text(&n, true) == ("Nadia".to_string(), "See you at 7".to_string())
                && notifs::toast_text(&n, false) == ("Messages".to_string(), "New notification".to_string())
                && notifs::toast_text(&Note { app: "Mail".into(), ..Default::default() }, true).0 == "Mail";
            out += &format!("{:<4} notification text (and hidden-text mode shows only the app)\n", if ok { "OK" } else { "FAIL" });
            let red: Vec<u8> = (0..64 * 64).flat_map(|_| [20u8, 20, 200, 255]).collect();
            let grey: Vec<u8> = (0..64 * 64).flat_map(|_| [128u8, 128, 128, 255]).collect();
            let c = notifs::icon_color(&red);
            let ok = c.map_or(false, |c| c.r > 0.8 && c.g < 0.5 && c.b < 0.5) && notifs::icon_color(&grey).map_or(false, |c| c.r == col::TEXT.r);
            out += &format!("{:<4} notification colour comes from the app icon\n", if ok { "OK" } else { "FAIL" });
            a.toast = None;
            a.queue.clear();
            a.expanded = false;
            a.hidden = false;
            let was_quiet = notifs::quiet();
            a.call(ni, |m, c| m.action(notifs::A_DEMO, (0.0, 0.0), c));
            a.frame();
            let shown = a.toast.as_ref().map_or(false, |t| t.0.key == "notif" && t.0.title == "Nadia" && t.0.click == Some(notifs::A_OPEN) && t.0.actions.len() == 2);
            if !was_quiet {
                out += &format!("{:<4} a notification shows in the notch with Open / Mute\n", if shown { "OK" } else { "FAIL" });
                out += &snap(a, "notification toast");
                // Mute: the app's next notification is dropped, and Settings counts it.
                a.toast = None;
                a.call(ni, |m, c| m.action(notifs::A_MUTE, (0.0, 0.0), c));
                a.toast = None;
                a.queue.clear();
                a.call(ni, |m, c| m.action(notifs::A_DEMO, (0.0, 0.0), c));
                a.frame();
                let silent = a.toast.is_none() && a.queue.is_empty();
                let counted = notifs::muted(&a.settings.extra) == vec!["WindyLand.Demo".to_string()];
                a.settings.extra.remove(notifs::MUTED_KEY);
                a.settings.save();
                out += &format!("{:<4} muting an app silences it\n", if silent && counted { "OK" } else { "FAIL" });
            } else {
                out += "INFO Do Not Disturb is on: notification display tests skipped\n";
            }
            out += &format!("INFO focus/Do Not Disturb state: {:?} (0 = off)\n", notifs::focus_profile());
            // Live: a silent test notification (straight to Notification
            // Center, no banner, no sound) must reach the notch.
            a.toast = None;
            a.queue.clear();
            let had = a.settings.notif_on;
            a.settings.notif_on = true;
            a.tick();
            let mut ready = false;
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if notifs::ACCESS.load(std::sync::atomic::Ordering::Relaxed) == 1 {
                    ready = true;
                    break;
                }
            }
            // Let the watcher take its first look (it never replays old ones).
            std::thread::sleep(std::time::Duration::from_millis(900));
            let aumid = HSTRING::from(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe");
            let sent = (|| -> windows::core::Result<std::time::Instant> {
                use windows::Data::Xml::Dom::XmlDocument;
                use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
                let xml = XmlDocument::new()?;
                xml.LoadXml(h!("<toast><visual><binding template='ToastGeneric'><text>WindyLand self-test</text><text>This test notification removes itself</text></binding></visual><audio silent='true'/></toast>"))?;
                let toast = ToastNotification::CreateToastNotification(&xml)?;
                toast.SetTag(h!("wl-selftest"))?;
                toast.SetGroup(h!("wl"))?;
                toast.SetSuppressPopup(true)?;
                ToastNotificationManager::CreateToastNotifierWithId(&aumid)?.Show(&toast)?;
                Ok(std::time::Instant::now())
            })();
            let mut got = None;
            if let Ok(t0) = sent {
                while t0.elapsed().as_secs_f64() < 5.0 && got.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    a.tick();
                    let hit = |t: &Toast| t.key == "notif" && t.title == "WindyLand self-test";
                    if a.toast.as_ref().map_or(false, |t| hit(&t.0)) || a.queue.iter().any(|t| hit(t)) {
                        got = Some(t0.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                let _ = windows::UI::Notifications::ToastNotificationManager::History().and_then(|h| h.RemoveGroupedTagWithId(h!("wl-selftest"), h!("wl"), &aumid));
            }
            let quiet = notifs::quiet();
            match (ready, &sent, got) {
                (true, Ok(_), Some(ms)) => out += &format!("OK   a real Windows notification reached the notch in {ms:.0} ms\n"),
                (_, _, None) if quiet => out += "INFO live notification test skipped: Do Not Disturb is on\n",
                (false, _, _) => out += "FAIL notification access: Windows didn't allow the listener\n",
                (_, Err(e), _) => out += &format!("INFO live notification test: couldn't send a test notification ({e})\n"),
                _ => out += "FAIL a real Windows notification never reached the notch (5 s)\n",
            }
            // Three arrive during a full-screen game: nothing shows until the
            // notch is back, then the newest with "+2 more".
            if !quiet {
                a.toast = None;
                a.queue.clear();
                a.hidden = true;
                for _ in 0..3 {
                    a.call(ni, |m, c| m.action(notifs::A_DEMO_INBOX, (0.0, 0.0), c));
                }
                a.tick();
                let held = a.toast.is_none() && a.queue.iter().all(|t| t.key != "notif");
                a.hidden = false;
                a.tick();
                a.frame();
                let sub = a.toast.as_ref().map(|t| t.0.sub.clone()).unwrap_or_default();
                out += &format!("{:<4} notifications during full screen wait, then show as one ({sub})\n", if held && sub.ends_with("(+2 more)") { "OK" } else { "FAIL" });
            }
            a.settings.notif_on = had;
            a.settings.save();
            a.tick();
            a.toast = None;
            a.queue.clear();
        }
        // Updates: version maths, release parsing, and a live GitHub check.
        {
            use crate::modules::update::*;
            let ok = is_newer("v0.1.10", "0.1.9") && !is_newer("0.1.4", "0.1.4") && !is_newer("v0.1.3", "0.1.4") && is_newer("1.0.0", "0.9.9");
            out += &format!("{:<4} update version comparison\n", if ok { "OK" } else { "FAIL" });
            let json = br#"{"tag_name":"v9.9.9","html_url":"https://x","draft":false,"prerelease":false,
                "assets":[{"name":"WindyLandSetup-9.9.9.exe","browser_download_url":"https://github.com/a/b.exe","size":123}]}"#;
            let rel = parse_release(json);
            let ok = rel.as_ref().map_or(false, |r| r.version == "9.9.9" && r.installer.as_ref().map_or(false, |i| i.size == Some(123)));
            out += &format!("{:<4} update release JSON parsing\n", if ok { "OK" } else { "FAIL" });
            let ok = sha256_hex(b"abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
            out += &format!("{:<4} SHA-256 matches the standard test vector\n", if ok { "OK" } else { "FAIL" });
            let html = format!(
                r#"<a href="/{REPO}/releases/download/v9.9.9/WindyLand-src.zip">x</a><clipboard-copy value="sha256:{a}">
                <a href="/{REPO}/releases/download/v9.9.9/WindyLandSetup-9.9.9.exe" rel="nofollow">x</a>
                <clipboard-copy value="sha256:{b}"></clipboard-copy><span>2.3 MB</span>"#,
                a = "1".repeat(64),
                b = "AB".repeat(32)
            );
            let inst = parse_assets_html(&html, "v9.9.9");
            let ok = inst.as_ref().map_or(false, |i| {
                i.url == format!("https://github.com/{REPO}/releases/download/v9.9.9/WindyLandSetup-9.9.9.exe") && i.sha256.as_deref() == Some(&"ab".repeat(32)[..])
            });
            out += &format!("{:<4} release page parsing finds the installer and its SHA-256\n", if ok { "OK" } else { "FAIL" });
            // Live: both routes must agree on the latest release.
            let api = fetch_api();
            let web = fetch_web();
            let desc = |r: &std::result::Result<Release, String>| match r {
                Ok(r) => format!(
                    "{} (installer {}, sha256 {})",
                    r.version,
                    r.installer.as_ref().map_or("missing".into(), |i| i.url.rsplit('/').next().unwrap_or("").to_string()),
                    r.installer.as_ref().and_then(|i| i.sha256.clone()).map_or("none".into(), |h| h[..12].to_string())
                ),
                Err(e) => format!("failed ({e})"),
            };
            out += &format!("INFO GitHub API: {}\n", desc(&api));
            let web_ok = web.as_ref().map_or(false, |r| r.installer.as_ref().map_or(false, |i| i.sha256.is_some()));
            let agree = match (&api, &web) {
                (Ok(a), Ok(w)) => a.version == w.version,
                _ => true,
            };
            out += &format!("{:<4} release pages (no API limit): {}{}\n", if web_ok && agree { "OK" } else { "FAIL" }, desc(&web), if agree { "" } else { " DISAGREES with the API" });
            if let Ok(r) = fetch_latest() {
                out += &format!("INFO latest release {} -> this build {VERSION} is {}\n", r.version, if is_newer(&r.version, VERSION) { "behind (update offered)" } else { "up to date" });
            }
            if std::env::var("WINDYLAND_TEST_DOWNLOAD").is_ok() {
                for (route, r) in [("release pages", web), ("API", api)] {
                    if let Ok(r) = r {
                        match download_installer(&r) {
                            Ok(p) => {
                                let n = std::fs::metadata(&p).map_or(0, |m| m.len());
                                out += &format!("OK   installer download via {route}, {n} bytes, checksum verified\n");
                                let _ = std::fs::remove_file(p);
                            }
                            Err(e) => out += &format!("FAIL installer download via {route}: {e}\n"),
                        }
                    }
                }
                // A tampered file must be refused.
                if let Ok(Some(mut i)) = fetch_web().map(|r| r.installer) {
                    i.sha256 = Some("0".repeat(64));
                    let bad = Release { version: "0.0.0".into(), page: String::new(), installer: Some(i) };
                    let refused = matches!(download_installer(&bad), Err(e) if e.contains("doesn't match"));
                    out += &format!("{:<4} a download that doesn't match its checksum is refused\n", if refused { "OK" } else { "FAIL" });
                }
            }
        }
        for i in 0..a.modules.len() {
            let s = a.modules[i].debug_state();
            if !s.is_empty() {
                out += &format!("INFO {}: {s}\n", a.modules[i].title());
            }
        }
        out
    })
    .unwrap_or_else(|| "app not running".into())
}

// ------------------------------------------------------------- docs renders

/// Write the current frame as `w:u32 h:u32` + premultiplied BGRA rows,
/// cropped to `crop` (inclusive pixel box) or to the drawn bounding box.
fn write_raw(a: &App, path: &std::path::Path, crop: Option<(i32, i32, i32, i32)>) {
    let px = a.gfx.pixels();
    let (pw, ph) = (a.gfx.pw, a.gfx.ph);
    let Some((x0, y0, x1, y1)) = crop.or_else(|| a.gfx.bbox()) else { return };
    let (x1, y1) = (x1.min(pw - 1), y1.min(ph - 1));
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(8 + (w * h * 4) as usize);
    out.extend_from_slice(&(w as u32).to_le_bytes());
    out.extend_from_slice(&(h as u32).to_le_bytes());
    for y in y0..=y1 {
        let s = ((y * pw + x0) * 4) as usize;
        out.extend_from_slice(&px[s..s + (w * 4) as usize]);
    }
    let _ = std::fs::write(path, out);
}

fn shot(a: &mut App, dir: &std::path::Path, name: &str) {
    let tgt = a.target();
    a.cur = tgt;
    // A few frames so smoothed things (lyric scroll) settle.
    for _ in 0..60 {
        a.render(tgt, 0.0);
    }
    write_raw(a, &dir.join(format!("{name}.raw")), None);
}

/// Animate toward the current target, writing one raw frame per step.
fn animate(a: &mut App, dir: &std::path::Path, n: &mut u32, steps: u32, crop: (i32, i32, i32, i32)) {
    for _ in 0..steps {
        let tgt = a.target();
        let k = 1.0 - (-(1.0f32 / 30.0) * 16.0).exp();
        a.cur = (lerp(a.cur.0, tgt.0, k), lerp(a.cur.1, tgt.1, k), lerp(a.cur.2, tgt.2, k));
        let dist = (a.cur.0 - tgt.0).abs() + (a.cur.1 - tgt.1).abs();
        a.render(tgt, dist);
        write_raw(a, &dir.join(format!("anim_{:03}.raw", *n)), Some(crop));
        *n += 1;
    }
}

/// `windyland --render-docs <dir>`: render every view with demo data for the
/// README/user guide. Run with `WINDYLAND_DATA_DIR` pointing at seeded demo data.
pub fn render_docs(dir: &std::path::Path) -> String {
    let _ = std::fs::create_dir_all(dir);
    with_app(|a| {
        let mut log = String::new();
        let scale = 2.0;
        a.scale = scale;
        let _ = a.gfx.resize((CANVAS_W * scale) as i32, (CANVAS_H * scale) as i32);
        a.settings.style = NotchStyle::Full;
        a.settings.edge = Edge::Top;
        a.settings.personality = true;
        a.anchor = CANVAS_W / 2.0;
        a.queue.clear();
        a.toast = None;
        a.expanded = false;
        a.mood = Mood::Clock;
        let music = find(a, "Now Playing").unwrap_or(0);
        let pomo = find(a, "Pomodoro").unwrap_or(0);
        let timer = find(a, "Timer").unwrap_or(0);
        let water = find(a, "Water Reminder").unwrap_or(0);
        let todo = find(a, "TodoList").unwrap_or(0);
        let shelf = find(a, "Shelf").unwrap_or(0);
        let act = |a: &mut App, i: usize, id: u32| a.call(i, |m, c| m.action(id, (0.0, 0.0), c));
        let tick = |a: &mut App, i: usize| a.call(i, |m, c| m.tick(c));
        tick(a, music);

        // Collapsed looks.
        shot(a, dir, "collapsed_full_music");
        for (s, n) in [(NotchStyle::Compact, "collapsed_compact_music"), (NotchStyle::Pill, "collapsed_pill_music")] {
            a.settings.style = s;
            shot(a, dir, n);
        }
        act(a, music, 1); // pause
        tick(a, music);
        shot(a, dir, "collapsed_pill_idle");
        a.settings.style = NotchStyle::Compact;
        shot(a, dir, "collapsed_compact_clock");
        a.settings.style = NotchStyle::Full;
        shot(a, dir, "collapsed_full_clock");
        for (m, n) in [(Mood::Eyes, "mascot_eyes"), (Mood::Doze, "mascot_doze"), (Mood::Wave, "mascot_wave")] {
            a.mood = m;
            a.mood_since = a.t() - 5.0;
            shot(a, dir, n);
        }
        for m in Mascot::ALL {
            a.settings.mascot = m;
            a.mood = Mood::Eyes;
            a.mood_since = a.t() - 5.0;
            shot(a, dir, &format!("mascot_{}", m.key()));
        }
        a.settings.mascot = Mascot::Eyes;
        a.mood = Mood::Clock;
        act(a, music, 1); // play
        tick(a, music);

        // Panels.
        a.expanded = true;
        a.tab = music;
        shot(a, dir, "panel_music");
        act(a, music, crate::modules::music::A_DEMO_LYRICS);
        shot(a, dir, "panel_lyrics");
        act(a, music, 5); // lyrics off
        a.tab = pomo;
        act(a, pomo, crate::modules::pomodoro::A_DEMO);
        shot(a, dir, "panel_pomodoro");
        a.expanded = false;
        shot(a, dir, "collapsed_full_pomodoro");
        act(a, pomo, 2); // reset
        a.expanded = true;
        a.tab = timer;
        act(a, timer, crate::modules::timer::A_DEMO_TIMER);
        shot(a, dir, "panel_timer");
        a.expanded = false;
        shot(a, dir, "collapsed_full_timer");
        a.settings.style = NotchStyle::Pill;
        shot(a, dir, "collapsed_pill_timer");
        a.settings.style = NotchStyle::Full;
        a.expanded = true;
        act(a, timer, crate::modules::timer::A_DEMO_WATCH);
        shot(a, dir, "panel_stopwatch");
        act(a, timer, 1); // countdown view
        act(a, timer, 4); // reset it
        a.tab = water;
        shot(a, dir, "panel_water");
        act(a, water, 2);
        shot(a, dir, "panel_water_settings");
        act(a, water, 3);
        a.tab = todo;
        shot(a, dir, "panel_todo");
        act(a, todo, 1); // add task
        a.call(todo, |m, c| {
            for ch in "Call the dentist".chars() {
                m.chr(ch, c);
            }
            m.action(11, (0.0, 0.0), c); // +30m
        });
        shot(a, dir, "panel_todo_editor");
        act(a, todo, 3); // cancel
        a.keyboard = None;
        a.tab = shelf;
        shot(a, dir, "panel_shelf");
        a.drag_files = true;
        for m in a.modules.iter_mut() {
            m.drag_hover(true);
        }
        shot(a, dir, "panel_shelf_drop");
        a.drag_files = false;
        for m in a.modules.iter_mut() {
            m.drag_hover(false);
        }
        if let Some(clip) = find(a, "Clipboard") {
            a.call(clip, |m, c| m.action(crate::modules::clipboard::A_DEMO_DOCS, (0.0, 0.0), c));
            a.tab = clip;
            shot(a, dir, "panel_clipboard");
        }
        a.open_settings();
        a.settings_page = 0;
        shot(a, dir, "settings_general");
        a.settings_page = 1;
        shot(a, dir, "settings_privacy");
        a.settings_page = 0;
        a.settings_open = false;
        a.start_onboarding();
        shot(a, dir, "tour_style");
        a.onboarding = None;
        a.expanded = true;

        // Toasts.
        a.expanded = false;
        let toasts = [
            ("toast_timer", Toast::new("timer", "\u{E121}", col::PINK, "Time's up!", "10 minute timer finished").action("Restart", 0).action("Dismiss", 0)),
            ("toast_water", Toast::new("water", glyph::DROP, col::CYAN, "Time to Drink", "Stay hydrated! 750 ml to go").action("Drank", 0).action("Snooze", 0)),
            ("toast_todo", Toast::new("todo", glyph::ALARM, col::YELLOW, "Send the weekly update", "Task due now").action("Done", 0).action("Snooze 10m", 0)),
            ("toast_pomodoro", Toast::new("pomodoro", glyph::TIMER, col::GREEN, "Break Time!", "5 minute short break").action("Start", 0).action("Later", 0)),
            ("toast_charging", Toast::new("power", glyph::CHARGING, col::GREEN, "Charging", "Charger connected").right("70%").progress(0.7)),
            ("toast_volume", Toast::new("volume", glyph::VOLUME, col::TEXT, "Volume", "Output").right("45%").progress(0.45)),
            ("toast_caps", Toast::new("caps", glyph::CAPS, col::ACCENT, "Caps Lock", "On")),
            ("toast_bt", Toast::new("bt", glyph::HEADPHONE, col::BLUE, "AirBuds Pro", "Connected")),
            ("toast_lang", Toast::new("lang", glyph::KEYBOARD, col::BLUE, "Bangla (Bangladesh)", "Keyboard language").right("BN")),
            (
                "toast_notification",
                Toast::new("notif", glyph::BELL, col::GREEN, "Nadia", "Are we still on for 7? I'll bring the slides")
                    .image(std::sync::Arc::new(crate::modules::notifs::demo_icon()))
                    .click(1)
                    .action("Open", 0)
                    .action("Mute app", 0),
            ),
            ("toast_mic", Toast::new("privacy", glyph::MIC, crate::modules::privacy::MIC, "Microphone in use", "Recorder")),
        ];
        for (name, t) in toasts {
            let end = a.t() + 100.0;
            a.toast = Some((t, end));
            shot(a, dir, name);
        }
        a.toast = None;

        // Other edges (music live).
        for (e, s, n) in [
            (Edge::Left, NotchStyle::Full, "edge_left_full"),
            (Edge::Right, NotchStyle::Compact, "edge_right_compact"),
            (Edge::Bottom, NotchStyle::Pill, "edge_bottom_pill"),
        ] {
            a.settings.edge = e;
            a.settings.style = s;
            a.anchor = if e.vertical() { CANVAS_H / 2.0 } else { CANVAS_W / 2.0 };
            shot(a, dir, n);
        }
        a.settings.edge = Edge::Left;
        a.settings.style = NotchStyle::Full;
        a.anchor = CANVAS_H / 2.0;
        a.expanded = true;
        a.tab = music;
        shot(a, dir, "edge_left_panel");
        a.expanded = false;
        a.settings.edge = Edge::Top;
        a.anchor = CANVAS_W / 2.0;

        // Hover-to-open animation at 1x for the README GIF.
        a.scale = 1.0;
        let _ = a.gfx.resize(CANVAS_W as i32, CANVAS_H as i32);
        let crop = (0, 0, CANVAS_W as i32 - 1, 259);
        let mut n = 0u32;
        a.tab = music;
        a.expanded = false;
        a.cur = a.target();
        animate(a, dir, &mut n, 1, crop); // hold: collapsed
        a.expanded = true;
        animate(a, dir, &mut n, 18, crop); // open
        animate(a, dir, &mut n, 1, crop); // hold: player
        act(a, music, crate::modules::music::A_DEMO_LYRICS);
        animate(a, dir, &mut n, 18, crop); // split into lyrics
        animate(a, dir, &mut n, 1, crop); // hold: lyrics
        act(a, music, 5);
        a.tab = timer;
        act(a, timer, crate::modules::timer::A_DEMO_TIMER);
        animate(a, dir, &mut n, 18, crop); // switch to timer
        animate(a, dir, &mut n, 1, crop); // hold: timer
        a.expanded = false;
        animate(a, dir, &mut n, 18, crop); // close (timer live)
        animate(a, dir, &mut n, 1, crop); // hold
        log += &format!("rendered to {} ({n} animation frames)\n", dir.display());
        log
    })
    .unwrap_or_else(|| "app not running".into())
}

/// `windyland --render-demo <dir>`: the scripted demo for the README video.
/// Drives the real UI (hover, clicks, tabs, toasts, settings) on a virtual
/// 30 fps clock with a scripted cursor, and writes every frame as a raw
/// 2x canvas plus `demo.log` (cursor position, clicks and scene markers)
/// for `docs/make_demo.py` to composite.
pub fn render_demo(dir: &std::path::Path) -> String {
    let _ = std::fs::create_dir_all(dir);
    with_app(|a| {
        struct D<'a> {
            a: &'a mut App,
            dir: std::path::PathBuf,
            n: u32,
            t: f64,
            cur: (f32, f32),
            inside: bool,
            click: u8,
            log: String,
        }
        impl D<'_> {
            fn step(&mut self) {
                self.t += 1.0 / 30.0;
                self.a.virt = Some(self.t);
                if self.n % 30 == 0 {
                    self.a.tick();
                }
                let (x, y) = self.cur;
                // Layered windows only get the mouse over drawn pixels.
                let over = self.a.hit_at(x, y).is_some();
                if over {
                    self.a.on_mouse_move(x, y);
                } else if self.inside {
                    self.a.on_mouse_leave();
                }
                self.inside = over;
                self.a.frame();
                write_raw(self.a, &self.dir.join(format!("f{:04}.raw", self.n)), Some((0, 0, self.a.gfx.pw - 1, self.a.gfx.ph - 1)));
                self.log += &format!("f {} {:.1} {:.1} {}\n", self.n, x * self.a.scale, y * self.a.scale, self.click);
                self.click = 0;
                self.n += 1;
            }
            fn wait(&mut self, secs: f64) {
                for _ in 0..(secs * 30.0).round() as u32 {
                    self.step();
                }
            }
            fn scene(&mut self, name: &str) {
                self.log += &format!("scene {} {name}\n", self.n);
            }
            /// Caption subtitle change (shown by make_demo.py).
            fn sub(&mut self, text: &str) {
                self.log += &format!("sub {} {text}\n", self.n);
            }
            /// Move to a hit and show a click there without sending it.
            fn point(&mut self, module: usize, id: u32, secs: f64) -> bool {
                let Some((x, y)) = self.hit(module, id) else { return false };
                self.move_to(x, y, secs);
                self.wait(0.12);
                self.click = 1;
                self.step();
                true
            }
            /// Glide the cursor to (x, y) with an ease-in-out curve.
            fn move_to(&mut self, x: f32, y: f32, secs: f64) {
                let (x0, y0) = self.cur;
                let k = (secs * 30.0).round().max(1.0) as u32;
                for i in 1..=k {
                    let p = i as f32 / k as f32;
                    let e = if p < 0.5 { 4.0 * p * p * p } else { 1.0 - (-2.0 * p + 2.0).powi(3) / 2.0 };
                    self.cur = (x0 + (x - x0) * e, y0 + (y - y0) * e);
                    self.step();
                }
            }
            /// Centre of a hit currently on screen.
            fn hit(&self, module: usize, id: u32) -> Option<(f32, f32)> {
                self.a.hits.iter().rev().find(|h| h.module == module && h.id == id).map(|h| (h.rect.cx(), h.rect.cy()))
            }
            fn click_at(&mut self, module: usize, id: u32, secs: f64) {
                let Some((x, y)) = self.hit(module, id) else {
                    self.log += &format!("missing hit {module} {id}\n");
                    return;
                };
                self.move_to(x, y, secs);
                self.wait(0.12);
                self.a.on_down(x, y);
                self.click = 1;
                self.step();
                self.step();
                self.a.on_up(x, y);
                self.step();
            }
        }

        let scale = 2.0;
        a.scale = scale;
        let _ = a.gfx.resize((CANVAS_W * scale) as i32, (CANVAS_H * scale) as i32);
        a.settings.style = NotchStyle::Full;
        a.settings.edge = Edge::Top;
        a.settings.personality = true;
        a.settings.mascot = Mascot::Eyes;
        a.anchor = CANVAS_W / 2.0;
        a.queue.clear();
        a.toast = None;
        a.expanded = false;
        a.onboarding = None;
        a.mood = Mood::Clock;
        sound::mute(true);
        let music = find(a, "Now Playing").unwrap_or(0);
        let pomo = find(a, "Pomodoro").unwrap_or(0);
        let timer = find(a, "Timer").unwrap_or(0);
        let water = find(a, "Water Reminder").unwrap_or(0);
        let todo = find(a, "TodoList").unwrap_or(0);
        let shelf = find(a, "Shelf").unwrap_or(0);
        let clip = find(a, "Clipboard").unwrap_or(0);
        let notes = find(a, "Notifications").unwrap_or(0);
        let privacy = find(a, "Privacy").unwrap_or(usize::MAX);
        a.call(clip, |m, c| m.action(crate::modules::clipboard::A_DEMO_DOCS, (0.0, 0.0), c));
        a.tab = music;
        a.tick();
        a.cur = a.target();
        let mut d = D { a, dir: dir.to_path_buf(), n: 0, t: 1.0, cur: (CANVAS_W * 0.82, CANVAS_H + 40.0), inside: false, click: 0, log: String::new() };

        d.scene("intro");
        d.sub("A Dynamic Island for Windows. Free and open source.");
        d.wait(2.6);

        d.scene("styles");
        for (s, text) in [
            (NotchStyle::Compact, "Compact: a smaller notch"),
            (NotchStyle::Pill, "Pill: just a thin bar"),
            (NotchStyle::Hidden, "Hidden: invisible until you hover"),
            (NotchStyle::Full, "Full: clock, music and a mascot"),
        ] {
            d.wait(0.25);
            d.a.settings.style = s;
            d.sub(text);
            d.wait(1.05);
        }

        d.scene("open");
        d.sub("Music controls, with synced lyrics");
        let notch = d.a.edge_rect(CANVAS_W / 2.0, 20.0);
        d.move_to(notch.cx() + 30.0, notch.y + 14.0, 0.9);
        d.wait(1.0);
        if d.point(music, 5, 0.6) {
            d.a.call(music, |m, c| m.action(crate::modules::music::A_DEMO_LYRICS, (0.0, 0.0), c));
        }
        d.wait(1.8);
        d.click_at(music, 5, 0.4);
        d.wait(0.3);

        d.scene("tabs");
        for (i, text) in [
            (timer, "Timer and stopwatch"),
            (pomo, "Pomodoro focus sessions"),
            (todo, "To-do list with reminders"),
            (water, "Water reminder"),
            (shelf, "A shelf for files you're working on"),
            (clip, "Clipboard history"),
        ] {
            // Start the timers only now, so music is the closed notch's
            // live activity everywhere else in the video.
            if i == timer {
                d.a.call(timer, |m, c| m.action(crate::modules::timer::A_DEMO_TIMER, (0.0, 0.0), c));
            } else if i == pomo {
                d.a.call(pomo, |m, c| m.action(crate::modules::pomodoro::A_DEMO, (0.0, 0.0), c));
            }
            d.click_at(APP_HIT, i as u32, 0.35);
            d.sub(text);
            d.wait(0.95);
        }
        d.a.call(timer, |m, c| m.action(4, (0.0, 0.0), c)); // reset
        d.a.call(pomo, |m, c| m.action(2, (0.0, 0.0), c)); // reset

        d.scene("alerts");
        d.move_to(CANVAS_W * 0.85, CANVAS_H + 60.0, 0.7);
        d.wait(0.4);
        d.sub("Your Windows notifications, in the island");
        d.a.call(notes, |m, c| m.action(crate::modules::notifs::A_DEMO, (0.0, 0.0), c));
        d.wait(2.6);
        d.a.toast = None;
        d.sub("Charging, battery, volume, Wi-Fi and Bluetooth");
        d.a.push_toast(Toast::new("power", glyph::CHARGING, col::GREEN, "Charging", "Charger connected").right("70%").progress(0.7).secs(1.8));
        d.wait(2.2);
        d.sub("A dot while an app uses your mic or camera");
        if privacy != usize::MAX {
            d.a.call(privacy, |m, c| m.action(crate::modules::privacy::A_DEMO, (0.0, 0.0), c));
            d.a.tick();
        }
        d.wait(2.4);

        d.scene("settings");
        d.sub("Styles, positions, mascots and sounds");
        let notch = d.a.edge_rect(CANVAS_W / 2.0, 20.0);
        d.move_to(notch.cx() - 20.0, notch.y + 14.0, 0.6);
        d.log += &format!("rclick {}\n", d.n);
        d.a.open_settings();
        d.wait(0.7);
        for k in [2u32, 5, 8] {
            d.click_at(SETTINGS, S_MASCOT + k, 0.45);
            d.wait(0.5);
        }
        d.click_at(SETTINGS, S_PAGE + 1, 0.5);
        d.sub("Notifications, mic dots and clipboard: all opt-in");
        d.wait(1.6);
        d.click_at(SETTINGS, S_PAGE, 0.4);
        d.wait(0.3);
        if privacy != usize::MAX {
            d.a.call(privacy, |m, c| m.action(crate::modules::privacy::A_DEMO, (0.0, 0.0), c));
        }

        d.scene("outro");
        d.sub("github.com/shabab239/WindyLand");
        d.move_to(CANVAS_W * 0.9, CANVAS_H + 60.0, 0.8);
        d.wait(3.2);
        d.log += &format!("end {}\n", d.n);
        let _ = std::fs::write(dir.join("demo.log"), &d.log);
        format!("{} frames", d.n)
    })
    .unwrap_or_else(|| "app not running".into())
}

/// The idle companion: two eyes that follow the cursor and blink, close when
/// you're away, and smile when you come back.
fn draw_mascot(ui: &mut Ui, area: Rect, kind: Mascot, mood: Mood, t: f64, origin: (i32, i32), scale: f32) {
    // Look toward the cursor (straight ahead in docs renders).
    let mut look = (0.0f32, 0.0f32);
    if !crate::time::demo() {
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_ok() {
                let mx = (p.x - origin.0) as f32 / scale;
                let my = (p.y - origin.1) as f32 / scale;
                look = gaze(area.cx(), area.cy(), (mx, my), 300.0);
            }
        }
    }
    let mood = if mood == Mood::Clock { Mood::Eyes } else { mood };
    mascot::draw(ui, area, kind, mood, t, look);
}

/// Short chip labels for the clipboard capture setting.
fn clip_mode_short(m: ClipMode) -> &'static str {
    match m {
        ClipMode::Text => "Text",
        ClipMode::TextFiles => "Text + files",
        ClipMode::All => "Everything",
    }
}

/// Shell windows never count as full screen (the desktop, taskbar, Start
/// and search all cover the screen or sit on top of it).
const SHELL_CLASSES: [&str; 7] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// A window is full screen when it covers the whole monitor (taskbar area
/// included) without being merely maximized, and isn't part of the shell.
/// A maximized window stops at the taskbar, or is "zoomed" when the taskbar
/// auto-hides, so it never counts.
fn is_fullscreen(win: RECT, mon: RECT, zoomed: bool, class: &str) -> bool {
    if zoomed || SHELL_CLASSES.contains(&class) {
        return false;
    }
    win.left <= mon.left && win.top <= mon.top && win.right >= mon.right && win.bottom >= mon.bottom
}

/// Re-check full screen the moment another window takes the foreground or
/// is minimized/restored (the 1 s tick covers everything else).
extern "system" fn on_foreground(_h: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _e: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    with_app(|a| a.check_fullscreen());
}

fn shift(r: Rect, dx: f32) -> Rect {
    Rect::new(r.x + dx, r.y, r.w, r.h)
}

/// Gaze offset toward `mouse` from (cx, cy); full strength beyond `range` DIPs.
fn gaze(cx: f32, cy: f32, mouse: (f32, f32), range: f32) -> (f32, f32) {
    if mouse.0 < -1e3 {
        return (0.0, 0.0);
    }
    let (dx, dy) = (mouse.0 - cx, mouse.1 - cy);
    let d = (dx * dx + dy * dy).sqrt().max(1.0);
    let k = (d / range).min(1.0);
    (dx / d * 2.5 * k, dy / d * 1.5 * k)
}

fn run_deferred(list: Vec<Deferred>) {
    for d in list {
        match d {
            Deferred::TrayMenu => tray::show_menu(),
            Deferred::Quit => {
                if let Some(h) = with_app(|a| a.hwnd) {
                    unsafe {
                        let _ = DestroyWindow(h);
                    }
                }
            }
            Deferred::DragOut(paths) => {
                with_app(|a| a.dragging_out = true);
                dragdrop::drag_out(&paths);
                with_app(|a| {
                    a.dragging_out = false;
                    a.request_frame();
                });
            }
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        tray::remove(hwnd);
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    let res = with_app(|a| {
        let r = a.handle(msg, wp, lp);
        (r, std::mem::take(&mut a.deferred))
    });
    if let Some((r, deferred)) = res {
        run_deferred(deferred);
        if let Some(r) = r {
            return r;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
