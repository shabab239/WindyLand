//! Notifications: other apps' Windows notifications, shown in the notch.
//!
//! Windows' own notification listener lets an ordinary (unpackaged) desktop
//! app read the notifications in Notification Center, but its "new
//! notification" event only works for packaged, signed apps. So a background
//! thread watches the modification time of Windows' notification database
//! (`wpndatabase.db-wal`, which changes within milliseconds of a new
//! notification, and costs ~0.02 ms to check) every 0.4 s, and only then asks
//! the listener for the list and reads the new entries. If that file isn't
//! there, it simply re-reads the list every few seconds.
//!
//! Off until the user turns it on. Nothing is stored or sent anywhere. It
//! stays quiet during Do Not Disturb, apps can be muted, and message text can
//! be hidden (only the app name shows). Windows still shows its own banner
//! unless the user turns banners off; Settings links to that page.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::Size;
use windows::UI::Notifications::Management::*;
use windows::UI::Notifications::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::WM_APP_WAKE;
use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};

/// Read by the watcher thread: whether to read notifications at all.
pub static ON: AtomicBool = AtomicBool::new(false);
/// Listener access: 0 unknown, 1 allowed, 2 denied by Windows' privacy settings.
pub static ACCESS: AtomicU8 = AtomicU8::new(0);

pub const A_OPEN: u32 = 1;
pub const A_MUTE: u32 = 2;
pub const A_BANNERS: u32 = 3;
pub const A_PRIVACY: u32 = 4;
/// Self-test / docs: show a made-up notification.
pub const A_DEMO: u32 = 990;
/// Self-test: queue a made-up notification as if the watcher found it.
pub const A_DEMO_INBOX: u32 = 991;

/// Settings key holding muted apps (AppUserModelIDs, `|`-separated).
pub const MUTED_KEY: &str = "notif_muted";

#[derive(Clone, Debug, Default)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub aumid: String,
    pub title: String,
    pub body: String,
    pub icon: Option<Arc<(u32, u32, Vec<u8>)>>,
    pub color: Option<Color>,
}

pub struct Notifs {
    hwnd: isize,
    inbox: Arc<Mutex<Vec<Note>>>,
    started: bool,
    was_on: Option<bool>,
    told_denied: bool,
    /// The note in the last toast (what Open/Mute act on).
    shown: Option<Note>,
    /// Notes skipped because of Do Not Disturb (for the self-test).
    pub quiet_skipped: u32,
    /// Notes that arrived while the notch was hidden (full-screen app),
    /// shown together when it's back.
    backlog: Vec<Note>,
}

impl Notifs {
    pub fn new(hwnd: HWND) -> Notifs {
        Notifs { hwnd: hwnd.0 as isize, inbox: Arc::default(), started: false, was_on: None, told_denied: false, shown: None, quiet_skipped: 0, backlog: Vec::new() }
    }

    /// Feed a note as if the watcher had found it (self-test, docs).
    pub fn inject(&self, n: Note) {
        self.inbox.lock().unwrap().push(n);
    }
}

/// Muted app ids from settings.
pub fn muted(ctx_extra: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    ctx_extra.get(MUTED_KEY).map(|s| s.split('|').filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default()
}

/// What the toast for `n` says: (title, sub). With `show_text` off only the
/// app is named, so nothing private appears on screen.
pub fn toast_text(n: &Note, show_text: bool) -> (String, String) {
    let app = if n.app.is_empty() { "Notification".to_string() } else { n.app.clone() };
    if !show_text {
        return (app, "New notification".into());
    }
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    match (n.title.trim().is_empty(), n.body.trim().is_empty()) {
        (false, false) => (one_line(&n.title), one_line(&n.body)),
        (false, true) => (app, one_line(&n.title)),
        (true, false) => (app, one_line(&n.body)),
        (true, true) => (app, "New notification".into()),
    }
}

/// Do Not Disturb / quiet hours. Windows 11 publishes its focus state as a
/// WNF state (the same one the shell uses); older builds report quiet time
/// through SHQueryUserNotificationState.
pub fn quiet() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_QUIET_TIME};
    if unsafe { matches!(SHQueryUserNotificationState(), Ok(QUNS_QUIET_TIME)) } {
        return true;
    }
    focus_profile().map_or(false, |p| p != 0)
}

/// WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED: 0 = off, 1 = priority only,
/// 2 = alarms only. None when it can't be read.
pub fn focus_profile() -> Option<u32> {
    type Query = unsafe extern "system" fn(*const u64, *const core::ffi::c_void, *const core::ffi::c_void, *mut u32, *mut core::ffi::c_void, *mut u32) -> i32;
    static F: std::sync::OnceLock<Option<Query>> = std::sync::OnceLock::new();
    let f = (*F.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let m = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        GetProcAddress(m, s!("NtQueryWnfStateData")).map(|p| std::mem::transmute::<_, Query>(p))
    }))?;
    let name: u64 = 0x0D83_063E_A3BF_1C75;
    let mut stamp = 0u32;
    let mut val = 0u32;
    let mut size = 4u32;
    let st = unsafe { f(&name, std::ptr::null(), std::ptr::null(), &mut stamp, &mut val as *mut u32 as *mut _, &mut size) };
    (st >= 0 && size >= 4).then_some(val)
}

/// A readable accent from an app icon: the average of its coloured pixels,
/// pushed bright and saturated enough to read on black.
pub fn icon_color(px: &[u8]) -> Option<Color> {
    let (mut r, mut g, mut b, mut n) = (0f32, 0f32, 0f32, 0f32);
    for p in px.chunks_exact(4) {
        let a = p[3] as f32 / 255.0;
        if a < 0.5 {
            continue;
        }
        let (pb, pg, pr) = (p[0] as f32 / 255.0 / a, p[1] as f32 / 255.0 / a, p[2] as f32 / 255.0 / a);
        let mx = pr.max(pg).max(pb);
        let mn = pr.min(pg).min(pb);
        // Weight colourful pixels; greys barely count.
        let w = (mx - mn).max(0.02);
        r += pr * w;
        g += pg * w;
        b += pb * w;
        n += w;
    }
    if n < 1.0 {
        return None;
    }
    let (r, g, b) = (r / n, g / n, b / n);
    // HSL, then clamp saturation and lightness.
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d < 0.06 {
        return Some(col::TEXT);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-3);
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    let (s, l) = (s.clamp(0.55, 0.9), l.clamp(0.58, 0.68));
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Some(Color { r: r1 + m, g: g1 + m, b: b1 + m, a: 1.0 })
}

