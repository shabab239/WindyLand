//! Water reminder: set bottle size, daily goal and active hours; WindyLand spreads
//! reminders across those hours, tells you when to refill, and charts intake.

use std::collections::BTreeMap;

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::store;
use crate::time::{unix_now, Now};
use crate::ui::Ui;

const BOTTLES: [u32; 4] = [500, 750, 1000, 1500];
const GOALS: [u32; 4] = [1500, 2000, 2500, 3000];

const A_DRINK: u32 = 1;
const A_SETTINGS: u32 = 2;
const A_BACK: u32 = 3;
const A_RESET: u32 = 4;
const A_START_DOWN: u32 = 5;
const A_START_UP: u32 = 6;
const A_END_DOWN: u32 = 7;
const A_END_UP: u32 = 8;
const A_UNDO: u32 = 9;
const A_TOGGLE_REMIND: u32 = 13;
const A_BOTTLE: u32 = 100; // + index
const A_GOAL: u32 = 110; // + index
const A_SIP: u32 = 120; // + index
const A_T_DRANK: u32 = 200;
const A_T_SNOOZE: u32 = 201;
const A_T_REFILLED: u32 = 202;

const SIPS: [u32; 3] = [150, 250, 350];

#[derive(Clone)]
struct Entry {
    day: String,
    minute: u32,
    ml: u32,
}

pub struct Water {
    bottle: u32,
    goal: u32,
    sip: u32,
    start_h: u32,
    end_h: u32,
    reminders: bool,
    log: Vec<Entry>,
    since_refill: u32,
    next_remind: f64,
    refill_warned: bool,
    settings_page: bool,
    confirm_reset: bool,
}

fn liters(ml: u32) -> String {
    let l = ml as f32 / 1000.0;
    if (l * 10.0).fract().abs() < 0.01 {
        format!("{l:.1}")
    } else {
        format!("{l:.2}")
    }
}

impl Water {
    pub fn load() -> Water {
        let text = store::read("water.txt");
        let kv = store::parse_kv(&text);
        let g = |k: &str, d: u32| kv.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let mut log = Vec::new();
        for l in text.lines() {
            if let Some(rest) = l.strip_prefix("log ") {
                let p: Vec<&str> = rest.split(',').collect();
                if p.len() == 3 {
                    if let (Ok(m), Ok(ml)) = (p[1].parse(), p[2].parse()) {
                        log.push(Entry { day: p[0].to_string(), minute: m, ml });
                    }
                }
            }
        }
        let mut w = Water {
            bottle: g("bottle", 750),
            goal: g("goal", 2000),
            sip: g("sip", 250),
            start_h: g("start", 9),
            end_h: g("end", 21),
            reminders: g("reminders", 1) == 1,
            log,
            since_refill: g("since_refill", 0),
            next_remind: 0.0,
            refill_warned: false,
            settings_page: false,
            confirm_reset: false,
        };
        w.schedule_next();
        w
    }

    fn save(&self) {
        let mut kv = BTreeMap::new();
        kv.insert("bottle".to_string(), self.bottle.to_string());
        kv.insert("goal".to_string(), self.goal.to_string());
        kv.insert("sip".to_string(), self.sip.to_string());
        kv.insert("start".to_string(), self.start_h.to_string());
        kv.insert("end".to_string(), self.end_h.to_string());
        kv.insert("reminders".to_string(), (self.reminders as u8).to_string());
        kv.insert("since_refill".to_string(), self.since_refill.to_string());
        let mut s = store::write_kv(&kv);
        // Keep two weeks of history.
        let mut days: Vec<&String> = self.log.iter().map(|e| &e.day).collect();
        days.sort();
        days.dedup();
        let keep: Vec<&String> = days.into_iter().rev().take(14).collect();
        for e in self.log.iter().filter(|e| keep.contains(&&e.day)) {
            s.push_str(&format!("log {},{},{}\n", e.day, e.minute, e.ml));
        }
        store::write("water.txt", &s);
    }

    fn today_total(&self, day: &str) -> u32 {
        self.log.iter().filter(|e| e.day == day).map(|e| e.ml).sum()
    }

