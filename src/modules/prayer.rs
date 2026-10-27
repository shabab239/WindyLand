//! Prayer times, calculated on this PC (nothing is fetched).
//!
//! The sun's position comes from the standard astronomical formulas (the same
//! ones PrayTimes.org and most apps use): Fajr and Isha when the sun is a set
//! angle below the horizon, sunrise and Maghrib at the horizon (with
//! refraction), Dhuhr at solar noon, Asr when a stick's shadow is its length
//! (Shafi) or twice it (Hanafi) plus the noon shadow. Seasons are handled by
//! the maths: the date sets the sun's declination. Times are shown in the
//! PC's time zone.
//!
//! Defaults suit Bangladesh: Dhaka, University of Islamic Sciences, Karachi
//! (Fajr and Isha at 18 degrees), Hanafi Asr. Each prayer can be nudged by a
//! few minutes to match a local mosque or calendar.

use std::collections::BTreeMap;

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::store;
use crate::time::{unix_now, Now};
use crate::ui::Ui;

pub const ACCENT: Color = hex(0x63E6BE);
const MOON: &str = "\u{E708}";

/// Fajr, Sunrise, Dhuhr, Asr, Maghrib, Isha.
pub const NAMES: [&str; 6] = ["Fajr", "Sunrise", "Dhuhr", "Asr", "Maghrib", "Isha"];

pub const CITIES: [(&str, f64, f64); 16] = [
    ("Dhaka", 23.8103, 90.4125),
    ("Chattogram", 22.3569, 91.7832),
    ("Khulna", 22.8456, 89.5403),
    ("Rajshahi", 24.3745, 88.6042),
    ("Sylhet", 24.8949, 91.8687),
    ("Barishal", 22.7010, 90.3535),
    ("Rangpur", 25.7439, 89.2752),
    ("Mymensingh", 24.7471, 90.4203),
    ("Cumilla", 23.4607, 91.1809),
    ("Gazipur", 23.9999, 90.4203),
    ("Narayanganj", 23.6238, 90.5000),
    ("Bogura", 24.8465, 89.3773),
    ("Cox's Bazar", 21.4272, 92.0058),
    ("Jashore", 23.1664, 89.2081),
    ("Dinajpur", 25.6217, 88.6355),
    ("Noakhali", 22.8696, 91.0995),
];

/// Calculation methods: (key, label, Fajr angle, Isha angle or minutes after Maghrib).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Isha {
    Angle(f64),
    Minutes(f64),
}
pub const METHODS: [(&str, &str, f64, Isha); 5] = [
    ("karachi", "Karachi", 18.0, Isha::Angle(18.0)),
    ("mwl", "MWL", 18.0, Isha::Angle(17.0)),
    ("isna", "ISNA", 15.0, Isha::Angle(15.0)),
    ("egypt", "Egypt", 19.5, Isha::Angle(17.5)),
    ("makkah", "Makkah", 18.5, Isha::Minutes(90.0)),
];
const BEFORE: [u32; 4] = [0, 5, 10, 15];

const A_SETTINGS: u32 = 1;
const A_DONE: u32 = 2;
const A_PAGE: u32 = 3; // + sub-page
const A_CITY: u32 = 10; // + index
const A_CUSTOM: u32 = 40;
const A_METHOD: u32 = 50; // + index
const A_ASR: u32 = 60; // + 0 Shafi, 1 Hanafi
const A_BEFORE: u32 = 70; // + index
const A_LIVE: u32 = 80;
const A_BELL: u32 = 90; // + prayer
const A_MINUS: u32 = 100; // + prayer
const A_PLUS: u32 = 110; // + prayer
const A_T_OK: u32 = 200;
/// Self-test: put Dhuhr at this minute (reminder due) / ten minutes away
/// (countdown in the notch), without saving; reload the saved settings.
pub const A_TEST_DUE: u32 = 990;
pub const A_TEST_SOON: u32 = 991;
pub const A_TEST_RELOAD: u32 = 992;
/// Demo video: the reminder for Asr, as if it were time now.
pub const A_DEMO: u32 = 993;

// ------------------------------------------------------------------ maths

