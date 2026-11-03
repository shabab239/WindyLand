//! Tools tab: colour picker, calculator, QR code, and a unit / time-zone
//! converter. Everything runs on this PC; nothing goes online.
//!
//! - Colour picker: "Pick from screen" freezes a screenshot of every monitor
//!   in a full-screen window with a magnifier under the cursor; click to pick
//!   (arrow keys nudge a pixel, Esc or right-click cancels). The colour is
//!   copied as HEX and kept in a short history.
//! - Calculator: type a sum and see the answer as you type. Understands
//!   `1250*15%`, `200+10%`, `2^10`, `sqrt(2)`, `sin(30)` (degrees), `5!`,
//!   `0xFF`, `0b1010`, `pi`, and implicit multiplication like `2(3+4)`.
//! - QR code: what you copied (or typed) as a QR code, to get a link onto
//!   your phone; "Copy image" puts the code on the clipboard as a picture.
//! - Converter: length, weight, temperature, area, volume, speed and data,
//!   including Bangladesh's katha, bigha, decimal, maund, seer and tola, and
//!   world time zones (Windows' own time-zone rules, so daylight saving is
//!   right).

use std::collections::BTreeMap;
use std::sync::Arc;

use windows::Win32::Foundation::HWND;

use crate::gfx::*;
use crate::modules::clipboard::{self, Content};
use crate::modules::{Ctx, Module, Rel};
use crate::store;
use crate::time::unix_now;
use crate::ui::Ui;

pub const ACCENT: Color = hex(0xB28DFF);
const ICON: &str = "\u{E90F}";
const G_COLOR: &str = "\u{E790}";
const G_CALC: &str = "\u{E8EF}";
const G_QR: &str = "\u{ED14}";
const G_CONVERT: &str = "\u{E8AB}";
const G_COPY: &str = glyph::COPY;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tool {
    Color,
    Calc,
    Qr,
    Convert,
}
const TOOLS: [(Tool, &str, &str, &str); 4] = [
    (Tool::Color, G_COLOR, "Color", "color"),
    (Tool::Calc, G_CALC, "Calculator", "calc"),
    (Tool::Qr, G_QR, "QR code", "qr"),
    (Tool::Convert, G_CONVERT, "Converter", "convert"),
];

// Hit ids.
const A_TOOL: u32 = 1; // + index
const A_PICK: u32 = 10;
const A_COPY_HEX: u32 = 11;
const A_COPY_RGB: u32 = 12;
const A_COPY_HSL: u32 = 13;
const A_SWATCH: u32 = 20; // + index
const A_CALC_FIELD: u32 = 40;
const A_CALC_COPY: u32 = 41;
const A_CALC_HIST: u32 = 50; // + index
const A_QR_CLIP: u32 = 60;
const A_QR_TYPE: u32 = 61;
const A_QR_COPY: u32 = 62;
const A_QR_FIELD: u32 = 63;
const A_CAT: u32 = 70; // + index
const A_FROM: u32 = 80; // + index
const A_TO: u32 = 90; // + index
const A_SWAP: u32 = 100;
const A_VALUE: u32 = 101;
const A_CONV_COPY: u32 = 102;
const A_TZ_CITY: u32 = 110; // + index
const A_TZ_LOCAL: u32 = 125;
const A_TZ_MINUS: u32 = 126;
const A_TZ_NOW: u32 = 127;
const A_TZ_PLUS: u32 = 128;
/// Demo video / docs renders: put each tool in a showable state.
pub const A_DEMO: u32 = 990;
/// Self-test: say whether "Pick from screen" would start (without opening
/// the full-screen picker on the user's PC).
pub const A_TEST_NO_OVERLAY: u32 = 991;

const MAX_COLORS: usize = 8;

// ------------------------------------------------------------ colour maths

pub fn hex_of(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}
pub fn rgb_of(c: [u8; 3]) -> String {
    format!("rgb({}, {}, {})", c[0], c[1], c[2])
}
pub fn hsl_of(c: [u8; 3]) -> String {
    let (r, g, b) = (c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    let (h, s) = if d < 1e-9 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
        let h = if mx == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    format!("hsl({:.0}, {:.0}%, {:.0}%)", h, s * 100.0, l * 100.0)
}
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}
/// WCAG contrast ratio between two colours.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let lum = |c: [u8; 3]| {
        let ch = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}
fn color_of(c: [u8; 3]) -> Color {
    Color { r: c[0] as f32 / 255.0, g: c[1] as f32 / 255.0, b: c[2] as f32 / 255.0, a: 1.0 }
}

// ------------------------------------------------------------- calculator

/// Evaluate a typed sum with our own small arithmetic parser (numbers,
/// operators and a fixed list of maths functions; it runs no code).
/// Errors are short, human messages.
pub fn eval(src: &str) -> Result<f64, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err(String::new());
    }
    let mut p = Parser { t: toks, i: 0 };
    let v = p.expr()?;
    if p.i < p.t.len() {
        return Err("Check the sum".into());
    }
    if !v.v.is_finite() {
        return Err("Not a number".into());
    }
    Ok(v.v)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(char),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let s: Vec<char> = src
        .chars()
        .map(|c| match c {
            '×' | '·' => '*',
            '÷' => '/',
            '\u{2212}' | '\u{2013}' => '-',
            c => c,
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c.is_whitespace() || c == ',' || c == '_' {
            i += 1;
            continue;
        }
        if c == '0' && i + 1 < s.len() && matches!(s[i + 1], 'x' | 'X' | 'b' | 'B') && i + 2 < s.len() && s[i + 2].is_ascii_hexdigit() {
            let radix = if matches!(s[i + 1], 'x' | 'X') { 16 } else { 2 };
            let mut j = i + 2;
            while j < s.len() && s[j].is_digit(radix) {
                j += 1;
            }
            let digits: String = s[i + 2..j].iter().collect();
            let v = u64::from_str_radix(&digits, radix).map_err(|_| "Number too big".to_string())?;
            out.push(Tok::Num(v as f64));
            i = j;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < s.len() && s[i + 1].is_ascii_digit()) {
            let mut j = i;
            // Digits with thousands commas: "1,250".
            while j < s.len() && (s[j].is_ascii_digit() || s[j] == '.' || (s[j] == ',' && j + 1 < s.len() && s[j + 1].is_ascii_digit())) {
                j += 1;
            }
            // Exponent: 2e3, 1.5e-4 (but "2e" alone is 2 times e).
            if j < s.len() && matches!(s[j], 'e' | 'E') {
                let k = if j + 1 < s.len() && matches!(s[j + 1], '+' | '-') { j + 2 } else { j + 1 };
                if k < s.len() && s[k].is_ascii_digit() {
                    j = k;
                    while j < s.len() && s[j].is_ascii_digit() {
                        j += 1;
                    }
                }
            }
            let text: String = s[i..j].iter().filter(|c| **c != ',').collect();
            out.push(Tok::Num(text.parse().map_err(|_| "Check the number".to_string())?));
            i = j;
            continue;
        }
        if matches!(c, 'x' | 'X') && matches!(out.last(), Some(Tok::Num(_)) | Some(Tok::Op(')'))) {
            out.push(Tok::Op('*'));
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == 'π' {
            let mut j = i;
            while j < s.len() && (s[j].is_alphanumeric() || s[j] == 'π') {
                j += 1;
            }
            let id: String = s[i..j].iter().collect::<String>().to_lowercase();
            // "3x4" and "3 x 4" mean times.
            if id == "x" {
                out.push(Tok::Op('*'));
            } else {
                out.push(Tok::Id(id));
            }
            i = j;
            continue;
        }
        if "+-*/^%()!".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
            continue;
        }
        return Err(format!("Unknown \"{c}\""));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Val {
    v: f64,
    /// Written as a percentage (`10%`): `a + 10%` adds 10 % of a.
    pct: bool,
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn op(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Op(c))
    }
    fn expr(&mut self) -> Result<Val, String> {
        let mut a = self.term()?;
        loop {
            let sign = if self.op('+') {
                1.0
            } else if self.op('-') {
                -1.0
            } else {
                break;
            };
            self.i += 1;
            let b = self.term()?;
            let bv = if b.pct { a.v * b.v } else { b.v };
            a = Val { v: a.v + sign * bv, pct: false };
        }
        Ok(a)
    }
    fn term(&mut self) -> Result<Val, String> {
        let mut a = self.unary()?;
        loop {
            let implicit = matches!(self.peek(), Some(Tok::Op('(')) | Some(Tok::Id(_)) | Some(Tok::Num(_)));
            if self.op('*') || self.op('/') {
                let div = self.op('/');
                self.i += 1;
                let b = self.unary()?;
                if div {
                    if b.v == 0.0 {
                        return Err("Can't divide by zero".into());
                    }
                    a = Val { v: a.v / b.v, pct: false };
                } else {
                    a = Val { v: a.v * b.v, pct: false };
                }
            } else if implicit {
                let b = self.unary()?;
                a = Val { v: a.v * b.v, pct: false };
            } else {
                break;
            }
        }
        Ok(a)
    }
    fn unary(&mut self) -> Result<Val, String> {
        if self.op('-') {
            self.i += 1;
            let v = self.unary()?;
            return Ok(Val { v: -v.v, pct: v.pct });
        }
        if self.op('+') {
            self.i += 1;
            return self.unary();
        }
        self.power()
    }
    fn power(&mut self) -> Result<Val, String> {
        let base = self.postfix()?;
        if self.op('^') {
            self.i += 1;
            let e = self.unary()?;
            return Ok(Val { v: base.v.powf(e.v), pct: false });
        }
        Ok(base)
    }
    fn postfix(&mut self) -> Result<Val, String> {
        let mut v = self.primary()?;
        loop {
            if self.op('%') {
                self.i += 1;
                v = Val { v: v.v / 100.0, pct: true };
            } else if self.op('!') {
                self.i += 1;
                if v.v < 0.0 || v.v.fract() != 0.0 || v.v > 170.0 {
                    return Err("! needs a whole number up to 170".into());
                }
                v = Val { v: (1..=v.v as u64).map(|k| k as f64).product(), pct: false };
            } else {
                break;
            }
        }
        Ok(v)
    }
    fn primary(&mut self) -> Result<Val, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Ok(Val { v: n, pct: false })
            }
            Some(Tok::Op('(')) => {
                self.i += 1;
                let v = self.expr()?;
                // A missing ")" at the very end is fine (still typing).
                if self.op(')') {
                    self.i += 1;
                } else if self.i < self.t.len() {
                    return Err("Missing )".into());
                }
                Ok(Val { v: v.v, pct: false })
            }
            Some(Tok::Id(id)) => {
                self.i += 1;
                let c = match id.as_str() {
                    "pi" | "π" => Some(std::f64::consts::PI),
                    "e" => Some(std::f64::consts::E),
                    _ => None,
                };
                if let Some(c) = c {
                    return Ok(Val { v: c, pct: false });
                }
                let f: fn(f64) -> f64 = match id.as_str() {
                    "sqrt" => f64::sqrt,
                    "sin" => |x| x.to_radians().sin(),
                    "cos" => |x| x.to_radians().cos(),
                    "tan" => |x| x.to_radians().tan(),
                    "asin" => |x| x.asin().to_degrees(),
                    "acos" => |x| x.acos().to_degrees(),
                    "atan" => |x| x.atan().to_degrees(),
                    "log" => f64::log10,
                    "ln" => f64::ln,
                    "abs" => f64::abs,
                    "round" => f64::round,
                    "floor" => f64::floor,
                    "ceil" => f64::ceil,
                    _ => return Err(format!("Unknown \"{id}\"")),
                };
                // sqrt(2) or sqrt 2
                let arg = self.postfix()?;
                let v = f(arg.v);
                // sin(30) is 0.49999999999999994: tidy tiny rounding noise.
                let v = if (v - v.round()).abs() < 1e-12 { v.round() } else { (v * 1e12).round() / 1e12 };
                Ok(Val { v, pct: false })
            }
            _ => Err(String::new()),
        }
    }
}

