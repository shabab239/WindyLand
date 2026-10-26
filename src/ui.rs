//! Immediate-mode UI helpers on top of `Gfx`.
//!
//! Modules draw every frame and register clickable areas with `ui.hit(rect, id)`
//! (buttons do this for you). When the user clicks, the app looks up the hit
//! under the cursor and calls `Module::action(id, ...)` on the owning module.

use std::ops::{Deref, DerefMut};

use crate::gfx::*;

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub module: usize,
    pub id: u32,
}

pub struct Ui<'a> {
    pub g: &'a mut Gfx,
    pub hits: &'a mut Vec<Hit>,
    /// Index of the module currently drawing (stamped on registered hits).
    pub module: usize,
    /// Mouse position in DIPs (or far away when outside the window).
    pub mouse: (f32, f32),
    /// Seconds since app start, for animations.
    pub t: f64,
}

impl<'a> Deref for Ui<'a> {
    type Target = Gfx;
    fn deref(&self) -> &Gfx {
        self.g
    }
}
impl<'a> DerefMut for Ui<'a> {
    fn deref_mut(&mut self) -> &mut Gfx {
        self.g
    }
}

impl<'a> Ui<'a> {
    pub fn hit(&mut self, r: Rect, id: u32) {
        self.hits.push(Hit { rect: r, module: self.module, id });
    }

    pub fn hovered(&self, r: Rect) -> bool {
        r.contains(self.mouse.0, self.mouse.1)
    }

    /// Pill button with a text label.
    pub fn button(&mut self, r: Rect, label: &str, id: u32) {
        self.button_c(r, label, id, col::BTN, col::TEXT);
    }

    /// Accent (filled) button.
    pub fn button_accent(&mut self, r: Rect, label: &str, id: u32, accent: Color) {
        let bg = if self.hovered(r) { accent.with_a(0.85) } else { accent };
        self.fill_rr(r, r.h / 2.0, bg);
        self.text(label, r, 12.0, hex(0x000000), Weight::Semi, Align::Center);
        self.hit(r, id);
    }

    pub fn button_c(&mut self, r: Rect, label: &str, id: u32, bg: Color, fg: Color) {
        let bg = if self.hovered(r) { col::BTN_HOVER } else { bg };
        self.fill_rr(r, r.h / 2.0, bg);
        self.text(label, r, 12.0, fg, Weight::Semi, Align::Center);
        self.hit(r, id);
    }

    /// Selectable chip (segmented control item).
    pub fn chip(&mut self, r: Rect, label: &str, id: u32, selected: bool, accent: Color) {
        let bg = if selected {
            accent.with_a(0.25)
        } else if self.hovered(r) {
            col::BTN_HOVER
        } else {
            col::BTN
        };
        self.fill_rr(r, r.h / 2.0, bg);
        let fg = if selected { accent } else { col::DIM };
        self.text(label, r, 11.5, fg, Weight::Semi, Align::Center);
        self.hit(r, id);
    }

    /// Round icon button. `bg = None` draws only a hover circle.
    pub fn icon_btn(&mut self, r: Rect, g: &str, id: u32, size: f32, fg: Color, bg: Option<Color>) {
        let hov = self.hovered(r);
        let rad = r.w.min(r.h) / 2.0;
        match bg {
            Some(b) => self.fill_rr(r, rad, if hov { b.lerp(col::TEXT, 0.15) } else { b }),
            None if hov => self.fill_rr(r, rad, col::BTN_HOVER),
            None => {}
        }
        self.icon(g, r, size, fg);
        self.hit(r, id);
    }

    /// Thin progress bar. Registers a hit if `id` is given (for seek bars:
    /// `Module::action` receives the click's relative x in `rel.0`).
    pub fn progress(&mut self, r: Rect, frac: f32, fg: Color, id: Option<u32>) {
        let hov = id.is_some() && self.hovered(r.inset_xy(0.0, -6.0));
        let h = if hov { r.h + 2.0 } else { r.h };
        let rr = Rect::new(r.x, r.cy() - h / 2.0, r.w, h);
        self.fill_rr(rr, h / 2.0, col::TEXT.with_a(0.16));
        let f = frac.clamp(0.0, 1.0);
        if f > 0.0 {
            self.fill_rr(Rect::new(rr.x, rr.y, (rr.w * f).max(h), rr.h), h / 2.0, fg);
        }
        if let Some(id) = id {
            self.hit(r.inset_xy(0.0, -6.0), id);
        }
    }

    /// Circular checkbox.
    pub fn check(&mut self, cx: f32, cy: f32, on: bool, accent: Color) {
        if on {
            self.circle(cx, cy, 8.0, accent);
            self.icon(crate::gfx::glyph::CHECK, Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0), 9.0, hex(0x000000));
        } else {
            self.stroke_circle(cx, cy, 7.5, col::DIM, 1.4);
        }
    }
}
