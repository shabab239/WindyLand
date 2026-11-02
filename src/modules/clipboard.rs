//! Clipboard history: what you copy is kept in a list; click an entry to
//! paste it into the app you were using (the copy icon only copies it).
//! Off until the user turns it on (Settings or the
//! tab), and records text only unless they choose Text + files or Everything
//! (adds images). Pinned text/files are saved and survive restarts;
//! everything else, and every image, lives in memory only. Anything a
//! password manager marks as private is never recorded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::WM_APP_WAKE;
use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel};
use crate::store::{self, esc, unesc, ClipMode};
use crate::time::unix_now;
use crate::ui::Ui;

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
/// Images kept (newest first), and their total size cap.
const MAX_IMAGES: usize = 10;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Thumbnail size (pixels) drawn in a row.
const THUMB: (u32, u32) = (80, 48);
/// Unpinned entries kept (oldest dropped first).
const MAX_ITEMS: usize = 30;
/// Longest text kept per entry.
const MAX_CHARS: usize = 20_000;
const ROW_H: f32 = 30.0;

#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Files(Vec<String>),
    Image(Arc<Image>),
}

/// A copied picture: the raw CF_DIB bytes (to copy it back) and a small
/// premultiplied BGRA thumbnail.
#[derive(Debug)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub dib: Vec<u8>,
    pub hash: u64,
    pub thumb: (u32, u32, Vec<u8>),
}

impl PartialEq for Content {
    fn eq(&self, o: &Content) -> bool {
        match (self, o) {
            (Content::Text(a), Content::Text(b)) => a == b,
            (Content::Files(a), Content::Files(b)) => a == b,
            (Content::Image(a), Content::Image(b)) => a.hash == b.hash && a.dib.len() == b.dib.len(),
            _ => false,
        }
    }
}

fn mode_byte(m: ClipMode) -> u8 {
    match m {
        ClipMode::Text => 0,
        ClipMode::TextFiles => 1,
        ClipMode::All => 2,
    }
}

#[derive(Clone)]
struct Entry {
    id: u32,
    content: Content,
    at: f64,
    pinned: bool,
}

const A_CLEAR: u32 = 1;
/// Turn recording on/off (same as the Settings switch).
pub const A_TOGGLE: u32 = 2;
pub const A_SEARCH: u32 = 3;
pub const A_SNIPPET: u32 = 4;
const A_FIELD: u32 = 5;
/// Self-test / docs: add 35 numbered demo entries.
pub const A_DEMO: u32 = 990;
/// Docs/demo video: a few realistic entries.
pub const A_DEMO_DOCS: u32 = 991;
const K_COPY: u32 = 1;
const K_PIN: u32 = 2;
const K_DELETE: u32 = 3;
const K_PASTE: u32 = 4;

fn eid(kind: u32, id: u32) -> u32 {
    (kind << 24) | (id & 0xFF_FFFF)
}

pub struct Clipboard {
    items: Vec<Entry>,
    next_id: u32,
    incoming: Arc<Mutex<Vec<Content>>>,
    on: Arc<AtomicBool>,
    mode: Arc<AtomicU8>,
    /// Image thumbnails as GPU bitmaps: entry id -> (render-target generation, bitmap).
    thumbs: HashMap<u32, (u32, ID2D1Bitmap)>,
    scroll: f32,
    max_scroll: f32,
    /// Entry id just copied back or pasted, when, and the label to show.
    copied: Option<(u32, f64, &'static str)>,
    /// The app you were using before reaching for the notch (paste goes there).
    target: isize,
    own: isize,
    pub last_paste: Option<Paste>,
    /// Search box text (Some while searching).
    search: Option<String>,
    /// A new snippet being typed.
    snippet: Option<String>,
}

impl Clipboard {
    /// Keep track of the app in front (not WindyLand), where a paste goes.
    fn note_target(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        if paste_target_ok(fg, HWND(self.own as *mut _)) {
            self.target = fg.0 as isize;
        }
    }

    fn paste_target(&mut self) -> HWND {
        let t = TEST_TARGET.load(Ordering::Relaxed);
        if t != 0 {
            return HWND(t as *mut _);
        }
        self.note_target();
        HWND(self.target as *mut _)
    }

    pub fn new(hwnd: HWND) -> Clipboard {
        let incoming = Arc::new(Mutex::new(Vec::new()));
        let st = store::Settings::load();
        let on = Arc::new(AtomicBool::new(st.clip_on));
        let mode = Arc::new(AtomicU8::new(mode_byte(st.clip_mode)));
        let mut c = Clipboard {
            items: Vec::new(),
            next_id: 1,
            incoming: incoming.clone(),
            on: on.clone(),
            mode: mode.clone(),
            thumbs: HashMap::new(),
            scroll: 0.0,
            max_scroll: 0.0,
            copied: None,
            target: 0,
            own: hwnd.0 as isize,
            last_paste: None,
            search: None,
            snippet: None,
        };
        c.load();
        spawn_listener(incoming, on, mode, hwnd.0 as isize);
        c
    }