fn dsin(d: f64) -> f64 {
    d.to_radians().sin()
}
fn dcos(d: f64) -> f64 {
    d.to_radians().cos()
}
fn dtan(d: f64) -> f64 {
    d.to_radians().tan()
}
fn darcsin(x: f64) -> f64 {
    x.asin().to_degrees()
}
fn darccos(x: f64) -> f64 {
    x.acos().to_degrees()
}
fn darctan2(y: f64, x: f64) -> f64 {
    y.atan2(x).to_degrees()
}
fn darccot(x: f64) -> f64 {
    (1.0 / x).atan().to_degrees()
}
fn fix(a: f64, b: f64) -> f64 {
    a.rem_euclid(b)
}

fn julian(y: i32, m: i32, d: i32) -> f64 {
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let a = (y as f64 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + d as f64 + b - 1524.5
}

/// Sun declination and equation of time for a Julian date.
fn sun(jd: f64) -> (f64, f64) {
    let d = jd - 2451545.0;
    let g = fix(357.529 + 0.98560028 * d, 360.0);
    let q = fix(280.459 + 0.98564736 * d, 360.0);
    let l = fix(q + 1.915 * dsin(g) + 0.020 * dsin(2.0 * g), 360.0);
    let e = 23.439 - 0.00000036 * d;
    let ra = darctan2(dcos(e) * dsin(l), dcos(l)) / 15.0;
    let eqt = q / 15.0 - fix(ra, 24.0);
    (darcsin(dsin(e) * dsin(l)), eqt)
}

/// Calculation inputs.
#[derive(Clone, Copy, Debug)]
pub struct Calc {
    pub lat: f64,
    pub lon: f64,
    /// Hours from UTC.
    pub tz: f64,
    pub fajr: f64,
    pub isha: Isha,
    /// Shadow factor: 1 Shafi, 2 Hanafi.
    pub asr: f64,
}

/// Today's six times in hours (local), or NaN where the sun never reaches
/// the angle (far north/south summers).
pub fn times(c: &Calc, y: i32, m: i32, d: i32) -> [f64; 6] {
    let jd = julian(y, m, d) - c.lon / (15.0 * 24.0);
    let mid = |t: f64| fix(12.0 - sun(jd + t).1, 24.0);
    let angle_time = |angle: f64, t: f64, ccw: bool| {
        let (decl, _) = sun(jd + t);
        let noon = mid(t);
        let x = (-dsin(angle) - dsin(decl) * dsin(c.lat)) / (dcos(decl) * dcos(c.lat));
        let h = darccos(x) / 15.0;
        noon + if ccw { -h } else { h }
    };
    let asr_time = |t: f64| {
        let (decl, _) = sun(jd + t);
        let angle = -darccot(c.asr + dtan((c.lat - decl).abs()));
        angle_time(angle, t, false)
    };
    // Two passes: the second uses the first pass's times as day fractions.
    let mut t = [5.0, 6.0, 12.0, 13.0, 18.0, 18.0];
    for _ in 0..2 {
        let f = t.map(|h| h / 24.0);
        let sunset = angle_time(0.833, f[4], false);
        t = [
            angle_time(c.fajr, f[0], true),
            angle_time(0.833, f[1], true),
            mid(f[2]),
            asr_time(f[3]),
            sunset,
            match c.isha {
                Isha::Angle(a) => angle_time(a, f[5], false),
                Isha::Minutes(min) => sunset + min / 60.0,
            },
        ];
    }
    let adj = c.tz - c.lon / 15.0;
    t.map(|h| h + adj)
}

/// "15:47" from hours, rounded to the minute.
pub fn hhmm(h: f64) -> String {
    if !h.is_finite() {
        return "--:--".into();
    }
    let m = (h * 60.0).round() as i64;
    let m = m.rem_euclid(24 * 60);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The PC's current UTC offset in hours (includes daylight saving).
pub fn local_tz() -> f64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let day = |s: &windows::Win32::Foundation::SYSTEMTIME| (s.wYear as i32, s.wMonth as i32, s.wDay as i32);
    let mins = |s: &windows::Win32::Foundation::SYSTEMTIME| s.wHour as f64 * 60.0 + s.wMinute as f64 + s.wSecond as f64 / 60.0;
    // The offset is under a day, so the dates differ by at most one.
    let dd = match day(&l).cmp(&day(&u)) {
        std::cmp::Ordering::Greater => 1440.0,
        std::cmp::Ordering::Less => -1440.0,
        std::cmp::Ordering::Equal => 0.0,
    };
    ((mins(&l) - mins(&u) + dd) / 15.0).round() * 15.0 / 60.0
}

// ------------------------------------------------------------------ module

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Main,
    Location,
    Method,
    Adjust,
}