    fn reminder_count(&self) -> u32 {
        (self.goal / self.sip.max(1)).max(1)
    }

    fn refill_count(&self) -> u32 {
        ((self.goal + self.bottle - 1) / self.bottle.max(1)).saturating_sub(1)
    }

    /// Minutes between reminders, spread over the active hours.
    fn interval_min(&self) -> f64 {
        let span = (self.end_h.saturating_sub(self.start_h)).max(1) as f64 * 60.0;
        (span / self.reminder_count() as f64).max(15.0)
    }

    fn schedule_next(&mut self) {
        self.next_remind = unix_now() + self.interval_min() * 60.0;
    }

    fn drink(&mut self, ml: u32) {
        let now = Now::get();
        self.log.push(Entry { day: now.day_key(), minute: now.minute_of_day(), ml });
        self.since_refill += ml;
        self.schedule_next();
        self.save();
    }

    fn undo(&mut self) {
        let day = Now::get().day_key();
        if let Some(pos) = self.log.iter().rposition(|e| e.day == day) {
            let e = self.log.remove(pos);
            self.since_refill = self.since_refill.saturating_sub(e.ml);
            self.save();
        }
    }

    fn draw_main(&mut self, ui: &mut Ui, mut r: Rect) {
        let day = Now::get().day_key();
        let total = self.today_total(&day);
        let pct = (total as f32 / self.goal.max(1) as f32 * 100.0).round() as u32;

        let mut left = r.cut_left(190.0, 18.0);
        let head = left.cut_top(36.0, 0.0);
        let big = format!("{}", liters(total));
        let bw = ui.measure(&big, 28.0, Weight::Semi);
        ui.text(&big, Rect::new(head.x, head.y, bw + 2.0, head.h), 28.0, col::CYAN, Weight::Semi, Align::Left);
        ui.text(&format!(" / {} L", liters(self.goal)), Rect::new(head.x + bw, head.y + 5.0, 100.0, head.h), 14.0, col::DIM, Weight::Regular, Align::Left);
        let status = if total >= self.goal { "Goal reached".to_string() } else { format!("{} ml to go", self.goal - total) };
        ui.text(&format!("Today · {pct}% · {status}"), left.cut_top(18.0, 8.0), 11.5, col::DIM, Weight::Regular, Align::Left);
        ui.progress(left.cut_top(6.0, 14.0), total as f32 / self.goal.max(1) as f32, col::CYAN, None);
        let mut row = left.cut_top(30.0, 0.0);
        ui.button_accent(row.cut_left(96.0, 6.0), &format!("Drink {}", self.sip), A_DRINK, col::CYAN);
        ui.icon_btn(row.cut_left(30.0, 6.0), glyph::BACK, A_UNDO, 12.0, col::DIM, Some(col::BTN), "Undo last drink");
        ui.icon_btn(row.cut_left(30.0, 0.0), glyph::SETTINGS, A_SETTINGS, 12.0, col::DIM, Some(col::BTN), "Settings");

        // Hourly chart for today.
        let hours: Vec<u32> = (self.start_h..self.end_h.max(self.start_h + 1)).collect();
        let mut per = vec![0u32; hours.len()];
        for e in self.log.iter().filter(|e| e.day == day) {
            let h = e.minute / 60;
            if let Some(i) = hours.iter().position(|&x| x == h) {
                per[i] += e.ml;
            }
        }
        let max = per.iter().copied().max().unwrap_or(0).max(400);
        let max = ((max + 199) / 200) * 200;
        let mut chart = r;
        let axis = chart.cut_left(28.0, 4.0);
        let labels = chart.cut_bottom(14.0, 4.0);
        for (k, v) in [(0.0f32, max), (0.5, max / 2), (1.0, 0)] {
            let y = chart.y + chart.h * k;
            ui.text(&v.to_string(), Rect::new(axis.x, y - 6.0, axis.w, 12.0), 9.0, col::FAINT, Weight::Regular, Align::Right);
            ui.line(chart.x, y, chart.r(), y, col::TEXT.with_a(0.06), 1.0);
        }
        let n = hours.len().max(1);
        let bw = chart.w / n as f32;
        let now_h = Now::get().hour as u32;
        for (i, &h) in hours.iter().enumerate() {
            let x = chart.x + i as f32 * bw;
            let v = per[i] as f32 / max as f32;
            let bh = (chart.h * v).max(if per[i] > 0 { 3.0 } else { 0.0 });
            let c = if h == now_h { col::CYAN } else { col::CYAN.with_a(0.55) };
            if bh > 0.0 {
                ui.fill_rr(Rect::new(x + bw * 0.18, chart.b() - bh, bw * 0.64, bh), 2.5, c);
            } else {
                ui.fill_rr(Rect::new(x + bw * 0.18, chart.b() - 2.0, bw * 0.64, 2.0), 1.0, col::TEXT.with_a(0.08));
            }
            if (h - self.start_h) % 3 == 0 {
                ui.text(&format!("{h:02}"), Rect::new(x - 6.0, labels.y, bw + 12.0, labels.h), 9.0, col::FAINT, Weight::Regular, Align::Center);
            }
        }
    }

