//! Calendar: the month at a glance. Today is highlighted, days with to-do
//! tasks due get a dot, and the chosen day's tasks show beside the grid.
//! Arrows (or the mouse wheel) change the month; Today jumps back. The week
//! starts on the day Windows' region settings say.

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel};
use crate::time::{days_in_month, local_time, unix_now, weekday, Now, DAYS};
use crate::ui::Ui;

pub const ACCENT: Color = hex(0xFF5E57);
pub const ICON: &str = "\u{E787}";
/// White that themes leave alone (today's number on its red circle).
const ON_ACCENT: Color = hex(0xFFFFFE);
const MONTH_NAMES: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const A_PREV: u32 = 1;
const A_NEXT: u32 = 2;
const A_TODAY: u32 = 3;
/// + cell index (0..42) in the grid.
const A_DAY: u32 = 100;

/// A task with a due date: (year, month, day, title, done, minute of day).
type Due = (i32, u32, u32, String, bool, u32);

pub struct Calendar {
    year: i32,
    month: u32,
    sel: (i32, u32, u32),
    due: Vec<Due>,
    due_at: f64,
    /// First day of the week, 0 = Sunday.
    first: u32,
}

/// Windows' first day of the week (region settings), 0 = Sunday.
fn first_day_of_week() -> u32 {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    let mut buf = [0u16; 4];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    // Windows counts 0 = Monday ... 6 = Sunday.
    if n > 0 && (b'0' as u16..=b'6' as u16).contains(&buf[0]) {
        ((buf[0] - b'0' as u16) as u32 + 1) % 7
    } else {
        1
    }
}

/// The 42 days shown for a month: (year, month, day), starting on `first`.
pub fn month_grid(y: i32, m: u32, first: u32) -> Vec<(i32, u32, u32)> {
    let lead = (weekday(y, m, 1) + 7 - first) % 7;
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let pdays = days_in_month(py, pm);
    let mut out = Vec::with_capacity(42);
    for k in 0..lead {
        out.push((py, pm, pdays - lead + 1 + k));
    }
    for d in 1..=days_in_month(y, m) {
        out.push((y, m, d));
    }
    let mut d = 1;
    while out.len() < 42 {
        out.push((ny, nm, d));
        d += 1;
    }
    out
}