/// 1,234,567.89 with up to 10 significant digits; scientific when huge/tiny.
pub fn fmt_num(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-6..1e15).contains(&a) {
        return format!("{v:.6e}");
    }
    let digits = (10 - a.log10().floor() as i32 - 1).clamp(0, 10) as usize;
    let s = format!("{:.*}", digits, v);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    let (neg, s) = if let Some(r) = s.strip_prefix('-') { (true, r.to_string()) } else { (false, s) };
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(i, f)| (i, f));
    let mut grouped = String::new();
    for (k, ch) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}{}{}{}", if neg { "-" } else { "" }, grouped, if frac.is_empty() { "" } else { "." }, frac)
}

// ------------------------------------------------------------- converter

pub struct Unit {
    pub label: &'static str,
    /// Size in the category's base unit (m, kg, m², L, m/s, bytes).
    pub f: f64,
}
const fn u(label: &'static str, f: f64) -> Unit {
    Unit { label, f }
}
const FT2: f64 = 0.09290304;
pub const CATS: [(&str, &[Unit], (usize, usize), &str); 7] = [
    ("Length", &[u("km", 1000.0), u("m", 1.0), u("cm", 0.01), u("mm", 0.001), u("mi", 1609.344), u("yd", 0.9144), u("ft", 0.3048), u("in", 0.0254)], (0, 4), ""),
    (
        "Weight",
        &[u("kg", 1.0), u("g", 0.001), u("lb", 0.45359237), u("oz", 0.028349523125), u("t", 1000.0), u("maund", 37.3242), u("seer", 0.933105), u("tola", 0.0116638)],
        (0, 2),
        "Bangladesh: 1 maund = 40 seer, 1 seer = 80 tola (1 tola = 1 vori)",
    ),
    ("Temp", &[u("°C", 0.0), u("°F", 0.0), u("K", 0.0)], (0, 1), ""),
    (
        "Area",
        &[u("m²", 1.0), u("ft²", FT2), u("km²", 1e6), u("ha", 1e4), u("acre", 4046.8564224), u("decimal", 40.468564224), u("katha", 720.0 * FT2), u("bigha", 14400.0 * FT2)],
        (6, 1),
        "Bangladesh: 1 katha = 720 ft², 1 bigha = 20 katha, 1 decimal = 1/100 acre",
    ),
    (
        "Volume",
        &[u("L", 1.0), u("mL", 0.001), u("m³", 1000.0), u("gal", 3.785411784), u("qt", 0.946352946), u("cup", 0.2365882365), u("fl oz", 0.0295735295625), u("tbsp", 0.01478676478125)],
        (0, 3),
        "US gallons, quarts, cups and fluid ounces",
    ),
    ("Speed", &[u("km/h", 1.0 / 3.6), u("m/s", 1.0), u("mph", 0.44704), u("knot", 1852.0 / 3600.0)], (0, 2), ""),
    (
        "Data",
        &[u("B", 1.0), u("KB", 1024.0), u("MB", 1048576.0), u("GB", 1073741824.0), u("TB", 1099511627776.0), u("Kbit", 125.0), u("Mbit", 125000.0), u("Gbit", 1.25e8)],
        (6, 2),
        "KB to TB are 1024-based like Windows; bits are 1000-based like internet speeds",
    ),
];
/// Converter chip after the unit categories.
const ZONES: usize = CATS.len();

pub fn convert(cat: usize, from: usize, to: usize, v: f64) -> f64 {
    let units = CATS[cat].1;
    if CATS[cat].0 == "Temp" {
        let k = match from {
            0 => v + 273.15,
            1 => (v - 32.0) * 5.0 / 9.0 + 273.15,
            _ => v,
        };
        return match to {
            0 => k - 273.15,
            1 => (k - 273.15) * 9.0 / 5.0 + 32.0,
            _ => k,
        };
    }
    v * units[from].f / units[to].f
}

// ----------------------------------------------------------- time zones

pub const CITIES: [(&str, &str); 12] = [
    ("Dhaka", "Bangladesh Standard Time"),
    ("Kolkata", "India Standard Time"),
    ("Dubai", "Arabian Standard Time"),
    ("Riyadh", "Arab Standard Time"),
    ("London", "GMT Standard Time"),
    ("Paris", "Romance Standard Time"),
    ("New York", "Eastern Standard Time"),
    ("Toronto", "Eastern Standard Time"),
    ("Los Angeles", "Pacific Standard Time"),
    ("Tokyo", "Tokyo Standard Time"),
    ("Sydney", "AUS Eastern Standard Time"),
    ("Kuala Lumpur", "Singapore Standard Time"),
];

pub mod tz {
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::SystemInformation::GetSystemTime;
    use windows::Win32::System::Time::*;

    struct Zones(Vec<(String, DYNAMIC_TIME_ZONE_INFORMATION)>);
    unsafe impl Send for Zones {}
    unsafe impl Sync for Zones {}

