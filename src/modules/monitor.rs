//! System monitor: CPU, memory and network speed, with a minute of history,
//! plus free space on the Windows drive and uptime. Optionally shows CPU and
//! download speed on the closed notch. Reads Windows' own counters once a
//! second (the notch already ticks once a second), so it costs nothing
//! extra while idle.

use windows::core::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::GetSystemTimes;

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel};
use crate::time::unix_now;
use crate::ui::Ui;

pub const ACCENT: Color = hex(0x5AC8FA);
pub const ICON: &str = "\u{E9D9}";
const C_CPU: Color = hex(0x5AC8FA);
const C_MEM: Color = hex(0xBF5AF2);
const C_DOWN: Color = hex(0x30D158);
const C_UP: Color = hex(0xFF9F0A);
const HISTORY: usize = 60;

const A_LIVE: u32 = 1;
/// Demo video / docs renders: steady made-up numbers.
pub const A_DEMO: u32 = 990;

#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub down: f64,
    pub up: f64,
}

fn ft(f: &windows::Win32::Foundation::FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// (idle, total) CPU time so far, in 100 ns units, over all cores.
pub fn cpu_times() -> Option<(u64, u64)> {
    let (mut i, mut k, mut u) = Default::default();
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)) }.ok()?;
    // Kernel time includes idle time.
    Some((ft(&i), ft(&k) + ft(&u)))
}

/// Bytes received and sent so far on real network adapters (Wi-Fi,
/// Ethernet, phone tethering). Loopback and the filter layers Windows
/// stacks on each adapter are skipped so nothing is counted twice.
pub fn net_bytes() -> Option<(u64, u64)> {
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_err() || t.is_null() {
            return None;
        }
        let n = (*t).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), n);
        let (mut rx, mut tx) = (0u64, 0u64);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 1 != 0;
            let filter = flags & 2 != 0;
            if hardware && !filter && r.Type != 24 {
                rx += r.InOctets;
                tx += r.OutOctets;
            }
        }
        FreeMibTable(t as *const _);
        Some((rx, tx))
    }
}

pub fn memory() -> (u64, u64) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() {
        (m.ullTotalPhys - m.ullAvailPhys, m.ullTotalPhys)
    } else {
        (0, 0)
    }
}

/// Free and total bytes on the Windows drive.
pub fn disk() -> (u64, u64) {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
    let (mut free, mut total) = (0u64, 0u64);
    let h = HSTRING::from(drive.as_str());
    let _ = unsafe { GetDiskFreeSpaceExW(&h, None, Some(&mut total), Some(&mut free)) };
    (free, total)
}

/// 1.2 MB/s, 340 KB/s, 0 B/s (1024-based, like Task Manager's file sizes).
pub fn speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / 1048576.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps.max(0.0))
    }
}

/// Short form for the closed notch: 1.2M, 340K, 0K.
fn speed_short(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1}M", bps / 1048576.0)
    } else {
        format!("{:.0}K", bps / 1024.0)
    }
}

pub fn gb(b: u64) -> String {
    let g = b as f64 / 1073741824.0;
    if g >= 100.0 {
        format!("{g:.0} GB")
    } else {
        format!("{g:.1} GB")
    }
}

