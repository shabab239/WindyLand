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

pub mod agents;
pub mod calendar;
pub mod clipboard;
pub mod example;
pub mod monitor;
pub mod music;
pub mod notifs;
pub mod pomodoro;
pub mod prayer;
pub mod privacy;
pub mod shelf;
pub mod snip;
pub mod system;
pub mod timer;
pub mod tools;
pub mod todo;
pub mod update;
pub mod water;
pub mod widgets;

/// Every tab opens to this one panel size (content area, DIPs), so the tab
/// bar and buttons stay put while switching. A module's `size()` is its
/// natural content size, centred inside the panel; return the full panel
/// height for lists that should fill it. Only a deliberately wider view
/// (Music with lyrics) may exceed the panel width.
pub const PANEL: (f32, f32) = (520.0, 176.0);

/// Every module, in tab order. Modules with `has_tab() == false` run in the
/// background (e.g. `system`, which only pops toasts).
pub fn all(hwnd: HWND) -> Vec<Box<dyn Module>> {
    vec![
        Box::new(music::Music::new(hwnd)),
        Box::new(pomodoro::Pomodoro::new()),
        Box::new(timer::Timer::new()),
        Box::new(water::Water::load()),
        Box::new(todo::Todo::load()),
        Box::new(calendar::Calendar::new()),
        Box::new(shelf::Shelf::load()),
        Box::new(clipboard::Clipboard::new(hwnd)),
        Box::new(prayer::Prayer::load()),
        Box::new(tools::Tools::new(hwnd)),
        Box::new(agents::Agents::new()),
        Box::new(monitor::Monitor::new()),
        Box::new(widgets::Widgets::new()),
        Box::new(system::System::new(hwnd)),
        Box::new(privacy::Privacy::new()),
        Box::new(notifs::Notifs::new(hwnd)),
        Box::new(update::Updater::new(hwnd)),
        // Box::new(example::Example::default()),
    ]
}

/// One widget value for the closed notch.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// Unique and stable (saved in settings), e.g. "battery".
    pub id: &'static str,
    /// Name in the Widgets tab, e.g. "Battery".
    pub name: &'static str,
    pub icon: &'static str,
    pub color: Color,
    /// Short value for the notch: "82%", "4:45", "3".
    pub value: String,
    /// An even shorter form for a tight side: "11" for "11 Nov".
    pub short: String,
    /// Word beside the icon when it spans both sides: "Charging".
    pub label: String,
    /// A progress ring (0..1) drawn instead of the icon.
    pub ring: Option<f32>,
    /// Longer line for the Widgets tab: "2 h 10 min left".
    pub detail: String,
    /// Nothing worth showing now ("When it matters" skips it).
    pub quiet: bool,
}

impl Widget {
    pub fn new(id: &'static str, name: &'static str, icon: &'static str, color: Color, value: impl Into<String>) -> Widget {
        let value = value.into();
        Widget { id, name, icon, color, label: name.to_string(), detail: value.clone(), short: value.clone(), value, ring: None, quiet: false }
    }
    pub fn label(mut self, s: impl Into<String>) -> Self {
        self.label = s.into();
        self
    }
    pub fn short(mut self, s: impl Into<String>) -> Self {
        self.short = s.into();
        self
    }
    pub fn ring(mut self, f: f32) -> Self {
        self.ring = Some(f.clamp(0.0, 1.0));
        self
    }
    pub fn detail(mut self, s: impl Into<String>) -> Self {
        self.detail = s.into();
        self
    }
    pub fn quiet(mut self, q: bool) -> Self {
        self.quiet = q;
        self
    }
}

/// Something that can go in the closed notch's list: a live activity or a
/// widget, as the Widgets tab lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    pub icon: &'static str,
    pub color: Color,
    pub ring: Option<f32>,
    /// "60%", "Shows while music plays".
    pub detail: String,
    pub live: bool,
    /// 0 live activities, 1 system, 2 from your panels (Show page groups).
    pub group: u8,
}