/// Days from `a` to `b` (both local dates).
fn days_between(a: (i32, u32, u32), b: (i32, u32, u32)) -> i64 {
    let n = |(y, m, d): (i32, u32, u32)| {
        // Days since 1970-01-01 (civil calendar).
        let (y, m) = if m <= 2 { (y as i64 - 1, m as i64 + 9) } else { (y as i64, m as i64 - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    };
    n(b) - n(a)
}

impl Calendar {
    pub fn new() -> Calendar {
        let now = Now::get();
        Calendar { year: now.year as i32, month: now.month as u32, sel: (now.year as i32, now.month as u32, now.day as u32), due: Vec::new(), due_at: 0.0, first: first_day_of_week() }
    }

    fn today() -> (i32, u32, u32) {
        let n = Now::get();
        (n.year as i32, n.month as u32, n.day as u32)
    }

    /// To-do tasks with a due date, read again at most every 20 seconds.
    fn refresh_due(&mut self) {
        if unix_now() - self.due_at < 20.0 {
            return;
        }
        self.due_at = unix_now();
        self.due = crate::modules::todo::due_tasks()
            .into_iter()
            .map(|(due, title, done)| {
                let (y, m, d, mins) = local_time(due);
                (y, m, d, title, done, mins)
            })
            .collect();
    }

    fn step(&mut self, by: i32) {
        let k = self.year * 12 + self.month as i32 - 1 + by;
        self.year = k.div_euclid(12);
        self.month = (k.rem_euclid(12) + 1) as u32;
    }

    fn tasks_on(&self, day: (i32, u32, u32)) -> Vec<&Due> {
        let mut v: Vec<&Due> = self.due.iter().filter(|t| (t.0, t.1, t.2) == day).collect();
        v.sort_by_key(|t| t.5);
        v
    }
}

impl Module for Calendar {
    fn title(&self) -> &'static str {
        "Calendar"
    }
    fn icon(&self) -> &'static str {
        ICON
    }
    fn accent(&self) -> Color {
        ACCENT
    }
    fn size(&self) -> (f32, f32) {
        crate::modules::PANEL
    }

    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_PREV => "Previous month",
            A_NEXT => "Next month",
            A_TODAY => "Back to today",
            i if (A_DAY..A_DAY + 42).contains(&i) => "See this day",
            _ => "",
        }
    }

    fn draw(&mut self, ui: &mut Ui, r: Rect) {
        self.refresh_due();
        let today = Self::today();
        let mut r = r;
        // The month grid on the left, the chosen day on the right.
        let side = r.cut_right((r.w * 0.36).max(150.0), 14.0);
        let mut grid = r;
        let mut head = grid.cut_top(24.0, 4.0);
        let next = head.cut_right(24.0, 0.0);
        let prev = head.cut_right(24.0, 4.0);
        ui.icon_btn(prev, "\u{E76B}", A_PREV, 11.0, col::DIM, None, "Previous month");
        ui.icon_btn(next, "\u{E76C}", A_NEXT, 11.0, col::DIM, None, "Next month");
        if (self.year, self.month) != (today.0, today.1) {
            ui.chip(head.cut_right(54.0, 6.0), "Today", A_TODAY, false, ACCENT);
        }
        let title = format!("{} {}", MONTH_NAMES[(self.month - 1) as usize], self.year);
        ui.text(&title, head, 14.0, col::TEXT, Weight::Semi, Align::Left);
        let wk = grid.cut_top(14.0, 2.0);
        let cw = wk.w / 7.0;
        for c in 0..7 {
            let dow = (self.first + c) % 7;
            let cell = Rect::new(wk.x + c as f32 * cw, wk.y, cw, wk.h);
            ui.text(&DAYS[dow as usize][..2], cell, 10.0, if dow == 0 || dow == 6 { col::FAINT } else { col::DIM }, Weight::Semi, Align::Center);
        }
        let days = month_grid(self.year, self.month, self.first);
        let ch = grid.h / 6.0;
        for (k, &(y, m, d)) in days.iter().enumerate() {
            let cell = Rect::new(grid.x + (k % 7) as f32 * cw, grid.y + (k / 7) as f32 * ch, cw, ch);
            let this_month = m == self.month;
            let dow = (self.first + (k % 7) as u32) % 7;
            let is_today = (y, m, d) == today;
            let is_sel = (y, m, d) == self.sel;
            let dot = cell.centered(ch.min(cw) - 2.0, ch.min(cw) - 2.0);
            if is_today {
                ui.circle(dot.cx(), dot.cy(), dot.w / 2.0, ACCENT);
            } else if is_sel {
                ui.stroke_rr(dot, dot.w / 2.0, ACCENT.with_a(0.8), 1.3);
            } else if ui.hovered(cell) {
                ui.circle(dot.cx(), dot.cy(), dot.w / 2.0, col::BTN_HOVER);
            }
            let fg = if is_today {
                ON_ACCENT
            } else if !this_month {
                col::FAINT
            } else if dow == 0 || dow == 6 {
                col::DIM
            } else {
                col::TEXT
            };
            ui.text(&d.to_string(), cell, 11.5, fg, if is_today { Weight::Bold } else { Weight::Semi }, Align::Center);
            // Tasks due that day: a small dot under the number.
            let tasks = self.tasks_on((y, m, d));
            if !tasks.is_empty() {
                let open = tasks.iter().any(|t| !t.4);
                let c = if is_today { ON_ACCENT } else if open { col::YELLOW } else { col::FAINT };
                ui.circle(cell.cx(), dot.b() - 2.5, 1.8, c);
            }
            ui.hit(cell, A_DAY + k as u32);
        }

        // The chosen day.
        ui.fill_rr(side, 12.0, col::BTN);
        let mut s = side.inset_xy(12.0, 10.0);
        let (y, m, d) = self.sel;
        let dow = weekday(y, m, d) as usize;
        let rel = match days_between(today, self.sel) {
            0 => "Today".to_string(),
            1 => "Tomorrow".to_string(),
            -1 => "Yesterday".to_string(),
            n if n > 1 => format!("In {n} days"),
            n => format!("{} days ago", -n),
        };
        ui.text(&rel, s.cut_top(14.0, 2.0), 10.5, ACCENT, Weight::Semi, Align::Left);
        let mut big = s.cut_top(34.0, 2.0);
        let num = big.cut_left(ui.measure(&d.to_string(), 28.0, Weight::Bold) + 6.0, 4.0);
        ui.text(&d.to_string(), num, 28.0, col::TEXT, Weight::Bold, Align::Left);
        let mut words = big;
        ui.text(["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][dow], words.cut_top(17.0, 0.0), 12.0, col::TEXT, Weight::Semi, Align::Left);
        ui.text(&format!("{} {y}", MONTH_NAMES[(m - 1) as usize]), words, 11.0, col::DIM, Weight::Regular, Align::Left);
        let tasks = self.tasks_on(self.sel);
        if tasks.is_empty() {
            ui.text("No tasks due", s.cut_top(16.0, 0.0), 11.0, col::FAINT, Weight::Regular, Align::Left);
        } else {
            let fits = ((s.h + 2.0) / 18.0).floor().max(1.0) as usize;
            for (k, t) in tasks.iter().enumerate() {
                if k + 1 == fits && tasks.len() > fits {
                    ui.text(&format!("+{} more", tasks.len() - k), s.cut_top(16.0, 2.0), 10.5, col::DIM, Weight::Regular, Align::Left);
                    break;
                }
                let mut row = s.cut_top(16.0, 2.0);
                let time = crate::time::fmt_mod(t.5);
                let tw = ui.measure(&time, 10.5, Weight::Regular) + 6.0;
                ui.text(&time, row.cut_right(tw, 0.0), 10.5, col::DIM, Weight::Regular, Align::Right);
                ui.circle(row.x + 3.0, row.cy(), 2.5, if t.4 { col::FAINT } else { col::YELLOW });
                row.cut_left(10.0, 0.0);
                ui.text(&t.3, row, 11.0, if t.4 { col::FAINT } else { col::TEXT }, Weight::Regular, Align::Left);
            }
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, _ctx: &mut Ctx) {
        match id {
            A_PREV => self.step(-1),
            A_NEXT => self.step(1),
            A_TODAY => {
                let t = Self::today();
                self.year = t.0;
                self.month = t.1;
                self.sel = t;
            }
            i if (A_DAY..A_DAY + 42).contains(&i) => {
                let days = month_grid(self.year, self.month, self.first);
                let day = days[(i - A_DAY) as usize];
                self.sel = day;
                // A day of the month before or after: go there.
                if day.1 != self.month {
                    self.year = day.0;
                    self.month = day.1;
                }
            }
            _ => {}
        }
    }

    fn scroll(&mut self, delta: f32) -> bool {
        self.step(if delta > 0.0 { -1 } else { 1 });
        true
    }

    fn debug_state(&self) -> String {
        format!("{} {} sel {:?}, first day {}, {} tasks with a due date", MONTH_NAMES[(self.month - 1) as usize], self.year, self.sel, DAYS[self.first as usize], self.due.len())
    }
}
