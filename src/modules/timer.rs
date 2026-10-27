//! Timer: a countdown with presets and fine adjust, plus a stopwatch with laps.
//! Running timers show in the collapsed notch; a finished countdown chimes and
//! pops a toast (Restart / Dismiss).

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::time::{fmt_mod, unix_now, Now};
use crate::ui::Ui;

const PRESETS: [u32; 8] = [1, 3, 5, 10, 15, 25, 45, 60];

const A_MODE_TIMER: u32 = 1;
const A_MODE_WATCH: u32 = 2;
const A_TOGGLE: u32 = 3;
const A_RESET: u32 = 4;
const A_MINUS: u32 = 5;
const A_PLUS: u32 = 6;
const A_LAP: u32 = 7;
const A_PRESET: u32 = 10; // + index
const A_T_RESTART: u32 = 20;
const A_T_DISMISS: u32 = 21;
/// Test hook used by `--selftest`: make the running countdown expire now.
pub const A_DEBUG_EXPIRE: u32 = 999;
/// `--render-docs` hooks.
pub const A_DEMO_TIMER: u32 = 998;
pub const A_DEMO_WATCH: u32 = 997;

const C_TIMER: Color = col::PINK;
const C_WATCH: Color = col::BLUE;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Countdown,
    Stopwatch,
}

pub struct Timer {
    view: View,
    // countdown
    duration: f64,
    running: bool,
    end: f64,
    left: f64,
    // stopwatch
    sw_running: bool,
    sw_start: f64,
    sw_acc: f64,
    laps: Vec<f64>,
    saved_duration: f64,
}