    fn load(&mut self) {
        for line in store::read("clipboard.txt").lines() {
            let p: Vec<&str> = line.splitn(3, '\t').collect();
            if p.len() < 3 {
                continue;
            }
            let content = match p[0] {
                "t" => Content::Text(unesc(p[2])),
                "f" => Content::Files(unesc(p[2]).lines().map(String::from).collect()),
                _ => continue,
            };
            let id = self.next_id;
            self.next_id += 1;
            self.items.push(Entry { id, content, at: p[1].parse().unwrap_or(0.0), pinned: true });
        }
    }

    /// Only pinned entries are written to disk.
    fn save(&self) {
        let s: String = self
            .items
            .iter()
            .filter(|e| e.pinned)
            .map(|e| match &e.content {
                Content::Text(t) => format!("t\t{}\t{}\n", e.at, esc(t)),
                Content::Files(f) => format!("f\t{}\t{}\n", e.at, esc(&f.join("\n"))),
                Content::Image(_) => String::new(),
            })
            .collect();
        store::write("clipboard.txt", &s);
    }

    /// Record a new clipboard value: an existing equal entry moves to the top.
    fn add(&mut self, content: Content) {
        if let Content::Text(t) = &content {
            if t.trim().is_empty() {
                return;
            }
        }
        let mut pinned = false;
        if let Some(i) = self.items.iter().position(|e| e.content == content) {
            pinned = self.items.remove(i).pinned;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.insert(0, Entry { id, content, at: unix_now(), pinned });
        // Drop the oldest unpinned entries beyond the limit.
        while self.items.iter().filter(|e| !e.pinned).count() > MAX_ITEMS {
            if let Some(i) = self.items.iter().rposition(|e| !e.pinned) {
                self.items.remove(i);
            }
        }
        // Images are big: keep the newest few within a memory budget.
        let mut n = 0;
        let mut bytes = 0;
        self.items.retain(|e| match &e.content {
            Content::Image(im) => {
                n += 1;
                bytes += im.dib.len();
                n <= MAX_IMAGES && bytes <= MAX_IMAGE_BYTES
            }
            _ => true,
        });
        let ids: Vec<u32> = self.items.iter().map(|e| e.id).collect();
        self.thumbs.retain(|id, _| ids.contains(id));
        if pinned {
            self.save();
        }
    }

    /// The entry matches the search box (case-insensitive; files by path).
    fn matches(&self, e: &Entry) -> bool {
        let q = match &self.search {
            Some(q) if !q.trim().is_empty() => q.trim().to_lowercase(),
            _ => return true,
        };
        match &e.content {
            Content::Text(t) => t.to_lowercase().contains(&q),
            Content::Files(f) => f.iter().any(|p| p.to_lowercase().contains(&q)),
            Content::Image(_) => false,
        }
    }

    /// Save typed text as a snippet: a pinned entry, kept after restarts.
    pub fn add_snippet(&mut self, text: &str) -> bool {
        let t = text.trim_end_matches(['\r', '\n']).to_string();
        if t.trim().is_empty() {
            return false;
        }
        let c = Content::Text(t);
        self.add(c.clone());
        if let Some(e) = self.items.iter_mut().find(|e| e.content == c) {
            e.pinned = true;
        }
        self.save();
        true
    }

    fn editing(&mut self) -> Option<&mut String> {
        if let Some(t) = self.search.as_mut() {
            return Some(t);
        }
        self.snippet.as_mut()
    }

    /// Pinned first, then by recency.
    fn order(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = self.items.iter().filter(|e| e.pinned).cloned().collect();
        v.extend(self.items.iter().filter(|e| !e.pinned).cloned());
        v
    }
}

/// The header line beside the switch.
pub fn status_line(on: bool, mode: u8) -> String {
    if !on {
        return "Not recording new copies".into();
    }
    let what = match mode {
        0 => "text",
        1 => "text + files",
        _ => "everything",
    };
    format!("Keeping {what} (click to paste)")
}

/// A one-line text box with a blinking caret (keyboard focus is the app's).
fn field(ui: &mut Ui, r: Rect, text: &str, placeholder: &str) {
    let r = r.centered(r.w, 26.0);
    ui.fill_rr(r, 8.0, col::TEXT.with_a(0.08));
    ui.stroke_rr(r, 8.0, col::BLUE.with_a(0.7), 1.0);
    ui.hit(r, A_FIELD);
    let tr = r.inset_xy(10.0, 0.0);
    if text.is_empty() {
        ui.text(placeholder, tr, 11.5, col::FAINT, Weight::Regular, Align::Left);
    }
    let one: String = text.replace(['\r', '\n'], " ");
    let tw = ui.measure(&one, 12.0, Weight::Regular);
    let align = if tw > tr.w - 6.0 { Align::Right } else { Align::Left };
    ui.text(&one, Rect::new(tr.x, tr.y, tr.w - 4.0, tr.h), 12.0, col::TEXT, Weight::Regular, align);
    if (ui.t * 1.8) as i64 % 2 == 0 {
        let cx = if align == Align::Right { tr.r() - 4.0 } else { tr.x + tw + 1.0 };
        ui.line(cx, tr.cy() - 7.0, cx, tr.cy() + 7.0, col::BLUE, 1.5);
    }
}

/// One line of preview text for an entry.
fn preview(c: &Content) -> String {
    match c {
        Content::Text(t) => {
            let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            one.chars().take(160).collect()
        }
        Content::Image(im) => format!("Image  {} x {}", im.w, im.h),
        Content::Files(f) => {
            let name = |p: &String| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string();
            match f.len() {
                0 => String::new(),
                1 => name(&f[0]),
                n => format!("{} + {} more", name(&f[0]), n - 1),
            }
        }
    }
}

fn ago(at: f64) -> String {
    let s = (unix_now() - at).max(0.0);
    if s < 60.0 {
        "now".into()
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0) as u64)
    } else if s < 86400.0 {
        format!("{}h", (s / 3600.0) as u64)
    } else {
        format!("{}d", (s / 86400.0) as u64)
    }
}

