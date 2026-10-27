//! Clipboard history: what you copy is kept in a list; click an entry to put
//! it back on the clipboard. Off until the user turns it on (Settings or the
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
/// Self-test / docs: add 35 numbered demo entries.
pub const A_DEMO: u32 = 990;
/// Docs/demo video: a few realistic entries.
pub const A_DEMO_DOCS: u32 = 991;
const K_COPY: u32 = 1;
const K_PIN: u32 = 2;
const K_DELETE: u32 = 3;

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
    /// Entry id just copied back, and when (for the "Copied" label).
    copied: Option<(u32, f64)>,
}

impl Clipboard {
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
            copied: None,
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

    /// Pinned first, then by recency.
    fn order(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = self.items.iter().filter(|e| e.pinned).cloned().collect();
        v.extend(self.items.iter().filter(|e| !e.pinned).cloned());
        v
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
        // Mirror the settings for the listener thread.
        self.on.store(ctx.settings.clip_on, Ordering::Relaxed);
        self.mode.store(mode_byte(ctx.settings.clip_mode), Ordering::Relaxed);
        let new: Vec<Content> = std::mem::take(&mut *self.incoming.lock().unwrap());
        for c in new {
            self.add(c);
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let on = self.on.load(Ordering::Relaxed);
        let mode = self.mode.load(Ordering::Relaxed);
        if !on && self.items.is_empty() {
            // Off by default: explain, and offer the switch.
            let mut c = r.centered(r.w.min(420.0), 96.0);
            ui.text("Clipboard history is off", c.cut_top(20.0, 4.0), 14.0, col::TEXT, Weight::Semi, Align::Center);
            ui.text("Turn it on to keep what you copy and paste it again later.", c.cut_top(18.0, 2.0), 12.0, col::DIM, Weight::Regular, Align::Center);
            ui.text("Text only by default; change it in Settings. Nothing leaves your PC.", c.cut_top(18.0, 10.0), 11.0, col::FAINT, Weight::Regular, Align::Center);
            ui.button_accent(c.cut_top(28.0, 0.0).centered(200.0, 28.0), "Turn on clipboard history", A_TOGGLE, col::BLUE);
            return;
        }
        let mut head = r.cut_top(26.0, 8.0);
        let sw = head.cut_right(150.0, 6.0);
        ui.toggle(sw, if on { "Recording" } else { "Off" }, on, A_TOGGLE, col::BLUE);
        if self.items.iter().any(|e| !e.pinned) {
            ui.button(head.cut_right(60.0, 8.0), "Clear", A_CLEAR);
        }
        let what = match mode {
            0 => "text",
            1 => "text and files",
            _ => "text, files and images",
        };
        let status = if on { format!("Keeping {what}") } else { "Not recording new copies".to_string() };
        ui.text(&status, head, 12.0, if on { col::DIM } else { col::ACCENT }, Weight::Regular, Align::Left);

        if self.items.is_empty() {
            ui.text("Copy something and it shows up here. Click an entry to copy it again.", r, 12.0, col::FAINT, Weight::Regular, Align::Center);
            return;
        }
        let order = self.order();
        let max_scroll = (order.len() as f32 * ROW_H - r.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
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
            if hov {
                let del = row.cut_right(26.0, 2.0);
                ui.icon_btn(del.centered(24.0, 24.0), glyph::DELETE, eid(K_DELETE, e.id), 11.0, col::DIM, None, "Remove");
                // Images stay in memory only, so they can't be pinned.
                if !is_image {
                    let pin = row.cut_right(26.0, 4.0);
                    let tip = if e.pinned { "Unpin" } else { "Pin (keeps it after restart)" };
                    let fg = if e.pinned { col::ACCENT } else { col::DIM };
                    ui.icon_btn(pin.centered(24.0, 24.0), glyph::PIN, eid(K_PIN, e.id), 11.0, fg, None, tip);
                }
            } else {
                let just = self.copied.map_or(false, |(id, at)| id == e.id && now - at < 1.5);
                let label = if just { "Copied".to_string() } else { ago(e.at) };
                let w = ui.measure(&label, 11.0, Weight::Semi) + 6.0;
                let tr = row.cut_right(w, 6.0);
                ui.text(&label, tr, 11.0, if just { col::GREEN } else { col::FAINT }, Weight::Semi, Align::Right);
                if e.pinned {
                    ui.icon(glyph::PIN, row.cut_right(16.0, 4.0), 10.0, col::ACCENT);
                }
            }
            ui.text(&preview(&e.content), row, 12.5, col::TEXT, Weight::Regular, Align::Left);
            // Register the row last-but-under the buttons: buttons were cut out of `row`.
            ui.hit(Rect::new(row_hit.x, row_hit.y, row.r() - row_hit.x, row_hit.h), eid(K_COPY, e.id));
        }
        ui.pop_clip();
    }

    fn scroll(&mut self, delta: f32) {
        self.scroll -= delta * ROW_H;
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_CLEAR => {
                self.items.retain(|e| e.pinned);
                self.scroll = 0.0;
            }
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
                    K_COPY => {
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
                            self.copied = Some((eid, unix_now()));
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
            "items={} unpinned={} pinned={pinned} images={} dupes={dupes} on={} mode={} top_id={top_id} top={top}",
            self.items.len(),
            self.items.len() - pinned,
            self.items.iter().filter(|e| matches!(e.content, Content::Image(_))).count(),
            self.on.load(Ordering::Relaxed),
            self.mode.load(Ordering::Relaxed)
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