    fn zones() -> &'static Zones {
        static Z: OnceLock<Zones> = OnceLock::new();
        Z.get_or_init(|| {
            let mut v = Vec::new();
            for i in 0..1000u32 {
                let mut d = DYNAMIC_TIME_ZONE_INFORMATION::default();
                if unsafe { EnumDynamicTimeZoneInformation(i, &mut d) } != 0 {
                    break;
                }
                let key = wstr(&d.TimeZoneKeyName);
                v.push((key, d));
            }
            Zones(v)
        })
    }

    /// A fixed-size UTF-16 buffer up to its first NUL.
    fn wstr(b: &[u16]) -> String {
        String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())])
    }

    pub fn known(key: &str) -> bool {
        zones().0.iter().any(|(k, _)| k == key)
    }
    /// Self-test: how many zones Windows listed, and the first key and status.
    pub fn debug() -> String {
        let mut d = DYNAMIC_TIME_ZONE_INFORMATION::default();
        let r = unsafe { EnumDynamicTimeZoneInformation(0, &mut d) };
        let key = wstr(&d.TimeZoneKeyName);
        format!("{} zones, first call {r} {key:?}", zones().0.len())
    }

    pub fn ft(st: &SYSTEMTIME) -> i64 {
        let mut f = FILETIME::default();
        let _ = unsafe { SystemTimeToFileTime(st, &mut f) };
        ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
    }
    pub fn st(ft: i64) -> SYSTEMTIME {
        let f = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
        let mut s = SYSTEMTIME::default();
        let _ = unsafe { FileTimeToSystemTime(&f, &mut s) };
        s
    }
    /// Now (UTC) moved by `minutes`, as a FILETIME count. Demo renders use
    /// the same frozen local time as the notch clock.
    pub fn now_plus(minutes: i64) -> i64 {
        let now = if crate::time::demo() {
            let mut l = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            (l.wHour, l.wMinute, l.wSecond, l.wMilliseconds) = (16, 20, 0, 0);
            let mut u = SYSTEMTIME::default();
            let _ = unsafe { TzSpecificLocalTimeToSystemTime(None, &l, &mut u) };
            u
        } else {
            unsafe { GetSystemTime() }
        };
        ft(&now) + minutes * 600_000_000
    }
    /// Wall time in zone `key` (None = this PC's zone) at UTC `utc`.
    pub fn local(key: Option<&str>, utc: i64) -> Option<SYSTEMTIME> {
        let u = st(utc);
        let mut out = SYSTEMTIME::default();
        let d = match key {
            Some(k) => zones().0.iter().find(|(kk, _)| kk == k)?.1,
            None => {
                let mut d = DYNAMIC_TIME_ZONE_INFORMATION::default();
                unsafe { GetDynamicTimeZoneInformation(&mut d) };
                d
            }
        };
        unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(&d), &u, &mut out) }.ok()?;
        Some(out)
    }
    /// This PC's zone, as Windows names it ("Bangladesh Standard Time").
    pub fn local_key() -> String {
        let mut d = DYNAMIC_TIME_ZONE_INFORMATION::default();
        unsafe { GetDynamicTimeZoneInformation(&mut d) };
        wstr(&d.TimeZoneKeyName)
    }
}

// --------------------------------------------------------- colour picker

pub mod picker {
    //! The full-screen "pick a colour" window (GDI; one at a time).
    use std::cell::RefCell;
    use std::sync::Mutex;