fn uptime() -> String {
    let s = unsafe { GetTickCount64() } / 1000;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

pub struct Monitor {
    hist: Vec<Sample>,
    last_cpu: Option<(u64, u64)>,
    last_net: Option<(u64, u64, f64)>,
    disk: (u64, u64),
    disk_at: f64,
    live: bool,
    demo: bool,
}

impl Monitor {
    pub fn new() -> Monitor {
        let live = crate::store::Settings::load().extra.get("monitor_live").map_or(false, |v| v == "1");
        let mut m = Monitor { hist: Vec::new(), last_cpu: None, last_net: None, disk: (0, 0), disk_at: 0.0, live, demo: false };
        m.sample();
        m
    }

    /// Read the counters; speeds are averaged since the previous read.
    pub fn sample(&mut self) {
        if self.demo {
            return;
        }
        let now = unix_now();
        let cpu = match (cpu_times(), self.last_cpu) {
            (Some((i, t)), Some((pi, pt))) if t > pt => 100.0 * (1.0 - (i.saturating_sub(pi)) as f64 / (t - pt) as f64),
            _ => -1.0,
        };
        self.last_cpu = cpu_times();
        let (mut down, mut up) = (-1.0, -1.0);
        if let Some((rx, tx)) = net_bytes() {
            if let Some((prx, ptx, pt)) = self.last_net {
                let dt = now - pt;
                if dt > 0.2 {
                    down = rx.saturating_sub(prx) as f64 / dt;
                    up = tx.saturating_sub(ptx) as f64 / dt;
                }
            }
            self.last_net = Some((rx, tx, now));
        }
        if now - self.disk_at > 30.0 {
            self.disk = disk();
            self.disk_at = now;
        }
        if cpu < 0.0 {
            return;
        }
        let (mem_used, mem_total) = memory();
        self.hist.push(Sample { cpu: cpu.clamp(0.0, 100.0) as f32, mem_used, mem_total, down: down.max(0.0), up: up.max(0.0) });
        if self.hist.len() > HISTORY {
            self.hist.remove(0);
        }
    }

    pub fn latest(&self) -> Option<Sample> {
        self.hist.last().copied()
    }

    fn spark(ui: &mut Ui, r: Rect, vals: &[f32], max: f32, c: Color) {
        if vals.len() < 2 || max <= 0.0 {
            return;
        }
        let step = r.w / (HISTORY - 1) as f32;
        let x0 = r.r() - step * (vals.len() - 1) as f32;
        let y = |v: f32| r.b() - (v / max).clamp(0.0, 1.0) * r.h;
        let mut pts: Vec<(f32, f32)> = vals.iter().enumerate().map(|(i, v)| (x0 + i as f32 * step, y(*v))).collect();
        for w in pts.windows(2) {
            ui.line(w[0].0, w[0].1, w[1].0, w[1].1, c, 1.6);
        }
        pts.push((r.r(), r.b()));
        pts.push((x0, r.b()));
        ui.polygon(&pts, c.with_a(0.14));
    }

    fn card(ui: &mut Ui, r: Rect, title: &str, big: &str, sub: &str, c: Color) -> Rect {
        ui.fill_rr(r, 12.0, col::BTN);
        let mut inner = r.inset_xy(12.0, 9.0);
        ui.text(title, inner.cut_top(14.0, 2.0), 10.5, col::DIM, Weight::Semi, Align::Left);
        ui.text(big, inner.cut_top(26.0, 0.0), 21.0, c, Weight::Semi, Align::Left);
        ui.text(sub, inner.cut_top(15.0, 4.0), 10.5, col::FAINT, Weight::Regular, Align::Left);
        inner
    }
}

impl Module for Monitor {
    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_LIVE => "Show CPU and download speed on the closed notch",
            _ => "",
        }
    }
    fn title(&self) -> &'static str {
        "System Monitor"
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

    fn tick(&mut self, _ctx: &mut Ctx) {
        // tick also runs right after background wake-ups: one read per second.
        let due = self.last_cpu.is_none() || self.last_net.map_or(true, |(_, _, t)| unix_now() - t >= 0.9);
        if due {
            self.sample();
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let foot = r.cut_bottom(24.0, 8.0);
        let s = self.latest().unwrap_or_default();
        let cols = r.columns(3, 10.0);
        let cpu: Vec<f32> = self.hist.iter().map(|h| h.cpu).collect();
        let a = Self::card(ui, cols[0], "CPU", &format!("{:.0}%", s.cpu), &format!("{} cores", std::thread::available_parallelism().map_or(0, |n| n.get())), C_CPU);
        Self::spark(ui, a, &cpu, 100.0, C_CPU);

        let mem_pct = if s.mem_total > 0 { s.mem_used as f32 / s.mem_total as f32 } else { 0.0 };
        let mut b = Self::card(ui, cols[1], "Memory", &format!("{:.0}%", mem_pct * 100.0), &format!("{} of {} used", gb(s.mem_used), gb(s.mem_total)), C_MEM);
        let mem: Vec<f32> = self.hist.iter().map(|h| if h.mem_total > 0 { h.mem_used as f32 / h.mem_total as f32 * 100.0 } else { 0.0 }).collect();
        let bar = b.cut_bottom(6.0, 0.0);
        Self::spark(ui, Rect::new(b.x, b.y, b.w, b.h - 6.0), &mem, 100.0, C_MEM);
        ui.fill_rr(bar, 3.0, col::TEXT.with_a(0.1));
        ui.fill_rr(Rect::new(bar.x, bar.y, bar.w * mem_pct.clamp(0.0, 1.0), bar.h), 3.0, C_MEM);

        let net = cols[2];
        ui.fill_rr(net, 12.0, col::BTN);
        let mut inner = net.inset_xy(12.0, 9.0);
        ui.text("Network", inner.cut_top(14.0, 2.0), 10.5, col::DIM, Weight::Semi, Align::Left);
        for (arrow, v, c) in [("\u{2193}", s.down, C_DOWN), ("\u{2191}", s.up, C_UP)] {
            let mut row = inner.cut_top(20.0, 0.0);
            ui.text(arrow, row.cut_left(14.0, 2.0), 14.0, c, Weight::Semi, Align::Left);
            ui.text(&speed(v), row, 14.5, c, Weight::Semi, Align::Left);
        }
        let down: Vec<f32> = self.hist.iter().map(|h| h.down as f32).collect();
        let up: Vec<f32> = self.hist.iter().map(|h| h.up as f32).collect();
        let max = down.iter().chain(up.iter()).cloned().fold(64.0 * 1024.0, f32::max) * 1.15;
        let sp = inner.inset_xy(0.0, 2.0);
        Self::spark(ui, sp, &up, max, C_UP);
        Self::spark(ui, sp, &down, max, C_DOWN);

        let mut foot = foot;
        ui.toggle(foot.cut_right(190.0, 0.0), "Show on closed notch", self.live, A_LIVE, ACCENT);
        let (free, total) = self.disk;
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let mut info = format!("Up {}", if crate::time::demo() { "2 d 5 h".to_string() } else { uptime() });
        if total > 0 {
            info = format!("{drive} {} free of {} · {info}", gb(free), gb(total));
        }
        ui.text(&info, foot, 11.0, col::DIM, Weight::Regular, Align::Left);
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_LIVE => {
                self.live = !self.live;
                ctx.settings.extra.insert("monitor_live".into(), if self.live { "1" } else { "0" }.into());
                ctx.settings.save();
            }
            A_DEMO => {
                self.demo = true;
                self.hist = (0..HISTORY)
                    .map(|i| {
                        let t = i as f32;
                        let cpu = 18.0 + 9.0 * (t * 0.35).sin() + 6.0 * (t * 0.9).cos() + if (38..44).contains(&i) { 30.0 } else { 0.0 };
                        let down = 1.6e6 + 1.2e6 * (t * 0.21).sin().abs() as f64 * 1.0 + if (20..30).contains(&i) { 3.5e6 } else { 0.0 };
                        Sample { cpu, mem_used: 10_200_000_000 + (i as u64) * 4_000_000, mem_total: 17_179_869_184, down, up: 180_000.0 + 90_000.0 * (t * 0.5).cos().abs() as f64 }
                    })
                    .collect();
                self.disk = (213_000_000_000, 511_000_000_000);
                self.disk_at = f64::MAX;
            }
            _ => {}
        }
    }

    fn live(&self) -> Option<u8> {
        (self.live && self.latest().is_some()).then_some(5)
    }

    fn live_color(&self) -> Color {
        ACCENT
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let Some(s) = self.latest() else { return };
        let size = (left.h * 0.46).clamp(9.0, 12.0);
        let mut l = left;
        ui.icon(ICON, l.cut_left(left.h, 2.0), (left.h * 0.46).min(12.0), C_CPU);
        ui.text(&format!("{:.0}%", s.cpu), l, size, col::TEXT, Weight::Semi, Align::Left);
        ui.text(&format!("\u{2193} {}", speed_short(s.down)), right, size, C_DOWN, Weight::Semi, Align::Right);
    }

    fn debug_state(&self) -> String {
        match self.latest() {
            Some(s) => format!("cpu {:.1}%, mem {} of {}, down {}, up {}, disk {} free, samples {}, live {}", s.cpu, gb(s.mem_used), gb(s.mem_total), speed(s.down), speed(s.up), gb(self.disk.0), self.hist.len(), self.live),
            None => format!("no samples yet, live {}", self.live),
        }
    }
}