impl Module for Clipboard {
    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_CLEAR => "Remove everything that isn't pinned",
            A_TOGGLE => "Record what you copy, or stop",
            A_SEARCH => "Search the history",
            A_SNIPPET => "New snippet (kept like a pin)",
            A_FIELD => "Type here",
            i => match i >> 24 {
                K_PASTE => "Click to paste · the copy icon only copies",
                K_COPY => "Copy only (don't paste)",
                K_PIN => "Pin: keeps it at the top and after a restart",
                K_DELETE => "Remove",
                _ => "",
            },
        }
    }
    fn title(&self) -> &'static str {
        "Clipboard"
    }
    fn accent(&self) -> Color {
        col::BLUE
    }
    fn icon(&self) -> &'static str {
        glyph::PASTE
    }
    fn size(&self) -> (f32, f32) {
        (500.0, crate::modules::PANEL.1)
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        // Mirror the settings for the listener thread (off too while the
        // Clipboard panel is hidden in Settings > Panels).
        self.on.store(ctx.settings.clip_on && !ctx.settings.panel_hidden("Clipboard"), Ordering::Relaxed);
        self.mode.store(mode_byte(ctx.settings.clip_mode), Ordering::Relaxed);
        let new: Vec<Content> = std::mem::take(&mut *self.incoming.lock().unwrap());
        for c in new {
            self.add(c);
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        self.note_target();
        let on = self.on.load(Ordering::Relaxed);
        let mode = self.mode.load(Ordering::Relaxed);
        if !on && self.items.is_empty() && self.snippet.is_none() {
            // Off by default: explain, and offer the switch.
            let mut c = r.centered(r.w.min(420.0), 96.0);
            ui.text("Clipboard history is off", c.cut_top(20.0, 4.0), 14.0, col::TEXT, Weight::Semi, Align::Center);
            ui.text("Turn it on to keep what you copy and paste it again later.", c.cut_top(18.0, 2.0), 12.0, col::DIM, Weight::Regular, Align::Center);
            ui.text("Text only by default; change it in Settings. Nothing leaves your PC.", c.cut_top(18.0, 10.0), 11.0, col::FAINT, Weight::Regular, Align::Center);
            let mut row = c.cut_top(28.0, 0.0).centered(320.0, 28.0);
            ui.button_accent(row.cut_left(200.0, 10.0), "Turn on clipboard history", A_TOGGLE, col::BLUE);
            ui.button(row, "New snippet", A_SNIPPET);
            return;
        }
        let mut head = r.cut_top(26.0, 8.0);
        let sw = head.cut_right(150.0, 6.0);
        ui.toggle(sw, if on { "Recording" } else { "Off" }, on, A_TOGGLE, col::BLUE);
        let editing = self.search.is_some() || self.snippet.is_some();
        if self.items.iter().any(|e| !e.pinned) && !editing {
            ui.button(head.cut_right(60.0, 8.0), "Clear", A_CLEAR);
        }
        let add = head.cut_right(26.0, 4.0);
        ui.icon_btn(add.centered(26.0, 26.0), glyph::ADD, A_SNIPPET, 11.0, if self.snippet.is_some() { col::BLUE } else { col::DIM }, Some(col::BTN), "New snippet (kept like a pin)");
        let find = head.cut_right(26.0, 8.0);
        let searching = self.search.is_some();
        ui.icon_btn(find.centered(26.0, 26.0), if searching { glyph::CLOSE } else { "\u{E721}" }, A_SEARCH, 11.0, if searching { col::BLUE } else { col::DIM }, Some(col::BTN), if searching { "Close search" } else { "Search the history" });
        if let Some(q) = self.search.clone() {
            field(ui, head, &q, "Search, Enter pastes the first match");
        } else if let Some(t) = self.snippet.clone() {
            field(ui, head, &t, "Type a snippet, Enter saves it");
        } else {
            let status = status_line(on, mode);
            ui.text(&status, head, 12.0, if on { col::DIM } else { col::ACCENT }, Weight::Regular, Align::Left);
        }

        if self.items.is_empty() {
            ui.text("Copy something and it shows up here. Click an entry to paste it.", r, 12.0, col::FAINT, Weight::Regular, Align::Center);
            return;
        }
        let order: Vec<Entry> = self.order().into_iter().filter(|e| self.matches(e)).collect();
        if order.is_empty() {
            let msg = if self.snippet.is_some() { "Type the text you paste often (an address, a number, a signature). Enter saves it." } else { "Nothing matches" };
            ui.text_wrap(msg, r.inset_xy(20.0, 10.0), 12.0, col::FAINT, Weight::Regular, Align::Center);
            return;
        }
        let max_scroll = (order.len() as f32 * ROW_H - r.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        self.max_scroll = max_scroll;
        let now = unix_now();
        ui.push_clip(r);
        for (i, e) in order.iter().enumerate() {
            let y = r.y + i as f32 * ROW_H - self.scroll;
            if y + ROW_H < r.y || y > r.b() {
                continue;
            }
            let mut row = Rect::new(r.x, y, r.w, ROW_H - 2.0);
            let hov = ui.hovered(row);
            if hov {
                ui.fill_rr(row, 8.0, col::TEXT.with_a(0.06));
            }
            let row_hit = row;
            if let Content::Image(im) = &e.content {
                // Thumbnail instead of an icon.
                let tr = row.cut_left(40.0, 8.0).centered(40.0, 24.0);
                let fresh = matches!(self.thumbs.get(&e.id), Some((g, _)) if *g == ui.gen);
                if !fresh {
                    let (tw, th, px) = &im.thumb;
                    if let Some(b) = ui.make_bitmap(*tw, *th, px) {
                        self.thumbs.insert(e.id, (ui.gen, b));
                    }
                }
                ui.fill_rr(tr, 4.0, col::TEXT.with_a(0.08));
                if let Some((_, b)) = self.thumbs.get(&e.id) {
                    let b = b.clone();
                    let (tw, th) = (im.thumb.0 as f32, im.thumb.1 as f32);
                    let k = (tr.w / tw).min(tr.h / th);
                    ui.image_rr(&b, tr.centered(tw * k, th * k), 3.0);
                }
            } else {
                let ic = row.cut_left(26.0, 4.0);
                let (g, c) = match e.content {
                    Content::Files(_) => (glyph::FILE, col::BLUE),
                    _ => (glyph::COPY, col::DIM),
                };
                ui.icon(g, ic, 12.0, c);
            }
            let is_image = matches!(e.content, Content::Image(_));
            // The result label shows for a moment (longer for the admin
            // hint), then the hover buttons come back.
            let just = self.copied.filter(|(id, at, l)| *id == e.id && now - *at < if l.len() > 8 { 3.0 } else if hov { 1.0 } else { 1.6 }).map(|(_, _, l)| l);
            if hov && just.is_none() {
                let del = row.cut_right(26.0, 2.0);
                ui.icon_btn(del.centered(24.0, 24.0), glyph::DELETE, eid(K_DELETE, e.id), 11.0, col::DIM, None, "Remove");
                // Images stay in memory only, so they can't be pinned.
                if !is_image {
                    let pin = row.cut_right(26.0, 4.0);
                    let tip = if e.pinned { "Unpin" } else { "Pin (keeps it after restart)" };
                    let fg = if e.pinned { col::ACCENT } else { col::DIM };
                    ui.icon_btn(pin.centered(24.0, 24.0), glyph::PIN, eid(K_PIN, e.id), 11.0, fg, None, tip);
                }
                let cp = row.cut_right(26.0, 4.0);
                ui.icon_btn(cp.centered(24.0, 24.0), glyph::COPY, eid(K_COPY, e.id), 11.0, col::DIM, None, "Copy only (don't paste)");
            } else {
                let label = just.map_or_else(|| ago(e.at), |l| l.to_string());
                let w = ui.measure(&label, 11.0, Weight::Semi) + 6.0;
                let tr = row.cut_right(w, 6.0);
                let c = match just {
                    Some("Pasted") | Some("Copied") => col::GREEN,
                    Some(_) => col::ACCENT,
                    None => col::FAINT,
                };
                ui.text(&label, tr, 11.0, c, Weight::Semi, Align::Right);
                if e.pinned {
                    ui.icon(glyph::PIN, row.cut_right(16.0, 4.0), 10.0, col::ACCENT);
                }
            }
            ui.text(&preview(&e.content), row, 12.5, col::TEXT, Weight::Regular, Align::Left);
            // Register the row last-but-under the buttons: buttons were cut out of `row`.
            ui.hit_tip(Rect::new(row_hit.x, row_hit.y, row.r() - row_hit.x, row_hit.h), eid(K_PASTE, e.id), "Click to paste · copy icon only copies");
        }
        ui.pop_clip();
    }

    fn scroll(&mut self, delta: f32) -> bool {
        if self.max_scroll < 1.0 {
            return false;
        }
        self.scroll -= delta * ROW_H;
        true
    }

    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        let searching = self.search.is_some();
        let Some(text) = self.editing() else { return false };
        match vk {
            0x0D if searching => {
                // Enter pastes the first match.
                let first = self.order().into_iter().find(|e| self.matches(e)).map(|e| e.id);
                if let Some(id) = first {
                    self.action(eid(K_PASTE, id), (0.0, 0.0), ctx);
                }
            }
            0x0D => {
                let t = text.clone();
                self.snippet = None;
                self.add_snippet(&t);
                ctx.want_keyboard(false);
            }
            0x1B => {
                if searching && !text.is_empty() {
                    text.clear();
                } else {
                    self.search = None;
                    self.snippet = None;
                    ctx.want_keyboard(false);
                }
            }
            0x08 => {
                text.pop();
            }
            0x56 if ctrl => {
                if let Some(t) = crate::modules::todo::clipboard_text() {
                    let t: String = if searching { t.chars().filter(|c| !c.is_control()).take(100).collect() } else { t.chars().take(4000).collect() };
                    text.push_str(&t);
                }
            }
            _ => return false,
        }
        self.scroll = 0.0;
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        let max = if self.search.is_some() { 100 } else { 4000 };
        let Some(text) = self.editing() else { return false };
        if c.is_control() || text.chars().count() >= max {
            return false;
        }
        text.push(c);
        self.scroll = 0.0;
        true
    }

    fn animating(&self) -> bool {
        // Caret blink while typing.
        self.search.is_some() || self.snippet.is_some()
    }

    fn blur(&mut self) {
        // An empty search or an unsaved empty snippet just closes.
        if self.search.as_ref().map_or(false, |q| q.is_empty()) {
            self.search = None;
        }
        if self.snippet.as_ref().map_or(false, |t| t.trim().is_empty()) {
            self.snippet = None;
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_CLEAR => {
                self.items.retain(|e| e.pinned);
                self.scroll = 0.0;
            }
            A_SEARCH => {
                if self.search.take().is_none() {
                    self.search = Some(String::new());
                    self.snippet = None;
                    ctx.want_keyboard(true);
                } else {
                    ctx.want_keyboard(false);
                }
                self.scroll = 0.0;
            }
            A_SNIPPET => {
                if self.snippet.take().is_none() {
                    self.snippet = Some(String::new());
                    self.search = None;
                    ctx.want_keyboard(true);
                } else {
                    ctx.want_keyboard(false);
                }
            }
            A_FIELD => ctx.want_keyboard(true),
            A_TOGGLE => {
                ctx.settings.clip_on = !ctx.settings.clip_on;
                ctx.settings.save();
                self.on.store(ctx.settings.clip_on, Ordering::Relaxed);
            }
            A_DEMO_DOCS => {
                let logo = crate::store::dir().join("Demo files").join("Logo final.png");
                self.add(Content::Files(vec![logo.display().to_string()]));
                for s in ["SELECT name, total FROM orders WHERE status = 'paid';", "Meeting moved to 4:30 pm, same room", "https://github.com/shabab239/WindyLand"] {
                    self.add(Content::Text(s.into()));
                }
            }
            A_DEMO => {
                for i in 1..=35 {
                    self.add(Content::Text(format!("Selftest entry {i}")));
                }
                // A tiny test picture, then a file on top.
                if let Some(im) = image_from_dib(make_test_dib(2, 2)) {
                    self.add(Content::Image(Arc::new(im)));
                }
                self.add(Content::Files(vec![r"C:\Windows\notepad.exe".into()]));
            }
            _ => {
                let (kind, eid) = (id >> 24, id & 0xFF_FFFF);
                let Some(i) = self.items.iter().position(|e| e.id == eid) else { return };
                match kind {
                    K_COPY | K_PASTE => {
                        if self.search.is_some() || self.snippet.is_some() {
                            ctx.want_keyboard(false);
                        }
                        let content = self.items[i].content.clone();
                        if set_clipboard(ctx.hwnd, &content) {
                            // Move it to the top now; the listener will see
                            // the same content and keep it there.
                            let pinned = self.items[i].pinned;
                            self.items[i].at = unix_now();
                            if !pinned {
                                let e = self.items.remove(i);
                                self.items.insert(0, e);
                            }
                            let label = if kind == K_PASTE {
                                let target = self.paste_target();
                                let res = paste_into(target, HWND(self.own as *mut _));
                                self.last_paste = Some(res);
                                match res {
                                    Paste::Sent => "Pasted",
                                    Paste::Elevated => "Copied: admin app, press Ctrl+V",
                                    Paste::NoTarget => "Copied",
                                }
                            } else {
                                self.last_paste = None;
                                "Copied"
                            };
                            self.copied = Some((eid, unix_now(), label));
                        }
                    }
                    K_PIN => {
                        if !matches!(self.items[i].content, Content::Image(_)) {
                            self.items[i].pinned = !self.items[i].pinned;
                            self.save();
                        }
                    }
                    K_DELETE => {
                        let was_pinned = self.items.remove(i).pinned;
                        if was_pinned {
                            self.save();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn debug_state(&self) -> String {
        let pinned = self.items.iter().filter(|e| e.pinned).count();
        let mut seen = Vec::new();
        let mut dupes = 0;
        for e in &self.items {
            if seen.contains(&&e.content) {
                dupes += 1;
            }
            seen.push(&e.content);
        }
        let top = self.items.first().map(|e| preview(&e.content)).unwrap_or_default();
        let top_id = self.items.first().map_or(0, |e| e.id);
        format!(
            "items={} unpinned={} pinned={pinned} images={} dupes={dupes} on={} mode={} paste={:?} shown={} top_id={top_id} top={top}",
            self.items.len(),
            self.items.len() - pinned,
            self.items.iter().filter(|e| matches!(e.content, Content::Image(_))).count(),
            self.on.load(Ordering::Relaxed),
            self.mode.load(Ordering::Relaxed),
            self.last_paste,
            self.items.iter().filter(|e| self.matches(e)).count(),
        )
    }
}

// ------------------------------------------------------------ win32 side

struct ListenerState {
    incoming: Arc<Mutex<Vec<Content>>>,
    on: Arc<AtomicBool>,
    mode: Arc<AtomicU8>,
    main: isize,
}

static LISTENER: OnceLock<ListenerState> = OnceLock::new();

/// A message-only window on its own thread gets WM_CLIPBOARDUPDATE the
/// moment anything is copied, reads it, and wakes the notch.
fn spawn_listener(incoming: Arc<Mutex<Vec<Content>>>, on: Arc<AtomicBool>, mode: Arc<AtomicU8>, main: isize) {
    if LISTENER.set(ListenerState { incoming, on, mode, main }).is_err() {
        return;
    }
    std::thread::spawn(|| unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else { return };
        let class = w!("WindyLandClipboard");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(listener_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, hinst, None) else {
            return;
        };
        if AddClipboardFormatListener(hwnd).is_err() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

extern "system" fn listener_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        if let Some(st) = LISTENER.get() {
            if st.on.load(Ordering::Relaxed) {
                if let Some(c) = read_clipboard(hwnd, st.mode.load(Ordering::Relaxed)) {
                    st.incoming.lock().unwrap().push(c);
                    unsafe {
                        let _ = PostMessageW(HWND(st.main as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// The clipboard is often briefly held by the app that just wrote it.
fn open_clipboard(owner: HWND) -> bool {
    for _ in 0..8 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

/// True when the app that copied asked monitors not to record it
/// (password managers do this for passwords and one-time codes).
unsafe fn is_private() -> bool {
    for name in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
        let f = RegisterClipboardFormatW(name);
        if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
            return true;
        }
    }
    // A DWORD 0 here means "keep this out of clipboard history".
    let f = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
    if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
        if let Ok(h) = GetClipboardData(f) {
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u32;
            let allowed = p.is_null() || *p != 0;
            let _ = GlobalUnlock(hg);
            if !allowed {
                return true;
            }
        }
    }
    false
}

/// Read what is on the clipboard, limited by `mode` (0 text, 1 + files,
/// 2 + images). Text wins over a picture when both are offered (Excel and
/// Word put both on the clipboard when you copy cells or text).
fn read_clipboard(owner: HWND, mode: u8) -> Option<Content> {
    if !open_clipboard(owner) {
        return None;
    }
    let out = unsafe {
        (|| {
            if is_private() {
                return None;
            }
            if mode >= 1 && IsClipboardFormatAvailable(CF_HDROP).is_ok() {
                let h = GetClipboardData(CF_HDROP).ok()?;
                let hd = HDROP(h.0);
                let n = DragQueryFileW(hd, 0xFFFF_FFFF, None);
                let mut files = Vec::new();
                for i in 0..n.min(500) {
                    let len = DragQueryFileW(hd, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(hd, i, Some(&mut buf));
                    files.push(String::from_utf16_lossy(&buf[..len]));
                }
                return (!files.is_empty()).then_some(Content::Files(files));
            }
            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
                let h = GetClipboardData(CF_UNICODETEXT).ok()?;
                let hg = HGLOBAL(h.0);
                let cap = GlobalSize(hg) / 2;
                let p = GlobalLock(hg) as *const u16;
                if p.is_null() {
                    return None;
                }
                let mut len = 0;
                while len < cap && *p.add(len) != 0 {
                    len += 1;
                }
                let mut s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                let _ = GlobalUnlock(hg);
                if s.chars().count() > MAX_CHARS {
                    s = s.chars().take(MAX_CHARS).collect();
                }
                return Some(Content::Text(s));
            }
            if mode >= 2 && IsClipboardFormatAvailable(CF_DIB).is_ok() {
                let h = GetClipboardData(CF_DIB).ok()?;
                let hg = HGLOBAL(h.0);
                let size = GlobalSize(hg);
                if size == 0 || size > 48 * 1024 * 1024 {
                    return None;
                }
                let p = GlobalLock(hg) as *const u8;
                if p.is_null() {
                    return None;
                }
                let dib = std::slice::from_raw_parts(p, size).to_vec();
                let _ = GlobalUnlock(hg);
                return image_from_dib(dib).map(|im| Content::Image(Arc::new(im)));
            }
            None
        })()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    out
}

/// Put `c` on the clipboard (text, or files as CF_HDROP for pasting in Explorer).
// ------------------------------------------------------------------ paste

/// What happened when an entry was clicked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Paste {
    /// Ctrl+V was sent to the app you were using.
    Sent,
    /// That app runs as administrator; Windows doesn't let a normal app type
    /// into it, so it's only copied.
    Elevated,
    /// No app to paste into (only the taskbar or WindyLand was in front).
    NoTarget,
}

/// Self-test: paste into this window instead of the app in front.
pub static TEST_TARGET: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Self-test: don't really press Ctrl+V.
pub static PASTE_DRY_RUN: AtomicBool = AtomicBool::new(false);

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut b) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// A window we can paste into: a real, visible top-level app window that
/// isn't WindyLand or the taskbar.
pub fn paste_target_ok(h: HWND, own: HWND) -> bool {
    if h.0.is_null() || h == own || !unsafe { IsWindow(h) }.as_bool() {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    // Our own windows, except the self-test's text box.
    if pid == std::process::id() && h.0 as isize != TEST_TARGET.load(Ordering::Relaxed) {
        return false;
    }
    !matches!(class_of(h).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland")
}

fn elevated(process: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut tok = windows::Win32::Foundation::HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len).is_ok();
        let _ = CloseHandle(tok);
        ok && e.TokenIsElevated != 0
    }
}

/// Is the app that owns `h` running as administrator while we aren't?
fn blocked_by_admin(h: HWND) -> bool {
    use windows::Win32::System::Threading::*;
    unsafe {
        if elevated(GetCurrentProcess()) {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(p) => {
                let e = elevated(p);
                let _ = CloseHandle(p);
                e
            }
            // Can't even look at it: most likely an admin or system app.
            Err(_) => true,
        }
    }
}

/// Press Ctrl+V in `target` (the clipboard must already hold the entry).
/// Holding Shift while clicking sends Ctrl+Shift+V, which many apps (browsers,
/// Word, Slack, VS Code) take as "paste as plain text".
pub fn paste_into(target: HWND, own: HWND) -> Paste {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    if !paste_target_ok(target, own) {
        return Paste::NoTarget;
    }
    if blocked_by_admin(target) {
        return Paste::Elevated;
    }
    if PASTE_DRY_RUN.load(Ordering::Relaxed) {
        return Paste::Sent;
    }
    unsafe {
        // The notch never takes focus on a click, but a text box in another
        // panel may have; hand it back first.
        if GetForegroundWindow() != target {
            let _ = SetForegroundWindow(target);
        }
    }
    let t = target.0 as isize;
    std::thread::spawn(move || {
        // Let the focus change land and the mouse button come up.
        std::thread::sleep(std::time::Duration::from_millis(70));
        unsafe {
            let target = HWND(t as *mut _);
            if GetForegroundWindow() != target {
                return;
            }
            let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
            };
            let mut seq = Vec::new();
            // Alt or Win held would turn Ctrl+V into something else.
            for vk in [VK_MENU, VK_LWIN, VK_RWIN] {
                if GetAsyncKeyState(vk.0 as i32) < 0 {
                    seq.push(key(vk, true));
                }
            }
            seq.extend([key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)]);
            SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
        }
    });
    Paste::Sent
}

pub fn set_clipboard(owner: HWND, c: &Content) -> bool {
    if !open_clipboard(owner) {
        return false;
    }
    let ok = unsafe {
        (|| -> Option<()> {
            EmptyClipboard().ok()?;
            match c {
                Content::Text(s) => {
                    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                    let hg = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2).ok()?;
                    let p = GlobalLock(hg) as *mut u16;
                    if p.is_null() {
                        return None;
                    }
                    std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                    let _ = GlobalUnlock(hg);
                    SetClipboardData(CF_UNICODETEXT, HANDLE(hg.0)).ok()?;
                }
                Content::Files(files) => {
                    let mut list: Vec<u16> = Vec::new();
                    for f in files {
                        list.extend(f.encode_utf16());
                        list.push(0);
                    }
                    list.push(0);
                    let head = std::mem::size_of::<DROPFILES>();
                    let hg = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, head + list.len() * 2).ok()?;
                    let p = GlobalLock(hg) as *mut u8;
                    if p.is_null() {
                        return None;
                    }
                    let df = DROPFILES { pFiles: head as u32, pt: POINT::default(), fNC: FALSE, fWide: TRUE };
                    std::ptr::write_unaligned(p as *mut DROPFILES, df);
                    std::ptr::copy_nonoverlapping(list.as_ptr() as *const u8, p.add(head), list.len() * 2);
                    let _ = GlobalUnlock(hg);
                    SetClipboardData(CF_HDROP, HANDLE(hg.0)).ok()?;
                }
                Content::Image(im) => {
                    let hg = GlobalAlloc(GMEM_MOVEABLE, im.dib.len()).ok()?;
                    let p = GlobalLock(hg) as *mut u8;
                    if p.is_null() {
                        return None;
                    }
                    std::ptr::copy_nonoverlapping(im.dib.as_ptr(), p, im.dib.len());
                    let _ = GlobalUnlock(hg);
                    SetClipboardData(CF_DIB, HANDLE(hg.0)).ok()?;
                }
            }
            Some(())
        })()
        .is_some()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

/// Read the clipboard right now (self-test helper).
pub fn current(owner: HWND) -> Option<Content> {
    read_clipboard(owner, 2)
}

/// Parse a CF_DIB (BITMAPINFOHEADER + pixels, 24 or 32 bit, uncompressed or
/// bitfields) into an `Image` with a small thumbnail. Other formats are
/// skipped rather than guessed at.
pub fn image_from_dib(dib: Vec<u8>) -> Option<Image> {
    let rd32 = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?)) };
    let rd16 = |o: usize| -> Option<u16> { Some(u16::from_le_bytes(dib.get(o..o + 2)?.try_into().ok()?)) };
    let hsize = rd32(0)? as usize;
    let w = rd32(4)? as i32;
    let h = rd32(8)? as i32;
    let bpp = rd16(14)? as usize;
    let comp = rd32(16)?;
    let used = rd32(32).unwrap_or(0) as usize;
    if hsize < 40 || w <= 0 || h == 0 || !(bpp == 24 || bpp == 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let (w, top_down, ah) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let masks = if comp == 3 && hsize == 40 { 12 } else { 0 };
    let off = hsize + masks + used * 4;
    let stride = (w * bpp + 31) / 32 * 4;
    if dib.len() < off + stride * ah {
        return None;
    }
    // Many apps leave the alpha byte at 0 in 32-bit DIBs; treat that as opaque.
    let alpha_used = bpp == 32 && (0..ah).any(|y| (0..w).any(|x| dib[off + y * stride + x * 4 + 3] != 0));
    let k = (THUMB.0 as f32 / w as f32).min(THUMB.1 as f32 / ah as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((ah as f32 * k).round() as u32).max(1));
    let mut px = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th as usize {
        let sy = (ty * ah / th as usize).min(ah - 1);
        let row = if top_down { sy } else { ah - 1 - sy };
        for tx in 0..tw as usize {
            let sx = (tx * w / tw as usize).min(w - 1);
            let s = off + row * stride + sx * (bpp / 8);
            let (b, g, r) = (dib[s], dib[s + 1], dib[s + 2]);
            let a = if alpha_used { dib[s + 3] } else { 255 };
            let pm = |c: u8| ((c as u16 * a as u16) / 255) as u8;
            let d = (ty * tw as usize + tx) * 4;
            px[d..d + 4].copy_from_slice(&[pm(b), pm(g), pm(r), a]);
        }
    }
    use std::hash::{Hash, Hasher};
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    dib.hash(&mut hs);
    Some(Image { w: w as u32, h: ah as u32, hash: hs.finish(), thumb: (tw, th, px), dib })
}

/// A small 32-bit bottom-up CF_DIB of a colour gradient (self-test data).
pub fn make_test_dib(w: u32, h: u32) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [40u32, w, h] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 24]);
    for y in 0..h {
        for x in 0..w {
            d.extend_from_slice(&[(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 200, 0]);
        }
    }
    d
}

/// Self-test helper: copy `text` the way a password manager does, marked
/// "exclude from clipboard monitors".
pub fn set_private_for_test(owner: HWND, text: &str) -> bool {
    if !open_clipboard(owner) {
        return false;
    }
    // Text and marker in one session, so monitors only ever see them together.
    let ok = unsafe {
        (|| -> Option<()> {
            EmptyClipboard().ok()?;
            let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let hg = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2).ok()?;
            let p = GlobalLock(hg) as *mut u16;
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            let _ = GlobalUnlock(hg);
            SetClipboardData(CF_UNICODETEXT, HANDLE(hg.0)).ok()?;
            let f = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            let hm = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4).ok()?;
            SetClipboardData(f, HANDLE(hm.0)).ok()?;
            Some(())
        })()
        .is_some()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}
