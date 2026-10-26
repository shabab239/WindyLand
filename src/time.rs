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
    pub fn hhmm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.min)
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

/// Minutes-of-day to "09:30".
pub fn fmt_mod(m: u32) -> String {
    format!("{:02}:{:02}", (m / 60) % 24, m % 60)
}