    use windows::core::w;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// A frozen screenshot of every monitor (virtual-screen pixels).
    pub struct Snap {
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
        /// Top-down BGRA.
        pub px: Vec<u8>,
        mem: HDC,
        bmp: HBITMAP,
        old: HGDIOBJ,
    }
    impl Snap {
        /// Memory DC holding the screenshot.
        pub fn mem(&self) -> HDC {
            self.mem
        }
        pub fn at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
            if x < 0 || y < 0 || x >= self.w || y >= self.h {
                return None;
            }
            let i = ((y * self.w + x) * 4) as usize;
            Some([self.px[i + 2], self.px[i + 1], self.px[i]])
        }
    }
    impl Drop for Snap {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.mem, self.old);
                let _ = DeleteObject(self.bmp);
                let _ = DeleteDC(self.mem);
            }
        }
    }

    unsafe extern "system" fn add_monitor(_m: HMONITOR, _dc: HDC, r: *mut RECT, lp: LPARAM) -> BOOL {
        let u = &mut *(lp.0 as *mut RECT);
        let r = &*r;
        if u.right <= u.left {
            *u = *r;
        } else {
            u.left = u.left.min(r.left);
            u.top = u.top.min(r.top);
            u.right = u.right.max(r.right);
            u.bottom = u.bottom.max(r.bottom);
        }
        TRUE
    }

    /// Set while the notch's window is hidden for a screen capture.
    static APP_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Hide the notch's own window before freezing the screen, so neither
    /// the closing panel nor the closed notch ends up in the picture (or
    /// covers part of it). `show_app` brings it back.
    pub fn hide_app(app: HWND) {
        unsafe {
            if app.0.is_null() || !IsWindowVisible(app).as_bool() {
                return;
            }
            let _ = ShowWindow(app, SW_HIDE);
            APP_HIDDEN.store(true, std::sync::atomic::Ordering::SeqCst);
            // Let the compositor drop it from the screen before reading it.
            let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            std::thread::sleep(std::time::Duration::from_millis(30));
            let _ = windows::Win32::Graphics::Dwm::DwmFlush();
        }
    }

    pub fn show_app(app: HWND) {
        if APP_HIDDEN.swap(false, std::sync::atomic::Ordering::SeqCst) && !app.0.is_null() {
            unsafe {
                let _ = ShowWindow(app, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// Screenshot of all monitors without the notch in it.
    pub fn capture_without(app: HWND) -> Option<Snap> {
        hide_app(app);
        let snap = capture();
        if snap.is_none() {
            show_app(app);
        }
        snap
    }

    /// Screenshot of all monitors.
    pub fn capture() -> Option<Snap> {
        unsafe {
            let mut u = RECT::default();
            let _ = EnumDisplayMonitors(None, None, Some(add_monitor), LPARAM(&mut u as *mut RECT as isize));
            let (w, h) = (u.right - u.left, u.bottom - u.top);
            if w <= 0 || h <= 0 {
                return None;
            }
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp);
            let ok = BitBlt(mem, 0, 0, w, h, screen, u.left, u.top, SRCCOPY | CAPTUREBLT).is_ok();
            ReleaseDC(None, screen);
            // GetDIBits wants the bitmap out of any DC.
            SelectObject(mem, old);
            let mut px = vec![0u8; (w * h * 4) as usize];
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() };
            let rows = GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
            let old = SelectObject(mem, bmp);
            let snap = Snap { x: u.left, y: u.top, w, h, px, mem, bmp, old };
            (ok && rows == h).then_some(snap)
        }
    }

    struct Overlay {
        hwnd: HWND,
        app: isize,
        snap: Snap,
        cur: (i32, i32),
        font: HFONT,
        small: HFONT,
    }

    thread_local! {
        static OV: RefCell<Option<Overlay>> = const { RefCell::new(None) };
    }
    /// Outcome of the last pick: Some(Some(colour)) picked, Some(None) cancelled.
    pub static RESULT: Mutex<Option<Option<[u8; 3]>>> = Mutex::new(None);

    pub fn active() -> bool {
        OV.with(|o| o.borrow().is_some())
    }

    const CELLS: i32 = 11;

    /// Loupe + label rect for a cursor at client (cx, cy).
    pub fn loupe_rect(cx: i32, cy: i32, w: i32, h: i32, scale: f32) -> RECT {
        let l = (150.0 * scale) as i32 / CELLS * CELLS;
        let label = (46.0 * scale) as i32;
        let gap = (22.0 * scale) as i32;
        let mut x = cx + gap;
        let mut y = cy + gap;
        if x + l > w {
            x = cx - gap - l;
        }
        if y + l + label > h {
            y = cy - gap - l - label;
        }
        RECT { left: x, top: y, right: x + l, bottom: y + l + label }
    }

    pub fn start(app: HWND) -> bool {
        if active() || crate::modules::snip::active() {
            return true;
        }
        let Some(snap) = capture_without(app) else { return false };
        unsafe {
            static REG: std::sync::Once = std::sync::Once::new();
            let Ok(hinst) = GetModuleHandleW(None) else { return false };
            REG.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: hinst.into(),
                    lpszClassName: w!("WindyLandColorPicker"),
                    hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("WindyLandColorPicker"),
                w!("WindyLand color picker"),
                WS_POPUP,
                snap.x,
                snap.y,
                snap.w,
                snap.h,
                None,
                None,
                hinst,
                None,
            ) else {
                show_app(app);
                return false;
            };
            let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
            let mk = |size: f32, weight: i32| {
                CreateFontW(-(size * scale) as i32, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"))
            };
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let ov = Overlay { hwnd, app: app.0 as isize, cur: (p.x - snap.x, p.y - snap.y), snap, font: mk(15.0, 600), small: mk(11.5, 400) };
            OV.with(|o| *o.borrow_mut() = Some(ov));
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(hwnd);
        }
        true
    }

    fn finish(pick: Option<[u8; 3]>) {
        let Some(ov) = OV.with(|o| o.borrow_mut().take()) else { return };
        unsafe {
            let _ = DestroyWindow(ov.hwnd);
            let _ = DeleteObject(ov.font);
            let _ = DeleteObject(ov.small);
            show_app(HWND(ov.app as *mut _));
            *RESULT.lock().unwrap() = Some(pick);
            let _ = PostMessageW(HWND(ov.app as *mut _), crate::app::WM_APP_WAKE, WPARAM(0), LPARAM(0));
        }
    }

    /// Self-test: pick at client (x, y) of an open picker.
    pub fn test_click(x: i32, y: i32) -> bool {
        let Some(hwnd) = OV.with(|o| o.borrow().as_ref().map(|v| v.hwnd)) else { return false };
        unsafe {
            let lp = LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize));
            SendMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), lp);
            SendMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(0), lp);
        }
        true
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    unsafe fn paint(ov: &Overlay) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(ov.hwnd, &mut ps);
        let rc = ps.rcPaint;
        let (rw, rh) = (rc.right - rc.left, rc.bottom - rc.top);
        if rw > 0 && rh > 0 {
            let bb = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rw, rh);
            let old = SelectObject(bb, bmp);
            let _ = SetViewportOrgEx(bb, -rc.left, -rc.top, None);
            let _ = BitBlt(bb, rc.left, rc.top, rw, rh, ov.snap.mem, rc.left, rc.top, SRCCOPY);
            let scale = GetDpiForWindow(ov.hwnd) as f32 / 96.0;
            let (cx, cy) = ov.cur;
            let r = loupe_rect(cx, cy, ov.snap.w, ov.snap.h, scale);
            let l = r.right - r.left;
            let cell = l / CELLS;
            SetStretchBltMode(bb, COLORONCOLOR);
            let _ = StretchBlt(bb, r.left, r.top, l, l, ov.snap.mem, cx - CELLS / 2, cy - CELLS / 2, CELLS, CELLS, SRCCOPY);
            let white = CreateSolidBrush(COLORREF(0x00FFFFFF));
            let black = CreateSolidBrush(COLORREF(0x00000000));
            let dark = CreateSolidBrush(COLORREF(0x00241E1C));
            let ring = |b: HBRUSH, rr: RECT| {
                FrameRect(bb, &rr, b);
            };
            ring(black, RECT { left: r.left - 1, top: r.top - 1, right: r.left + l + 1, bottom: r.top + l + 1 });
            ring(white, RECT { left: r.left - 2, top: r.top - 2, right: r.left + l + 2, bottom: r.top + l + 2 });
            let c0 = RECT { left: r.left + cell * (CELLS / 2), top: r.top + cell * (CELLS / 2), right: r.left + cell * (CELLS / 2 + 1), bottom: r.top + cell * (CELLS / 2 + 1) };
            ring(black, c0);
            ring(white, RECT { left: c0.left - 1, top: c0.top - 1, right: c0.right + 1, bottom: c0.bottom + 1 });
            // Label: swatch, HEX, hint.
            let lab = RECT { left: r.left - 2, top: r.top + l + 4, right: r.left + l + 2, bottom: r.bottom };
            FillRect(bb, &lab, dark);
            let c = ov.snap.at(cx, cy).unwrap_or([0, 0, 0]);
            let sw = (16.0 * scale) as i32;
            let pad = (8.0 * scale) as i32;
            let swr = RECT { left: lab.left + pad, top: lab.top + pad, right: lab.left + pad + sw, bottom: lab.top + pad + sw };
            let sb = CreateSolidBrush(COLORREF(c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16));
            FillRect(bb, &swr, sb);
            ring(white, swr);
            let _ = DeleteObject(sb);
            SetBkMode(bb, TRANSPARENT);
            SetTextColor(bb, COLORREF(0x00F7F5F5));
            let oldf = SelectObject(bb, ov.font);
            let mut t = wide(&super::hex_of(c));
            let mut tr = RECT { left: swr.right + pad, top: lab.top + pad - (2.0 * scale) as i32, right: lab.right - pad, bottom: lab.top + pad + sw + (4.0 * scale) as i32 };
            DrawTextW(bb, &mut t, &mut tr, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
            SelectObject(bb, ov.small);
            SetTextColor(bb, COLORREF(0x00A09A9A));
            let mut t = wide("Click to pick · Esc cancels");
            let mut tr = RECT { left: lab.left + pad, top: swr.bottom + (2.0 * scale) as i32, right: lab.right - pad, bottom: lab.bottom - (2.0 * scale) as i32 };
            DrawTextW(bb, &mut t, &mut tr, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            SelectObject(bb, oldf);
            for b in [white, black, dark] {
                let _ = DeleteObject(b);
            }
            let _ = SetViewportOrgEx(bb, 0, 0, None);
            let _ = BitBlt(hdc, rc.left, rc.top, rw, rh, bb, 0, 0, SRCCOPY);
            SelectObject(bb, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(bb);
        }
        let _ = EndPaint(ov.hwnd, &ps);
    }

    fn invalidate(ov: &Overlay, old: (i32, i32)) {
        unsafe {
            let scale = GetDpiForWindow(ov.hwnd) as f32 / 96.0;
            for (x, y) in [old, ov.cur] {
                let mut r = loupe_rect(x, y, ov.snap.w, ov.snap.h, scale);
                r.left -= 4;
                r.top -= 4;
                r.right += 4;
                r.bottom += 4;
                let _ = InvalidateRect(ov.hwnd, Some(&r), false);
            }
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let xy = || (((lp.0 & 0xffff) as u16 as i16) as i32, (((lp.0 >> 16) & 0xffff) as u16 as i16) as i32);
        match msg {
            WM_PAINT => {
                let done = OV.with(|o| {
                    if let Some(ov) = o.borrow().as_ref() {
                        paint(ov);
                        true
                    } else {
                        false
                    }
                });
                if !done {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                OV.with(|o| {
                    if let Some(ov) = o.borrow_mut().as_mut() {
                        let old = ov.cur;
                        ov.cur = xy();
                        invalidate(ov, old);
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = xy();
                let c = OV.with(|o| o.borrow().as_ref().and_then(|ov| ov.snap.at(x, y)));
                finish(c.or(Some([0, 0, 0])));
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                finish(None);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let step = if GetKeyState(VK_SHIFT.0 as i32) < 0 { 10 } else { 1 };
                let (dx, dy) = match vk {
                    VK_LEFT => (-step, 0),
                    VK_RIGHT => (step, 0),
                    VK_UP => (0, -step),
                    VK_DOWN => (0, step),
                    VK_ESCAPE => {
                        finish(None);
                        return LRESULT(0);
                    }
                    VK_RETURN | VK_SPACE => {
                        let c = OV.with(|o| o.borrow().as_ref().and_then(|ov| ov.snap.at(ov.cur.0, ov.cur.1)));
                        finish(c);
                        return LRESULT(0);
                    }
                    _ => (0, 0),
                };
                if dx != 0 || dy != 0 {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    let _ = SetCursorPos(p.x + dx, p.y + dy);
                }
                LRESULT(0)
            }
            // Leaving with Alt+Tab or a click elsewhere cancels.
            WM_ACTIVATEAPP if wp.0 == 0 => {
                finish(None);
                LRESULT(0)
            }
            // One window over monitors with different scaling: keep its size.
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ----------------------------------------------------------------- module

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    None,
    Calc,
    Qr,
    Value,
}

pub struct Tools {
    hwnd: HWND,
    tool: Tool,
    color: Option<[u8; 3]>,
    colors: Vec<[u8; 3]>,
    /// Which value was just copied, and when (for "Copied").
    copied: Option<(u32, f64)>,
    /// "Pick from screen" waits for the notch to close first.
    pick_at: Option<f64>,
    /// Self-test: don't open the full-screen picker.
    no_overlay: bool,
    calc: String,
    calc_hist: Vec<(String, String)>,
    qr_text: String,
    qr_typed: bool,
    qr_seq: u32,
    qr: Option<(String, Option<qrcodegen::QrCode>)>,
    cat: usize,
    units: Vec<(usize, usize)>,
    value: String,
    tz_base: Option<usize>,
    tz_offset: i64,
    focus: Focus,
}

impl Tools {
    pub fn new(hwnd: HWND) -> Tools {
        let kv = store::parse_kv(&store::read("tools.txt"));
        let tool = kv.get("tool").and_then(|k| TOOLS.iter().find(|t| t.3 == k)).map_or(Tool::Color, |t| t.0);
        let colors: Vec<[u8; 3]> = kv.get("colors").map_or(Vec::new(), |v| v.split(',').filter_map(parse_hex).take(MAX_COLORS).collect());
        let mut units: Vec<(usize, usize)> = CATS.iter().map(|c| c.2).collect();
        for (i, c) in CATS.iter().enumerate() {
            if let Some((a, b)) = kv.get(&format!("units_{}", c.0.to_lowercase())).and_then(|v| v.split_once(',')) {
                if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                    if a < c.1.len() && b < c.1.len() {
                        units[i] = (a, b);
                    }
                }
            }
        }
        Tools {
            hwnd,
            tool,
            color: colors.first().copied(),
            colors,
            copied: None,
            pick_at: None,
            no_overlay: false,
            calc: String::new(),
            calc_hist: Vec::new(),
            qr_text: String::new(),
            qr_typed: false,
            qr_seq: 0,
            qr: None,
            cat: kv.get("cat").and_then(|v| v.parse().ok()).filter(|c| *c <= ZONES).unwrap_or(3),
            units,
            value: kv.get("value").cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| "1".into()),
            tz_base: kv.get("tz_base").and_then(|v| v.parse().ok()).filter(|c| *c < CITIES.len()),
            tz_offset: 0,
            focus: Focus::None,
        }
    }

    fn save(&self) {
        let mut kv = BTreeMap::new();
        kv.insert("tool".to_string(), TOOLS.iter().find(|t| t.0 == self.tool).map_or("color", |t| t.3).to_string());
        kv.insert("colors".to_string(), self.colors.iter().map(|c| hex_of(*c)).collect::<Vec<_>>().join(","));
        kv.insert("cat".to_string(), self.cat.to_string());
        kv.insert("value".to_string(), self.value.clone());
        for (i, c) in CATS.iter().enumerate() {
            kv.insert(format!("units_{}", c.0.to_lowercase()), format!("{},{}", self.units[i].0, self.units[i].1));
        }
        kv.insert("tz_base".to_string(), self.tz_base.map_or(String::new(), |b| b.to_string()));
        store::write("tools.txt", &store::write_kv(&kv));
    }

    fn copy(&mut self, id: u32, text: String) {
        clipboard::set_clipboard(self.hwnd, &Content::Text(text));
        self.copied = Some((id, unix_now()));
    }

    fn just_copied(&self, id: u32) -> bool {
        self.copied.map_or(false, |(i, t)| i == id && unix_now() - t < 1.6)
    }

    fn set_color(&mut self, c: [u8; 3]) {
        self.color = Some(c);
        self.colors.retain(|x| *x != c);
        self.colors.insert(0, c);
        self.colors.truncate(MAX_COLORS);
        self.save();
    }

    fn calc_result(&self) -> Result<f64, String> {
        eval(&self.calc)
    }

    fn value_num(&self) -> Option<f64> {
        eval(&self.value).ok()
    }

    /// The QR code for the current text (cached).
    fn qr_code(&mut self) -> Option<&qrcodegen::QrCode> {
        if self.qr.as_ref().map_or(true, |(t, _)| *t != self.qr_text) {
            let t = self.qr_text.clone();
            let code = if t.trim().is_empty() {
                None
            } else {
                qrcodegen::QrCode::encode_text(&t, qrcodegen::QrCodeEcc::Medium)
                    .or_else(|_| qrcodegen::QrCode::encode_text(&t, qrcodegen::QrCodeEcc::Low))
                    .ok()
            };
            self.qr = Some((t, code));
        }
        self.qr.as_ref().and_then(|(_, c)| c.as_ref())
    }

    fn refresh_qr_from_clipboard(&mut self, force: bool) {
        if self.qr_typed && !force {
            return;
        }
        let seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        if seq != self.qr_seq || force {
            self.qr_seq = seq;
            if let Some(t) = crate::modules::todo::clipboard_text() {
                self.qr_text = t.trim().to_string();
            }
            self.qr_typed = false;
        }
    }

    fn set_focus(&mut self, f: Focus, ctx: &mut Ctx) {
        self.focus = f;
        ctx.want_keyboard(f != Focus::None);
    }

    // ------------------------------------------------------------ drawing

    fn field(ui: &mut Ui, r: Rect, text: &str, placeholder: &str, focused: bool, id: u32, size: f32) {
        ui.fill_rr(r, 8.0, col::TEXT.with_a(0.08));
        if focused {
            ui.stroke_rr(r, 8.0, ACCENT.with_a(0.7), 1.0);
        }
        ui.hit(r, id);
        let tr = r.inset_xy(10.0, 0.0);
        if text.is_empty() && !focused {
            ui.text(placeholder, tr, 12.0, col::FAINT, Weight::Regular, Align::Left);
        }
        let tw = ui.measure(text, size, Weight::Regular);
        let align = if tw > tr.w - 6.0 { Align::Right } else { Align::Left };
        ui.text(text, Rect::new(tr.x, tr.y, tr.w - 4.0, tr.h), size, col::TEXT, Weight::Regular, align);
        if focused && (ui.t * 1.8) as i64 % 2 == 0 {
            let cx = if align == Align::Right { tr.r() - 4.0 } else { tr.x + tw + 1.0 };
            ui.line(cx, tr.cy() - size * 0.6, cx, tr.cy() + size * 0.6, ACCENT, 1.5);
        }
    }

    fn draw_color(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut left = r.cut_left(140.0, 14.0);
        let sw = left.cut_top(74.0, 8.0);
        match self.color {
            Some(c) => {
                ui.fill_rr(sw, 12.0, color_of(c));
                ui.stroke_rr(sw, 12.0, col::TEXT.with_a(0.15), 1.0);
            }
            None => {
                ui.fill_rr(sw, 12.0, col::BTN);
                ui.text_wrap("Pick any color on your screen", sw.inset(12.0), 11.5, col::DIM, Weight::Regular, Align::Center);
            }
        }
        ui.button_accent(left.cut_top(30.0, 6.0), "Pick from screen", A_PICK, ACCENT);
        ui.text("Esc cancels, arrows nudge", left, 10.0, col::FAINT, Weight::Regular, Align::Center);

        let mut mid = r.cut_left(r.w - 132.0, 12.0);
        if let Some(c) = self.color {
            for (id, label, value) in [(A_COPY_HEX, "HEX", hex_of(c)), (A_COPY_RGB, "RGB", rgb_of(c)), (A_COPY_HSL, "HSL", hsl_of(c))] {
                let row = mid.cut_top(28.0, 6.0);
                let hov = ui.hovered(row);
                ui.fill_rr(row, 8.0, if hov { col::BTN_HOVER } else { col::BTN });
                let mut rr = row.inset_xy(10.0, 0.0);
                ui.text(label, rr.cut_left(32.0, 4.0), 10.5, col::DIM, Weight::Semi, Align::Left);
                let tail = rr.cut_right(54.0, 0.0);
                if self.just_copied(id) {
                    ui.text("Copied", tail, 10.5, ACCENT, Weight::Semi, Align::Right);
                } else {
                    ui.icon(G_COPY, Rect::new(tail.r() - 14.0, tail.y, 14.0, tail.h), 10.5, col::DIM);
                }
                ui.text(&value, rr, 12.0, col::TEXT, Weight::Semi, Align::Left);
                ui.hit_tip(row, id, "Copy");
            }
            // Readability of text in this colour on white and on black.
            let row = mid.cut_top(22.0, 0.0);
            for (cell, bg, name) in [(row.columns(2, 6.0)[0], [255u8, 255, 255], "white"), (row.columns(2, 6.0)[1], [0, 0, 0], "black")] {
                ui.fill_rr(cell, 6.0, color_of(bg));
                let ratio = contrast(c, bg);
                let mut cc = cell.inset_xy(8.0, 0.0);
                ui.text("Aa", cc.cut_left(20.0, 4.0), 12.0, color_of(c), Weight::Semi, Align::Left);
                let fg = if name == "white" { hex(0x333333) } else { hex(0xCCCCCC) };
                ui.text(&format!("{ratio:.1}:1"), cc, 10.0, fg, Weight::Regular, Align::Right);
            }
        } else {
            ui.text_wrap("Picked colors show here as HEX, RGB and HSL. Click one to copy it.", mid.inset_xy(0.0, 10.0), 11.5, col::DIM, Weight::Regular, Align::Left);
        }

        ui.text("Recent", r.cut_top(16.0, 4.0), 10.5, col::DIM, Weight::Semi, Align::Left);
        for (k, c) in self.colors.clone().iter().enumerate() {
            let (row, colm) = (k / 4, k % 4);
            let cell = Rect::new(r.x + colm as f32 * 31.0, r.y + row as f32 * 31.0, 25.0, 25.0);
            ui.fill_rr(cell, 7.0, color_of(*c));
            if Some(*c) == self.color {
                ui.stroke_rr(cell.inset(-2.0), 9.0, col::TEXT.with_a(0.8), 1.4);
            }
            ui.hit_tip(cell, A_SWATCH + k as u32, "Use this color");
        }
    }

    fn draw_calc(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut hist = r.cut_right(168.0, 16.0);
        let focused = self.focus == Focus::Calc;
        let calc = self.calc.clone();
        Self::field(ui, r.cut_top(34.0, 10.0), &calc, "Type a sum, e.g. 1250*15%", focused, A_CALC_FIELD, 15.0);
        let mut row = r.cut_top(36.0, 4.0);
        match self.calc_result() {
            Ok(v) => {
                let copy = row.cut_right(30.0, 6.0);
                if self.just_copied(A_CALC_COPY) {
                    ui.text("Copied", copy.inset_xy(-20.0, 0.0), 10.5, ACCENT, Weight::Semi, Align::Right);
                } else {
                    ui.icon_btn(copy, G_COPY, A_CALC_COPY, 11.0, col::DIM, Some(col::BTN), "Copy the answer");
                }
                ui.text(&format!("= {}", fmt_num(v)), row, 24.0, ACCENT, Weight::Semi, Align::Left);
                if v.fract() == 0.0 && v.abs() < 9.007e15 {
                    let n = v as i64;
                    let extra = if (0..65536).contains(&n) { format!("hex 0x{n:X} · bin 0b{n:b}") } else { format!("hex 0x{:X}", n) };
                    ui.text(&extra, r.cut_top(16.0, 4.0), 10.5, col::FAINT, Weight::Regular, Align::Left);
                } else {
                    r.cut_top(16.0, 4.0);
                }
            }
            Err(e) => {
                let msg = if calc.trim().is_empty() { "" } else if e.is_empty() { "..." } else { e.as_str() };
                ui.text(msg, row, 13.0, col::DIM, Weight::Regular, Align::Left);
                r.cut_top(16.0, 4.0);
            }
        }
        ui.text_wrap("Enter saves · Esc clears · try 200+10%, 2^10, sqrt(2), 5!", r, 10.5, col::FAINT, Weight::Regular, Align::Left);

        ui.text("History", hist.cut_top(16.0, 4.0), 10.5, col::DIM, Weight::Semi, Align::Left);
        if self.calc_hist.is_empty() {
            ui.text_wrap("Press Enter to keep a result here.", hist, 10.5, col::FAINT, Weight::Regular, Align::Left);
        }
        for (k, (e, v)) in self.calc_hist.clone().iter().enumerate().take(5) {
            let row = hist.cut_top(22.0, 2.0);
            if ui.hovered(row) {
                ui.fill_rr(row, 6.0, col::BTN_HOVER);
            }
            let mut rr = row.inset_xy(6.0, 0.0);
            let vw = ui.measure(v, 11.5, Weight::Semi).min(rr.w * 0.6);
            ui.text(v, rr.cut_right(vw + 2.0, 6.0), 11.5, col::TEXT, Weight::Semi, Align::Right);
            ui.text(e, rr, 10.5, col::DIM, Weight::Regular, Align::Left);
            ui.hit_tip(row, A_CALC_HIST + k as u32, "Use this again");
        }
    }

    fn draw_qr(&mut self, ui: &mut Ui, mut r: Rect) {
        if self.focus != Focus::Qr {
            self.refresh_qr_from_clipboard(false);
        }
        let side = r.h.min(144.0);
        let box_r = r.cut_left(side, 18.0);
        let box_r = Rect::new(box_r.x, box_r.y + (r.h - side) / 2.0, side, side);
        let text_len = self.qr_text.chars().count();
        match self.qr_code().cloned() {
            Some(code) => {
                ui.fill_rr(box_r, 10.0, hex(0xFFFFFF));
                let n = code.size();
                let quiet = 2;
                let m = side / (n + 2 * quiet) as f32;
                let ox = box_r.x + (side - m * n as f32) / 2.0;
                let oy = box_r.y + (side - m * n as f32) / 2.0;
                for y in 0..n {
                    let mut x = 0;
                    while x < n {
                        if code.get_module(x, y) {
                            let x0 = x;
                            while x < n && code.get_module(x, y) {
                                x += 1;
                            }
                            ui.fill_rect(Rect::new(ox + x0 as f32 * m, oy + y as f32 * m, (x - x0) as f32 * m + 0.35, m + 0.35), hex(0x000000));
                        } else {
                            x += 1;
                        }
                    }
                }
            }
            None => {
                ui.fill_rr(box_r, 10.0, col::BTN);
                let msg = if self.qr_text.trim().is_empty() { "Copy a link or some text, and it shows here as a QR code" } else { "Too long for a QR code" };
                ui.text_wrap(msg, box_r.inset(14.0), 11.5, col::DIM, Weight::Regular, Align::Center);
            }
        }
        let mut row = r.cut_top(26.0, 8.0);
        let w = (row.w - 6.0) / 2.0;
        ui.chip(row.cut_left(w, 6.0), "From clipboard", A_QR_CLIP, !self.qr_typed, ACCENT);
        ui.chip(row, "Type text", A_QR_TYPE, self.qr_typed, ACCENT);
        let mut preview = r.cut_top(60.0, 6.0);
        if self.focus == Focus::Qr {
            let t = self.qr_text.clone();
            Self::field(ui, preview.cut_top(30.0, 0.0), &t, "Type a link or text", true, A_QR_FIELD, 12.5);
        } else {
            ui.fill_rr(preview, 8.0, col::TEXT.with_a(0.05));
            let shown: String = self.qr_text.chars().take(160).collect();
            let shown = if shown.is_empty() { "Nothing copied yet".to_string() } else { shown };
            ui.text_wrap(&shown, preview.inset_xy(10.0, 6.0), 11.0, col::TEXT, Weight::Regular, Align::Left);
            ui.hit(preview, A_QR_TYPE);
        }
        let mut row = r.cut_top(28.0, 0.0);
        let copied = self.just_copied(A_QR_COPY);
        ui.button(row.cut_left(110.0, 10.0), if copied { "Copied" } else { "Copy image" }, A_QR_COPY);
        let info = if text_len == 0 { "Scan it with your phone's camera".to_string() } else { format!("{text_len} characters · scan with your phone") };
        ui.text(&info, row, 10.5, col::FAINT, Weight::Regular, Align::Left);
    }

    fn draw_convert(&mut self, ui: &mut Ui, mut r: Rect) {
        let row = r.cut_top(22.0, if self.cat == ZONES { 6.0 } else { 10.0 });
        let names: Vec<&str> = CATS.iter().map(|c| c.0).chain(std::iter::once("Zones")).collect();
        for (i, cell) in row.columns(names.len(), 5.0).into_iter().enumerate() {
            ui.chip(cell, names[i], A_CAT + i as u32, self.cat == i, ACCENT);
        }
        if self.cat == ZONES {
            return self.draw_zones(ui, r);
        }
        let (cat, units) = (self.cat, CATS[self.cat].1);
        let (from, to) = self.units[cat];
        let value = self.value.clone();
        let focused = self.focus == Focus::Value;

        let mut row = r.cut_top(28.0, 6.0);
        Self::field(ui, row.cut_left(118.0, 8.0), &value, "Number", focused, A_VALUE, 13.0);
        let cells = unit_cells(ui, row, units);
        for (i, cell) in cells.iter().enumerate() {
            ui.chip(*cell, units[i].label, A_FROM + i as u32, i == from, ACCENT);
        }
        let mut row = r.cut_top(28.0, 8.0);
        let res = row.cut_left(118.0, 8.0);
        let out = self.value_num().map(|v| fmt_num(convert(cat, from, to, v)));
        ui.fill_rr(res, 8.0, ACCENT.with_a(0.16));
        let shown = out.clone().unwrap_or_else(|| "-".into());
        let shown = if self.just_copied(A_CONV_COPY) { "Copied".to_string() } else { shown };
        ui.text(&shown, res.inset_xy(10.0, 0.0), 13.0, ACCENT, Weight::Semi, Align::Left);
        ui.hit_tip(res, A_CONV_COPY, "Copy");
        for (i, cell) in unit_cells(ui, row, units).iter().enumerate() {
            ui.chip(*cell, units[i].label, A_TO + i as u32, i == to, ACCENT);
        }
        let mut row = r.cut_top(22.0, 0.0);
        ui.button(row.cut_left(70.0, 10.0), "Swap", A_SWAP);
        let note = CATS[cat].3;
        let line = if note.is_empty() {
            match (self.value_num(), out) {
                (Some(v), Some(o)) => format!("{} {} = {} {}", fmt_num(v), units[from].label, o, units[to].label),
                _ => String::new(),
            }
        } else {
            note.to_string()
        };
        ui.text(&line, row, 10.5, col::FAINT, Weight::Regular, Align::Left);
    }

    fn draw_zones(&mut self, ui: &mut Ui, mut r: Rect) {
        let utc = tz::now_plus(self.tz_offset);
        let base_key = self.tz_base.map(|i| CITIES[i].1);
        let Some(base) = tz::local(base_key, utc) else {
            ui.text("Time zones aren't available on this PC", r, 12.0, col::DIM, Weight::Regular, Align::Left);
            return;
        };
        let mut row = r.cut_top(28.0, 6.0);
        ui.icon_btn(row.cut_right(28.0, 4.0), glyph::ADD, A_TZ_PLUS, 10.0, col::TEXT, Some(col::BTN), "An hour later");
        ui.button(row.cut_right(52.0, 4.0), "Now", A_TZ_NOW);
        ui.icon_btn(row.cut_right(28.0, 8.0), "\u{E738}", A_TZ_MINUS, 10.0, col::TEXT, Some(col::BTN), "An hour earlier");
        ui.chip(row.cut_right(72.0, 10.0), "This PC", A_TZ_LOCAL, self.tz_base.is_none(), ACCENT);
        let name = self.tz_base.map_or("This PC", |i| CITIES[i].0);
        let nw = ui.measure(name, 13.0, Weight::Semi) + 10.0;
        ui.text(name, row.cut_left(nw, 0.0), 13.0, col::TEXT, Weight::Semi, Align::Left);
        let time = crate::time::clock_full(base.wHour as u32, base.wMinute as u32);
        let tw = ui.measure(&time, 20.0, Weight::Semi) + 10.0;
        ui.text(&time, row.cut_left(tw, 0.0), 20.0, ACCENT, Weight::Semi, Align::Left);
        let off = if self.tz_offset == 0 { String::new() } else { format!(" ({:+} h)", self.tz_offset / 60) };
        ui.text(&format!("{}{off}", day_name(&base)), row, 11.0, col::DIM, Weight::Regular, Align::Left);

        let base_ft = tz::ft(&base);
        let rows = r.rows(2, 6.0);
        for (k, (city, key)) in CITIES.iter().enumerate() {
            let cell = rows[k / 6].columns(6, 6.0)[k % 6];
            let sel = self.tz_base == Some(k);
            let hov = ui.hovered(cell);
            ui.fill_rr(cell, 8.0, if sel { ACCENT.with_a(0.22) } else if hov { col::BTN_HOVER } else { col::BTN });
            let mut c = cell.inset_xy(8.0, 2.0);
            // City on top with the difference on its right; the time below.
            let top = c.cut_top(13.0, 0.0);
            ui.text(city, top, 10.5, col::DIM, Weight::Regular, Align::Left);
            if let Some(t) = tz::local(Some(key), utc) {
                let diff_min = (tz::ft(&t) - base_ft) / 600_000_000;
                let day = (t.wYear, t.wMonth, t.wDay).cmp(&(base.wYear, base.wMonth, base.wDay));
                let tag = match day {
                    std::cmp::Ordering::Greater => " +1d",
                    std::cmp::Ordering::Less => " -1d",
                    _ => "",
                };
                let diff = if diff_min == 0 {
                    String::new()
                } else if diff_min % 60 == 0 {
                    format!("{:+}h", diff_min / 60)
                } else {
                    format!("{:+.1}h", diff_min as f64 / 60.0)
                };
                // "4:20" big with a small "PM" (or "16:20"), to fit the cell.
                let tt = crate::time::clock(t.wHour as u32, t.wMinute as u32);
                let tw = ui.measure(&tt, 13.0, Weight::Semi) + 2.0;
                ui.text(&tt, c.cut_left(tw, 0.0), 13.0, col::TEXT, Weight::Semi, Align::Left);
                if crate::time::h12() {
                    let ap = if t.wHour < 12 { "AM" } else { "PM" };
                    let aw = ui.measure(ap, 9.0, Weight::Semi) + 4.0;
                    ui.text(ap, c.cut_left(aw, 0.0), 9.0, col::DIM, Weight::Semi, Align::Left);
                }
                let note = format!("{diff}{tag}");
                if ui.measure(city, 10.5, Weight::Regular) + ui.measure(&note, 9.5, Weight::Regular) + 4.0 <= top.w {
                    ui.text(&note, top, 9.5, col::FAINT, Weight::Regular, Align::Right);
                } else {
                    ui.text(&note, c, 9.5, col::FAINT, Weight::Regular, Align::Right);
                }
            }
            ui.hit_tip(cell, A_TZ_CITY + k as u32, "Compare from this city");
        }
    }
}

/// Unit chips across `row`, each as wide as its label plus padding, with
/// the spare room shared out.
fn unit_cells(ui: &mut Ui, row: Rect, units: &[Unit]) -> Vec<Rect> {
    let gap = 4.0;
    let ws: Vec<f32> = units.iter().map(|u| ui.measure(u.label, 11.5, Weight::Semi) + 12.0).collect();
    let need: f32 = ws.iter().sum::<f32>() + gap * (units.len() as f32 - 1.0);
    let extra = ((row.w - need) / units.len() as f32).max(0.0);
    let mut x = row.x;
    ws.iter()
        .map(|w| {
            let c = Rect::new(x, row.y, w + extra, row.h);
            x += w + extra + gap;
            c
        })
        .collect()
}

/// Self-test: room left in a unit row of `w` (negative = labels don't fit).
pub fn unit_room(ui_measure: impl Fn(&str) -> f32, w: f32) -> Vec<(&'static str, f32)> {
    CATS.iter()
        .map(|c| (c.0, w - c.1.iter().map(|u| ui_measure(u.label) + 12.0).sum::<f32>() - 4.0 * (c.1.len() as f32 - 1.0)))
        .collect()
}

fn day_name(t: &windows::Win32::Foundation::SYSTEMTIME) -> String {
    const D: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {}", D[t.wDayOfWeek as usize % 7], t.wDay, M[(t.wMonth as usize).clamp(1, 12) - 1])
}

/// A QR code as a 24-bit DIB (BITMAPINFOHEADER + rows), `px` pixels per module.
pub fn qr_dib(code: &qrcodegen::QrCode, px: i32) -> (u32, Vec<u8>) {
    let quiet = 4;
    let n = code.size();
    let side = (n + 2 * quiet) * px;
    let stride = ((side * 3 + 3) / 4 * 4) as usize;
    let mut dib = Vec::with_capacity(40 + stride * side as usize);
    let push32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    push32(&mut dib, 40);
    push32(&mut dib, side as u32);
    push32(&mut dib, side as u32); // bottom-up
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&24u16.to_le_bytes());
    push32(&mut dib, 0);
    push32(&mut dib, (stride * side as usize) as u32);
    for _ in 0..4 {
        push32(&mut dib, 0);
    }
    for row in (0..side).rev() {
        let start = dib.len();
        for col in 0..side {
            let (mx, my) = (col / px - quiet, row / px - quiet);
            let dark = mx >= 0 && my >= 0 && mx < n && my < n && code.get_module(mx, my);
            let v = if dark { 0u8 } else { 255u8 };
            dib.extend_from_slice(&[v, v, v]);
        }
        dib.resize(start + stride, 0);
    }
    (side as u32, dib)
}

impl Module for Tools {
    fn tip(&self, id: u32) -> &'static str {
        match id {
            1 => "Color picker",
            2 => "Calculator",
            3 => "QR code",
            4 => "Unit and time-zone converter",
            A_PICK => "Click anywhere on screen to pick its color",
            A_COPY_HEX | A_COPY_RGB | A_COPY_HSL => "Copy",
            A_CALC_FIELD => "Type a sum; the answer shows as you type",
            A_QR_CLIP => "QR code of what you last copied",
            A_QR_TYPE | A_QR_FIELD => "Type your own text or link",
            A_QR_COPY => "Copy the QR code as a picture",
            i if (A_CAT..=A_CAT + ZONES as u32).contains(&i) => if i == A_CAT + ZONES as u32 { "Time in 12 cities" } else { "Convert this kind of unit" },
            i if (A_FROM..A_FROM + 8).contains(&i) => "Convert from this unit",
            i if (A_TO..A_TO + 8).contains(&i) => "Convert to this unit",
            A_SWAP => "Swap the two units",
            A_VALUE => "Type a number (or a sum)",
            A_TZ_LOCAL => "Compare from this PC's time",
            A_TZ_NOW => "Back to the time now",
            _ => "",
        }
    }
    fn title(&self) -> &'static str {
        "Tools"
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

    fn tick(&mut self, ctx: &mut Ctx) {
        if let Some(at) = self.pick_at {
            if unix_now() >= at {
                self.pick_at = None;
                if !self.no_overlay && !picker::start(self.hwnd) {
                    ctx.open_my_tab();
                }
            }
        }
        let res = picker::RESULT.lock().unwrap().take();
        if let Some(res) = res {
            if let Some(c) = res {
                self.set_color(c);
                if self.no_overlay {
                    self.copied = Some((A_COPY_HEX, unix_now()));
                } else {
                    self.copy(A_COPY_HEX, hex_of(c));
                }
            }
            self.tool = Tool::Color;
            ctx.open_my_tab();
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let row = r.cut_top(26.0, 10.0);
        for (i, (cell, (tool, g, name, _))) in row.columns(TOOLS.len(), 6.0).into_iter().zip(TOOLS).enumerate() {
            let sel = self.tool == tool;
            ui.chip(cell, "", A_TOOL + i as u32, sel, ACCENT);
            let fg = if sel { ACCENT } else { col::DIM };
            let tw = ui.measure(name, 11.5, Weight::Semi);
            let w = 14.0 + 6.0 + tw;
            let x = cell.cx() - w / 2.0;
            ui.icon(g, Rect::new(x, cell.y, 14.0, cell.h), 11.5, fg);
            ui.text(name, Rect::new(x + 20.0, cell.y, tw + 4.0, cell.h), 11.5, fg, Weight::Semi, Align::Left);
        }
        match self.tool {
            Tool::Color => self.draw_color(ui, r),
            Tool::Calc => self.draw_calc(ui, r),
            Tool::Qr => self.draw_qr(ui, r),
            Tool::Convert => self.draw_convert(ui, r),
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        let editing = matches!(id, A_CALC_FIELD | A_QR_FIELD | A_VALUE);
        if !editing && self.focus != Focus::None && !(id == A_QR_TYPE && self.focus == Focus::Qr) {
            self.set_focus(Focus::None, ctx);
        }
        match id {
            i if (A_TOOL..A_TOOL + TOOLS.len() as u32).contains(&i) => {
                self.tool = TOOLS[(i - A_TOOL) as usize].0;
                match self.tool {
                    Tool::Calc => self.set_focus(Focus::Calc, ctx),
                    Tool::Qr => self.refresh_qr_from_clipboard(false),
                    _ => {}
                }
                self.save();
            }
            A_PICK => {
                // Let the notch close first so it's out of the screenshot.
                self.pick_at = Some(unix_now() + 0.35);
                ctx.close_panel();
                let hwnd = self.hwnd.0 as isize;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(380));
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            HWND(hwnd as *mut _),
                            crate::app::WM_APP_WAKE,
                            windows::Win32::Foundation::WPARAM(0),
                            windows::Win32::Foundation::LPARAM(0),
                        );
                    }
                });
            }
            A_COPY_HEX | A_COPY_RGB | A_COPY_HSL => {
                if let Some(c) = self.color {
                    let s = match id {
                        A_COPY_HEX => hex_of(c),
                        A_COPY_RGB => rgb_of(c),
                        _ => hsl_of(c),
                    };
                    self.copy(id, s);
                }
            }
            i if (A_SWATCH..A_SWATCH + MAX_COLORS as u32).contains(&i) => {
                if let Some(c) = self.colors.get((i - A_SWATCH) as usize).copied() {
                    self.color = Some(c);
                }
            }
            A_CALC_FIELD => self.set_focus(Focus::Calc, ctx),
            A_CALC_COPY => {
                if let Ok(v) = self.calc_result() {
                    self.copy(A_CALC_COPY, fmt_num(v).replace(',', ""));
                }
            }
            i if (A_CALC_HIST..A_CALC_HIST + 5).contains(&i) => {
                if let Some((e, _)) = self.calc_hist.get((i - A_CALC_HIST) as usize).cloned() {
                    self.calc = e;
                    self.set_focus(Focus::Calc, ctx);
                }
            }
            A_QR_CLIP => {
                self.qr_typed = false;
                self.refresh_qr_from_clipboard(true);
            }
            A_QR_TYPE | A_QR_FIELD => {
                if !self.qr_typed {
                    self.qr_text.clear();
                }
                self.qr_typed = true;
                self.set_focus(Focus::Qr, ctx);
            }
            A_QR_COPY => {
                if let Some(code) = self.qr_code().cloned() {
                    let (side, dib) = qr_dib(&code, 8);
                    let img = clipboard::Image { w: side, h: side, dib, hash: 0, thumb: (0, 0, Vec::new()) };
                    clipboard::set_clipboard(self.hwnd, &Content::Image(Arc::new(img)));
                    self.copied = Some((A_QR_COPY, unix_now()));
                }
            }
            i if (A_CAT..=A_CAT + ZONES as u32).contains(&i) => {
                self.cat = (i - A_CAT) as usize;
                self.save();
            }
            i if (A_FROM..A_FROM + 8).contains(&i) => {
                if self.cat < ZONES && ((i - A_FROM) as usize) < CATS[self.cat].1.len() {
                    self.units[self.cat].0 = (i - A_FROM) as usize;
                    self.save();
                }
            }
            i if (A_TO..A_TO + 8).contains(&i) => {
                if self.cat < ZONES && ((i - A_TO) as usize) < CATS[self.cat].1.len() {
                    self.units[self.cat].1 = (i - A_TO) as usize;
                    self.save();
                }
            }
            A_SWAP => {
                if self.cat < ZONES {
                    let (a, b) = self.units[self.cat];
                    self.units[self.cat] = (b, a);
                    self.save();
                }
            }
            A_VALUE => self.set_focus(Focus::Value, ctx),
            A_CONV_COPY => {
                if let (Some(v), true) = (self.value_num(), self.cat < ZONES) {
                    let (a, b) = self.units[self.cat];
                    self.copy(A_CONV_COPY, fmt_num(convert(self.cat, a, b, v)).replace(',', ""));
                }
            }
            i if (A_TZ_CITY..A_TZ_CITY + CITIES.len() as u32).contains(&i) => {
                self.tz_base = Some((i - A_TZ_CITY) as usize);
                self.save();
            }
            A_TZ_LOCAL => {
                self.tz_base = None;
                self.save();
            }
            A_TZ_MINUS => self.tz_offset -= 60,
            A_TZ_PLUS => self.tz_offset += 60,
            A_TZ_NOW => self.tz_offset = 0,
            A_TEST_NO_OVERLAY => self.no_overlay = true,
            A_DEMO => {
                self.colors = [[0x3D, 0x8B, 0xFF], [0xFF, 0x7A, 0x59], [0x30, 0xD1, 0x58], [0xFF, 0xD6, 0x0A], [0xBF, 0x5A, 0xF2], [0x64, 0xD2, 0xFF]].to_vec();
                self.color = self.colors.first().copied();
                self.calc = "1250*15%".into();
                self.calc_hist = vec![("200+10%".into(), "220".into()), ("2^10".into(), "1,024".into()), ("sqrt(2)".into(), "1.414213562".into())];
                self.qr_text = format!("https://github.com/{}", crate::modules::update::REPO);
                self.qr_typed = true;
                self.cat = 3;
                self.units[3] = (6, 1);
                self.value = "5".into();
                self.tz_base = Some(0);
                self.tz_offset = 0;
            }
            _ => {}
        }
    }

    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        let text = match self.focus {
            Focus::Calc => &mut self.calc,
            Focus::Qr => &mut self.qr_text,
            Focus::Value => &mut self.value,
            Focus::None => return false,
        };
        match vk {
            0x0D => {
                if self.focus == Focus::Calc {
                    if let Ok(v) = eval(&self.calc) {
                        let e = self.calc.trim().to_string();
                        let res = fmt_num(v);
                        self.calc_hist.retain(|(x, _)| *x != e);
                        self.calc_hist.insert(0, (e, res.clone()));
                        self.calc_hist.truncate(5);
                        self.calc = res.replace(',', "");
                    }
                } else {
                    if self.focus == Focus::Value {
                        self.save();
                    }
                    self.set_focus(Focus::None, ctx);
                }
            }
            0x1B => {
                if self.focus == Focus::Calc && !self.calc.is_empty() {
                    self.calc.clear();
                } else {
                    self.set_focus(Focus::None, ctx);
                }
            }
            0x08 => {
                text.pop();
            }
            // Ctrl+V pastes, Ctrl+C copies the answer.
            0x56 if ctrl => {
                if let Some(t) = crate::modules::todo::clipboard_text() {
                    let t: String = t.chars().filter(|c| !c.is_control()).take(2000).collect();
                    text.push_str(&t);
                }
            }
            0x43 if ctrl && self.focus == Focus::Calc => {
                if let Ok(v) = eval(&self.calc) {
                    self.copy(A_CALC_COPY, fmt_num(v).replace(',', ""));
                }
            }
            _ => return false,
        }
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        let (text, max) = match self.focus {
            Focus::Calc => (&mut self.calc, 200),
            Focus::Qr => (&mut self.qr_text, 2000),
            Focus::Value => (&mut self.value, 40),
            Focus::None => return false,
        };
        if c.is_control() || text.chars().count() >= max {
            return false;
        }
        text.push(c);
        true
    }

    fn blur(&mut self) {
        if self.focus == Focus::Value {
            self.save();
        }
        self.focus = Focus::None;
    }

    fn animating(&self) -> bool {
        self.focus != Focus::None || self.copied.map_or(false, |(_, t)| unix_now() - t < 1.7) || self.pick_at.is_some()
    }

    fn debug_state(&self) -> String {
        format!(
            "tool {:?}, colors {}, calc {:?}, qr {} chars, converter {} | last {}",
            self.tool,
            self.colors.len(),
            self.calc,
            self.qr_text.chars().count(),
            if self.cat == ZONES { "zones" } else { CATS[self.cat].0 },
            self.calc_hist.first().map_or(String::new(), |(e, v)| format!("{e} = {v}"))
        )
    }
}

