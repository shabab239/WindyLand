//! File shelf: drop files onto the notch to keep them close, click to open,
//! drag them back out into any app. Stores references (paths), not copies.

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel};
use crate::store;
use crate::ui::Ui;

struct Item {
    path: String,
    name: String,
    detail: String,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    File,
    Folder,
    Link,
}

const A_CLEAR: u32 = 1;
pub const A_SNIP: u32 = 3;
/// Self-test: take the screenshot path without opening the full-screen window
/// or touching the clipboard.
pub const A_TEST_NO_OVERLAY: u32 = 991;
const K_OPEN: u32 = 1;
const K_REMOVE: u32 = 2;

fn iid(kind: u32, i: usize) -> u32 {
    (kind << 24) | i as u32
}

fn human_size(b: u64) -> String {
    let f = b as f64;
    if b < 1024 {
        format!("{b} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / 1048576.0)
    } else {
        format!("{:.2} GB", f / 1073741824.0)
    }
}

impl Item {
    fn from_path(path: &str) -> Option<Item> {
        let p = std::path::Path::new(path);
        let meta = std::fs::metadata(p).ok()?;
        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let (kind, detail) = if meta.is_dir() {
            (Kind::Folder, "Folder".to_string())
        } else if name.to_lowercase().ends_with(".url") {
            (Kind::Link, "Link".to_string())
        } else {
            (Kind::File, human_size(meta.len()))
        };
        let name = if kind == Kind::Link { name.trim_end_matches(".url").to_string() } else { name };
        Some(Item { path: path.to_string(), name, detail, kind })
    }
}

pub struct Shelf {
    items: Vec<Item>,
    drag_hover: bool,
    scroll: f32,
    max_scroll: f32,
    /// "Screenshot" waits for the notch to close first.
    snip_at: Option<f64>,
    no_overlay: bool,
    /// Where the last screenshot went, shown in the footer for a moment.
    note: Option<(String, f64)>,
}

impl Shelf {
    pub fn load() -> Shelf {
        let items = store::read("shelf.txt").lines().filter_map(|l| Item::from_path(l.trim())).collect();
        Shelf { items, drag_hover: false, scroll: 0.0, max_scroll: 0.0, snip_at: None, no_overlay: false, note: None }
    }

    fn save(&self) {
        let s: String = self.items.iter().map(|i| format!("{}\n", i.path)).collect();
        store::write("shelf.txt", &s);
    }
}

impl Module for Shelf {
    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_CLEAR => "Take everything off the Shelf (files stay where they are)",
            A_SNIP => "Drag over part of the screen; it lands here and on the clipboard",
            i => match i >> 24 {
                K_OPEN => "Click to open, or drag it into another app",
                K_REMOVE => "Remove from the Shelf",
                _ => "",
            },
        }
    }
    fn title(&self) -> &'static str {
        "Shelf"
    }
    fn accent(&self) -> Color {
        col::PURPLE
    }
    fn icon(&self) -> &'static str {
        glyph::SHELF
    }
    fn size(&self) -> (f32, f32) {
        (480.0, 128.0)
    }
    fn wants_drops(&self) -> bool {
        true
    }
    fn drag_hover(&mut self, on: bool) {
        self.drag_hover = on;
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut foot = r.cut_bottom(16.0, 6.0);
        if !self.items.is_empty() {
            ui.button(foot.cut_right(60.0, 8.0).inset_xy(0.0, -3.0), "Clear", A_CLEAR);
        }
        let shot = foot.cut_right(106.0, 0.0).inset_xy(0.0, -3.0);
        ui.button_c(shot, "", A_SNIP, if ui.hovered(shot) { col::BTN_HOVER } else { col::BTN }, col::TEXT);
        ui.icon(glyph::CAMERA, Rect::new(shot.x + 10.0, shot.y, 16.0, shot.h), 11.0, col::PURPLE);
        ui.text("Screenshot", Rect::new(shot.x + 30.0, shot.y, shot.w - 34.0, shot.h), 11.5, col::TEXT, Weight::Semi, Align::Left);
        ui.hit_tip(shot, A_SNIP, "Drag over part of the screen; it lands here and on the clipboard");
        let note = self.note.as_ref().filter(|(_, at)| crate::time::unix_now() - at < 5.0).map(|(n, _)| n.clone());
        let hint = if self.drag_hover {
            "Release to add".to_string()
        } else {
            note.clone().unwrap_or_else(|| "Drag out anytime · click to open".into())
        };
        ui.text(&hint, foot, 11.0, if note.is_some() { col::PURPLE } else { col::FAINT }, Weight::Regular, Align::Left);

        if self.items.is_empty() || self.drag_hover {
            let c = if self.drag_hover { col::ACCENT } else { col::FAINT };
            ui.fill_rr(r, 14.0, c.with_a(if self.drag_hover { 0.12 } else { 0.05 }));
            ui.stroke_rr(r.inset(0.5), 14.0, c.with_a(0.6), 1.2);
            let msg = if self.drag_hover { "Release to add" } else { "Drop files here, or take a screenshot" };
            let mut mid = r;
            let ic = mid.cut_top(mid.h * 0.55, 0.0);
            ui.icon(glyph::SHELF, Rect::new(ic.x, ic.b() - 26.0, ic.w, 24.0), 20.0, c);
            ui.text(msg, Rect::new(mid.x, mid.y + 2.0, mid.w, 20.0), 13.0, if self.drag_hover { c } else { col::DIM }, Weight::Semi, Align::Center);
            if !self.drag_hover || self.items.is_empty() {
                return;
            }
        }

        let tile_w = 96.0;
        let gap = 8.0;
        let content_w = self.items.len() as f32 * (tile_w + gap) - gap;
        let max_scroll = (content_w - r.w).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        self.max_scroll = max_scroll;
        ui.push_clip(r);
        for (i, it) in self.items.iter().enumerate() {
            let x = r.x + i as f32 * (tile_w + gap) - self.scroll;
            if x + tile_w < r.x || x > r.r() {
                continue;
            }
            let tile = Rect::new(x, r.y, tile_w, r.h);
            let hov = ui.hovered(tile);
            ui.fill_rr(tile, 12.0, if hov { col::TEXT.with_a(0.12) } else { col::TEXT.with_a(0.06) });
            let mut inner = tile.inset(8.0);
            let ic = inner.cut_top(inner.h - 34.0, 4.0);
            let (g, c) = match it.kind {
                Kind::Folder => (glyph::FOLDER, col::BLUE),
                Kind::Link => (glyph::LINK, col::PURPLE),
                Kind::File => (glyph::FILE, col::TEXT),
            };
            ui.icon(g, ic, 26.0, c);
            if it.kind == Kind::File {
                if let Some(ext) = it.name.rsplit_once('.').map(|(_, e)| e.to_uppercase()) {
                    if ext.len() <= 5 {
                        let er = Rect::new(ic.cx() - 18.0, ic.b() - 16.0, 36.0, 14.0);
                        ui.fill_rr(er, 4.0, col::ACCENT);
                        ui.text(&ext, er, 8.5, hex(0x000000), Weight::Bold, Align::Center);
                    }
                }
            }
            ui.text(&it.name, inner.cut_top(17.0, 0.0), 11.5, col::TEXT, Weight::Semi, Align::Center);
            ui.text(&it.detail, inner.cut_top(15.0, 0.0), 10.0, col::DIM, Weight::Regular, Align::Center);
            ui.hit(tile, iid(K_OPEN, i));
            if hov {
                let x = Rect::new(tile.r() - 24.0, tile.y + 4.0, 20.0, 20.0);
                ui.icon_btn(x, glyph::CLOSE, iid(K_REMOVE, i), 9.0, col::DIM, Some(hex(0x2A2A2E)), "Remove from shelf");
            }
        }
        ui.pop_clip();
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        if let Some(at) = self.snip_at {
            if crate::time::unix_now() >= at {
                self.snip_at = None;
                if !self.no_overlay && !crate::modules::snip::start(ctx.hwnd) {
                    self.note = Some(("Couldn't take a screenshot".into(), crate::time::unix_now()));
                    ctx.open_my_tab();
                }
            }
        }
        let res = crate::modules::snip::RESULT.lock().unwrap().take();
        if let Some(Some(shot)) = res {
            let dir = crate::modules::snip::folder();
            let _ = std::fs::create_dir_all(&dir);
            let path = crate::modules::snip::new_path(&dir);
            if crate::modules::snip::save_png(&path, &shot) {
                let p = path.display().to_string();
                if let Some(it) = Item::from_path(&p) {
                    self.items.push(it);
                    self.save();
                    self.scroll = f32::MAX;
                }
                if !self.no_overlay {
                    let img = crate::modules::clipboard::Image { w: shot.w, h: shot.h, dib: crate::modules::snip::dib32(&shot), hash: 0, thumb: (0, 0, Vec::new()) };
                    crate::modules::clipboard::set_clipboard(ctx.hwnd, &crate::modules::clipboard::Content::Image(std::sync::Arc::new(img)));
                }
                self.note = Some((format!("Saved {} x {} to Pictures\\Screenshots, and copied", shot.w, shot.h), crate::time::unix_now()));
            } else {
                self.note = Some(("Couldn't save the screenshot".into(), crate::time::unix_now()));
            }
            ctx.open_my_tab();
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        if id == A_CLEAR {
            self.items.clear();
            self.save();
            return;
        }
        if id == A_TEST_NO_OVERLAY {
            self.no_overlay = true;
            return;
        }
        if id == A_SNIP {
            // Let the notch close first so it's not in the picture.
            self.snip_at = Some(crate::time::unix_now() + 0.35);
            ctx.close_panel();
            let hwnd = ctx.hwnd.0 as isize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(380));
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        windows::Win32::Foundation::HWND(hwnd as *mut _),
                        crate::app::WM_APP_WAKE,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
            });
            return;
        }
        let (kind, i) = (id >> 24, (id & 0xFF_FFFF) as usize);
        if i >= self.items.len() {
            return;
        }
        match kind {
            K_OPEN => crate::app::open_later(self.items[i].path.clone()),
            K_REMOVE => {
                self.items.remove(i);
                self.save();
            }
            _ => {}
        }
    }

    fn scroll(&mut self, delta: f32) -> bool {
        if self.max_scroll < 1.0 {
            return false;
        }
        self.scroll -= delta * 60.0;
        true
    }

    fn drop_files(&mut self, paths: Vec<String>, _ctx: &mut Ctx) -> bool {
        for p in paths {
            if self.items.iter().any(|i| i.path.eq_ignore_ascii_case(&p)) {
                continue;
            }
            if let Some(it) = Item::from_path(&p) {
                self.items.push(it);
            }
        }
        self.save();
        true
    }

    fn debug_state(&self) -> String {
        format!("items {}: {}", self.items.len(), self.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(" | "))
    }

    fn drag_out(&mut self, id: u32) -> Option<Vec<String>> {
        let (kind, i) = (id >> 24, (id & 0xFF_FFFF) as usize);
        if kind == K_OPEN {
            self.items.get(i).map(|it| vec![it.path.clone()])
        } else {
            None
        }
    }
}
