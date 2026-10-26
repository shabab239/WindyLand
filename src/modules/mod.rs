//! Feature modules.
//!
//! Every feature of the notch is a `Module`. To add your own feature:
//!   1. create `src/modules/my_thing.rs` with a struct implementing `Module`
//!      (copy `example.rs` as a starting point),
//!   2. add `pub mod my_thing;` below,
//!   3. push it in `all()`; its position is its tab order.
//! That's it: the app handles hover/expand, tabs, clicks, toasts and redraws.

use windows::Win32::Foundation::HWND;

use crate::gfx::{Color, Rect};
use crate::store::Settings;
use crate::time::Now;
use crate::ui::Ui;

pub mod example;
pub mod music;
pub mod pomodoro;
pub mod shelf;
pub mod system;
pub mod timer;
pub mod todo;
pub mod water;

/// Every module, in tab order. Modules with `has_tab() == false` run in the
/// background (e.g. `system`, which only pops toasts).
pub fn all(hwnd: HWND) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(music::Music::new(hwnd)),
        Box::new(pomodoro::Pomodoro::new()),
        Box::new(timer::Timer::new()),
        Box::new(water::Water::load()),
        Box::new(todo::Todo::load()),
        Box::new(shelf::Shelf::load()),
        Box::new(system::System::new(hwnd)),
        // Box::new(example::Example::default()),
    ]
}

/// A short notification shown by expanding the notch for a few seconds.
#[derive(Clone, Debug)]
pub struct Toast {
    /// Toasts with the same key replace each other (e.g. repeated volume changes).
    pub key: &'static str,
    pub icon: &'static str,
    pub color: Color,
    pub title: String,
    pub sub: String,
    /// Text on the right edge, e.g. "70%".
    pub right: String,
    /// Optional bar under the text (0..1), e.g. volume level.
    pub progress: Option<f32>,
    /// Buttons: (label, action id sent to the owning module).
    pub actions: Vec<(String, u32)>,
    pub secs: f32,
    pub chime: bool,
    /// Filled in by `Ctx::toast`.
    pub module: usize,
}

impl Toast {
    pub fn new(key: &'static str, icon: &'static str, color: Color, title: impl Into<String>, sub: impl Into<String>) -> Toast {
        Toast {
            key,
            icon,
            color,
            title: title.into(),
            sub: sub.into(),
            right: String::new(),
            progress: None,
            actions: Vec::new(),
            secs: 3.0,
            chime: false,
            module: 0,
        }
    }
    pub fn right(mut self, s: impl Into<String>) -> Self {
        self.right = s.into();
        self
    }
    pub fn progress(mut self, p: f32) -> Self {
        self.progress = Some(p);
        self
    }
    pub fn action(mut self, label: impl Into<String>, id: u32) -> Self {
        self.actions.push((label.into(), id));
        self
    }
    pub fn secs(mut self, s: f32) -> Self {
        self.secs = s;
        self
    }
    pub fn chime(mut self) -> Self {
        self.chime = true;
        self
    }
}

/// What a module can ask of the app while handling a call.
pub struct Ctx<'a> {
    pub now: Now,
    pub hwnd: HWND,
    pub settings: &'a mut Settings,
    pub(crate) module: usize,
    pub(crate) toasts: &'a mut Vec<Toast>,
    pub(crate) open_tab: &'a mut Option<usize>,
    pub(crate) keyboard: &'a mut Option<bool>,
}

impl<'a> Ctx<'a> {
    pub fn toast(&mut self, mut t: Toast) {
        t.module = self.module;
        self.toasts.push(t);
    }
    /// Expand the notch and switch to this module's tab.
    pub fn open_my_tab(&mut self) {
        *self.open_tab = Some(self.module);
    }
    /// Ask for (true) or release (false) keyboard focus, for text input.
    pub fn want_keyboard(&mut self, on: bool) {
        *self.keyboard = Some(on);
    }
}

/// Relative click position inside the clicked hit rect (0..1, 0..1).
pub type Rel = (f32, f32);

#[allow(unused_variables)]
pub trait Module {
    /// Tab tooltip / title.
    fn title(&self) -> &'static str {
        ""
    }
    /// Tab icon glyph (see `gfx::glyph`).
    fn icon(&self) -> &'static str {
        ""
    }
    fn has_tab(&self) -> bool {
        true
    }
    /// Content size (DIPs) of this module's expanded panel, below the tab bar.
    fn size(&self) -> (f32, f32) {
        (440.0, 150.0)
    }
    /// Called once a second, and right after background threads wake the app.
    fn tick(&mut self, ctx: &mut Ctx) {}
    /// Draw the expanded panel into `r`.
    fn draw(&mut self, ui: &mut Ui, r: Rect) {}
    /// A registered hit was clicked.
    fn action(&mut self, id: u32, rel: Rel, ctx: &mut Ctx) {}
    /// Mouse wheel over the panel (positive = up).
    fn scroll(&mut self, delta: f32) {}
    /// Key press while this module holds keyboard focus. Return true if handled.
    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        false
    }
    /// Typed character while this module holds keyboard focus.
    fn chr(&mut self, c: char, ctx: &mut Ctx) -> bool {
        false
    }
    /// Lost keyboard focus (user clicked elsewhere).
    fn blur(&mut self) {}
    /// Priority of this module's "live activity" in the collapsed notch
    /// (highest wins), or None when it has nothing to show.
    fn live(&self) -> Option<u8> {
        None
    }
    /// Tint of the Pill-style bar while this module is live.
    fn live_color(&self) -> Color {
        crate::gfx::col::ACCENT
    }
    /// Draw the live activity into the two wings of the collapsed notch.
    /// Wings are about 28 px tall in Full style and 18 px in Compact.
    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {}
    /// True while the module needs continuous frames (animations).
    fn animating(&self) -> bool {
        false
    }
    /// True if dragging files over the notch should open this module's tab.
    fn wants_drops(&self) -> bool {
        false
    }
    /// Files dropped onto the notch. Return true if this module took them.
    fn drop_files(&mut self, paths: Vec<String>, ctx: &mut Ctx) -> bool {
        false
    }
    /// The user is dragging the hit `id` out of the notch: return paths to drag.
    fn drag_out(&mut self, id: u32) -> Option<Vec<String>> {
        None
    }
    /// Set while files are being dragged over the notch.
    fn drag_hover(&mut self, on: bool) {}
    /// One-line state summary printed by `windyland --selftest`.
    fn debug_state(&self) -> String {
        String::new()
    }
}