/// Lay out a lead (icon, ring, art) and a text on one side of the clock:
/// the lead faces outward, the text toward the clock; on the right side
/// both hug the right edge. Returns (lead rect, text rect).
pub fn side_rects(r: Rect, right: bool, lead_w: f32, text_w: f32, gap: f32) -> (Rect, Rect) {
    let tw = text_w.min((r.w - lead_w - gap).max(0.0));
    let x0 = if right { r.r() - lead_w - gap - tw } else { r.x };
    (Rect::new(x0, r.y, lead_w, r.h), Rect::new(x0 + lead_w + gap, r.y, tw + 1.0, r.h))
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
    /// Picture shown instead of the icon glyph (premultiplied BGRA, square),
    /// e.g. the app icon of a Windows notification.
    pub image: Option<std::sync::Arc<(u32, u32, Vec<u8>)>>,
    /// Action sent when the toast itself (not a button) is clicked.
    pub click: Option<u32>,
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
            image: None,
            click: None,
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
    pub fn image(mut self, img: std::sync::Arc<(u32, u32, Vec<u8>)>) -> Self {
        self.image = Some(img);
        self
    }
    pub fn click(mut self, id: u32) -> Self {
        self.click = Some(id);
        self
    }
}

/// What a module can ask of the app while handling a call.
pub struct Ctx<'a> {
    pub now: Now,
    pub hwnd: HWND,
    /// The notch is hidden (a full-screen app is in front).
    pub hidden: bool,
    pub settings: &'a mut Settings,
    pub(crate) module: usize,
    pub(crate) toasts: &'a mut Vec<Toast>,
    pub(crate) open_tab: &'a mut Option<usize>,
    pub(crate) keyboard: &'a mut Option<bool>,
    pub(crate) close: &'a mut bool,
}

impl<'a> Ctx<'a> {
    pub fn toast(&mut self, mut t: Toast) {
        t.module = self.module;
        self.toasts.push(t);
    }
    /// Close the open panel (e.g. before taking a screenshot).
    pub fn close_panel(&mut self) {
        *self.close = true;
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
    /// Natural content size (DIPs) below the tab bar; centred in `PANEL`.
    fn size(&self) -> (f32, f32) {
        (440.0, 150.0)
    }
    /// Smallest panel content this view needs in the Slim style, which
    /// otherwise trims the panel. Views too dense to shrink return PANEL.
    fn slim_min(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    /// Colour of this tab's highlight glow when you switch to it.
    fn accent(&self) -> Color {
        crate::gfx::col::ACCENT
    }
    /// A privacy dot to show on the notch while something is active
    /// (e.g. orange while the microphone is in use), or None.
    fn indicator(&self) -> Vec<Color> {
        Vec::new()
    }
    /// Called once a second, and right after background threads wake the app.
    fn tick(&mut self, ctx: &mut Ctx) {}
    /// Draw the expanded panel into `r`.
    fn draw(&mut self, ui: &mut Ui, r: Rect) {}
    /// A registered hit was clicked.
    fn action(&mut self, id: u32, rel: Rel, ctx: &mut Ctx) {}
    /// Mouse wheel over the panel (positive = up). Return true if the module
    /// used it (a list that scrolls); otherwise the wheel switches panels.
    fn scroll(&mut self, delta: f32) -> bool {
        false
    }
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
    /// Tooltip for a hit registered without one (by id): what clicking it
    /// does. Every hit in every panel should have one (self-tested).
    fn tip(&self, id: u32) -> &'static str {
        ""
    }
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
    /// Wings are about 28 px tall in Full style and 18 px in Compact; in
    /// Slim they are only about 64 px wide (the clock sits between them),
    /// so measure and shorten or drop a label rather than overflow.
    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {}
    /// Id of this module's live activity in the closed notch's priority
    /// list (Widgets tab), e.g. "music". Empty: no live activity.
    fn live_id(&self) -> &'static str {
        ""
    }
    /// Draw the live activity on one side of the clock only (`right`: the
    /// right side, so align to its right edge). `r` is about 64 px wide in
    /// Slim and 92 in Full.
    fn draw_live_side(&mut self, ui: &mut Ui, r: Rect, right: bool) {}
    /// Small values this module offers for the closed notch. Called about
    /// three times a second while the notch is closed, so read what you
    /// already have.
    fn widgets(&mut self) -> Vec<Widget> {
        Vec::new()
    }
    /// The Widgets tab gets everything that can go on the notch before it
    /// draws.
    fn set_widget_catalog(&mut self, items: Vec<CatalogItem>, settings: &crate::store::Settings, showing: Vec<String>) {}
    /// A press on hit `id` started to move: true starts a drag inside the
    /// panel (then `drag_to` while it moves and `drag_drop` on release).
    fn drag_begin(&mut self, id: u32, x: f32, y: f32) -> bool {
        false
    }
    fn drag_to(&mut self, x: f32, y: f32) {}
    fn drag_drop(&mut self, x: f32, y: f32, ctx: &mut Ctx) {}
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