fn clock(s: f64) -> String {
    let s = s.max(0.0).ceil() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn watch(s: f64) -> String {
    let s = s.max(0.0);
    let whole = s as u64;
    let tenth = ((s - whole as f64) * 10.0) as u64;
    if whole >= 3600 {
        format!("{}:{:02}:{:02}", whole / 3600, (whole / 60) % 60, whole % 60)
    } else {
        format!("{:02}:{:02}.{}", whole / 60, whole % 60, tenth)
    }
}

impl Timer {
    pub fn new() -> Timer {
        let mins = crate::store::Settings::load()
            .extra
            .get("timer_secs")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(300.0)
            .clamp(10.0, 24.0 * 3600.0);
        Timer {
            view: View::Countdown,
            duration: mins,
            running: false,
            end: 0.0,
            left: mins,
            sw_running: false,
            sw_start: 0.0,
            sw_acc: 0.0,
            laps: Vec::new(),
            saved_duration: mins,
        }
    }

    fn remaining(&self) -> f64 {
        if self.running {
            (self.end - unix_now()).max(0.0)
        } else {
            self.left
        }
    }

    fn elapsed(&self) -> f64 {
        self.sw_acc + if self.sw_running { unix_now() - self.sw_start } else { 0.0 }
    }

    fn idle(&self) -> bool {
        !self.running && (self.left - self.duration).abs() < 0.01
    }

    fn set_duration(&mut self, secs: f64, ctx: &mut Ctx) {
        self.duration = secs.clamp(10.0, 24.0 * 3600.0);
        self.left = self.duration;
        self.running = false;
        if (self.duration - self.saved_duration).abs() > 0.5 {
            self.saved_duration = self.duration;
            ctx.settings.extra.insert("timer_secs".into(), format!("{}", self.duration as u64));
            ctx.settings.save();
        }
    }

    fn start(&mut self) {
        if !self.running && self.left > 0.0 {
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

    fn draw_countdown(&mut self, ui: &mut Ui, mut r: Rect) {
        let rem = self.remaining();
        let mut left = r.cut_left(170.0, 18.0);
        let big = left.cut_top(48.0, 4.0);
        ui.digits(&clock(rem), big, 36.0, col::TEXT, Align::Left);
        let frac = if self.duration > 0.0 { (1.0 - rem / self.duration) as f32 } else { 0.0 };
        ui.progress(left.cut_top(6.0, 8.0), frac, C_TIMER, None);
        let info = if self.running {
            let now = Now::get();
            let end_min = now.minute_of_day() as f64 + (rem + now.sec as f64) / 60.0;
            format!("Ends at {}", fmt_mod(end_min as u32 % 1440))
        } else if self.idle() {
            "Scroll to adjust".to_string()
        } else {
            "Paused".to_string()
        };
        ui.text(&info, left.cut_top(16.0, 0.0), 11.0, col::DIM, Weight::Regular, Align::Left);

        for row in 0..2 {
            let line = r.cut_top(24.0, 6.0);
            for (k, cell) in line.columns(4, 6.0).into_iter().enumerate() {
                let i = row * 4 + k;
                let m = PRESETS[i];
                let label = if m == 60 { "1 h".to_string() } else { format!("{m} min") };
                let sel = self.idle() && (self.duration - m as f64 * 60.0).abs() < 0.5;
                ui.chip(cell, &label, A_PRESET + i as u32, sel, C_TIMER);
            }
        }
        // Four buttons sharing the row: toggle, reset, -1 min, +1 min.
        let ctl = r.cut_top(30.0, 0.0);
        let unit = (ctl.w - 18.0) / 5.0;
        let mut row = ctl;
        let label = if self.running { "Pause" } else if self.idle() { "Start" } else { "Resume" };
        ui.button_accent(row.cut_left(unit * 1.6, 6.0), label, A_TOGGLE, C_TIMER);
        ui.button(row.cut_left(unit * 1.2, 6.0), "Reset", A_RESET);
        ui.button(row.cut_left(unit * 1.1, 6.0), "-1 min", A_MINUS);
        ui.button(row, "+1 min", A_PLUS);
    }

    fn draw_stopwatch(&mut self, ui: &mut Ui, mut r: Rect) {
        let el = self.elapsed();
        let mut left = r.cut_left(170.0, 18.0);
        let big = left.cut_top(48.0, 4.0);
        ui.digits(&watch(el), big, 36.0, col::TEXT, Align::Left);
        let state = if self.sw_running {
            "Running"
        } else if el > 0.0 {
            "Paused"
        } else {
            "Ready"
        };
        ui.text(state, left.cut_top(16.0, 0.0), 11.0, col::DIM, Weight::Regular, Align::Left);

        let mut ctl = r.cut_bottom(30.0, 8.0);
        let label = if self.sw_running { "Pause" } else if el > 0.0 { "Resume" } else { "Start" };
        ui.button_accent(ctl.cut_left(96.0, 8.0), label, A_TOGGLE, C_WATCH);
        ui.button(ctl.cut_left(64.0, 8.0), "Lap", A_LAP);
        ui.button(ctl.cut_left(70.0, 8.0), "Reset", A_RESET);

        if self.laps.is_empty() {
            ui.text("Laps show here", r, 11.0, col::FAINT, Weight::Regular, Align::Left);
            return;
        }
        let rows = (r.h / 18.0).floor().max(1.0) as usize;
        let n = self.laps.len();
        for (k, i) in (0..n).rev().take(rows).enumerate() {
            let prev = if i == 0 { 0.0 } else { self.laps[i - 1] };
            let mut line = Rect::new(r.x, r.y + k as f32 * 18.0, r.w, 18.0);
            ui.text(&format!("Lap {}", i + 1), line.cut_left(60.0, 0.0), 11.5, col::DIM, Weight::Regular, Align::Left);
            ui.text(&format!("+{}", watch(self.laps[i] - prev)), line.cut_left(90.0, 0.0), 11.5, col::TEXT, Weight::Semi, Align::Left);
            ui.text(&watch(self.laps[i]), line, 11.5, col::DIM, Weight::Regular, Align::Right);
        }
    }
}

impl Module for Timer {
    fn title(&self) -> &'static str {
        "Timer"
    }
    fn accent(&self) -> Color {
        C_TIMER
    }
    fn icon(&self) -> &'static str {
        "\u{E121}"
    }
    fn size(&self) -> (f32, f32) {
        (460.0, 128.0)
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        if self.running && self.remaining() <= 0.0 {
            self.running = false;
            self.left = self.duration;
            let mins = (self.duration / 60.0).round() as u64;
            let what = if self.duration < 60.0 {
                format!("{} second timer finished", self.duration as u64)
            } else if mins >= 60 && mins % 60 == 0 {
                format!("{} hour timer finished", mins / 60)
            } else {
                format!("{mins} minute timer finished")
            };
            ctx.toast(
                Toast::new("timer", "\u{E121}", C_TIMER, "Time's up!", what)
                    .action("Restart", A_T_RESTART)
                    .action("Dismiss", A_T_DISMISS)
                    .secs(30.0)
                    .chime(),
            );
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut head = r.cut_top(24.0, 10.0);
        ui.chip(head.cut_left(84.0, 6.0), "Timer", A_MODE_TIMER, self.view == View::Countdown, C_TIMER);
        ui.chip(head.cut_left(96.0, 6.0), "Stopwatch", A_MODE_WATCH, self.view == View::Stopwatch, C_WATCH);
        // Show the other mode's status so a running one isn't forgotten.
        let other = match self.view {
            View::Countdown if self.sw_running || self.elapsed() > 0.0 => format!("Stopwatch {}", watch(self.elapsed())),
            View::Stopwatch if self.running || !self.idle() => format!("Timer {}", clock(self.remaining())),
            _ => String::new(),
        };
        ui.text(&other, head, 11.0, col::DIM, Weight::Regular, Align::Right);
        match self.view {
            View::Countdown => self.draw_countdown(ui, r),
            View::Stopwatch => self.draw_stopwatch(ui, r),
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_MODE_TIMER => self.view = View::Countdown,
            A_MODE_WATCH => self.view = View::Stopwatch,
            A_TOGGLE => match self.view {
                View::Countdown => {
                    if self.running {
                        self.pause()
                    } else {
                        self.start()
                    }
                }
                View::Stopwatch => {
                    if self.sw_running {
                        self.sw_acc = self.elapsed();
                        self.sw_running = false;
                    } else {
                        self.sw_start = unix_now();
                        self.sw_running = true;
                    }
                }
            },
            A_RESET => match self.view {
                View::Countdown => {
                    self.running = false;
                    self.left = self.duration;
                }
                View::Stopwatch => {
                    self.sw_running = false;
                    self.sw_acc = 0.0;
                    self.laps.clear();
                }
            },
            A_LAP => {
                let e = self.elapsed();
                if e > 0.0 {
                    self.laps.push(e);
                }
            }
            A_MINUS | A_PLUS => {
                let d = if id == A_PLUS { 60.0 } else { -60.0 };
                if self.running {
                    // Adjust a running timer without restarting it.
                    self.end = (self.end + d).max(unix_now() + 1.0);
                    self.duration = (self.duration + d).max(10.0);
                } else if self.idle() {
                    self.set_duration(self.duration + d, ctx);
                } else {
                    self.left = (self.left + d).max(10.0);
                    self.duration = self.duration.max(self.left);
                }
            }
            i if (A_PRESET..A_PRESET + PRESETS.len() as u32).contains(&i) => {
                self.set_duration(PRESETS[(i - A_PRESET) as usize] as f64 * 60.0, ctx);
            }
            A_T_RESTART => {
                self.left = self.duration;
                self.start();
            }
            A_DEBUG_EXPIRE => self.end = unix_now() - 1.0,
            A_DEMO_TIMER => {
                self.view = View::Countdown;
                self.duration = 600.0;
                self.end = unix_now() + 452.0;
                self.running = true;
            }
            A_DEMO_WATCH => {
                self.view = View::Stopwatch;
                self.sw_running = false;
                self.sw_acc = 83.4;
                self.laps = vec![21.2, 44.9, 67.3];
            }
            _ => {}
        }
    }

    fn live(&self) -> Option<u8> {
        if self.running {
            Some(58)
        } else if self.sw_running {
            Some(40)
        } else {
            None
        }
    }

    fn live_color(&self) -> Color {
        if self.running {
            C_TIMER
        } else {
            C_WATCH
        }
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let (c, text, frac) = if self.running {
            let rem = self.remaining();
            (C_TIMER, clock(rem), (1.0 - rem / self.duration.max(1.0)) as f32)
        } else {
            (C_WATCH, watch(self.elapsed()), -1.0)
        };
        let ic = Rect::new(left.x, left.y, left.h, left.h);
        if frac >= 0.0 {
            let rad = (left.h / 2.0 - 3.0).min(9.0);
            ui.stroke_circle(ic.cx(), ic.cy(), rad, c.with_a(0.25), 2.5);
            ui.arc(ic.cx(), ic.cy(), rad, 0.0, std::f32::consts::TAU * frac, c, 2.5);
        } else {
            ui.icon(glyph::TIMER, ic, (left.h * 0.55).min(13.0), c);
        }
        if left.h >= 22.0 {
            let lbl = Rect::new(ic.r() + 6.0, left.y, left.w - ic.w - 6.0, left.h);
            ui.text(if self.running { "Timer" } else { "Stopwatch" }, lbl, 12.0, c, Weight::Semi, Align::Left);
        }
        ui.digits(&text, right, if left.h < 22.0 { 12.0 } else { 14.0 }, col::TEXT, Align::Right);
    }

    fn animating(&self) -> bool {
        // Tenths of a second on the stopwatch need frames between ticks.
        self.sw_running
    }

    fn debug_state(&self) -> String {
        format!(
            "countdown {} ({}), stopwatch {:.1}s running={} laps={}",
            clock(self.remaining()),
            if self.running { "running" } else { "stopped" },
            self.elapsed(),
            self.sw_running,
            self.laps.len()
        )
    }
}
