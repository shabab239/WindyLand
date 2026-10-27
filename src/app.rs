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
use crate::store::{Edge, NotchStyle, Settings};
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

const TIMER_TICK: usize = 1;
const TIMER_FRAME: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Collapsed,
    Toast,
    Expanded,
}

#[derive(Clone, Copy, PartialEq)]
enum Mood {
    Clock,
    Eyes,
    Doze,
    Wave,
}

pub enum Deferred {
    TrayMenu,
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
    start: Instant,
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
                start: Instant::now(),
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
                deferred: Vec::new(),
            };
            app.place();
            if visible {
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
        self.start.elapsed().as_secs_f64()
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

    fn push_toast(&mut self, t: Toast) {
        if t.chime {
            sound::chime();
        }
        let now = self.t();
        if let Some((cur, end)) = &mut self.toast {
            if cur.key == t.key {
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

    fn check_fullscreen(&mut self) {
        let busy = unsafe {
            matches!(
                SHQueryUserNotificationState(),
                Ok(QUNS_BUSY) | Ok(QUNS_RUNNING_D3D_FULL_SCREEN) | Ok(QUNS_PRESENTATION_MODE)
            )
        };
        if busy != self.hidden {
            self.hidden = busy;
            unsafe {
                let _ = ShowWindow(self.hwnd, if busy { SW_HIDE } else { SW_SHOWNOACTIVATE });
                if !busy {
                    let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
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
        self.set_frame_timer(15);
    }

    fn set_frame_timer(&mut self, ms: u32) {
        if self.frame_interval != ms {
            self.frame_interval = ms;
            unsafe {
                if ms == 0 {
                    let _ = KillTimer(self.hwnd, TIMER_FRAME);
                } else {
                    SetTimer(self.hwnd, TIMER_FRAME, ms, None);
                }
            }
        }
    }

    fn mode(&self) -> Mode {
        if self.expanded || self.keyboard.is_some() || self.drag_files {
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
                let (w, h) = self.modules[self.tab].size();
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
        if self.hover && !self.expanded && !self.dragging && t - self.hover_since >= self.settings.style.hover_delay() {
            self.open_from_hover();
        }
        // Collapse after the mouse has left for a moment.
        if let Some(at) = self.leave_at {
            if t >= at && !self.hover && self.keyboard.is_none() && !self.drag_files {
                self.expanded = false;
                self.leave_at = None;
            }
        }
        // Toast lifecycle (paused while the panel is open).
        if self.mode() != Mode::Expanded {
            if let Some((_, end)) = &self.toast {
                if t >= *end && !self.hover {
                    self.toast = None;
                }
            }
            if self.toast.is_none() {
                if let Some(next) = self.queue.pop_front() {
                    let end = t + next.secs as f64;
                    self.toast = Some((next, end));
                }
            }
        }

        let target = self.target();
        let k = 1.0 - (-dt * 16.0).exp();
        self.cur = (lerp(self.cur.0, target.0, k), lerp(self.cur.1, target.1, k), lerp(self.cur.2, target.2, k));
        let dist = (self.cur.0 - target.0).abs() + (self.cur.1 - target.1).abs();
        if dist < 0.3 {
            self.cur = target;
        }

        self.render(target, dist);

        let ambient = self.modules.iter().any(|m| m.animating())
            || matches!(self.mood, Mood::Eyes | Mood::Doze | Mood::Wave)
            || self.toast.is_some()
            || self.leave_at.is_some()
            || self.hover;
        if dist > 0.0 {
            self.set_frame_timer(15);
        } else if ambient {
            self.set_frame_timer(if self.hover || self.toast.is_some() { 33 } else { 60 });
        } else {
            self.set_frame_timer(0);
        }
    }

    fn render(&mut self, target: (f32, f32, f32), dist: f32) {
        let t = self.t();
        let (along, depth, rad) = self.cur;
        let notch = self.edge_rect(along, depth);
        let goal = self.edge_rect(target.0, target.1);
        let edge = self.settings.edge;
        let content_alpha = (1.0 - dist / 50.0).clamp(0.0, 1.0);
        let mode = self.mode();

        self.hits.clear();
        self.gfx.begin(self.scale);
        let ear = (depth / 4.0).min(9.0);
        let barless = matches!(self.settings.style, NotchStyle::Pill | NotchStyle::Hidden);
        if depth >= 0.5 {
            if let Some(geo) = self.gfx.notch_geometry(notch, rad, ear, edge) {
                self.gfx.fill_geometry(&geo, col::NOTCH);
                self.gfx.push_layer(Some(&geo), content_alpha);
                match mode {
                    Mode::Expanded => self.draw_expanded(goal, t),
                    Mode::Toast => self.draw_toast(goal, t),
                    Mode::Collapsed if !barless => self.draw_collapsed(goal, t),
                    Mode::Collapsed => {}
                }
                self.gfx.pop_layer();
                // Keep the whole silhouette clickable even while content is fading.
                self.hits.insert(0, Hit { rect: notch, module: APP_HIT, id: u32::MAX });
            }
        }
        if barless {
            self.draw_bar(along, depth);
        }
        self.gfx.end_and_present(self.hwnd, self.origin.0, self.origin.1);
    }

    fn draw_expanded(&mut self, goal: Rect, t: f64) {
        let mut r = self.content_rect(goal, 6.0, PAD, PAD);
        let mut bar = r.cut_top(TAB_H, 8.0);
        let now = Now::get();
        let mouse = self.mouse;
        {
            let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: APP_HIT, mouse, t };
            // Tabs
            for (i, m) in self.modules.iter().enumerate() {
                if !m.has_tab() {
                    continue;
                }
                let tr = bar.cut_left(TAB_H, 4.0);
                let sel = i == self.tab;
                let bg = if sel { Some(col::TEXT.with_a(0.16)) } else { None };
                let fg = if sel { col::TEXT } else { col::DIM };
                ui.icon_btn(tr, m.icon(), i as u32, 14.0, fg, bg);
            }
            // Title of the current tab + date/time on the right.
            let time = now.hhmm();
            let tw = ui.measure(&time, 14.0, Weight::Semi);
            let tr = bar.cut_right(tw + 2.0, 8.0);
            ui.text(&time, tr, 14.0, col::TEXT, Weight::Semi, Align::Right);
            let dr = bar.cut_right(90.0, 0.0);
            ui.text(&now.short_date(), dr, 12.0, col::DIM, Weight::Regular, Align::Right);
            let title = self.modules[self.tab].title();
            bar.x += 6.0;
            ui.text(title, bar, 13.0, col::TEXT, Weight::Semi, Align::Left);
        }
        let i = self.tab;
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: i, mouse, t };
        self.modules[i].draw(&mut ui, r);
    }

    fn draw_toast(&mut self, goal: Rect, t: f64) {
        let Some((toast, _)) = self.toast.clone() else { return };
        let mouse = self.mouse;
        let mut r = self.content_rect(goal, 8.0, 10.0, 18.0);
        let mut ui = Ui { g: &mut self.gfx, hits: &mut self.hits, module: TOAST, mouse, t };
        let mut top = r.cut_top(44.0, 6.0);
        let ic = top.cut_left(36.0, 12.0);
        ui.circle(ic.cx(), ic.cy(), 18.0, toast.color.with_a(0.18));
        ui.icon(toast.icon, ic, 16.0, toast.color);
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
        self.hits.insert(0, Hit { rect: zone, module: APP_HIT, id: u32::MAX });
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
                draw_mascot(&mut ui, area, m, t, self.origin, self.scale);
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
            if !self.expanded && !self.dragging && self.settings.style.hover_delay() <= 0.0 {
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

    fn open_from_hover(&mut self) {
        self.expanded = true;
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
        self.last_leave = self.t();
        self.mouse = (-1e4, -1e4);
        self.press = None;
        self.leave_at = Some(self.t() + 0.35);
        self.request_frame();
    }

    fn on_down(&mut self, x: f32, y: f32) {
        self.press = self.hit_at(x, y).map(|h| (h, (x, y)));
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
                } else {
                    self.tab = hit.id as usize;
                    if let Some(k) = self.keyboard {
                        self.modules[k].blur();
                        self.release_keyboard();
                    }
                }
            }
            TOAST => {
                if let Some((toast, _)) = self.toast.take() {
                    if let Some((_, id)) = toast.actions.get(hit.id as usize) {
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
                    TIMER_FRAME => self.frame(),
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
                    self.deferred.push(Deferred::TrayMenu);
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
                if ev == WM_LBUTTONUP || ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
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
        // Updates: version maths, release parsing, and a live GitHub check.
        {
            use crate::modules::update::*;
            let ok = is_newer("v0.1.10", "0.1.9") && !is_newer("0.1.4", "0.1.4") && !is_newer("v0.1.3", "0.1.4") && is_newer("1.0.0", "0.9.9");
            out += &format!("{:<4} update version comparison\n", if ok { "OK" } else { "FAIL" });
            let json = br#"{"tag_name":"v9.9.9","html_url":"https://x","draft":false,"prerelease":false,
                "assets":[{"name":"WindyLandSetup-9.9.9.exe","browser_download_url":"https://github.com/a/b.exe","size":123}]}"#;
            let rel = parse_release(json);
            let ok = rel.as_ref().map_or(false, |r| r.version == "9.9.9" && r.installer.as_ref().map_or(false, |i| i.1 == 123));
            out += &format!("{:<4} update release JSON parsing\n", if ok { "OK" } else { "FAIL" });
            match fetch_latest() {
                Ok(r) => {
                    out += &format!(
                        "INFO latest GitHub release: {} (installer {}), this build {} -> {}\n",
                        r.version,
                        r.installer.as_ref().map_or("missing".to_string(), |i| format!("{} bytes", i.1)),
                        VERSION,
                        if is_newer(&r.version, VERSION) { "update offered" } else { "up to date" }
                    );
                    if std::env::var("WINDYLAND_TEST_DOWNLOAD").is_ok() {
                        match download_installer(&r) {
                            Ok(p) => {
                                out += &format!("OK   installer download from GitHub ({})\n", p.display());
                                let _ = std::fs::remove_file(p);
                            }
                            Err(e) => out += &format!("FAIL installer download from GitHub: {e}\n"),
                        }
                    }
                }
                Err(e) => out += &format!("INFO latest GitHub release: check failed ({e})\n"),
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

/// The idle companion: two eyes that follow the cursor and blink, close when
/// you're away, and smile when you come back.
fn draw_mascot(ui: &mut Ui, area: Rect, mood: Mood, t: f64, origin: (i32, i32), scale: f32) {
    let (cx, cy) = (area.cx(), area.cy());
    let gap = 13.0;
    let white = col::TEXT;
    // Look toward the cursor (straight ahead in docs renders).
    let mut look = (0.0f32, 0.0f32);
    if !crate::time::demo() {
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_ok() {
                let mx = (p.x - origin.0) as f32 / scale;
                let my = (p.y - origin.1) as f32 / scale;
                let dx = mx - cx;
                let dy = my - cy;
                let d = (dx * dx + dy * dy).sqrt().max(1.0);
                let k = (d / 300.0).min(1.0);
                look = (dx / d * 2.5 * k, dy / d * 1.5 * k);
            }
        }
    }
    match mood {
        Mood::Eyes | Mood::Clock => {
            // Blink for ~120 ms every ~4 s.
            let phase = (t % 4.2) as f32;
            let open = if phase < 0.12 { (phase / 0.06 - 1.0).abs() } else { 1.0 };
            for s in [-1.0f32, 1.0] {
                let ex = cx + s * gap + look.0;
                let ey = cy + look.1;
                ui.ellipse(ex, ey, 4.2, (6.0 * open).max(0.8), white);
            }
        }
        Mood::Doze => {
            for s in [-1.0f32, 1.0] {
                let ex = cx + s * gap;
                ui.line(ex - 4.5, cy + 1.0, ex + 4.5, cy + 1.0, white.with_a(0.8), 2.0);
            }
            let z = ((t * 0.6) % 1.0) as f32;
            let zr = Rect::new(cx + gap + 8.0 + z * 6.0, cy - 10.0 - z * 6.0, 14.0, 14.0);
            let a = ui.alpha;
            ui.alpha = a * (1.0 - z);
            ui.text("z", zr, 10.0, col::DIM, Weight::Semi, Align::Left);
            ui.alpha = a;
        }
        Mood::Wave => {
            // Happy ^ ^ eyes and a little waving hand.
            for s in [-1.0f32, 1.0] {
                let ex = cx + s * gap;
                ui.arc(ex, cy + 3.0, 4.5, -1.2, 2.4, white, 2.0);
            }
            let wob = ((t * 9.0).sin() * 0.35) as f32;
            let hx = cx + gap + 20.0;
            let hy = cy + 2.0;
            ui.circle(hx, hy, 4.0, col::YELLOW);
            for k in 0..3 {
                let a = wob - 0.5 + k as f32 * 0.5;
                ui.line(hx, hy, hx + a.sin() * 7.0, hy - a.cos() * 7.0, col::YELLOW, 1.8);
            }
        }
    }
}

fn run_deferred(list: Vec<Deferred>) {
    for d in list {
        match d {
            Deferred::TrayMenu => tray::show_menu(),
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
