//! Widgets tab: what the closed notch shows, in order. "Pick" switches
//! items on (live activities like music and the timer, and widgets like
//! the battery); "Order" arranges them in rows, dragged or moved with the
//! arrows: a row holds one item across both sides of the clock or two
//! one-side items. The first row with something to show wins. Other panels offer their own widgets through
//! `Module::widgets` (CPU, water, next prayer...); this module adds the PC
//! battery, Bluetooth device batteries, free disk space, uptime and the date.
//!
//! Cheap by design: the PC battery is one Windows call every 5 s. Bluetooth
//! runs on its own thread only while its widget is placed or this tab is
//! open: once a minute it reads the battery Windows keeps for connected
//! devices, and for connected AirPods (which don't report it to Windows) it
//! listens about 3 s to the battery broadcast AirPods send. Wi-Fi is read
//! every 15 s and the speaker / microphone every second, only while their
//! widgets are on (or this tab is open). Weather is the one widget that
//! goes online: Open-Meteo, every 30 minutes, only while it is switched on.
//! Nothing is stored or sent anywhere else.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Devices::Bluetooth::Advertisement::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::gfx::*;
use crate::modules::{CatalogItem, Ctx, Module, Rel, Widget};
use crate::store::{AlertMode, NotchLayout, NotchRow, PlaceMode, ShowWhen};
use crate::time::{unix_now, Now, DAYS, MONTHS};
use crate::ui::Ui;

pub const ACCENT: Color = hex(0x64D2FF);
pub const ICON: &str = "\u{ECA5}";
const C_GOOD: Color = hex(0x30D158);
const C_WARN: Color = hex(0xFF9F0A);
const C_LOW: Color = hex(0xFF453A);
const DISK: &str = "\u{EDA2}";
const UPTIME: &str = "\u{E7E8}";
const CALENDAR: &str = "\u{E787}";


// ------------------------------------------------------------ Bluetooth

/// A connected Bluetooth device and what we know of its battery.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BtDevice {
    pub name: String,
    /// Battery Windows reports (headsets, mice, keyboards...).
    pub pct: Option<u8>,
    /// AirPods: left, right and case (from their broadcast, 10 % steps).
    pub pods: Option<Pods>,
    pub audio: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pods {
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    pub charging: bool,
}

impl BtDevice {
    /// The level to show: the lower bud for AirPods.
    pub fn level(&self) -> Option<u8> {
        match self.pods {
            Some(p) => p.left.into_iter().chain(p.right).min().or(p.case),
            None => self.pct,
        }
    }
    /// "AirPods", "MX Master" (for the notch).
    fn short_name(&self) -> String {
        if self.name.contains("AirPods") {
            return "AirPods".into();
        }
        if self.name.contains("Beats") {
            return "Beats".into();
        }
        let n: String = self.name.chars().take(10).collect();
        n.trim().to_string()
    }
    fn detail(&self) -> String {
        let pc = |v: Option<u8>| v.map_or("–".to_string(), |v| format!("{v}%"));
        match self.pods {
            Some(p) => format!("L {} · R {} · Case {}{}", pc(p.left), pc(p.right), pc(p.case), if p.charging { " · charging" } else { "" }),
            None => format!("{} {}", self.short_name(), pc(self.pct)),
        }
    }
}

/// Decode the battery broadcast AirPods (and Beats) send: Apple's
/// manufacturer data, type 0x07, 27 bytes. Levels come in tenths; 15
/// means unknown (out of the case, or the case is closed).
pub fn decode_pods(d: &[u8]) -> Option<Pods> {
    if d.len() != 27 || d[0] != 0x07 {
        return None;
    }
    let lvl = |n: u8| (n <= 10).then_some(n * 10);
    // Which bud reports first depends on which one is the primary.
    let flip = (d[5] >> 4) & 0x02 == 0;
    let (hi, lo) = (d[6] >> 4, d[6] & 0x0F);
    let (l, r) = if flip { (hi, lo) } else { (lo, hi) };
    let charge = d[7] >> 4;
    Some(Pods { left: lvl(l), right: lvl(r), case: lvl(d[7] & 0x0F), charging: charge & 0x07 != 0 })
}

struct BtShared {
    devices: Vec<BtDevice>,
    read_at: f64,
}

static BT_WANTED_AT: AtomicU64 = AtomicU64::new(0);
static BT_STARTED: AtomicBool = AtomicBool::new(false);

fn want_bluetooth() {
    BT_WANTED_AT.store(unix_now() as u64, Ordering::Relaxed);
}