fn wake(h: isize) {
    unsafe {
        let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

/// Windows FILETIME now (100 ns since 1601), to compare with CreationTime.
fn filetime_now() -> i64 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64
}

/// Read everything WindyLand shows from one notification.
fn read_note(n: &UserNotification) -> Note {
    let mut note = Note { id: n.Id().unwrap_or(0), ..Default::default() };
    if let Ok(info) = n.AppInfo() {
        note.aumid = info.AppUserModelId().map(|s| s.to_string()).unwrap_or_default();
        if let Ok(d) = info.DisplayInfo() {
            note.app = d.DisplayName().map(|s| s.to_string()).unwrap_or_default();
            let logo = d.GetLogo(Size { Width: 64.0, Height: 64.0 }).ok();
            let bytes = logo.and_then(|r| r.cast().ok()).and_then(|r| unsafe { crate::modules::music::read_stream_bytes(&r) });
            if let Some(img) = bytes.and_then(|b| unsafe { crate::modules::music::decode_square(&b, 64) }) {
                note.color = icon_color(&img.2);
                note.icon = Some(Arc::new(img));
            }
        }
    }
    let texts: Vec<String> = n
        .Notification()
        .and_then(|x| x.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|t| t.into_iter().filter_map(|e| e.Text().ok()).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let mut it = texts.into_iter().filter(|s| !s.trim().is_empty());
    note.title = it.next().unwrap_or_default();
    note.body = it.collect::<Vec<_>>().join(" ");
    note
}

/// The watcher thread (see the module docs).
fn watch(inbox: Arc<Mutex<Vec<Note>>>, hwnd: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Notifications");
    let stat = |f: &str| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok();
    let mut listener: Option<UserNotificationListener> = None;
    let mut seen: Option<HashSet<u32>> = None;
    let mut last_sig = (None, None);
    let mut last_read = Instant::now() - Duration::from_secs(60);
    loop {
        std::thread::sleep(Duration::from_millis(400));
        if !ON.load(Ordering::Relaxed) {
            // Turned off: forget what we saw, so turning it back on never
            // replays old notifications.
            seen = None;
            continue;
        }
        if listener.is_none() {
            listener = UserNotificationListener::Current().ok();
        }
        let Some(l) = listener.as_ref() else {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        let allowed = l.GetAccessStatus().map_or(false, |s| s == UserNotificationListenerAccessStatus::Allowed);
        let prev = ACCESS.swap(if allowed { 1 } else { 2 }, Ordering::Relaxed);
        if !allowed {
            if prev != 2 {
                wake(hwnd);
            }
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        let sig = (stat("wpndatabase.db-wal"), stat("wpndatabase.db"));
        let fallback = if sig.0.is_some() { 20.0 } else { 3.0 };
        if seen.is_some() && sig == last_sig && last_read.elapsed().as_secs_f64() < fallback {
            continue;
        }
        last_sig = sig;
        last_read = Instant::now();
        let Ok(list) = l.GetNotificationsAsync(NotificationKinds::Toast).and_then(|op| op.get()) else { continue };
        let items: Vec<UserNotification> = list.into_iter().collect();
        let ids: HashSet<u32> = items.iter().filter_map(|n| n.Id().ok()).collect();
        let Some(old) = seen.replace(ids.clone()) else { continue };
        let now = filetime_now();
        let mut fresh: Vec<Note> = items
            .iter()
            .filter(|n| n.Id().map_or(false, |id| !old.contains(&id)))
            // Only notifications from the last minute (not a backlog after sleep).
            .filter(|n| n.CreationTime().map_or(true, |t| now - t.UniversalTime < 60 * 10_000_000))
            .map(read_note)
            .collect();
        if fresh.is_empty() {
            continue;
        }
        fresh.sort_by_key(|n| n.id);
        let keep = fresh.len().saturating_sub(5);
        inbox.lock().unwrap().extend(fresh.into_iter().skip(keep));
        wake(hwnd);
    }
}

impl Module for Notifs {
    fn title(&self) -> &'static str {
        "Notifications"
    }
    fn has_tab(&self) -> bool {
        false
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        let on = ctx.settings.notif_on && !crate::time::demo();
        ON.store(on, Ordering::Relaxed);
        if on && !self.started {
            self.started = true;
            let (inbox, hwnd) = (self.inbox.clone(), self.hwnd);
            std::thread::spawn(move || watch(inbox, hwnd));
        }
        // Just turned on: Windows keeps showing its own banners unless the
        // user switches them off, so offer the way there once.
        if self.was_on == Some(false) && ctx.settings.notif_on {
            ctx.toast(
                Toast::new("notif", glyph::BELL, col::BLUE, "Notifications in the notch", "Turn off Windows' banners to see each one only once")
                    .action("Open Windows settings", A_BANNERS)
                    .action("Later", 0)
                    .secs(12.0),
            );
        }
        self.was_on = Some(ctx.settings.notif_on);
        if on && ACCESS.load(Ordering::Relaxed) == 2 && !self.told_denied {
            self.told_denied = true;
            ctx.toast(
                Toast::new("notif", glyph::BELL, col::YELLOW, "Windows blocks notification access", "Allow it under Privacy > Notifications")
                    .action("Open settings", A_PRIVACY)
                    .action("Later", 0)
                    .secs(12.0),
            );
        }

        let notes: Vec<Note> = std::mem::take(&mut *self.inbox.lock().unwrap());
        if !ctx.settings.notif_on {
            self.backlog.clear();
            return;
        }
        // Hold them while a full-screen app hides the notch, then show the
        // newest with a count of the rest.
        self.backlog.extend(notes);
        if !ctx.hidden {
            let all = std::mem::take(&mut self.backlog);
            self.show(all, ctx);
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        self.act(id, ctx);
    }

    fn debug_state(&self) -> String {
        format!("on {}, access {}, focus profile {:?}", ON.load(Ordering::Relaxed), ACCESS.load(Ordering::Relaxed), focus_profile())
    }
}

impl Notifs {
    /// Toast the newest of `notes` (muted apps and Do Not Disturb respected).
    fn show(&mut self, notes: Vec<Note>, ctx: &mut Ctx) {
        if notes.is_empty() {
            return;
        }
        let muted = muted(&ctx.settings.extra);
        let notes: Vec<Note> = notes.into_iter().filter(|n| !muted.contains(&n.aumid)).collect();
        if notes.is_empty() {
            return;
        }
        if quiet() {
            self.quiet_skipped += notes.len() as u32;
            return;
        }
        // Several at once (e.g. after a full-screen game): show the newest,
        // and say how many more there were.
        let extra = notes.len() - 1;
        let n = notes.into_iter().last().unwrap();
        let (title, mut sub) = toast_text(&n, ctx.settings.notif_text);
        if extra > 0 {
            sub = format!("{sub}  (+{extra} more)");
        }
        let color = n.color.unwrap_or(col::BLUE);
        let mut t = Toast::new("notif", glyph::BELL, color, title, sub).click(A_OPEN).action("Open", A_OPEN).action("Mute app", A_MUTE).secs(6.0);
        if let Some(img) = &n.icon {
            t = t.image(img.clone());
        }
        ctx.toast(t);
        self.shown = Some(n);
    }

    fn act(&mut self, id: u32, ctx: &mut Ctx) {
        match id {
            A_OPEN => {
                // Open the app that sent it (packaged and desktop apps alike).
                if let Some(n) = self.shown.as_ref().filter(|n| !n.aumid.is_empty()) {
                    let target = HSTRING::from(format!("shell:AppsFolder\\{}", n.aumid));
                    unsafe {
                        ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL);
                    }
                }
            }
            A_MUTE => {
                if let Some(n) = self.shown.take().filter(|n| !n.aumid.is_empty()) {
                    let mut m = muted(&ctx.settings.extra);
                    if !m.contains(&n.aumid) {
                        m.push(n.aumid.clone());
                    }
                    ctx.settings.extra.insert(MUTED_KEY.into(), m.join("|"));
                    ctx.settings.save();
                    ctx.toast(Toast::new("notif", glyph::MUTE, col::DIM, format!("{} muted", n.app), "Unmute in Settings > Privacy & alerts").secs(4.0));
                }
            }
            A_BANNERS => open_uri(w!("ms-settings:notifications")),
            A_PRIVACY => open_uri(w!("ms-settings:privacy-notifications")),
            A_DEMO | A_DEMO_INBOX => {
                let n = Note {
                    id: 1,
                    app: "Messages".into(),
                    aumid: "WindyLand.Demo".into(),
                    title: "Nadia".into(),
                    body: "Are we still on for 7? I'll bring the slides".into(),
                    icon: Some(Arc::new(demo_icon())),
                    color: Some(col::GREEN),
                };
                if id == A_DEMO_INBOX {
                    self.inject(n);
                } else {
                    self.show(vec![n], ctx);
                }
            }
            _ => {}
        }
    }
}

/// A made-up "Messages" app icon for demos and the self-test: a green
/// rounded square with a white speech bubble (premultiplied BGRA, 64 px).
pub fn demo_icon() -> (u32, u32, Vec<u8>) {
    let n = 64usize;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square, radius 14, with a soft 1 px edge.
            let (qx, qy) = ((fx - 32.0).abs() - 18.0, (fy - 32.0).abs() - 18.0);
            let outside = (qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)) - 14.0;
            let a = (0.5 - outside).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            // Vertical green gradient.
            let k = fy / 64.0;
            let (mut r, mut g, mut b) = (0.36 - 0.12 * k, 0.86 - 0.16 * k, 0.42 - 0.12 * k);
            // Bubble: ellipse plus a little tail.
            let e = ((fx - 32.0) / 17.0).powi(2) + ((fy - 30.0) / 13.0).powi(2);
            let tail = fy > 36.0 && fy < 47.0 && fx > 20.0 && fx < 30.0 && (fx - 20.0) > (fy - 36.0) * 0.9;
            let w = ((1.0 - e) * 14.0).clamp(0.0, 1.0).max(if tail { 1.0 } else { 0.0 });
            r += (1.0 - r) * w;
            g += (1.0 - g) * w;
            b += (1.0 - b) * w;
            let i = (y * n + x) * 4;
            px[i] = (b * a * 255.0) as u8;
            px[i + 1] = (g * a * 255.0) as u8;
            px[i + 2] = (r * a * 255.0) as u8;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    (64, 64, px)
}

pub fn open_uri(uri: PCWSTR) {
    unsafe {
        ShellExecuteW(None, w!("open"), uri, None, None, SW_SHOWNORMAL);
    }
}
