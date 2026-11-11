//! Local wall-clock helpers (no chrono dependency).

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};

pub const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Docs/demo mode (`windyland --render-docs`): fixed 16:20 clock, demo media.
pub static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn demo() -> bool {
    DEMO.load(std::sync::atomic::Ordering::Relaxed)
}

/// 12-hour clock (Settings > General); mirrors `Settings::clock12`.
pub static H12: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Show seconds on the clock face (Settings > Look); mirrors
/// `Settings::seconds`.
pub static SECONDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn seconds() -> bool {
    SECONDS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn h12() -> bool {
    H12.load(std::sync::atomic::Ordering::Relaxed)
}

/// Clock face: "4:20" (12-hour, like a phone's status bar) or "16:20".
pub fn clock(hour: u32, min: u32) -> String {
    if h12() {
        let h = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h}:{min:02}")
    } else {
        format!("{:02}:{min:02}", hour % 24)
    }
}

/// A time in a list or message: "4:20 PM" or "16:20".
pub fn clock_full(hour: u32, min: u32) -> String {
    if h12() {
        format!("{} {}", clock(hour, min), if hour % 24 < 12 { "AM" } else { "PM" })
    } else {
        clock(hour, min)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Now {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub dow: u16,
    pub hour: u16,
    pub min: u16,
    pub sec: u16,
    /// Unix time in seconds (UTC, fractional).
    pub unix: f64,
}

impl Now {
    pub fn get() -> Now {
        let mut st: SYSTEMTIME = unsafe { GetLocalTime() };
        if demo() {
            st.wHour = 16;
            st.wMinute = 20;
            st.wSecond = 0;
        }
        Now {
            year: st.wYear,
            month: st.wMonth,
            day: st.wDay,
            dow: st.wDayOfWeek,
            hour: st.wHour,
            min: st.wMinute,
            sec: st.wSecond,
            unix: unix_now(),
        }
    }
    /// "2026-10-27", used as a key for per-day data.
    pub fn day_key(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
    /// Minutes since local midnight.
    pub fn minute_of_day(&self) -> u32 {
        self.hour as u32 * 60 + self.min as u32
    }
    /// The clock face ("4:20" or "16:20").
    pub fn hhmm(&self) -> String {
        if seconds() {
            format!("{}:{:02}", clock(self.hour as u32, self.min as u32), self.sec)
        } else {
            clock(self.hour as u32, self.min as u32)
        }
    }
    /// "4:20 PM" or "16:20".
    pub fn hhmm_full(&self) -> String {
        clock_full(self.hour as u32, self.min as u32)
    }
    /// "Tue 27 Oct"
    pub fn short_date(&self) -> String {
        format!(
            "{} {} {}",
            DAYS[self.dow as usize % 7],
            self.day,
            MONTHS[(self.month as usize).saturating_sub(1) % 12]
        )
    }
}

/// Unix seconds from the system clock.
/// Days in a month (1-12).
pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Day of the week, 0 = Sunday.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    // Sakamoto's method.
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32).rem_euclid(7)) as u32
}

/// The local date (year, month, day) of a unix time.
pub fn local_date(unix: f64) -> (i32, u32, u32) {
    let (y, m, d, _) = local_time(unix);
    (y, m, d)
}

/// The local date and minute of the day of a unix time.
pub fn local_time(unix: f64) -> (i32, u32, u32, u32) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let t = ((unix * 10_000_000.0) as i64 + 116_444_736_000_000_000) as u64;
    let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut loc = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc).is_err() {
            return (1970, 1, 1, 0);
        }
    }
    (loc.wYear as i32, loc.wMonth as u32, loc.wDay as u32, loc.wHour as u32 * 60 + loc.wMinute as u32)
}

pub fn unix_now() -> f64 {
    let ft = unsafe { GetSystemTimeAsFileTime() };
    let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    filetime_to_unix(t as i64)
}

/// Convert 100-ns ticks since 1601 (FILETIME / WinRT DateTime) to Unix seconds.
pub fn filetime_to_unix(t: i64) -> f64 {
    (t as f64 - 116_444_736_000_000_000.0) / 10_000_000.0
}

/// "3:07" or "1:03:07"
pub fn fmt_dur(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Minutes-of-day to "9:30 AM" (or "09:30" on the 24-hour clock).
pub fn fmt_mod(m: u32) -> String {
    clock_full((m / 60) % 24, m % 60)
}