fn spawn_bt(shared: Arc<Mutex<BtShared>>) {
    if BT_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut last = 0.0;
        loop {
            let now = unix_now();
            // Asked for in the last 2 minutes (widget placed, tab open).
            let wanted = now - (BT_WANTED_AT.load(Ordering::Relaxed) as f64) < 120.0;
            if wanted && now - last >= 60.0 {
                let devices = read_bluetooth();
                last = unix_now();
                let mut s = shared.lock().unwrap();
                s.devices = devices;
                s.read_at = last;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

const fn key(fmtid: u128, pid: u32) -> DEVPROPKEY {
    DEVPROPKEY { fmtid: GUID::from_u128(fmtid), pid }
}
const K_BATTERY: DEVPROPKEY = key(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5, 2);
const K_CONNECTED: DEVPROPKEY = key(0x83DA6326_97A6_4088_9453_A1923F573B29, 15);
const K_ADDRESS: DEVPROPKEY = key(0x2BD67D8B_8BEB_48D5_87E0_6CDA3428040A, 1);
const K_VID: DEVPROPKEY = key(0x2BD67D8B_8BEB_48D5_87E0_6CDA3428040A, 7);
const K_CLASS: DEVPROPKEY = key(0x2BD67D8B_8BEB_48D5_87E0_6CDA3428040A, 10);

/// Raw property bytes of one device.
unsafe fn prop(set: HDEVINFO, d: &SP_DEVINFO_DATA, k: &DEVPROPKEY) -> Option<Vec<u8>> {
    let mut ty = DEVPROPTYPE(0);
    let mut buf = vec![0u8; 512];
    let mut need = 0u32;
    SetupDiGetDevicePropertyW(set, d, k, &mut ty, Some(&mut buf), Some(&mut need), 0).ok()?;
    buf.truncate(need as usize);
    Some(buf)
}

fn utf16(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    String::from_utf16_lossy(&w)
}

/// Connected Bluetooth devices with what Windows knows of their battery;
/// AirPods get theirs from a short listen to their broadcast.
fn read_bluetooth() -> Vec<BtDevice> {
    // address -> (name, connected, apple, audio); address -> battery
    let mut devs: Vec<(String, String, bool, bool, bool)> = Vec::new();
    let mut batt: Vec<(String, u8)> = Vec::new();
    unsafe {
        for e in [w!("BTHENUM"), w!("BTHLE")] {
            let Ok(set) = SetupDiGetClassDevsW(None, e, None, DIGCF_PRESENT | DIGCF_ALLCLASSES) else { continue };
            let mut i = 0;
            loop {
                let mut d = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInfo(set, i, &mut d).is_err() {
                    break;
                }
                i += 1;
                let Some(addr) = prop(set, &d, &K_ADDRESS).map(|b| utf16(&b)) else { continue };
                if let Some(b) = prop(set, &d, &K_BATTERY) {
                    if let Some(&p) = b.first() {
                        if p <= 100 {
                            batt.push((addr.clone(), p));
                        }
                    }
                }
                let mut id = [0u16; 400];
                let _ = SetupDiGetDeviceInstanceIdW(set, &d, Some(&mut id), None);
                let id = String::from_utf16_lossy(&id).trim_end_matches('\0').to_string();
                // The device itself (not one of its services).
                if id.contains("\\DEV_") {
                    let connected = prop(set, &d, &K_CONNECTED).map_or(false, |b| b.first().map_or(false, |v| *v != 0));
                    let name = prop(set, &d, &DEVPKEY_Device_FriendlyName).map(|b| utf16(&b)).unwrap_or_default();
                    let apple = prop(set, &d, &K_VID).map_or(false, |b| b.len() >= 2 && u16::from_le_bytes([b[0], b[1]]) == 0x004C);
                    // Class of device: major class 4 = audio/video.
                    let audio = prop(set, &d, &K_CLASS).map_or(false, |b| b.len() >= 4 && (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 8) & 0x1F == 4);
                    if connected && !name.trim().is_empty() {
                        devs.push((addr, name.trim().to_string(), connected, apple, audio));
                    }
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
    }
    let mut out: Vec<BtDevice> = Vec::new();
    let mut want_pods = false;
    for (addr, name, _, apple, audio) in &devs {
        if out.iter().any(|d| &d.name == name) {
            continue;
        }
        let pct = batt.iter().find(|(a, _)| a.eq_ignore_ascii_case(addr)).map(|(_, p)| *p);
        let pods_like = *apple && *audio;
        want_pods |= pods_like && pct.is_none();
        out.push(BtDevice { name: name.clone(), pct, pods: None, audio: *audio });
    }
    if want_pods {
        if let Some(p) = listen_pods(3.0) {
            for d in out.iter_mut().filter(|d| d.pct.is_none() && (d.name.contains("AirPods") || d.name.contains("Beats"))) {
                d.pods = Some(p);
            }
        }
    }
    out
}

/// Listen to Bluetooth LE broadcasts for `secs` and return the AirPods
/// battery from the strongest one heard (almost always your own).
fn listen_pods(secs: f64) -> Option<Pods> {
    let best: Arc<Mutex<Option<(i16, Pods)>>> = Arc::new(Mutex::new(None));
    let watcher = BluetoothLEAdvertisementWatcher::new().ok()?;
    let _ = watcher.SetScanningMode(BluetoothLEScanningMode::Passive);
    let b2 = best.clone();
    let handler = windows::Foundation::TypedEventHandler::<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let rssi = args.RawSignalStrengthInDBm().unwrap_or(-127);
        if rssi < -75 {
            return Ok(());
        }
        let Ok(list) = args.Advertisement().and_then(|a| a.ManufacturerData()) else { return Ok(()) };
        for md in list {
            if md.CompanyId().unwrap_or(0) != 0x004C {
                continue;
            }
            let Ok(buf) = md.Data() else { continue };
            let Ok(r) = DataReader::FromBuffer(&buf) else { continue };
            let n = r.UnconsumedBufferLength().unwrap_or(0) as usize;
            let mut bytes = vec![0u8; n];
            if r.ReadBytes(&mut bytes).is_err() {
                continue;
            }
            if let Some(p) = decode_pods(&bytes) {
                let mut b = b2.lock().unwrap();
                if b.map_or(true, |(r0, _)| rssi > r0) {
                    *b = Some((rssi, p));
                }
            }
        }
        Ok(())
    });
    let token = watcher.Received(&handler).ok()?;
    if watcher.Start().is_err() {
        let _ = watcher.RemoveReceived(token);
        return None;
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    let _ = watcher.Stop();
    let _ = watcher.RemoveReceived(token);
    let r = best.lock().unwrap().map(|(_, p)| p);
    r
}

// ------------------------------------------------------------ PC battery

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Power {
    pub pct: u8,
    pub on_ac: bool,
    pub charging: bool,
    /// Seconds left on battery, when Windows knows.
    pub left: Option<u32>,
}

/// None on a PC without a battery.
pub fn power() -> Option<Power> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some(Power {
        pct: s.BatteryLifePercent,
        on_ac: s.ACLineStatus == 1,
        charging: s.BatteryFlag & 8 != 0,
        left: (s.BatteryLifeTime != u32::MAX && s.ACLineStatus != 1).then_some(s.BatteryLifeTime),
    })
}

fn level_color(p: u8) -> Color {
    if p <= 15 {
        C_LOW
    } else if p <= 30 {
        C_WARN
    } else {
        C_GOOD
    }
}

fn hm(secs: u32) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

/// Uptime, short: 2d 5h, 3h 10m, 25m.
fn uptime_short() -> String {
    if crate::time::demo() {
        return "2d 5h".into();
    }
    let s = unsafe { GetTickCount64() } / 1000;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

// ------------------------------------------------------------ weather

/// Current weather for the prayer-times city, from Open-Meteo (free, no
/// account). Only fetched while the Weather widget is on (or this tab is
/// open), every 30 minutes, with the place rounded to about 10 km.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherNow {
    pub temp: f32,
    pub code: u32,
    pub day: bool,
    pub fahrenheit: bool,
    pub place: String,
    pub at: f64,
}

static WEATHER_WANTED_AT: AtomicU64 = AtomicU64::new(0);
static WEATHER_STARTED: AtomicBool = AtomicBool::new(false);

/// The weather thread has started (for the self-test: not while it's off).
pub fn weather_started() -> bool {
    WEATHER_STARTED.load(Ordering::Relaxed)
}

/// °F instead of °C (Place page), shared with the weather thread.
static WEATHER_F: AtomicBool = AtomicBool::new(false);

/// The weather URL path for a place (rounded to 0.1°, about 10 km).
pub fn weather_path(lat: f64, lon: f64, fahrenheit: bool) -> String {
    format!(
        "/v1/forecast?latitude={:.1}&longitude={:.1}&current=temperature_2m,weather_code,is_day{}",
        lat,
        lon,
        if fahrenheit { "&temperature_unit=fahrenheit" } else { "" }
    )
}

/// Read Open-Meteo's answer.
pub fn parse_weather(body: &str) -> Option<(f32, u32, bool)> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let c = v.get("current")?;
    Some((c.get("temperature_2m")?.as_f64()? as f32, c.get("weather_code")?.as_u64()? as u32, c.get("is_day").and_then(|d| d.as_u64()).unwrap_or(1) == 1))
}

/// A word and an icon for a WMO weather code.
pub fn weather_kind(code: u32, day: bool) -> (&'static str, &'static str) {
    const SUN: &str = "\u{E706}";
    const MOON: &str = "\u{E708}";
    const CLOUD: &str = "\u{E753}";
    const BOLT: &str = "\u{E945}";
    match code {
        0 => ("Clear", if day { SUN } else { MOON }),
        1 | 2 => ("Partly cloudy", if day { SUN } else { MOON }),
        3 => ("Cloudy", CLOUD),
        45 | 48 => ("Fog", CLOUD),
        51..=57 => ("Drizzle", glyph::DROP),
        61..=67 | 80..=82 => ("Rain", glyph::DROP),
        71..=77 | 85 | 86 => ("Snow", CLOUD),
        95..=99 => ("Storm", BOLT),
        _ => ("Weather", CLOUD),
    }
}

fn want_weather() {
    WEATHER_WANTED_AT.store(unix_now() as u64, Ordering::Relaxed);
}

fn spawn_weather(shared: Arc<Mutex<Option<WeatherNow>>>) {
    if WEATHER_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let mut next = 0.0;
        let mut last_place = String::new();
        let mut last_f = false;
        let mut loc = (0.0, 0.0, String::new());
        let mut loc_at = 0.0;
        loop {
            let now = unix_now();
            let wanted = now - (WEATHER_WANTED_AT.load(Ordering::Relaxed) as f64) < 120.0;
            if wanted {
                // The place the Place page resolved.
                if now - loc_at >= 5.0 {
                    if let Some(p) = PLACE.lock().unwrap().clone() {
                        loc = p;
                    }
                    loc_at = now;
                }
                let (lat, lon, place) = loc.clone();
                let f = WEATHER_F.load(Ordering::Relaxed);
                if !place.is_empty() && (now >= next || place != last_place || f != last_f) {
                    last_f = f;
                    let got = crate::http::get("api.open-meteo.com", &weather_path(lat, lon, f))
                        .filter(|r| r.status == 200)
                        .and_then(|r| parse_weather(&String::from_utf8_lossy(&r.body)));
                    match got {
                        Some((temp, code, day)) => {
                            *shared.lock().unwrap() = Some(WeatherNow { temp, code, day, fahrenheit: f, place: place.clone(), at: unix_now() });
                            next = unix_now() + 30.0 * 60.0;
                        }
                        // Offline: try again in 5 minutes.
                        None => next = unix_now() + 5.0 * 60.0,
                    }
                    last_place = place;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

// ------------------------------------------------------------ Wi-Fi

#[derive(Clone, Debug, PartialEq)]
pub enum Wifi {
    NoAdapter,
    Disconnected,
    /// Network name (empty when Windows keeps it private) and signal 0..100.
    Connected(String, u32),
}

/// The Wi-Fi connection right now. On Windows 11 the network's name needs
/// location access for desktop apps; without it only the signal shows.
pub fn wifi() -> Wifi {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::*;
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
            return Wifi::NoAdapter;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut out = Wifi::NoAdapter;
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            if n > 0 {
                out = Wifi::Disconnected;
            }
            for it in items {
                if it.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                let r = WlanQueryInterface(h, &it.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None);
                if r == 0 && !data.is_null() {
                    let a = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let s = &a.wlanAssociationAttributes.dot11Ssid;
                    let name = String::from_utf8_lossy(&s.ucSSID[..(s.uSSIDLength as usize).min(32)]).to_string();
                    out = Wifi::Connected(name, a.wlanAssociationAttributes.wlanSignalQuality.min(100));
                    WlanFreeMemory(data);
                } else {
                    out = Wifi::Connected(String::new(), 0);
                }
                break;
            }
            WlanFreeMemory(list as *const core::ffi::c_void);
        }
        let _ = WlanCloseHandle(h, None);
        out
    }
}

// ------------------------------------------------------------ sound

/// Speaker volume and mute, and whether the microphone is muted, from the
/// default devices.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoundNow {
    pub volume: Option<(f32, bool)>,
    pub mic_muted: Option<bool>,
}

struct AudioEps {
    out: Option<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume>,
    mic: Option<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume>,
    at: f64,
}

impl AudioEps {
    fn open() -> AudioEps {
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
        unsafe {
            let en: Option<IMMDeviceEnumerator> = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();
            let get = |flow| en.as_ref().and_then(|e| e.GetDefaultAudioEndpoint(flow, eConsole).ok()).and_then(|d| d.Activate(CLSCTX_ALL, None).ok());
            AudioEps { out: get(eRender), mic: get(eCapture), at: unix_now() }
        }
    }
    fn read(&self) -> SoundNow {
        unsafe {
            SoundNow {
                volume: self.out.as_ref().and_then(|e| Some((e.GetMasterVolumeLevelScalar().ok()?, e.GetMute().ok()?.as_bool()))),
                mic_muted: self.mic.as_ref().and_then(|e| e.GetMute().ok().map(|b| b.as_bool())),
            }
        }
    }
}

// ------------------------------------------------------------ place

/// Where you are, as the Widgets tab's Place page resolved it: latitude,
/// longitude and a name. The weather (and prayer times, if you chose so)
/// read it.
pub static PLACE: Mutex<Option<(f64, f64, String)>> = Mutex::new(None);

/// The main city of each Windows time zone: the Auto place (offline, no
/// permission, not fooled by a VPN).
const TZ_CITIES: [(&str, &str, f64, f64); 49] = [
    ("Sri Lanka Standard Time", "Colombo", 6.93, 79.85),
    ("Bangladesh Standard Time", "Dhaka", 23.81, 90.41),
    ("India Standard Time", "New Delhi", 28.61, 77.21),
    ("Pakistan Standard Time", "Karachi", 24.86, 67.01),
    ("Nepal Standard Time", "Kathmandu", 27.72, 85.32),
    ("Myanmar Standard Time", "Yangon", 16.84, 96.17),
    ("SE Asia Standard Time", "Bangkok", 13.76, 100.50),
    ("Singapore Standard Time", "Singapore", 1.35, 103.82),
    ("Malay Peninsula Standard Time", "Kuala Lumpur", 3.14, 101.69),
    ("China Standard Time", "Beijing", 39.90, 116.40),
    ("Taipei Standard Time", "Taipei", 25.03, 121.57),
    ("Tokyo Standard Time", "Tokyo", 35.68, 139.69),
    ("Korea Standard Time", "Seoul", 37.57, 126.98),
    ("Arabian Standard Time", "Dubai", 25.20, 55.27),
    ("Arab Standard Time", "Riyadh", 24.71, 46.68),
    ("Arabic Standard Time", "Baghdad", 33.31, 44.36),
    ("Iran Standard Time", "Tehran", 35.69, 51.39),
    ("Afghanistan Standard Time", "Kabul", 34.56, 69.21),
    ("West Asia Standard Time", "Tashkent", 41.30, 69.24),
    ("Central Asia Standard Time", "Almaty", 43.24, 76.89),
    ("Turkey Standard Time", "Istanbul", 41.01, 28.98),
    ("Egypt Standard Time", "Cairo", 30.04, 31.24),
    ("South Africa Standard Time", "Johannesburg", -26.20, 28.05),
    ("E. Africa Standard Time", "Nairobi", -1.29, 36.82),
    ("W. Central Africa Standard Time", "Lagos", 6.52, 3.38),
    ("Morocco Standard Time", "Casablanca", 33.57, -7.59),
    ("GMT Standard Time", "London", 51.51, -0.13),
    ("Greenwich Standard Time", "Reykjavik", 64.15, -21.94),
    ("W. Europe Standard Time", "Berlin", 52.52, 13.40),
    ("Romance Standard Time", "Paris", 48.86, 2.35),
    ("Central Europe Standard Time", "Budapest", 47.50, 19.04),
    ("Central European Standard Time", "Warsaw", 52.23, 21.01),
    ("FLE Standard Time", "Kyiv", 50.45, 30.52),
    ("GTB Standard Time", "Athens", 37.98, 23.73),
    ("Russian Standard Time", "Moscow", 55.76, 37.62),
    ("Eastern Standard Time", "New York", 40.71, -74.01),
    ("Central Standard Time", "Chicago", 41.88, -87.63),
    ("Mountain Standard Time", "Denver", 39.74, -104.99),
    ("Pacific Standard Time", "Los Angeles", 34.05, -118.24),
    ("Alaskan Standard Time", "Anchorage", 61.22, -149.90),
    ("Hawaiian Standard Time", "Honolulu", 21.31, -157.86),
    ("Atlantic Standard Time", "Halifax", 44.65, -63.58),
    ("SA Pacific Standard Time", "Bogota", 4.71, -74.07),
    ("E. South America Standard Time", "Sao Paulo", -23.55, -46.63),
    ("Argentina Standard Time", "Buenos Aires", -34.60, -58.38),
    ("Central Standard Time (Mexico)", "Mexico City", 19.43, -99.13),
    ("AUS Eastern Standard Time", "Sydney", -33.87, 151.21),
    ("W. Australia Standard Time", "Perth", -31.95, 115.86),
    ("New Zealand Standard Time", "Auckland", -36.85, 174.76),
];

/// The city of Windows' time zone setting.
pub fn tz_place() -> Option<(f64, f64, String)> {
    use windows::Win32::System::Time::{GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION};
    let mut z = DYNAMIC_TIME_ZONE_INFORMATION::default();
    unsafe { GetDynamicTimeZoneInformation(&mut z) };
    let key = String::from_utf16_lossy(&z.TimeZoneKeyName).trim_end_matches('\0').to_string();
    tz_city(&key)
}

pub fn tz_city(key: &str) -> Option<(f64, f64, String)> {
    TZ_CITIES.iter().find(|c| c.0 == key).map(|c| (c.2, c.3, c.1.to_string()))
}

/// "Near Colombo" when a known city is within about 60 km.
pub fn near_name(lat: f64, lon: f64) -> String {
    let d = |la: f64, lo: f64| ((la - lat).powi(2) + ((lo - lon) * lat.to_radians().cos()).powi(2)).sqrt() * 111.0;
    let known = TZ_CITIES.iter().map(|c| (c.1, c.2, c.3)).chain(crate::modules::prayer::CITIES.iter().copied());
    match known.map(|(n, la, lo)| (n, d(la, lo))).min_by(|a, b| a.1.total_cmp(&b.1)) {
        Some((n, km)) if km < 60.0 => format!("Near {n}"),
        _ => format!("{lat:.2}, {lon:.2}"),
    }
}

/// Windows' location service, on its own thread while "Windows location"
/// is picked: every 30 minutes (2 when it failed).
static WINLOC: Mutex<Option<std::result::Result<(f64, f64), String>>> = Mutex::new(None);
static WINLOC_WANTED_AT: AtomicU64 = AtomicU64::new(0);
static WINLOC_STARTED: AtomicBool = AtomicBool::new(false);

fn windows_position() -> std::result::Result<(f64, f64), String> {
    use windows::Devices::Geolocation::*;
    let off = || "Windows location is off for desktop apps".to_string();
    let st = Geolocator::RequestAccessAsync().and_then(|op| op.get()).map_err(|_| off())?;
    if st != GeolocationAccessStatus::Allowed {
        return Err(off());
    }
    let g = Geolocator::new().map_err(|_| off())?;
    let pos = g.GetGeopositionAsync().and_then(|op| op.get()).map_err(|_| "Windows couldn't find your location".to_string())?;
    let p = pos.Coordinate().and_then(|c| c.Point()).and_then(|pt| pt.Position()).map_err(|_| "Windows couldn't find your location".to_string())?;
    Ok((p.Latitude, p.Longitude))
}

fn spawn_winloc() {
    WINLOC_WANTED_AT.store(unix_now() as u64, Ordering::Relaxed);
    if WINLOC_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut next = 0.0;
        loop {
            let now = unix_now();
            let wanted = now - (WINLOC_WANTED_AT.load(Ordering::Relaxed) as f64) < 120.0;
            if wanted && now >= next {
                let r = windows_position();
                next = unix_now() + if r.is_ok() { 1800.0 } else { 120.0 };
                *WINLOC.lock().unwrap() = Some(r);
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

/// City search (Place page): Open-Meteo's geocoding, only when you search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    pub busy: bool,
    pub results: Vec<(String, f64, f64)>,
    pub error: String,
}

pub fn parse_cities(body: &str) -> Vec<(String, f64, f64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    v.get("results")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let name = c.get("name")?.as_str()?;
                    let country = c.get("country").and_then(|x| x.as_str()).unwrap_or("");
                    let label = if country.is_empty() { name.to_string() } else { format!("{name}, {country}") };
                    Some((label, c.get("latitude")?.as_f64()?, c.get("longitude")?.as_f64()?))
                })
                .take(4)
                .collect()
        })
        .unwrap_or_default()
}

fn spawn_search(q: String, shared: Arc<Mutex<Search>>) {
    shared.lock().unwrap().busy = true;
    std::thread::spawn(move || {
        let path = format!("/v1/search?name={}&count=4&language=en&format=json", crate::http::enc(&q));
        let r = crate::http::get("geocoding-api.open-meteo.com", &path).filter(|r| r.status == 200);
        let mut s = shared.lock().unwrap();
        s.busy = false;
        match r {
            Some(r) => {
                s.results = parse_cities(&String::from_utf8_lossy(&r.body));
                s.error = if s.results.is_empty() { format!("No place called \"{q}\"") } else { String::new() };
            }
            None => {
                s.results.clear();
                s.error = "Couldn't search (offline?)".into();
            }
        }
    });
}

// ------------------------------------------------------------ module

// Pages and switches.
const A_SHOW: u32 = 1;
const A_ORDER: u32 = 2;
const A_ALERTS: u32 = 3;
const A_SMART: u32 = 4;
const A_PLACE: u32 = 5;
const A_CUSTOM: u32 = 6;
const A_RESET: u32 = 7;
const A_ALERT_RESET: u32 = 8;
/// Docs renders: made-up Bluetooth devices, weather, Wi-Fi and sound.
pub const A_DEMO: u32 = 990;
/// + index into the catalog: switch an item on or off (Show page).
const A_ITEM: u32 = 1000;
/// + row index (Order page).
const A_HANDLE: u32 = 2000;
const A_UP: u32 = 3000;
const A_DOWN: u32 = 4000;
const A_SIZE: u32 = 5000;
const A_LEFT: u32 = 6000;
const A_RIGHT: u32 = 7000;
const A_WHEN_L: u32 = 9000;
const A_WHEN_R: u32 = 9500;
const A_X_L: u32 = 10000;
const A_X_R: u32 = 10500;
/// + alert index * 3 + mode (Alerts page).
const A_ALERT: u32 = 8000;
/// Place page.
const A_PLACE_MODE: u32 = 11000;
const A_PLACE_PRAYER: u32 = 11010;
const A_PLACE_FIELD: u32 = 11011;
const A_PLACE_RESULT: u32 = 11020;
const A_UNIT: u32 = 11030;

const ROW_H: f32 = 30.0;
const ROW_GAP: f32 = 6.0;
const CARD_H: f32 = 40.0;
const GRIP: &str = "\u{E784}";
const UP: &str = "\u{E70E}";
const DOWN: &str = "\u{E70D}";
const PAGES: [(&str, u32); 4] = [("Show", A_SHOW), ("Order", A_ORDER), ("Alerts", A_ALERTS), ("Place", A_PLACE)];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// An item from a row's left (or only) or right side.
    Item(usize, bool),
    Row(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// A row's side (left or the whole of a both-sides row; right).
    Slot(usize, bool),
    /// Between rows: a new row there.
    Gap(usize),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    what: Grab,
    x: f32,
    y: f32,
}

pub struct Widgets {
    power: Option<Power>,
    power_at: f64,
    disk: (u64, u64),
    disk_at: f64,
    bt: Arc<Mutex<BtShared>>,
    demo_bt: Option<Vec<BtDevice>>,
    /// Everything that can go on the notch, the settings and what shows
    /// now, handed in before each draw.
    items: Vec<CatalogItem>,
    layout: NotchLayout,
    place_set: crate::store::Place,
    alert_modes: Vec<AlertMode>,
    showing: Vec<String>,
    /// 0 Show, 1 Order, 2 Alerts, 3 Place.
    page: u8,
    scroll: [f32; 2],
    max_scroll: [f32; 2],
    drag: Option<Drag>,
    /// Where a drag can land, from the last draw.
    targets: Vec<(Rect, Target)>,
    weather: Arc<Mutex<Option<WeatherNow>>>,
    wifi: Option<Wifi>,
    wifi_at: f64,
    eps: Option<AudioEps>,
    sound: SoundNow,
    sound_at: f64,
    /// When this tab last drew (keeps its readings fresh).
    tab_seen: f64,
    demo: bool,
    /// The resolved place's name (Weather card), its time-zone fallback and
    /// the last Windows location problem.
    place: String,
    tz: Option<(f64, f64, String)>,
    place_note: String,
    /// City search on the Place page: the text typed, and its results.
    query: Option<String>,
    search: Arc<Mutex<Search>>,
    /// Reset asks for a second click within 4 s.
    armed: (u32, f64),
}

/// Move an item or a row of the list. `Item` onto a side: swap with what
/// is there (or move into the empty side); onto a gap: its own new row.
/// `Row` onto a gap: move the row there.
fn apply_drop(rows: &mut Vec<NotchRow>, what: Grab, to: Target) {
    match (what, to) {
        (Grab::Row(k), Target::Gap(g)) if k < rows.len() => {
            let row = rows.remove(k);
            let g = if g > k { g - 1 } else { g };
            rows.insert(g.min(rows.len()), row);
        }
        (Grab::Row(k), Target::Slot(t, _)) if k < rows.len() && t < rows.len() && t != k => {
            rows.swap(k, t);
        }
        (Grab::Item(k, side), to) if k < rows.len() => {
            let side = side && !rows[k].wide;
            match to {
                Target::Slot(t, tside) if t < rows.len() => {
                    let tside = tside && !rows[t].wide;
                    if (t, tside) == (k, side) {
                        return;
                    }
                    let a = if side { rows[k].right.clone() } else { rows[k].left.clone() };
                    let b = if tside { rows[t].right.clone() } else { rows[t].left.clone() };
                    if side {
                        rows[k].right = b;
                    } else {
                        rows[k].left = b;
                    }
                    if tside {
                        rows[t].right = a;
                    } else {
                        rows[t].left = a;
                    }
                }
                Target::Gap(g) => {
                    let wide = rows[k].wide;
                    let id = if side { std::mem::take(&mut rows[k].right) } else { std::mem::take(&mut rows[k].left) };
                    if id.is_empty() {
                        return;
                    }
                    let row = if wide { NotchRow::wide(&id) } else { NotchRow::pair(&id, "") };
                    rows.insert(g.min(rows.len()), row);
                }
                _ => {}
            }
            // A both-sides row whose item moved to a right side elsewhere
            // keeps only its left, so lift a lone right into it.
            for r in rows.iter_mut() {
                if r.wide && r.left.is_empty() && !r.right.is_empty() {
                    r.left = std::mem::take(&mut r.right);
                }
            }
        }
        _ => {}
    }
    rows.retain(|r| !r.is_empty());
}

/// Both sides <-> one side. Two one-side items becoming "both sides": the
/// right one gets its own row just below.
fn toggle_size(rows: &mut Vec<NotchRow>, k: usize) {
    let Some(r) = rows.get_mut(k) else { return };
    if r.wide {
        r.wide = false;
        return;
    }
    let (l, rt) = (r.left.clone(), r.right.clone());
    if l.is_empty() {
        *r = NotchRow::wide(&rt);
    } else {
        *r = NotchRow::wide(&l);
        if !rt.is_empty() {
            rows.insert(k + 1, NotchRow::pair(&rt, ""));
        }
    }
}

fn when_word(w: ShowWhen) -> &'static str {
    match w {
        ShowWhen::Always => "Always",
        ShowWhen::Matters => "Matters",
        ShowWhen::Change => "Changes",
    }
}

impl Widgets {
    pub fn new() -> Widgets {
        let st = crate::store::Settings::load();
        Widgets {
            power: power(),
            power_at: unix_now(),
            disk: (0, 0),
            disk_at: 0.0,
            bt: Arc::new(Mutex::new(BtShared { devices: Vec::new(), read_at: 0.0 })),
            demo_bt: None,
            items: Vec::new(),
            alert_modes: crate::store::ALERTS.iter().map(|(k, _)| st.alert_mode(k)).collect(),
            layout: st.layout,
            place_set: st.place,
            showing: Vec::new(),
            page: 0,
            scroll: [0.0; 2],
            max_scroll: [0.0; 2],
            drag: None,
            targets: Vec::new(),
            weather: Arc::new(Mutex::new(None)),
            wifi: None,
            wifi_at: 0.0,
            eps: None,
            sound: SoundNow::default(),
            sound_at: 0.0,
            tab_seen: 0.0,
            demo: false,
            place: String::new(),
            tz: tz_place(),
            place_note: String::new(),
            query: None,
            search: Arc::new(Mutex::new(Search::default())),
            armed: (0, 0.0),
        }
    }

    fn bt_devices(&self) -> Vec<BtDevice> {
        match &self.demo_bt {
            Some(d) => d.clone(),
            None => self.bt.lock().unwrap().devices.clone(),
        }
    }

    fn item(&self, id: &str) -> Option<&CatalogItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Resolve the place (Place page) and share it with the weather and
    /// prayer times.
    fn resolve_place(&mut self, ctx: &mut Ctx) {
        let fallback = self.tz.clone().unwrap_or_else(crate::modules::prayer::location);
        let pl = &mut ctx.settings.place;
        let got = match pl.mode {
            PlaceMode::Auto => {
                self.place_note.clear();
                fallback
            }
            PlaceMode::Manual if !pl.name.is_empty() => {
                self.place_note.clear();
                (pl.lat, pl.lon, pl.name.clone())
            }
            PlaceMode::Manual => {
                self.place_note = "Search for your city".into();
                fallback
            }
            PlaceMode::Windows => {
                spawn_winloc();
                match WINLOC.lock().unwrap().clone() {
                    Some(Ok((la, lo))) => {
                        self.place_note.clear();
                        if (la - pl.lat).abs() > 0.01 || (lo - pl.lon).abs() > 0.01 || pl.name.is_empty() {
                            pl.lat = la;
                            pl.lon = lo;
                            pl.name = near_name(la, lo);
                            ctx.settings.save();
                        }
                        let pl = &ctx.settings.place;
                        (pl.lat, pl.lon, pl.name.clone())
                    }
                    Some(Err(e)) => {
                        self.place_note = format!("{e}: using the time zone's city");
                        fallback
                    }
                    None if !pl.name.is_empty() => (pl.lat, pl.lon, pl.name.clone()),
                    None => {
                        self.place_note = "Asking Windows...".into();
                        fallback
                    }
                }
            }
        };
        self.place = got.2.clone();
        *PLACE.lock().unwrap() = Some(got);
    }

    fn lead(ui: &mut Ui, r: Rect, icon: &str, ring: Option<f32>, c: Color) {
        match ring {
            Some(f) => {
                ui.stroke_circle(r.cx(), r.cy(), 6.5, c.with_a(0.25), 2.0);
                ui.arc(r.cx(), r.cy(), 6.5, 0.0, std::f32::consts::TAU * f, c, 2.0);
            }
            None => ui.icon(icon, r, 13.0, c),
        }
    }

    /// An item in the Order list: icon and name on a tinted pill, with its
    /// "when to show" rule (widgets) and a remove button.
    fn chip(&self, ui: &mut Ui, r: Rect, id: &str, hits: Option<(u32, u32, u32)>, lifted: bool) {
        let (name, icon, c, ring, live) = match self.item(id) {
            Some(i) => (i.name.clone(), i.icon, i.color, i.ring, i.live),
            None => (id.to_string(), glyph::BELL, col::DIM, None, false),
        };
        let bg = if lifted { c.with_a(0.4) } else if ui.hovered(r) { c.with_a(0.26) } else { c.with_a(0.16) };
        ui.fill_rr(r, r.h / 2.0, bg);
        let mut inner = r.inset_xy(8.0, 0.0);
        if let Some((drag, when, x)) = hits {
            ui.hit(r, drag);
            let xr = inner.cut_right(16.0, 4.0);
            ui.icon_btn(xr.centered(16.0, 16.0), glyph::CLOSE, x, 8.0, col::DIM, None, "");
            if !live && inner.w > 110.0 {
                let wr = inner.cut_right(54.0, 4.0).centered(54.0, 18.0);
                let word = when_word(self.layout.when_of(id));
                let bg = if ui.hovered(wr) { col::TEXT.with_a(0.2) } else { col::TEXT.with_a(0.1) };
                ui.fill_rr(wr, 9.0, bg);
                ui.text(word, wr, 10.0, col::TEXT, Weight::Semi, Align::Center);
                ui.hit(wr, when);
            }
        }
        let ic = inner.cut_left(16.0, 6.0);
        Self::lead(ui, ic, icon, ring, c);
        ui.text(&name, inner, 12.0, col::TEXT, Weight::Semi, Align::Left);
    }

    fn draw_show(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut bar = r.cut_top(24.0, 8.0);
        let reset = bar.cut_right(110.0, 8.0);
        let armed = self.armed.0 == A_RESET && unix_now() - self.armed.1 < 4.0;
        ui.button_c(reset, if armed { "Click to confirm" } else { "Reset to default" }, A_RESET, if armed { col::RED.with_a(0.2) } else { col::BTN }, if armed { col::RED } else { col::TEXT });
        let halves = bar.columns(2, 6.0);
        ui.toggle(halves[0], "Custom order", self.layout.custom, A_CUSTOM, ACCENT);
        ui.toggle(halves[1], "Only when it matters", self.layout.smart, A_SMART, ACCENT);
        // Cards in three groups.
        let list = r;
        let cols = 3;
        let cw = (list.w - ROW_GAP * (cols as f32 - 1.0)) / cols as f32;
        let mut y = list.y - self.scroll[0];
        let items = self.items.clone();
        for (g, title) in [(0u8, "Live activities: while they run"), (1, "System"), (2, "From your panels")] {
            let group: Vec<(usize, &CatalogItem)> = items.iter().enumerate().filter(|(_, it)| it.group == g).collect();
            if group.is_empty() {
                continue;
            }
            let head = Rect::new(list.x + 2.0, y, list.w, 16.0);
            if head.b() > list.y && head.y < list.b() {
                ui.text(title, head, 10.5, col::DIM, Weight::Semi, Align::Left);
            }
            y += 20.0;
            for (n, (k, it)) in group.iter().enumerate() {
                let cell = Rect::new(list.x + (n % cols) as f32 * (cw + ROW_GAP), y + (n / cols) as f32 * (CARD_H + ROW_GAP), cw, CARD_H);
                if cell.b() < list.y || cell.y > list.b() {
                    continue;
                }
                let on = self.layout.contains(&it.id);
                let hov = ui.hovered(cell) && list.contains(ui.mouse.0, ui.mouse.1);
                ui.fill_rr(cell, 10.0, if on { it.color.with_a(0.18) } else if hov { col::BTN_HOVER } else { col::BTN });
                if on {
                    ui.stroke_rr(cell, 10.0, it.color.with_a(0.6), 1.2);
                }
                let mut c = cell.inset_xy(8.0, 5.0);
                let ic = c.cut_left(18.0, 6.0);
                Self::lead(ui, ic, it.icon, it.ring, it.color);
                let mark = c.cut_right(14.0, 2.0);
                if on {
                    ui.icon(glyph::CHECK, mark, 10.0, it.color);
                }
                let top = c.cut_top(c.h * 0.5, 0.0);
                ui.text(&it.name, top, 12.0, col::TEXT, Weight::Semi, Align::Left);
                ui.text(&it.detail, c, 10.5, col::DIM, Weight::Regular, Align::Left);
                if list.contains(cell.cx(), cell.cy()) {
                    ui.hit(cell, A_ITEM + *k as u32);
                }
            }
            y += ((group.len() + cols - 1) / cols) as f32 * (CARD_H + ROW_GAP) + 4.0;
        }
        let content = y + self.scroll[0] - list.y;
        self.max_scroll[0] = (content - list.h).max(0.0);
        self.scroll[0] = self.scroll[0].clamp(0.0, self.max_scroll[0]);
    }

    fn draw_order(&mut self, ui: &mut Ui, list: Rect) {
        let rows = self.layout.rows.clone();
        let content = (rows.len() as f32 + 1.0) * (ROW_H + ROW_GAP);
        self.max_scroll[1] = (content - list.h).max(0.0);
        self.scroll[1] = self.scroll[1].clamp(0.0, self.max_scroll[1]);
        self.targets.clear();
        let dragging = self.drag.map(|d| d.what);
        let mouse = ui.mouse;
        let in_list = |r: Rect| r.b() > list.y && r.y < list.b();
        let mut y = list.y - self.scroll[1];
        for (k, row) in rows.iter().enumerate() {
            // The gap above this row: a new row lands here.
            let gap = Rect::new(list.x, y - ROW_GAP - 4.0, list.w, ROW_GAP + 8.0);
            self.targets.push((gap, Target::Gap(k)));
            let rr = Rect::new(list.x, y, list.w, ROW_H);
            y += ROW_H + ROW_GAP;
            if !in_list(rr) {
                continue;
            }
            let lifted_row = dragging == Some(Grab::Row(k));
            ui.fill_rr(rr, 10.0, if lifted_row { col::TEXT.with_a(0.12) } else { col::TEXT.with_a(0.04) });
            let mut inner = rr.inset_xy(4.0, 3.0);
            let grip = inner.cut_left(20.0, 4.0);
            let grip_c = if ui.hovered(grip) { col::TEXT } else { col::FAINT };
            ui.icon(GRIP, grip, 12.0, grip_c);
            ui.hit(grip, A_HANDLE + k as u32);
            let down = inner.cut_right(22.0, 0.0);
            let up = inner.cut_right(22.0, 2.0);
            ui.icon_btn(up, UP, A_UP + k as u32, 10.0, if k == 0 { col::FAINT } else { col::DIM }, None, "");
            ui.icon_btn(down, DOWN, A_DOWN + k as u32, 10.0, if k + 1 == rows.len() { col::FAINT } else { col::DIM }, None, "");
            let size = inner.cut_right(76.0, 6.0);
            ui.chip(size, if row.wide { "Both sides" } else { "One side" }, A_SIZE + k as u32, row.wide, ACCENT);
            if row.wide {
                let lifted = dragging == Some(Grab::Item(k, false));
                self.chip(ui, inner, &row.left, Some((A_LEFT + k as u32, A_WHEN_L + k as u32, A_X_L + k as u32)), lifted);
                self.targets.push((inner, Target::Slot(k, false)));
            } else {
                let halves = inner.columns(2, 6.0);
                for (side, h) in [(false, halves[0]), (true, halves[1])] {
                    let id = if side { &row.right } else { &row.left };
                    let hits = if side { (A_RIGHT, A_WHEN_R, A_X_R) } else { (A_LEFT, A_WHEN_L, A_X_L) };
                    if id.is_empty() {
                        ui.stroke_rr(h, h.h / 2.0, col::TEXT.with_a(0.18), 1.0);
                        ui.text(if side { "Empty right side" } else { "Empty left side" }, h, 11.0, col::FAINT, Weight::Regular, Align::Center);
                    } else {
                        let lifted = dragging == Some(Grab::Item(k, side));
                        self.chip(ui, h, id, Some((hits.0 + k as u32, hits.1 + k as u32, hits.2 + k as u32)), lifted);
                    }
                    self.targets.push((h, Target::Slot(k, side)));
                }
            }
        }
        // Last: a new row at the end.
        let end = Rect::new(list.x, y, list.w, ROW_H);
        self.targets.push((Rect::new(end.x, end.y - ROW_GAP, end.w, end.h + ROW_GAP), Target::Gap(rows.len())));
        if in_list(end) {
            let msg = if dragging.is_some() { "Drop here for a row of its own" } else { "Drag an item onto an empty side to pair it, or between rows for a row of its own" };
            ui.stroke_rr(end, 10.0, col::TEXT.with_a(if dragging.is_some() { 0.3 } else { 0.1 }), 1.0);
            ui.text(msg, end, 11.0, col::FAINT, Weight::Regular, Align::Center);
        }
        // Where the drag would land, and the lifted item under the mouse.
        if let Some(d) = self.drag {
            if let Some((t, to)) = self.targets.iter().rev().find(|(t, _)| t.contains(mouse.0, mouse.1)).copied() {
                match to {
                    Target::Gap(_) => ui.fill_rr(Rect::new(t.x, t.cy() - 1.5, t.w, 3.0), 1.5, ACCENT),
                    Target::Slot(..) => ui.stroke_rr(t, t.h / 2.0, ACCENT, 2.0),
                }
            }
            if let Grab::Item(k, side) = d.what {
                if let Some(row) = rows.get(k) {
                    let id = if side && !row.wide { &row.right } else { &row.left };
                    let w = 150.0;
                    self.chip(ui, Rect::new(d.x - w / 2.0, d.y - 12.0, w, 24.0), &id.clone(), None, true);
                }
            }
        }
    }

    fn draw_alerts(&mut self, ui: &mut Ui, r: Rect) {
        let cols = r.columns(2, 16.0);
        for (k, (_, name)) in crate::store::ALERTS.iter().enumerate() {
            let mut col_r = cols[k % 2];
            let row = Rect::new(col_r.x, col_r.y + (k / 2) as f32 * 32.0, col_r.w, 26.0);
            col_r = row;
            let mut row = col_r;
            let chips = row.cut_right(150.0, 6.0).columns(3, 4.0);
            ui.text(name, row, 12.0, col::TEXT, Weight::Semi, Align::Left);
            let cur = self.alert_modes.get(k).copied().unwrap_or(AlertMode::Popup);
            for (m, (cell, word)) in chips.into_iter().zip(["Off", "Small", "Pop-up"]).enumerate() {
                ui.chip(cell, word, A_ALERT + (k * 3 + m) as u32, cur == AlertMode::ALL[m], ACCENT);
            }
        }
    }

    fn draw_place(&mut self, ui: &mut Ui, mut r: Rect) {
        let row = r.cut_top(26.0, 8.0);
        let cells = row.columns(3, 6.0);
        let mode = self.place_set.mode;
        for (k, (cell, word)) in cells.into_iter().zip(["Auto (time zone)", "Windows location", "Choose a city"]).enumerate() {
            let m = [PlaceMode::Auto, PlaceMode::Windows, PlaceMode::Manual][k];
            ui.chip(cell, word, A_PLACE_MODE + k as u32, mode == m, ACCENT);
        }
        let mut now = r.cut_top(22.0, 2.0);
        let units = now.cut_right(96.0, 8.0).columns(2, 4.0);
        ui.chip(units[0], "°C", A_UNIT, !self.place_set.fahrenheit, ACCENT);
        ui.chip(units[1], "°F", A_UNIT + 1, self.place_set.fahrenheit, ACCENT);
        let here = PLACE.lock().unwrap().clone();
        let line = match here {
            Some((la, lo, n)) => format!("Now: {n}  ({la:.2}, {lo:.2})"),
            None => "Now: working it out...".to_string(),
        };
        ui.text(&line, now, 12.5, col::TEXT, Weight::Semi, Align::Left);
        let note = r.cut_top(16.0, 8.0);
        let hint = match mode {
            PlaceMode::Auto => "From Windows' time zone setting: works offline and with a VPN.".to_string(),
            PlaceMode::Windows if self.place_note.is_empty() => "Windows' location service. Without GPS it may guess from your network.".to_string(),
            PlaceMode::Manual if self.place_note.is_empty() => "Type a city and press Enter (searches Open-Meteo).".to_string(),
            _ => self.place_note.clone(),
        };
        ui.text(&hint, note, 11.0, col::DIM, Weight::Regular, Align::Left);
        let mut row = r.cut_top(26.0, 8.0);
        let toggle = row.cut_right(230.0, 0.0);
        ui.toggle(toggle, "Use it for prayer times too", self.place_set.prayer, A_PLACE_PRAYER, ACCENT);
        if mode == PlaceMode::Manual {
            let field = row.cut_left(row.w - 8.0, 0.0);
            let typing = self.query.is_some();
            ui.fill_rr(field, 10.0, col::BTN);
            ui.stroke_rr(field, 10.0, if typing { ACCENT.with_a(0.7) } else { col::TEXT.with_a(0.12) }, 1.0);
            let tr = field.inset_xy(10.0, 0.0);
            let q = self.query.clone().unwrap_or_default();
            if q.is_empty() {
                ui.text(if typing { "Type a city" } else { "Search for a city" }, tr, 12.0, col::FAINT, Weight::Regular, Align::Left);
            } else {
                ui.text(&q, tr, 12.0, col::TEXT, Weight::Regular, Align::Left);
            }
            if typing && (ui.t * 1.8) as i64 % 2 == 0 {
                let cx = tr.x + ui.measure(&q, 12.0, Weight::Regular) + 1.0;
                ui.line(cx, tr.cy() - 7.0, cx, tr.cy() + 7.0, ACCENT, 1.4);
            }
            ui.hit(field, A_PLACE_FIELD);
            let s = self.search.lock().unwrap().clone();
            let res = r.cut_top(26.0, 0.0);
            if s.busy {
                ui.text("Searching...", res, 11.5, col::DIM, Weight::Regular, Align::Left);
            } else if !s.results.is_empty() {
                for (k, (cell, (name, _, _))) in res.columns(s.results.len(), 6.0).into_iter().zip(s.results.iter()).enumerate() {
                    ui.chip(cell, name, A_PLACE_RESULT + k as u32, false, ACCENT);
                }
            } else if !s.error.is_empty() {
                ui.text(&s.error, res, 11.5, col::DIM, Weight::Regular, Align::Left);
            }
        }
    }

    fn save(&mut self, ctx: &mut Ctx) {
        self.layout.sync_picked();
        ctx.settings.layout = self.layout.clone();
        ctx.settings.save();
    }

    /// Reset needs a second click within 4 s.
    fn confirm(&mut self, id: u32) -> bool {
        if self.armed.0 == id && unix_now() - self.armed.1 < 4.0 {
            self.armed = (0, 0.0);
            true
        } else {
            self.armed = (id, unix_now());
            false
        }
    }
}

impl Module for Widgets {
    fn title(&self) -> &'static str {
        "Widgets"
    }
    fn icon(&self) -> &'static str {
        ICON
    }
    fn accent(&self) -> Color {
        ACCENT
    }
    fn size(&self) -> (f32, f32) {
        // Show and Order hold long lists: a taller panel there.
        (crate::modules::PANEL.0, if self.page <= 1 { 250.0 } else { crate::modules::PANEL.1 })
    }
    fn slim_min(&self) -> (f32, f32) {
        self.size()
    }

    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_SHOW => "Choose what the closed notch can show",
            A_ORDER => "Your own order: the first row with something to show wins",
            A_ALERTS => "How each alert shows: off, small inside the notch, or a pop-up",
            A_PLACE => "Where you are, for the weather (and prayer times if you like)",
            A_SMART => "Skip a widget with nothing worth showing (battery on the charger, good Wi-Fi)",
            A_CUSTOM => "Off: WindyLand orders things for you. On: arrange them yourself on the Order page",
            A_RESET => "Back to the default items and order (asks first)",
            A_ALERT_RESET => "Every alert back to a pop-up (asks first)",
            A_PLACE_PRAYER => "Prayer times use this place instead of their own city",
            A_UNIT => "Weather in Celsius",
            i if i == A_UNIT + 1 => "Weather in Fahrenheit",
            A_PLACE_FIELD => "Type a city, then press Enter",
            i if (A_ITEM..A_ITEM + 500).contains(&i) => "Show this on the closed notch (click again to remove it)",
            i if (A_HANDLE..A_HANDLE + 500).contains(&i) => "Drag to move this row",
            i if (A_UP..A_UP + 500).contains(&i) => "Move this row up (higher priority)",
            i if (A_DOWN..A_DOWN + 500).contains(&i) => "Move this row down",
            i if (A_SIZE..A_SIZE + 500).contains(&i) => "Both sides of the clock, or one side (two share a row)",
            i if (A_LEFT..A_LEFT + 500).contains(&i) || (A_RIGHT..A_RIGHT + 500).contains(&i) => "Drag onto another side, or between rows",
            i if (A_WHEN_L..A_WHEN_L + 1000).contains(&i) => "When to show it: always, only when it matters, or for a few seconds when it changes",
            i if (A_X_L..A_X_L + 1000).contains(&i) => "Remove it from the notch",
            i if (A_ALERT..A_ALERT + 100).contains(&i) => "Off, small inside the closed notch, or the pop-up card",
            i if (A_PLACE_MODE..A_PLACE_MODE + 3).contains(&i) => "How WindyLand knows where you are",
            i if (A_PLACE_RESULT..A_PLACE_RESULT + 10).contains(&i) => "Use this place",
            _ => "",
        }
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let now = unix_now();
        if now - self.power_at >= 5.0 {
            self.power = power();
            self.power_at = now;
        }
        if now - self.disk_at >= 60.0 {
            self.disk = crate::modules::monitor::disk();
            self.disk_at = now;
        }
        if ctx.settings.layout.contains("bluetooth") {
            want_bluetooth();
            spawn_bt(self.bt.clone());
        }
        if self.demo {
            return;
        }
        self.resolve_place(ctx);
        WEATHER_F.store(ctx.settings.place.fahrenheit, Ordering::Relaxed);
        let tab = now - self.tab_seen < 5.0;
        let on = |id: &str| tab || ctx.settings.layout.contains(id);
        // Only on: this is the one widget that goes online by itself.
        if ctx.settings.layout.contains("weather") {
            want_weather();
            spawn_weather(self.weather.clone());
        }
        if on("wifi") && now - self.wifi_at >= 15.0 {
            self.wifi = Some(wifi());
            self.wifi_at = now;
        }
        if (on("volume") || on("mic")) && now - self.sound_at >= 1.0 {
            // Reopen now and then: the default device may have changed.
            if self.eps.as_ref().map_or(true, |e| now - e.at > 60.0) {
                self.eps = Some(AudioEps::open());
            }
            if let Some(e) = &self.eps {
                self.sound = e.read();
            }
            self.sound_at = now;
        }
    }

    fn widgets(&mut self) -> Vec<Widget> {
        let mut out = Vec::new();
        if let Some(p) = self.power {
            let label = if p.charging { "Charging" } else if p.on_ac { "Plugged in" } else { "Battery" };
            let detail = match (p.charging, p.on_ac, p.left) {
                (true, _, _) => format!("{}% · charging", p.pct),
                (_, true, _) => format!("{}% · plugged in", p.pct),
                (_, _, Some(s)) => format!("{}% · {} left", p.pct, hm(s)),
                _ => format!("{}%", p.pct),
            };
            let color = if p.charging || p.on_ac { C_GOOD } else { level_color(p.pct) };
            out.push(
                Widget::new("battery", "Battery", if p.charging { glyph::CHARGING } else { glyph::BATTERY }, color, format!("{}%", p.pct))
                    .label(label)
                    .ring(p.pct as f32 / 100.0)
                    .detail(detail)
                    // Worth showing on battery power or while charging.
                    .quiet(p.on_ac && !p.charging),
            );
        }
        let devs = self.bt_devices();
        let lowest = devs.iter().filter(|d| d.level().is_some()).min_by_key(|d| d.level());
        out.push(match lowest {
            Some(d) => {
                let lv = d.level().unwrap_or(0);
                let detail = devs.iter().filter(|d| d.level().is_some()).map(|d| d.detail()).collect::<Vec<_>>().join(" · ");
                Widget::new("bluetooth", "Bluetooth", if d.audio { glyph::HEADPHONE } else { glyph::BLUETOOTH }, level_color(lv), format!("{lv}%"))
                    .label(d.short_name())
                    .ring(lv as f32 / 100.0)
                    .detail(detail)
            }
            None => Widget::new("bluetooth", "Bluetooth", glyph::BLUETOOTH, col::DIM, "–")
                .detail(if devs.is_empty() { "No device connected".to_string() } else { "No battery reported".to_string() })
                .quiet(true),
        });
        let (free, total) = self.disk;
        if total > 0 {
            let g = free as f64 / 1073741824.0;
            let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
            out.push(
                Widget::new("disk", "Free space", DISK, hex(0xAC8E68), if g >= 100.0 { format!("{g:.0}G") } else { format!("{g:.1}G") })
                    .label(drive.clone())
                    .ring(1.0 - free as f32 / total as f32)
                    .detail(format!("{drive} {} free", crate::modules::monitor::gb(free))),
            );
        }
        let place = self.place.clone();
        let weather = self.weather.lock().unwrap().clone();
        out.push(match weather {
            Some(w) => {
                let (word, icon) = weather_kind(w.code, w.day);
                Widget::new("weather", "Weather", icon, hex(0xFFD60A), format!("{:.0}°", w.temp))
                    // Short enough to sit beside the clock.
                    .label(if word == "Partly cloudy" { "Fair" } else { word })
                    .detail(format!("{:.0}°{} · {word} · {}", w.temp, if w.fahrenheit { "F" } else { "C" }, w.place))
                    // Over 3 hours old (offline): nothing to show.
                    .quiet(unix_now() - w.at > 3.0 * 3600.0)
            }
            None => Widget::new("weather", "Weather", "\u{E753}", col::DIM, "–").detail(format!("Online, for {place}")).quiet(true),
        });
        match self.wifi.clone() {
            Some(Wifi::Connected(name, q)) => out.push(
                Widget::new("wifi", "Wi-Fi", glyph::WIFI, hex(0x0A84FF), format!("{q}%"))
                    .label(if name.is_empty() { "Wi-Fi".to_string() } else { name.chars().take(10).collect() })
                    .ring(q as f32 / 100.0)
                    .detail(if name.is_empty() { format!("Signal {q}%") } else { format!("{name} · {q}%") })
                    // Worth showing when the signal is weak.
                    .quiet(q >= 40),
            ),
            Some(Wifi::Disconnected) => out.push(Widget::new("wifi", "Wi-Fi", glyph::WIFI_OFF, C_LOW, "Off").label("No Wi-Fi").detail("Not connected")),
            Some(Wifi::NoAdapter) => {}
            None => out.push(Widget::new("wifi", "Wi-Fi", glyph::WIFI, col::DIM, "–").detail("Network and signal").quiet(true)),
        }
        out.push(match self.sound.volume {
            Some((v, true)) => Widget::new("volume", "Volume", glyph::MUTE, C_LOW, "Muted").detail(format!("Muted ({:.0}%)", v * 100.0)),
            Some((v, false)) => Widget::new("volume", "Volume", glyph::VOLUME, hex(0xBF5AF2), format!("{:.0}%", v * 100.0))
                .ring(v)
                .detail(format!("{:.0}%", v * 100.0))
                // Worth showing when muted.
                .quiet(true),
            None => Widget::new("volume", "Volume", glyph::VOLUME, col::DIM, "–").detail("Speaker volume").quiet(true),
        });
        out.push(match self.sound.mic_muted {
            Some(true) => Widget::new("mic", "Microphone", glyph::MIC, C_LOW, "Muted").label("Mic").detail("Muted"),
            Some(false) => Widget::new("mic", "Microphone", glyph::MIC, C_GOOD, "On").label("Mic").detail("Not muted").quiet(true),
            None => Widget::new("mic", "Microphone", glyph::MIC, col::DIM, "–").label("Mic").detail("Mute status").quiet(true),
        });
        out.push(Widget::new("uptime", "Uptime", UPTIME, hex(0x8E8E93), uptime_short()).label("Up").detail(format!("Up {}", uptime_short())));
        let n = Now::get();
        let month = MONTHS[(n.month as usize).saturating_sub(1) % 12];
        out.push(
            Widget::new("date", "Date", CALENDAR, hex(0xFF6482), format!("{} {month}", n.day))
                .short(n.day.to_string())
                .label(DAYS[n.dow as usize % 7])
                .detail(format!("{} {} {month}", DAYS[n.dow as usize % 7], n.day)),
        );
        out
    }

    fn set_widget_catalog(&mut self, items: Vec<CatalogItem>, st: &crate::store::Settings, showing: Vec<String>) {
        self.items = items;
        if self.drag.is_none() {
            self.layout = st.layout.clone();
        }
        self.place_set = st.place.clone();
        self.alert_modes = crate::store::ALERTS.iter().map(|(k, _)| st.alert_mode(k)).collect();
        self.showing = showing;
        // Order is only there with Custom order on.
        if self.page == 1 && !self.layout.custom {
            self.page = 0;
        }
        // The tab is open: keep Bluetooth, Wi-Fi and sound fresh.
        self.tab_seen = unix_now();
        want_bluetooth();
        spawn_bt(self.bt.clone());
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let mut top = r.cut_top(24.0, 8.0);
        let pages: Vec<(&str, u32)> = PAGES.iter().copied().filter(|(_, id)| *id != A_ORDER || self.layout.custom).collect();
        let tabs = top.cut_left(pages.len() as f32 * 76.0, 12.0).columns(pages.len(), 6.0);
        let cur = [A_SHOW, A_ORDER, A_ALERTS, A_PLACE][self.page as usize];
        for (cell, (word, id)) in tabs.into_iter().zip(pages) {
            ui.chip(cell, word, id, id == cur, ACCENT);
        }
        if self.page == 2 {
            // Alerts: Reset in the bar (the list needs the room).
            let reset = top.cut_right(110.0, 0.0);
            let armed = self.armed.0 == A_ALERT_RESET && unix_now() - self.armed.1 < 4.0;
            ui.button_c(reset, if armed { "Click to confirm" } else { "Reset to default" }, A_ALERT_RESET, if armed { col::RED.with_a(0.2) } else { col::BTN }, if armed { col::RED } else { col::TEXT });
        } else {
            let now: Vec<String> = self.showing.iter().map(|id| self.item(id).map_or(id.clone(), |i| i.name.clone())).collect();
            let now = if now.is_empty() { "Now: just the clock".to_string() } else { format!("Now: {}", now.join(" · ")) };
            ui.text(&now, top, 11.0, col::DIM, Weight::Regular, Align::Right);
        }
        ui.push_clip(r.inset(-2.0));
        match self.page {
            1 => self.draw_order(ui, r),
            2 => self.draw_alerts(ui, r),
            3 => self.draw_place(ui, r),
            _ => self.draw_show(ui, r),
        }
        ui.pop_clip();
    }

    fn scroll(&mut self, delta: f32) -> bool {
        let p = match self.page {
            0 => 0,
            1 => 1,
            _ => return false,
        };
        if self.max_scroll[p] <= 0.0 {
            return false;
        }
        self.scroll[p] = (self.scroll[p] - delta * 36.0).clamp(0.0, self.max_scroll[p]);
        true
    }

    fn drag_begin(&mut self, id: u32, x: f32, y: f32) -> bool {
        if self.page != 1 {
            return false;
        }
        let n = self.layout.rows.len() as u32;
        let what = match id {
            i if (A_HANDLE..A_HANDLE + n).contains(&i) => Grab::Row((i - A_HANDLE) as usize),
            i if (A_LEFT..A_LEFT + n).contains(&i) => Grab::Item((i - A_LEFT) as usize, false),
            i if (A_RIGHT..A_RIGHT + n).contains(&i) => Grab::Item((i - A_RIGHT) as usize, true),
            _ => return false,
        };
        self.drag = Some(Drag { what, x, y });
        true
    }

    fn drag_to(&mut self, x: f32, y: f32) {
        if let Some(d) = &mut self.drag {
            d.x = x;
            d.y = y;
        }
    }

    fn drag_drop(&mut self, x: f32, y: f32, ctx: &mut Ctx) {
        let Some(d) = self.drag.take() else { return };
        let Some((_, to)) = self.targets.iter().rev().find(|(t, _)| t.contains(x, y)).copied() else { return };
        apply_drop(&mut self.layout.rows, d.what, to);
        self.save(ctx);
    }

    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        let Some(q) = &mut self.query else { return false };
        match vk {
            0x0D => {
                let q = q.trim().to_string();
                if q.chars().count() >= 2 {
                    spawn_search(q, self.search.clone());
                }
            }
            0x1B => {
                self.query = None;
                ctx.want_keyboard(false);
            }
            0x08 if ctrl => q.clear(),
            0x08 => {
                q.pop();
            }
            _ => return false,
        }
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        match &mut self.query {
            Some(q) => {
                if !c.is_control() && q.chars().count() < 60 {
                    q.push(c);
                }
                true
            }
            None => false,
        }
    }

    fn blur(&mut self) {
        self.query = None;
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        let n = self.layout.rows.len() as u32;
        match id {
            A_SHOW => self.page = 0,
            A_ORDER => self.page = 1,
            A_ALERTS => self.page = 2,
            A_PLACE => self.page = 3,
            A_SMART => {
                self.layout.smart = !self.layout.smart;
                self.save(ctx);
            }
            A_CUSTOM => {
                self.layout.custom = !self.layout.custom;
                if self.layout.custom {
                    self.page = 1;
                }
                self.save(ctx);
            }
            A_RESET => {
                if self.confirm(A_RESET) {
                    let alerts = std::mem::take(&mut self.layout.alerts);
                    self.layout = NotchLayout { alerts, ..NotchLayout::default() };
                    self.page = 0;
                    self.save(ctx);
                }
            }
            A_ALERT_RESET => {
                if self.confirm(A_ALERT_RESET) {
                    for (k, _) in crate::store::ALERTS {
                        ctx.settings.set_alert_mode(k, AlertMode::Popup);
                    }
                    self.layout.alerts = ctx.settings.layout.alerts.clone();
                    ctx.settings.save();
                }
            }
            A_DEMO => {
                self.demo_bt = Some(vec![BtDevice {
                    name: "Banu's AirPods".into(),
                    pct: None,
                    pods: Some(Pods { left: Some(60), right: Some(70), case: Some(40), charging: false }),
                    audio: true,
                }]);
                self.demo = true;
                *self.weather.lock().unwrap() = Some(WeatherNow { temp: 29.0, code: 2, day: true, fahrenheit: false, place: "Dhaka".into(), at: unix_now() });
                self.wifi = Some(Wifi::Connected("Home".into(), 78));
                self.sound = SoundNow { volume: Some((0.6, false)), mic_muted: Some(true) };
                *PLACE.lock().unwrap() = Some((23.81, 90.41, "Dhaka".into()));
            }
            i if (A_ITEM..A_ITEM + 500).contains(&i) => {
                let Some(it) = self.items.get((i - A_ITEM) as usize) else { return };
                let id = it.id.clone();
                if self.layout.contains(&id) {
                    self.layout.remove(&id);
                } else {
                    self.layout.add(&id);
                }
                ctx.settings.layout = self.layout.clone();
                ctx.settings.save();
            }
            i if (A_UP..A_UP + n).contains(&i) => {
                let k = (i - A_UP) as usize;
                if k > 0 {
                    self.layout.rows.swap(k, k - 1);
                    self.save(ctx);
                }
            }
            i if (A_DOWN..A_DOWN + n).contains(&i) => {
                let k = (i - A_DOWN) as usize;
                if k + 1 < self.layout.rows.len() {
                    self.layout.rows.swap(k, k + 1);
                    self.save(ctx);
                }
            }
            i if (A_SIZE..A_SIZE + n).contains(&i) => {
                toggle_size(&mut self.layout.rows, (i - A_SIZE) as usize);
                self.save(ctx);
            }
            i if (A_WHEN_L..A_WHEN_L + n).contains(&i) || (A_WHEN_R..A_WHEN_R + n).contains(&i) => {
                let right = i >= A_WHEN_R;
                let k = (i - if right { A_WHEN_R } else { A_WHEN_L }) as usize;
                let row = &self.layout.rows[k];
                let id = if right && !row.wide { row.right.clone() } else { row.left.clone() };
                let next = self.layout.when_of(&id).next();
                self.layout.when.insert(id, next);
                self.save(ctx);
            }
            i if (A_X_L..A_X_L + n).contains(&i) || (A_X_R..A_X_R + n).contains(&i) => {
                let right = i >= A_X_R;
                let k = (i - if right { A_X_R } else { A_X_L }) as usize;
                let row = &self.layout.rows[k];
                let id = if right && !row.wide { row.right.clone() } else { row.left.clone() };
                self.layout.remove(&id);
                self.save(ctx);
            }
            i if (A_ALERT..A_ALERT + (crate::store::ALERTS.len() * 3) as u32).contains(&i) => {
                let k = ((i - A_ALERT) / 3) as usize;
                let m = AlertMode::ALL[((i - A_ALERT) % 3) as usize];
                ctx.settings.set_alert_mode(crate::store::ALERTS[k].0, m);
                self.alert_modes[k] = m;
                self.layout.alerts = ctx.settings.layout.alerts.clone();
                ctx.settings.save();
            }
            i if (A_PLACE_MODE..A_PLACE_MODE + 3).contains(&i) => {
                let m = [PlaceMode::Auto, PlaceMode::Windows, PlaceMode::Manual][(i - A_PLACE_MODE) as usize];
                if m != ctx.settings.place.mode {
                    ctx.settings.place.mode = m;
                    // A Windows position or a chosen city don't carry over.
                    if m != PlaceMode::Manual {
                        ctx.settings.place.name.clear();
                    }
                    ctx.settings.save();
                }
                self.place_set = ctx.settings.place.clone();
                self.resolve_place(ctx);
            }
            A_UNIT | 11031 => {
                ctx.settings.place.fahrenheit = id != A_UNIT;
                self.place_set = ctx.settings.place.clone();
                WEATHER_F.store(ctx.settings.place.fahrenheit, Ordering::Relaxed);
                ctx.settings.save();
            }
            A_PLACE_PRAYER => {
                ctx.settings.place.prayer = !ctx.settings.place.prayer;
                self.place_set = ctx.settings.place.clone();
                ctx.settings.save();
            }
            A_PLACE_FIELD => {
                if self.query.is_none() {
                    self.query = Some(String::new());
                }
                ctx.want_keyboard(true);
            }
            i if (A_PLACE_RESULT..A_PLACE_RESULT + 10).contains(&i) => {
                let pick = self.search.lock().unwrap().results.get((i - A_PLACE_RESULT) as usize).cloned();
                if let Some((name, la, lo)) = pick {
                    let pl = &mut ctx.settings.place;
                    pl.mode = PlaceMode::Manual;
                    pl.lat = la;
                    pl.lon = lo;
                    pl.name = name;
                    ctx.settings.save();
                    self.place_set = ctx.settings.place.clone();
                    self.query = None;
                    ctx.want_keyboard(false);
                    *self.search.lock().unwrap() = Search::default();
                    self.resolve_place(ctx);
                }
            }
            _ => {}
        }
    }

    fn debug_state(&self) -> String {
        let bt = self.bt.lock().unwrap();
        format!("power {:?}, bluetooth {:?} (read {:.0}s ago), layout {} custom {}, place {:?}", self.power, bt.devices, unix_now() - bt.read_at, self.layout.to_text(), self.layout.custom, PLACE.lock().unwrap())
    }
}

/// For the self-test.
pub fn test_drop(rows: &mut Vec<NotchRow>, item: Option<(usize, bool)>, row: Option<usize>, slot: Option<(usize, bool)>, gap: Option<usize>) {
    let what = match (item, row) {
        (Some((k, s)), _) => Grab::Item(k, s),
        (_, Some(k)) => Grab::Row(k),
        _ => return,
    };
    let to = match (slot, gap) {
        (Some((k, s)), _) => Target::Slot(k, s),
        (_, Some(g)) => Target::Gap(g),
        _ => return,
    };
    apply_drop(rows, what, to);
}

pub fn test_toggle_size(rows: &mut Vec<NotchRow>, k: usize) {
    toggle_size(rows, k);
}