    fn draw_settings(&mut self, ui: &mut Ui, mut r: Rect) {
        let row_h = 24.0;
        let lab_w = 62.0;

        let mut row = r.cut_top(row_h, 8.0);
        ui.text("Bottle", row.cut_left(lab_w, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        for (i, cell) in row.columns(4, 6.0).into_iter().enumerate() {
            let b = BOTTLES[i];
            let label = if b >= 1000 { format!("{} L", liters(b)) } else { format!("{b} ml") };
            ui.chip(cell, &label, A_BOTTLE + i as u32, self.bottle == b, col::CYAN);
        }

        let mut row = r.cut_top(row_h, 8.0);
        ui.text("Goal", row.cut_left(lab_w, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        for (i, cell) in row.columns(4, 6.0).into_iter().enumerate() {
            ui.chip(cell, &format!("{} L", liters(GOALS[i])), A_GOAL + i as u32, self.goal == GOALS[i], col::CYAN);
        }

        let mut row = r.cut_top(row_h, 8.0);
        ui.text("Sip", row.cut_left(lab_w, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        let sips = row.cut_left(row.w * 0.55, 10.0);
        for (i, cell) in sips.columns(3, 6.0).into_iter().enumerate() {
            ui.chip(cell, &format!("{} ml", SIPS[i]), A_SIP + i as u32, self.sip == SIPS[i], col::CYAN);
        }
        ui.chip(row, if self.reminders { "Reminders on" } else { "Reminders off" }, A_TOGGLE_REMIND, self.reminders, col::CYAN);

        let mut row = r.cut_top(row_h, 10.0);
        ui.text("Hours", row.cut_left(lab_w, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        ui.icon_btn(row.cut_left(24.0, 4.0), "\u{E738}", A_START_DOWN, 10.0, col::TEXT, Some(col::BTN), "Start an hour earlier");
        ui.text(&format!("{:02}:00", self.start_h), row.cut_left(48.0, 4.0), 13.0, col::TEXT, Weight::Semi, Align::Center);
        ui.icon_btn(row.cut_left(24.0, 10.0), glyph::ADD, A_START_UP, 10.0, col::TEXT, Some(col::BTN), "Start an hour later");
        ui.text("to", row.cut_left(20.0, 10.0), 12.0, col::DIM, Weight::Regular, Align::Center);
        ui.icon_btn(row.cut_left(24.0, 4.0), "\u{E738}", A_END_DOWN, 10.0, col::TEXT, Some(col::BTN), "End an hour earlier");
        ui.text(&format!("{:02}:00", self.end_h), row.cut_left(48.0, 4.0), 13.0, col::TEXT, Weight::Semi, Align::Center);
        ui.icon_btn(row.cut_left(24.0, 10.0), glyph::ADD, A_END_UP, 10.0, col::TEXT, Some(col::BTN), "End an hour later");

        let mut row = r.cut_top(28.0, 0.0);
        ui.button(row.cut_left(80.0, 8.0), "Done", A_BACK);
        let reset_label = if self.confirm_reset { "Sure?" } else { "Reset today" };
        let rr = row.cut_left(100.0, 12.0);
        if self.confirm_reset {
            ui.button_accent(rr, reset_label, A_RESET, col::RED);
        } else {
            ui.button(rr, reset_label, A_RESET);
        }
        ui.text(
            &format!("{} reminders · {} refills", self.reminder_count(), self.refill_count()),
            row,
            11.5,
            col::DIM,
            Weight::Regular,
            Align::Right,
        );
    }
}

impl Module for Water {
    fn title(&self) -> &'static str {
        "Water Reminder"
    }
    fn accent(&self) -> Color {
        col::CYAN
    }
    fn icon(&self) -> &'static str {
        glyph::DROP
    }
    fn size(&self) -> (f32, f32) {
        (460.0, if self.settings_page { 172.0 } else { 128.0 })
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let now = ctx.now;
        let h = now.hour as u32;
        let in_hours = h >= self.start_h && h < self.end_h;
        let total = self.today_total(&now.day_key());
        if self.reminders && in_hours && total < self.goal && now.unix >= self.next_remind && self.next_remind > 0.0 {
            self.schedule_next();
            ctx.toast(
                Toast::new("water", glyph::DROP, col::CYAN, "Time to Drink", format!("Stay hydrated! {} ml to go", self.goal - total))
                    .action("Drank", A_T_DRANK)
                    .action("Snooze", A_T_SNOOZE)
                    .secs(15.0)
                    .chime(),
            );
        }
        if self.since_refill >= self.bottle && !self.refill_warned {
            self.refill_warned = true;
            ctx.toast(
                Toast::new("water-refill", glyph::DROP, col::BLUE, "Time to Refill", "Your bottle is probably empty")
                    .action("Refilled", A_T_REFILLED)
                    .secs(15.0),
            );
        }
    }

    fn draw(&mut self, ui: &mut Ui, r: Rect) {
        if self.settings_page {
            self.draw_settings(ui, r);
        } else {
            self.draw_main(ui, r);
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, _ctx: &mut Ctx) {
        if id != A_RESET {
            self.confirm_reset = false;
        }
        match id {
            A_DRINK | A_T_DRANK => self.drink(self.sip),
            A_UNDO => self.undo(),
            A_T_SNOOZE => self.next_remind = unix_now() + 15.0 * 60.0,
            A_T_REFILLED => {
                self.since_refill = 0;
                self.refill_warned = false;
                self.save();
            }
            A_SETTINGS => self.settings_page = true,
            A_BACK => self.settings_page = false,
            A_RESET => {
                if self.confirm_reset {
                    let day = Now::get().day_key();
                    self.log.retain(|e| e.day != day);
                    self.since_refill = 0;
                    self.refill_warned = false;
                    self.confirm_reset = false;
                    self.save();
                } else {
                    self.confirm_reset = true;
                }
            }
            A_START_DOWN => self.start_h = self.start_h.saturating_sub(1),
            A_START_UP => self.start_h = (self.start_h + 1).min(self.end_h.saturating_sub(1)),
            A_END_DOWN => self.end_h = (self.end_h.saturating_sub(1)).max(self.start_h + 1),
            A_END_UP => self.end_h = (self.end_h + 1).min(24),
            A_TOGGLE_REMIND => self.reminders = !self.reminders,
            i if (A_BOTTLE..A_BOTTLE + 4).contains(&i) => self.bottle = BOTTLES[(i - A_BOTTLE) as usize],
            i if (A_GOAL..A_GOAL + 4).contains(&i) => self.goal = GOALS[(i - A_GOAL) as usize],
            i if (A_SIP..A_SIP + 3).contains(&i) => self.sip = SIPS[(i - A_SIP) as usize],
            _ => {}
        }
        if matches!(id, A_START_DOWN..=A_END_UP | A_TOGGLE_REMIND) || id >= A_BOTTLE && id < A_T_DRANK {
            self.schedule_next();
            self.save();
        }
    }
}