pub struct Prayer {
    city: Option<usize>,
    lat: f64,
    lon: f64,
    method: usize,
    hanafi: bool,
    offsets: [i32; 6],
    bells: [bool; 6],
    before: u32,
    live_on: bool,
    page: Page,
    /// Custom location text being typed ("23.81, 90.41").
    editing: Option<String>,
    /// Reminders already given today: (day, key).
    fired: Vec<(String, String)>,
    /// Today's times (cached per day + settings).
    cache: Option<(String, [f64; 6])>,
}

impl Prayer {
    pub fn load() -> Prayer {
        let kv = store::parse_kv(&store::read("prayer.txt"));
        let f = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok());
        let city = match kv.get("city").map(String::as_str) {
            Some("custom") => None,
            Some(name) => Some(CITIES.iter().position(|c| c.0 == name).unwrap_or(0)),
            None => Some(0),
        };
        let (dlat, dlon) = city.map_or((CITIES[0].1, CITIES[0].2), |i| (CITIES[i].1, CITIES[i].2));
        let mut p = Prayer {
            city,
            lat: f("lat").unwrap_or(dlat),
            lon: f("lon").unwrap_or(dlon),
            method: kv.get("method").and_then(|m| METHODS.iter().position(|x| x.0 == m)).unwrap_or(0),
            hanafi: kv.get("asr").map_or(true, |v| v != "shafi"),
            offsets: [0; 6],
            bells: [false; 6],
            before: kv.get("before").and_then(|v| v.parse().ok()).filter(|b| BEFORE.contains(b)).unwrap_or(0),
            live_on: kv.get("live").map_or(false, |v| v == "1"),
            page: Page::Main,
            editing: None,
            fired: Vec::new(),
            cache: None,
        };
        for (i, n) in NAMES.iter().enumerate() {
            let k = n.to_lowercase();
            if let Some(o) = kv.get(&format!("off_{k}")).and_then(|v| v.parse().ok()) {
                p.offsets[i] = o;
            }
            if let Some(b) = kv.get(&format!("bell_{k}")) {
                p.bells[i] = b == "1";
            }
        }
        if let Some(i) = p.city {
            p.lat = CITIES[i].1;
            p.lon = CITIES[i].2;
        }
        p
    }

    fn save(&mut self) {
        self.cache = None;
        let mut kv = BTreeMap::new();
        kv.insert("city".to_string(), self.city.map_or("custom".to_string(), |i| CITIES[i].0.to_string()));
        kv.insert("lat".to_string(), format!("{:.4}", self.lat));
        kv.insert("lon".to_string(), format!("{:.4}", self.lon));
        kv.insert("method".to_string(), METHODS[self.method].0.to_string());
        kv.insert("asr".to_string(), if self.hanafi { "hanafi" } else { "shafi" }.to_string());
        kv.insert("before".to_string(), self.before.to_string());
        kv.insert("live".to_string(), (self.live_on as u8).to_string());
        for (i, n) in NAMES.iter().enumerate() {
            let k = n.to_lowercase();
            kv.insert(format!("off_{k}"), self.offsets[i].to_string());
            kv.insert(format!("bell_{k}"), (self.bells[i] as u8).to_string());
        }
        store::write("prayer.txt", &store::write_kv(&kv));
    }

    pub fn calc(&self) -> Calc {
        let (_, _, fajr, isha) = METHODS[self.method];
        Calc { lat: self.lat, lon: self.lon, tz: local_tz(), fajr, isha, asr: if self.hanafi { 2.0 } else { 1.0 } }
    }

    fn place(&self) -> String {
        match self.city {
            Some(i) => CITIES[i].0.to_string(),
            None => format!("{:.2}, {:.2}", self.lat, self.lon),
        }
    }

    /// Today's times in hours with the user's offsets applied.
    pub fn today(&mut self) -> [f64; 6] {
        let now = Now::get();
        let day = now.day_key();
        if let Some((d, t)) = &self.cache {
            if *d == day {
                return *t;
            }
        }
        let mut t = times(&self.calc(), now.year as i32, now.month as i32, now.day as i32);
        for i in 0..6 {
            t[i] += self.offsets[i] as f64 / 60.0;
        }
        self.cache = Some((day, t));
        t
    }

    /// (index of the current prayer period, index of the next prayer,
    /// minutes until it). After Isha the next is tomorrow's Fajr.
    fn status(&mut self) -> (Option<usize>, usize, f64) {
        let now = Now::get();
        let m = now.hour as f64 * 60.0 + now.min as f64 + now.sec as f64 / 60.0;
        let t = self.today();
        let mins = t.map(|h| h * 60.0);
        let next = (0..6).find(|&i| mins[i] > m);
        let current = (0..6).rev().find(|&i| mins[i] <= m && i != 1);
        match next {
            Some(n) => (current, n, mins[n] - m),
            None => (Some(5), 0, mins[0] + 24.0 * 60.0 - m),
        }
    }

    fn commit_custom(&mut self) {
        if let Some(s) = self.editing.take() {
            let nums: Vec<f64> = s.split([',', ' ']).filter_map(|x| x.trim().parse().ok()).collect();
            if nums.len() == 2 && nums[0].abs() <= 66.0 && nums[1].abs() <= 180.0 {
                self.lat = nums[0];
                self.lon = nums[1];
                self.city = None;
                self.save();
            }
        }
    }

    fn draw_main(&mut self, ui: &mut Ui, mut r: Rect) {
        let t = self.today();
        let (current, next, left) = self.status();
        let mut l = r.cut_left(196.0, 18.0);
        // Next prayer, big, with a countdown and progress through the gap.
        ui.text(&format!("Next · {}", NAMES[next]), l.cut_top(18.0, 2.0), 12.0, col::DIM, Weight::Semi, Align::Left);
        let head = l.cut_top(40.0, 2.0);
        ui.text(&hhmm(t[next]), head, 32.0, ACCENT, Weight::Semi, Align::Left);
        let (h, m) = ((left / 60.0).floor() as i64, (left % 60.0).floor() as i64);
        let count = if h > 0 { format!("in {h} h {m} min") } else { format!("in {m} min") };
        ui.text(&count, l.cut_top(18.0, 8.0), 12.5, col::TEXT, Weight::Regular, Align::Left);
        let prev = if next == 0 { t[5] - 24.0 } else { t[next - 1] };
        let span = (t[next] - prev).max(1e-6) * 60.0;
        ui.progress(l.cut_top(5.0, 14.0), (1.0 - left / span).clamp(0.0, 1.0) as f32, ACCENT, None);
        let mut row = l.cut_top(28.0, 0.0);
        ui.icon_btn(row.cut_right(28.0, 0.0), glyph::SETTINGS, A_SETTINGS, 12.0, col::DIM, Some(col::BTN), "Location and method");
        ui.text(&format!("{} · {}{}", self.place(), METHODS[self.method].1, if self.hanafi { " · Hanafi" } else { "" }), row, 10.5, col::FAINT, Weight::Regular, Align::Left);

        // The six times.
        for (i, cell) in (0..6).zip(r.rows(6, 4.0)) {
            let is_cur = current == Some(i);
            let is_next = i == next;
            if is_cur {
                ui.fill_rr(cell, 8.0, ACCENT.with_a(0.16));
            }
            let mut c = cell.inset_xy(10.0, 0.0);
            let bell = c.cut_right(22.0, 6.0);
            let color = if is_cur || is_next { col::TEXT } else { col::DIM };
            ui.text(NAMES[i], c, 12.5, color, if is_cur { Weight::Semi } else { Weight::Regular }, Align::Left);
            ui.text(&hhmm(t[i]), c, 12.5, if is_next { ACCENT } else { color }, Weight::Semi, Align::Right);
            let on = self.bells[i];
            ui.icon_btn(bell, if on { glyph::BELL } else { glyph::MUTE }, A_BELL + i as u32, 10.5, if on { ACCENT } else { col::FAINT }, None, "Reminder for this prayer");
        }
    }

    fn draw_nav(&self, ui: &mut Ui, r: &mut Rect) {
        let mut row = r.cut_top(24.0, 10.0);
        ui.button_accent(row.cut_right(64.0, 0.0), "Done", A_DONE, ACCENT);
        let pages = [(Page::Location, "Location"), (Page::Method, "Method"), (Page::Adjust, "Adjust")];
        for (k, (cell, (p, name))) in row.columns(3, 6.0).into_iter().zip(pages).enumerate() {
            ui.chip(cell.inset_xy(0.0, 0.0), name, A_PAGE + k as u32, self.page == p, ACCENT);
        }
    }

    fn draw_location(&mut self, ui: &mut Ui, mut r: Rect) {
        self.draw_nav(ui, &mut r);
        let grid = r.cut_top(4.0 * 22.0 + 3.0 * 6.0, 10.0);
        for (row_i, row) in grid.rows(4, 6.0).into_iter().enumerate() {
            for (col_i, cell) in row.columns(4, 6.0).into_iter().enumerate() {
                let i = row_i * 4 + col_i;
                ui.chip(cell, CITIES[i].0, A_CITY + i as u32, self.city == Some(i), ACCENT);
            }
        }
        let mut row = r.cut_top(24.0, 0.0);
        match &self.editing {
            Some(text) => {
                ui.button_accent(row.cut_right(64.0, 0.0), "Save", A_CUSTOM, ACCENT);
                let field = row.cut_left(row.w - 8.0, 0.0);
                ui.fill_rr(field, 6.0, col::BTN);
                let caret = if (unix_now() * 2.0) as i64 % 2 == 0 { "|" } else { " " };
                ui.text(&format!("{text}{caret}"), field.inset_xy(8.0, 0.0), 12.0, col::TEXT, Weight::Regular, Align::Left);
            }
            None => {
                ui.chip(row.cut_right(124.0, 0.0), "Custom location", A_CUSTOM, self.city.is_none(), ACCENT);
                ui.text(&format!("Latitude, longitude: {:.4}, {:.4}", self.lat, self.lon), row.inset_xy(2.0, 0.0), 11.0, col::DIM, Weight::Regular, Align::Left);
            }
        }
    }

    fn draw_method(&mut self, ui: &mut Ui, mut r: Rect) {
        self.draw_nav(ui, &mut r);
        let lab = 74.0;
        let mut row = r.cut_top(24.0, 8.0);
        ui.text("Method", row.cut_left(lab, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        for (i, cell) in row.columns(METHODS.len(), 6.0).into_iter().enumerate() {
            ui.chip(cell, METHODS[i].1, A_METHOD + i as u32, self.method == i, ACCENT);
        }
        let mut row = r.cut_top(24.0, 8.0);
        ui.text("Asr", row.cut_left(lab, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        let half = row.cut_left(row.w * 0.5, 0.0);
        for (i, (cell, name)) in half.columns(2, 6.0).into_iter().zip(["Shafi", "Hanafi"]).enumerate() {
            ui.chip(cell, name, A_ASR + i as u32, self.hanafi == (i == 1), ACCENT);
        }
        let mut row = r.cut_top(24.0, 8.0);
        ui.text("Remind", row.cut_left(lab, 0.0), 12.0, col::DIM, Weight::Regular, Align::Left);
        for (i, cell) in row.columns(BEFORE.len(), 6.0).into_iter().enumerate() {
            let label = if BEFORE[i] == 0 { "On time".to_string() } else { format!("{} min before", BEFORE[i]) };
            ui.chip(cell, &label, A_BEFORE + i as u32, self.before == BEFORE[i], ACCENT);
        }
        ui.toggle(r.cut_top(26.0, 0.0), "Countdown in the notch 15 minutes before", self.live_on, A_LIVE, ACCENT);
    }

    fn draw_adjust(&mut self, ui: &mut Ui, mut r: Rect) {
        self.draw_nav(ui, &mut r);
        let t = self.today();
        ui.text("Nudge a time to match your mosque or calendar", r.cut_top(16.0, 8.0), 11.0, col::DIM, Weight::Regular, Align::Left);
        for (row_i, row) in r.rows(3, 8.0).into_iter().enumerate() {
            for (col_i, cell) in row.columns(2, 16.0).into_iter().enumerate() {
                let i = col_i * 3 + row_i;
                let mut c = cell;
                ui.text(NAMES[i], c.cut_left(62.0, 0.0), 12.0, col::TEXT, Weight::Regular, Align::Left);
                ui.text(&hhmm(t[i]), c.cut_left(44.0, 6.0), 12.0, col::DIM, Weight::Regular, Align::Left);
                ui.icon_btn(c.cut_left(24.0, 4.0), "\u{E738}", A_MINUS + i as u32, 10.0, col::TEXT, Some(col::BTN), "A minute earlier");
                let o = self.offsets[i];
                let label = if o == 0 { "0".to_string() } else { format!("{o:+}") };
                ui.text(&label, c.cut_left(32.0, 4.0), 12.0, if o == 0 { col::DIM } else { ACCENT }, Weight::Semi, Align::Center);
                ui.icon_btn(c.cut_left(24.0, 0.0), glyph::ADD, A_PLUS + i as u32, 10.0, col::TEXT, Some(col::BTN), "A minute later");
            }
        }
    }
}

impl Module for Prayer {
    fn title(&self) -> &'static str {
        "Prayer Times"
    }
    fn icon(&self) -> &'static str {
        MOON
    }
    fn accent(&self) -> Color {
        ACCENT
    }
    fn size(&self) -> (f32, f32) {
        crate::modules::PANEL
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let t = self.today();
        let now = Now::get();
        let day = now.day_key();
        let m = now.hour as f64 * 60.0 + now.min as f64 + now.sec as f64 / 60.0;
        self.fired.retain(|(d, _)| *d == day);
        for i in 0..6 {
            if !self.bells[i] || !t[i].is_finite() {
                continue;
            }
            let at = t[i] * 60.0;
            // On time: within the first two minutes (so a PC waking from sleep
            // much later doesn't announce an old prayer).
            let mut due = vec![(at, format!("{i}"), 0u32)];
            if self.before > 0 {
                due.push((at - self.before as f64, format!("{i}-before"), self.before));
            }
            for (when, key, before) in due {
                if m >= when && m < when + 2.0 && !self.fired.iter().any(|(_, k)| *k == key) {
                    self.fired.push((day.clone(), key));
                    let (title, sub) = if before == 0 {
                        if i == 1 {
                            ("Sunrise".to_string(), format!("{} · {}", hhmm(t[i]), self.place()))
                        } else {
                            (format!("Time for {}", NAMES[i]), format!("{} · {}", hhmm(t[i]), self.place()))
                        }
                    } else {
                        (format!("{} in {before} minutes", NAMES[i]), format!("At {} · {}", hhmm(t[i]), self.place()))
                    };
                    ctx.toast(Toast::new("prayer", MOON, ACCENT, title, sub).action("OK", A_T_OK).secs(20.0).chime());
                }
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, r: Rect) {
        match self.page {
            Page::Main => self.draw_main(ui, r),
            Page::Location => self.draw_location(ui, r),
            Page::Method => self.draw_method(ui, r),
            Page::Adjust => self.draw_adjust(ui, r),
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        if self.editing.is_some() && id != A_CUSTOM {
            self.editing = None;
            ctx.want_keyboard(false);
        }
        match id {
            A_TEST_DUE | A_TEST_SOON => {
                self.offsets = [0; 6];
                self.cache = None;
                let t = self.today();
                let now = Now::get();
                let m = now.hour as f64 * 60.0 + now.min as f64 + now.sec as f64 / 60.0;
                let target = if id == A_TEST_DUE { m - 0.7 } else { m + 10.0 };
                self.offsets[2] = (target - t[2] * 60.0).round() as i32;
                self.bells = [false, false, true, false, false, false];
                self.live_on = true;
                self.fired.clear();
                self.cache = None;
            }
            A_TEST_RELOAD => *self = Prayer::load(),
            A_DEMO => {
                let sub = format!("{} · {}", Now::get().hhmm(), self.place());
                ctx.toast(Toast::new("prayer", MOON, ACCENT, "Time for Asr", sub).action("OK", A_T_OK).secs(20.0).chime());
            }
            A_SETTINGS => self.page = Page::Location,
            A_DONE => self.page = Page::Main,
            i if (A_PAGE..A_PAGE + 3).contains(&i) => self.page = [Page::Location, Page::Method, Page::Adjust][(i - A_PAGE) as usize],
            i if (A_CITY..A_CITY + CITIES.len() as u32).contains(&i) => {
                let c = (i - A_CITY) as usize;
                self.city = Some(c);
                self.lat = CITIES[c].1;
                self.lon = CITIES[c].2;
                self.save();
            }
            A_CUSTOM => {
                if self.editing.is_some() {
                    self.commit_custom();
                    ctx.want_keyboard(false);
                } else {
                    self.editing = Some(format!("{:.4}, {:.4}", self.lat, self.lon));
                    ctx.want_keyboard(true);
                }
            }
            i if (A_METHOD..A_METHOD + METHODS.len() as u32).contains(&i) => {
                self.method = (i - A_METHOD) as usize;
                self.save();
            }
            i if (A_ASR..A_ASR + 2).contains(&i) => {
                self.hanafi = i == A_ASR + 1;
                self.save();
            }
            i if (A_BEFORE..A_BEFORE + BEFORE.len() as u32).contains(&i) => {
                self.before = BEFORE[(i - A_BEFORE) as usize];
                self.save();
            }
            A_LIVE => {
                self.live_on = !self.live_on;
                self.save();
            }
            i if (A_BELL..A_BELL + 6).contains(&i) => {
                let k = (i - A_BELL) as usize;
                self.bells[k] = !self.bells[k];
                self.save();
            }
            i if (A_MINUS..A_MINUS + 6).contains(&i) => {
                let k = (i - A_MINUS) as usize;
                self.offsets[k] = (self.offsets[k] - 1).max(-30);
                self.save();
            }
            i if (A_PLUS..A_PLUS + 6).contains(&i) => {
                let k = (i - A_PLUS) as usize;
                self.offsets[k] = (self.offsets[k] + 1).min(30);
                self.save();
            }
            _ => {}
        }
    }

    fn key(&mut self, vk: u16, _ctrl: bool, ctx: &mut Ctx) -> bool {
        let Some(text) = &mut self.editing else { return false };
        match vk {
            0x0D => {
                self.commit_custom();
                ctx.want_keyboard(false);
            }
            0x1B => {
                self.editing = None;
                ctx.want_keyboard(false);
            }
            0x08 => {
                text.pop();
            }
            _ => return false,
        }
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        let Some(text) = &mut self.editing else { return false };
        if (c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | ' ')) && text.len() < 32 {
            text.push(c);
            return true;
        }
        false
    }

    fn blur(&mut self) {
        self.editing = None;
    }

    fn live(&self) -> Option<u8> {
        if !self.live_on {
            return None;
        }
        let mut me = Prayer { cache: self.cache.clone(), fired: Vec::new(), editing: None, page: Page::Main, ..*self };
        let (_, next, left) = me.status();
        (next != 1 && left <= 15.0).then_some(55)
    }

    fn live_color(&self) -> Color {
        ACCENT
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let (_, next, mins) = self.status();
        ui.icon(MOON, Rect::new(left.x, left.y, left.h, left.h), 13.0, ACCENT);
        let name = Rect::new(left.x + left.h + 2.0, left.y, (left.w - left.h - 2.0).max(0.0), left.h);
        ui.text(NAMES[next], name, 12.0, col::TEXT, Weight::Semi, Align::Left);
        let secs = (mins * 60.0).max(0.0) as i64;
        ui.digits(&format!("{}:{:02}", secs / 60, secs % 60), right, 13.0, ACCENT, Align::Right);
    }

    fn animating(&self) -> bool {
        // The countdown in the closed notch ticks every second.
        self.editing.is_some() || self.live().is_some()
    }

    fn debug_state(&self) -> String {
        let mut me = Prayer { cache: self.cache.clone(), fired: Vec::new(), editing: None, page: Page::Main, ..*self };
        let t = me.today();
        format!("{} ({:.4}, {:.4}) {} {} tz {:+} : {}", me.place(), self.lat, self.lon, METHODS[self.method].1, if self.hanafi { "Hanafi" } else { "Shafi" }, local_tz(), t.iter().map(|h| hhmm(*h)).collect::<Vec<_>>().join(" "))
    }
}
