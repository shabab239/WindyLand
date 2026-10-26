//! Background watcher (no tab): pops toasts for battery/charger, network,
//! Caps Lock, keyboard-language switches, volume and Bluetooth devices.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Networking::NetworkListManager::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Power::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::WM_APP_WAKE;
use crate::gfx::*;
use crate::modules::{Ctx, Module, Toast};
use crate::time::unix_now;

static HOOK_HWND: AtomicIsize = AtomicIsize::new(0);

fn wake(h: isize) {
    unsafe {
        let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

/// Low-level keyboard hook: wakes the app right after Caps Lock or a
/// language-switch chord so the toast appears instantly instead of on the
/// next 1 s tick.
unsafe extern "system" fn kb_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && (wp.0 as u32 == WM_KEYUP || wp.0 as u32 == WM_SYSKEYUP) {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk = VIRTUAL_KEY(k.vkCode as u16);
        if matches!(vk, VK_CAPITAL | VK_SPACE | VK_LSHIFT | VK_RSHIFT | VK_SHIFT | VK_LMENU | VK_RMENU | VK_MENU) {
            wake(HOOK_HWND.load(Ordering::Relaxed));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolCb {
    shared: Arc<Mutex<Option<(f32, bool)>>>,
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolCb_Impl {
    fn OnNotify(&self, p: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if !p.is_null() {
            let d = unsafe { &*p };
            *self.shared.lock().unwrap() = Some((d.fMasterVolume, d.bMuted.as_bool()));
            wake(self.hwnd);
        }
        Ok(())
    }
}

struct Volume {
    _ep: IAudioEndpointVolume,
    _cb: IAudioEndpointVolumeCallback,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Net {
    Internet,
    Local,
    Offline,
}

#[derive(Default)]
struct BtShared {
    events: Vec<(String, bool)>,
}

pub struct System {
    started: f64,
    hwnd: isize,
    // battery
    ac: Option<bool>,
    low_warned: bool,
    crit_warned: bool,
    full_warned: bool,
    // network
    nlm: Option<INetworkListManager>,
    net: Option<Net>,
    net_pending: Option<(Net, f64)>,
    // keys
    caps: Option<bool>,
    hkl: Option<isize>,
    // volume
    _volume: Option<Volume>,
    vol_shared: Arc<Mutex<Option<(f32, bool)>>>,
    // bluetooth
    bt: Arc<Mutex<BtShared>>,
}

impl System {
    pub fn new(hwnd: HWND) -> System {
        let h = hwnd.0 as isize;
        HOOK_HWND.store(h, Ordering::Relaxed);
        unsafe {
            let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), None, 0);
        }
        let nlm: Option<INetworkListManager> = unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok() };
        let vol_shared = Arc::new(Mutex::new(None));
        let volume = Self::watch_volume(vol_shared.clone(), h);
        let bt = Arc::new(Mutex::new(BtShared::default()));
        spawn_bt_watcher(bt.clone(), h);
        System {
            started: unix_now(),
            hwnd: h,
            ac: None,
            low_warned: false,
            crit_warned: false,
            full_warned: false,
            nlm,
            net: None,
            net_pending: None,
            caps: None,
            hkl: None,
            _volume: volume,
            vol_shared,
            bt,
        }
    }

    fn watch_volume(shared: Arc<Mutex<Option<(f32, bool)>>>, hwnd: isize) -> Option<Volume> {
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
            let cb: IAudioEndpointVolumeCallback = VolCb { shared, hwnd }.into();
            ep.RegisterControlChangeNotify(&cb).ok()?;
            Some(Volume { _ep: ep, _cb: cb })
        }
    }

    fn warm(&self) -> bool {
        unix_now() - self.started > 3.0
    }

    fn battery(&mut self, ctx: &mut Ctx) {
        let mut s = SYSTEM_POWER_STATUS::default();
        if unsafe { GetSystemPowerStatus(&mut s) }.is_err() || s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 {
            return; // no battery (desktop) or unknown
        }
        let pct = s.BatteryLifePercent as u32;
        let ac = s.ACLineStatus == 1;
        let right = format!("{pct}%");
        if let Some(prev) = self.ac {
            if prev != ac {
                if ac {
                    let t = if pct >= 100 { "Fully charged" } else { "Charging" };
                    ctx.toast(Toast::new("power", glyph::CHARGING, col::GREEN, t, "Charger connected").right(right.clone()).progress(pct as f32 / 100.0));
                } else {
                    ctx.toast(
                        Toast::new("power", glyph::BATTERY, col::YELLOW, "Charger unplugged", format!("{pct}% remaining"))
                            .right(right.clone())
                            .progress(pct as f32 / 100.0),
                    );
                }
            }
        }
        self.ac = Some(ac);
        if ac {
            self.low_warned = false;
            self.crit_warned = false;
            if pct >= 100 && !self.full_warned && self.warm() {
                self.full_warned = true;
                ctx.toast(Toast::new("power", glyph::CHARGING, col::GREEN, "Fully charged", "You can unplug").right(right));
            }
        } else {
            self.full_warned = false;
            if pct <= 10 && !self.crit_warned {
                self.crit_warned = true;
                self.low_warned = true;
                ctx.toast(Toast::new("power", glyph::BATTERY, col::RED, "Battery critical", "Plug in now").right(right).secs(8.0).chime());
            } else if pct <= 20 && !self.low_warned {
                self.low_warned = true;
                ctx.toast(Toast::new("power", glyph::BATTERY, col::ACCENT, "Low battery", "Plug in soon").right(right).secs(6.0).chime());
            }
        }
    }

    fn network(&mut self, ctx: &mut Ctx) {
        let Some(nlm) = &self.nlm else { return };
        let Ok(c) = (unsafe { nlm.GetConnectivity() }) else { return };
        let now = if c.0 & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0) != 0 {
            Net::Internet
        } else if c.0 & (NLM_CONNECTIVITY_IPV4_LOCALNETWORK.0 | NLM_CONNECTIVITY_IPV6_LOCALNETWORK.0 | NLM_CONNECTIVITY_IPV4_SUBNET.0 | NLM_CONNECTIVITY_IPV6_SUBNET.0) != 0 {
            Net::Local
        } else {
            Net::Offline
        };
        match self.net {
            None => self.net = Some(now),
            Some(prev) if prev != now => {
                // Debounce: the state must hold for 2 s (Wi-Fi roaming flaps).
                match self.net_pending {
                    Some((p, since)) if p == now && unix_now() - since >= 2.0 => {
                        self.net = Some(now);
                        self.net_pending = None;
                        let t = match now {
                            Net::Internet => Toast::new("net", glyph::WIFI, col::GREEN, "Connected", "You're back online"),
                            Net::Local => Toast::new("net", glyph::NO_NET, col::YELLOW, "No internet access", "Connected to a network without internet"),
                            Net::Offline => Toast::new("net", glyph::WIFI_OFF, col::RED, "Disconnected", "No network connection"),
                        };
                        ctx.toast(t);
                    }
                    Some((p, _)) if p == now => {}
                    _ => self.net_pending = Some((now, unix_now())),
                }
            }
            _ => self.net_pending = None,
        }
    }

    fn keys(&mut self, ctx: &mut Ctx) {
        let caps = unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 == 1;
        if let Some(prev) = self.caps {
            if prev != caps {
                let (sub, c) = if caps { ("On", col::ACCENT) } else { ("Off", col::DIM) };
                ctx.toast(Toast::new("caps", glyph::CAPS, c, "Caps Lock", sub).secs(1.6));
            }
        }
        self.caps = Some(caps);

        let hkl = unsafe {
            let fg = GetForegroundWindow();
            let tid = GetWindowThreadProcessId(fg, None);
            GetKeyboardLayout(tid).0 as isize
        };
        if hkl == 0 {
            return;
        }
        if let Some(prev) = self.hkl {
            if prev != hkl {
                let lang = (hkl & 0xffff) as u32;
                let (name, short) = lang_name(lang);
                ctx.toast(Toast::new("lang", glyph::KEYBOARD, col::BLUE, name, "Keyboard language").right(short).secs(1.8));
            }
        }
        self.hkl = Some(hkl);
    }

    fn volume(&mut self, ctx: &mut Ctx) {
        let v = self.vol_shared.lock().unwrap().take();
        if let Some((level, muted)) = v {
            if !ctx.settings.volume_overlay || !self.warm() {
                return;
            }
            let pct = (level * 100.0).round() as u32;
            let (g, sub) = if muted || pct == 0 { (glyph::MUTE, "Muted") } else { (glyph::VOLUME, "Output") };
            ctx.toast(
                Toast::new("volume", g, col::TEXT, "Volume", sub)
                    .right(format!("{pct}%"))
                    .progress(if muted { 0.0 } else { level })
                    .secs(1.4),
            );
        }
    }

    fn bluetooth(&mut self, ctx: &mut Ctx) {
        let events = std::mem::take(&mut self.bt.lock().unwrap().events);
        for (name, connected) in events {
            if connected {
                ctx.toast(Toast::new("bt", glyph::HEADPHONE, col::BLUE, name, "Connected").secs(3.0));
            } else {
                ctx.toast(Toast::new("bt", glyph::BLUETOOTH, col::DIM, name, "Disconnected").secs(2.5));
            }
        }
    }
}

fn lang_name(langid: u32) -> (String, String) {
    unsafe {
        let mut buf = [0u16; 85];
        let n = LCIDToLocaleName(langid, Some(&mut buf), 0);
        if n <= 1 {
            return (format!("Layout {langid:04x}"), String::new());
        }
        let loc = String::from_utf16_lossy(&buf[..(n - 1) as usize]);
        let locw = HSTRING::from(loc.as_str());
        let mut name = [0u16; 128];
        let m = GetLocaleInfoEx(&locw, LOCALE_SLOCALIZEDDISPLAYNAME, Some(&mut name));
        let display = if m > 1 { String::from_utf16_lossy(&name[..(m - 1) as usize]) } else { loc.clone() };
        let short = loc.split('-').next().unwrap_or("").to_uppercase();
        (display, short)
    }
}

/// Poll connected Bluetooth (classic) devices every 4 s on a worker thread.
fn spawn_bt_watcher(shared: Arc<Mutex<BtShared>>, hwnd: isize) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(selector) = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected) else {
            return;
        };
        let mut known: Option<BTreeSet<String>> = None;
        loop {
            let mut now = BTreeSet::new();
            if let Ok(op) = DeviceInformation::FindAllAsyncAqsFilter(&selector) {
                if let Ok(list) = op.get() {
                    if let Ok(n) = list.Size() {
                        for i in 0..n {
                            if let Ok(d) = list.GetAt(i) {
                                if let Ok(name) = d.Name() {
                                    now.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
            if let Some(prev) = &known {
                let mut ev = Vec::new();
                for n in now.difference(prev) {
                    ev.push((n.clone(), true));
                }
                for n in prev.difference(&now) {
                    ev.push((n.clone(), false));
                }
                if !ev.is_empty() {
                    shared.lock().unwrap().events.extend(ev);
                    wake(hwnd);
                }
            }
            known = Some(now);
            std::thread::sleep(std::time::Duration::from_secs(4));
        }
    });
}

impl Module for System {
    fn has_tab(&self) -> bool {
        false
    }
    fn tick(&mut self, ctx: &mut Ctx) {
        let _ = self.hwnd;
        self.battery(ctx);
        self.network(ctx);
        self.keys(ctx);
        self.volume(ctx);
        self.bluetooth(ctx);
    }
}
