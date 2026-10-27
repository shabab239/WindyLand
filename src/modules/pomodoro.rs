//! Pomodoro focus timer: 25 min focus, 5 min short break, 15 min long break
//! after every 4th focus session. Shows a countdown in the collapsed notch.

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::time::{unix_now, Now};
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Focus,
    Short,
    Long,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Phase::Focus => "Focus time",
            Phase::Short => "Short break",
            Phase::Long => "Long break",
        }
    }
    fn color(self) -> Color {
        match self {
            Phase::Focus => col::RED,
            Phase::Short => col::GREEN,
            Phase::Long => col::CYAN,
        }
    }
}

const A_TOGGLE: u32 = 1;
const A_RESET: u32 = 2;
const A_SKIP: u32 = 3;
const A_FOCUS: u32 = 10;
const A_SHORT: u32 = 11;
const A_LONG: u32 = 12;
const A_TOAST_START: u32 = 20;
const A_TOAST_LATER: u32 = 21;
/// `--render-docs` hook: a focus session part-way through.
pub const A_DEMO: u32 = 999;

pub struct Pomodoro {
    phase: Phase,
    running: bool,
    /// Unix time the running phase ends.
    end: f64,
    /// Seconds left while paused.
    left: f64,
    /// Focus sessions completed in the current cycle of 4.
    cycle: u32,
    today: u32,
    today_key: String,
    mins: [f64; 3],
}

impl Pomodoro {
    pub fn new() -> Pomodoro {
        let mut p = Pomodoro {
            phase: Phase::Focus,
            running: false,
            end: 0.0,
            left: 0.0,
            cycle: 0,
            today: 0,
            today_key: Now::get().day_key(),
            mins: [25.0, 5.0, 15.0],
        };
        p.left = p.total();
        p
    }

    fn total(&self) -> f64 {
        let i = match self.phase {
            Phase::Focus => 0,
            Phase::Short => 1,
            Phase::Long => 2,
        };
        self.mins[i] * 60.0
    }

    fn remaining(&self) -> f64 {
        if self.running {
            (self.end - unix_now()).max(0.0)
        } else {
            self.left
        }
    }

    fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.running = false;
        self.left = self.total();
    }

    fn start(&mut self) {
        if !self.running {
            self.end = unix_now() + self.left;
            self.running = true;
        }
    }

    fn pause(&mut self) {
        if self.running {
            self.left = self.remaining();
            self.running = false;
        }
    }

    fn finish(&mut self, ctx: &mut Ctx) {
        let next = match self.phase {
            Phase::Focus => {
                self.cycle += 1;
                self.today += 1;
                if self.cycle % 4 == 0 {
                    Phase::Long
                } else {
                    Phase::Short
                }
            }
            _ => Phase::Focus,
        };
        self.set_phase(next);
        let (title, sub) = match next {
            Phase::Short => ("Break Time!", format!("{} minute short break", self.mins[1] as u32)),
            Phase::Long => ("Long Break!", format!("{} min, you earned it", self.mins[2] as u32)),
            Phase::Focus => ("Focus Time!", format!("Back to work, {} min", self.mins[0] as u32)),
        };
        ctx.toast(
            Toast::new("pomodoro", glyph::TIMER, next.color(), title, sub)
                .action("Start", A_TOAST_START)
                .action("Later", A_TOAST_LATER)
                .secs(12.0)
                .chime(),
        );
    }
}

