//! File shelf: drop files onto the notch to keep them close, click to open,
//! drag them back out into any app. Stores references (paths), not copies.

use windows::core::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

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
}

impl Shelf {
    pub fn load() -> Shelf {
        let items = store::read("shelf.txt").lines().filter_map(|l| Item::from_path(l.trim())).collect();
        Shelf { items, drag_hover: false, scroll: 0.0 }
    }

    fn save(&self) {
        let s: String = self.items.iter().map(|i| format!("{}\n", i.path)).collect();
        store::write("shelf.txt", &s);
    }
}

impl Module for Shelf {
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
            ui.button(foot.cut_right(60.0, 0.0).inset_xy(0.0, -3.0), "Clear", A_CLEAR);
        }
        let hint = if self.drag_hover { "Release to add" } else { "Drag out anytime · click to open" };
        ui.text(hint, foot, 11.0, col::FAINT, Weight::Regular, Align::Left);

        if self.items.is_empty() || self.drag_hover {
            let c = if self.drag_hover { col::ACCENT } else { col::FAINT };
            ui.fill_rr(r, 14.0, c.with_a(if self.drag_hover { 0.12 } else { 0.05 }));
            ui.stroke_rr(r.inset(0.5), 14.0, c.with_a(0.6), 1.2);
            let msg = if self.drag_hover { "Release to add" } else { "Drop files here" };
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

    fn action(&mut self, id: u32, _rel: Rel, _ctx: &mut Ctx) {
        if id == A_CLEAR {
            self.items.clear();
            self.save();
            return;
        }
        let (kind, i) = (id >> 24, (id & 0xFF_FFFF) as usize);
        if i >= self.items.len() {
            return;
        }
        match kind {
            K_OPEN => unsafe {
                let p = HSTRING::from(self.items[i].path.as_str());
                ShellExecuteW(None, w!("open"), &p, None, None, SW_SHOWNORMAL);
            },
            K_REMOVE => {
                self.items.remove(i);
                self.save();
            }
            _ => {}
        }
    }

    fn scroll(&mut self, delta: f32) {
        self.scroll -= delta * 60.0;
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

    fn drag_out(&mut self, id: u32) -> Option<Vec<String>> {
        let (kind, i) = (id >> 24, (id & 0xFF_FFFF) as usize);
        if kind == K_OPEN {
            self.items.get(i).map(|it| vec![it.path.clone()])
        } else {
            None
        }
    }
}