/// Self-test: QR size for `s`, and whether its clipboard image has the
/// right byte count; None when the text is too long.
pub fn qr_check(s: &str) -> Option<(i32, bool)> {
    let code = qrcodegen::QrCode::encode_text(s, qrcodegen::QrCodeEcc::Medium)
        .or_else(|_| qrcodegen::QrCode::encode_text(s, qrcodegen::QrCodeEcc::Low))
        .ok()?;
    let (side, dib) = qr_dib(&code, 8);
    let stride = ((side * 3 + 3) / 4 * 4) as usize;
    Some((code.size(), dib.len() == 40 + stride * side as usize && side as i32 == (code.size() + 8) * 8))
}

/// Self-test: the newest history entry from `debug_state`.
pub fn test_last(state: String) -> String {
    state.split(" | last ").nth(1).unwrap_or("").to_string()
}

impl Tools {
    /// Self-test helpers.
    pub fn test_state(&self) -> (Option<[u8; 3]>, bool, String) {
        (self.color, self.pick_at.is_some(), self.calc.clone())
    }
    pub fn test_set_calc(&mut self, s: &str) {
        self.calc = s.to_string();
    }
    pub fn test_hist(&self) -> Vec<(String, String)> {
        self.calc_hist.clone()
    }
    pub fn test_tool(&mut self, i: usize) {
        self.tool = TOOLS[i].0;
    }
    pub fn test_qr(&mut self, s: &str) -> Option<i32> {
        self.qr_text = s.to_string();
        self.qr_typed = true;
        self.qr_code().map(|c| c.size())
    }
    pub fn test_cat(&mut self, c: usize) {
        self.cat = c;
    }
}
