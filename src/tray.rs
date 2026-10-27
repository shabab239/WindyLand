//! System-tray icon. Left-click opens the settings panel in the notch;
//! right-click shows a short menu (Settings, Check for updates, About, Exit).
//! The settings themselves live in the notch (`App::draw_settings`).

use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{with_app, App, WM_APP_TRAY};
use crate::gfx::make_tray_icon;

const ID_SETTINGS: u32 = 1;
const ID_TOUR: u32 = 2;
const ID_UPDATES: u32 = 6;
const ID_ABOUT: u32 = 8;
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
    let Some(hwnd) = with_app(|a| a.hwnd) else {
        return;
    };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: u32, text: PCWSTR| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, text);
        };
        item(ID_SETTINGS, w!("Settings"));
        let _ = SetMenuDefaultItem(menu, ID_SETTINGS, 0);
        item(ID_TOUR, w!("Welcome tour"));
        item(ID_UPDATES, w!("Check for updates"));
        item(ID_ABOUT, w!("About WindyLand"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        item(ID_EXIT, w!("Exit"));

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
        ID_SETTINGS => app.open_settings(),
        ID_TOUR => app.start_onboarding(),
        ID_UPDATES => {
            if let Some(i) = app.modules.iter().position(|m| m.title() == "Updates") {
                app.call(i, |m, c| m.action(crate::modules::update::A_CHECK_NOW, (0.0, 0.0), c));
            }
        }
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
            "WindyLand {}\n\nA living notch for Windows.\nMusic, lyrics, timer, Pomodoro, to-do, water reminder, file shelf and clipboard history.\n\nLyrics provided by LRCLIB (lrclib.net).\nYour data stays in %APPDATA%\\WindyLand.\n\nhttps://github.com/{}",
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
