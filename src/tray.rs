//! System-tray icon and menu (Start with Windows, Personality, Minimal notch,
//! Volume overlay, About, Exit).

use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{with_app, App, WM_APP_TRAY};
use crate::gfx::make_tray_icon;
use crate::store::{Edge, NotchStyle};

const ID_STARTUP: u32 = 1;
const ID_PERSONALITY: u32 = 2;
const ID_STYLE: u32 = 20; // + index into NotchStyle::ALL
const ID_EDGE: u32 = 30; // + index into Edge::ALL
const ID_CENTER: u32 = 39;
const ID_VOLUME: u32 = 4;
const ID_OPEN_DATA: u32 = 5;
const ID_ABOUT: u32 = 8;
const ID_UPDATES: u32 = 6;
const ID_EXIT: u32 = 9;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("WindyLand");

pub fn taskbar_created() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    *M.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut n = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let tip: Vec<u16> = "WindyLand".encode_utf16().collect();
    n.szTip[..tip.len()].copy_from_slice(&tip);
    n
}

pub fn add(hwnd: HWND) {
    static ICON: OnceLock<isize> = OnceLock::new();
    let icon = *ICON.get_or_init(|| make_tray_icon().0 as isize);
    let mut n = nid(hwnd);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = WM_APP_TRAY;
    n.hIcon = HICON(icon as *mut _);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &n);
    }
}

pub fn remove(hwnd: HWND) {
    let n = nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &n);
    }
}

fn exe_path() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
}

pub fn startup_enabled() -> bool {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn set_startup(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            let v = format!("\"{}\"", exe_path());
            let w: Vec<u16> = v.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
            let _ = RegSetValueExW(key, RUN_NAME, 0, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, RUN_NAME);
        }
        let _ = RegCloseKey(key);
    }
}

pub fn show_menu() {
    let Some((hwnd, personality, style, volume, edge)) =
        with_app(|a| (a.hwnd, a.settings.personality, a.settings.style, a.settings.volume_overlay, a.settings.edge))
    else {
        return;
    };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: u32, text: PCWSTR, checked: bool| {
            let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, flags, id as usize, text);
        };
        item(ID_STARTUP, w!("Start with Windows"), startup_enabled());
        if let Ok(sub) = CreatePopupMenu() {
            for (i, s) in NotchStyle::ALL.into_iter().enumerate() {
                let flags = MF_STRING | if s == style { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(sub, flags, (ID_STYLE + i as u32) as usize, &HSTRING::from(s.label()));
            }
            let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Notch style"));
        }
        if let Ok(sub) = CreatePopupMenu() {
            for (i, e) in Edge::ALL.into_iter().enumerate() {
                let flags = MF_STRING | if e == edge { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(sub, flags, (ID_EDGE + i as u32) as usize, &HSTRING::from(e.label()));
            }
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(sub, MF_STRING, ID_CENTER as usize, w!("Center on edge"));
            let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, 0, w!("Tip: right-click and drag the notch"));
            let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Position"));
        }
        item(ID_PERSONALITY, w!("Personality (mascot eyes, full style)"), personality);
        item(ID_VOLUME, w!("Volume overlay"), volume);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        item(ID_OPEN_DATA, w!("Open data folder"), false);
        item(ID_UPDATES, w!("Check for updates"), false);
        item(ID_ABOUT, w!("About WindyLand"), false);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        item(ID_EXIT, w!("Exit"), false);

        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, p.x, p.y, 0, hwnd, None);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let cmd = cmd.0 as u32;
        match cmd {
            0 => {}
            ID_ABOUT => about(),
            ID_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            c => {
                with_app(|a| command(a, c));
            }
        }
    }
}

pub fn command(app: &mut App, id: u32) {
    match id {
        ID_STARTUP => set_startup(!startup_enabled()),
        ID_PERSONALITY => app.settings.personality = !app.settings.personality,
        i if (ID_STYLE..ID_STYLE + NotchStyle::ALL.len() as u32).contains(&i) => {
            app.settings.style = NotchStyle::ALL[(i - ID_STYLE) as usize];
        }
        i if (ID_EDGE..ID_EDGE + Edge::ALL.len() as u32).contains(&i) => {
            app.set_edge(Edge::ALL[(i - ID_EDGE) as usize], false);
        }
        ID_CENTER => {
            let e = app.settings.edge;
            app.set_edge(e, true);
        }
        ID_VOLUME => app.settings.volume_overlay = !app.settings.volume_overlay,
        ID_UPDATES => {
            if let Some(i) = app.modules.iter().position(|m| m.title() == "Updates") {
                app.call(i, |m, c| m.action(crate::modules::update::A_CHECK_NOW, (0.0, 0.0), c));
            }
        }
        ID_OPEN_DATA => unsafe {
            let d = HSTRING::from(crate::store::dir().display().to_string());
            ShellExecuteW(None, w!("open"), &d, None, None, SW_SHOWNORMAL);
        },
        ID_EXIT => unsafe {
            let _ = DestroyWindow(app.hwnd);
        },
        _ => {}
    }
    app.settings.save();
    app.request_frame();
}

pub fn about() {
    unsafe {
        let text = HSTRING::from(format!(
            "WindyLand {}\n\nA living notch for Windows.\nMusic, lyrics, timer, Pomodoro, to-do, water reminder and file shelf.\n\nLyrics provided by LRCLIB (lrclib.net).\nYour data stays in %APPDATA%\\WindyLand.\n\nhttps://github.com/{}",
            crate::modules::update::VERSION,
            crate::modules::update::REPO
        ));
        MessageBoxW(
            None,
            &text,
            w!("About WindyLand"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