fn mmss(s: f64) -> String {
    let s = s.ceil() as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Module for Pomodoro {
    fn title(&self) -> &'static str {
        "Pomodoro"
    }
    fn accent(&self) -> Color {
        col::GREEN
    }
    fn icon(&self) -> &'static str {
        glyph::TIMER
    }
    fn size(&self) -> (f32, f32) {
        (440.0, 124.0)
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let key = ctx.now.day_key();
        if key != self.today_key {
            self.today_key = key;
            self.today = 0;
        }
        if self.running && self.remaining() <= 0.0 {
            self.finish(ctx);
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let c = self.phase.color();
        let rem = self.remaining();
        let frac = 1.0 - (rem / self.total()) as f32;

        // Ring with the countdown inside.
        let ring = r.cut_left(116.0, 18.0);
        let (cx, cy) = (ring.cx(), ring.cy());
        let rad = 50.0;
        ui.stroke_circle(cx, cy, rad, col::TEXT.with_a(0.1), 6.0);
        ui.arc(cx, cy, rad, 0.0, std::f32::consts::TAU * frac.clamp(0.0, 1.0), c, 6.0);
        ui.digits(&mmss(rem), Rect::new(cx - 50.0, cy - 16.0, 100.0, 26.0), 22.0, col::TEXT, Align::Center);
        ui.text(self.phase.label(), Rect::new(cx - 50.0, cy + 10.0, 100.0, 16.0), 10.5, col::DIM, Weight::Regular, Align::Center);

        // Phase picker.
        let chips = r.cut_top(28.0, 12.0);
        let cols = chips.columns(3, 6.0);
        ui.chip(cols[0], "Pomodoro", A_FOCUS, self.phase == Phase::Focus, Phase::Focus.color());
        ui.chip(cols[1], "Short Break", A_SHORT, self.phase == Phase::Short, Phase::Short.color());
        ui.chip(cols[2], "Long Break", A_LONG, self.phase == Phase::Long, Phase::Long.color());

        // Controls.
        let mut row = r.cut_top(32.0, 10.0);
        let label = if self.running { "Pause" } else if rem < self.total() { "Resume" } else { "Start" };
        ui.button_accent(row.cut_left(110.0, 8.0), label, A_TOGGLE, c);
        ui.button(row.cut_left(80.0, 8.0), "Reset", A_RESET);
        ui.button(row.cut_left(80.0, 8.0), "Skip", A_SKIP);

        let dots: String = (0..4).map(|i| if i < self.cycle % 4 { '●' } else { '○' }).collect();
        ui.text(
            &format!("{dots}   {} focus session{} today", self.today, if self.today == 1 { "" } else { "s" }),
            r.cut_top(18.0, 0.0),
            11.0,
            col::DIM,
            Weight::Regular,
            Align::Left,
        );
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_TOGGLE => {
                if self.running {
                    self.pause()
                } else {
                    self.start()
                }
            }
            A_RESET => self.set_phase(self.phase),
            A_SKIP => self.finish(ctx),
            A_FOCUS => self.set_phase(Phase::Focus),
            A_SHORT => self.set_phase(Phase::Short),
            A_LONG => self.set_phase(Phase::Long),
            A_TOAST_START => self.start(),
            A_DEMO => {
                self.set_phase(Phase::Focus);
                self.cycle = 2;
                self.today = 2;
                self.end = unix_now() + 18.0 * 60.0 + 24.0;
                self.running = true;
            }
            _ => {}
        }
    }

    fn live(&self) -> Option<u8> {
        if self.running {
            Some(60)
        } else {
            None
        }
    }

    fn live_color(&self) -> Color {
        self.phase.color()
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let c = self.phase.color();
        let small = left.h < 22.0;
        let ic = Rect::new(left.x, left.y, left.h, left.h);
        let rad = (left.h / 2.0 - 3.0).min(9.0);
        let frac = 1.0 - (self.remaining() / self.total()) as f32;
        ui.stroke_circle(ic.cx(), ic.cy(), rad, c.with_a(0.25), 2.5);
        ui.arc(ic.cx(), ic.cy(), rad, 0.0, std::f32::consts::TAU * frac, c, 2.5);
        if !small {
            let lbl = Rect::new(ic.r() + 6.0, left.y, left.w - ic.w - 6.0, left.h);
            let name = if self.phase == Phase::Focus { "Focus" } else { "Break" };
            ui.text(name, lbl, 12.0, c, Weight::Semi, Align::Left);
        }
        ui.digits(&mmss(self.remaining()), right, if small { 12.0 } else { 14.0 }, col::TEXT, Align::Right);
    }
}
