//! Template for your own module. Copy this file, rename the struct, then
//! register it in `modules::all()`.
//!
//! The pattern:
//!  * keep your state in the struct,
//!  * `draw()` paints the panel every frame and registers clickable areas
//!    (buttons do it for you) with an `id`,
//!  * `action(id, ...)` is called when one of those is clicked,
//!  * `tick()` runs every second, use it for timers/reminders,
//!  * `ctx.toast(...)` pops a notification out of the notch.

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::ui::Ui;

const BTN_ADD: u32 = 1;
const BTN_RESET: u32 = 2;

#[derive(Default)]
pub struct Example {
    count: u32,
}

impl Module for Example {
    fn title(&self) -> &'static str {
        "Example"
    }
    fn icon(&self) -> &'static str {
        glyph::ADD
    }
    fn size(&self) -> (f32, f32) {
        (400.0, 90.0)
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let label = r.cut_top(40.0, 10.0);
        ui.text(&format!("Clicked {} times", self.count), label, 20.0, col::TEXT, Weight::Semi, Align::Left);
        let mut row = r.cut_top(30.0, 0.0);
        ui.button_accent(row.cut_left(100.0, 8.0), "Click me", BTN_ADD, col::ACCENT);
        ui.button(row.cut_left(80.0, 8.0), "Reset", BTN_RESET);
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            BTN_ADD => {
                self.count += 1;
                if self.count % 10 == 0 {
                    ctx.toast(Toast::new("example", glyph::CHECK, col::GREEN, "Nice!", format!("{} clicks", self.count)).chime());
                }
            }
            BTN_RESET => self.count = 0,
            _ => {}
        }
    }
}
