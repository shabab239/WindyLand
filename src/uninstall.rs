//! `windyland.exe --uninstall [--quiet] [--keep-data]`: remove WindyLand
//! without needing the installer's own uninstaller to be allowed to run
//! (Smart App Control can block an unsigned uninstaller).
//!
//! Order: ask (unless --quiet), close the running copy (only if it is this
//! same exe), unhook Claude Code / Codex, take it out of startup (only if
//! the startup entry points at this exe), delete the data folder if asked,
//! then hand over to the installer's uninstaller when there is one, or
//! delete this exe after it exits. Screenshots in Pictures are never touched,
//! and a folder is only removed if it ends up empty.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string().to_lowercase();
    canon(a) == canon(b)
}

/// Full path of the process that owns `hwnd`.
fn window_exe(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
    }
}

/// Close the running notch if it is this exe; wait up to 5 s.
fn close_running() -> bool {
    unsafe {
        let Ok(h) = FindWindowW(w!("WindyLandNotch"), None) else { return true };
        if h.0.is_null() {
            return true;
        }
        if !window_exe(h).map_or(false, |p| same_file(&p, &exe())) {
            return true;
        }
        let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !IsWindow(h).as_bool() {
                return true;
            }
        }
        false
    }
}

fn ask(text: &str, buttons: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let t = HSTRING::from(text);
    unsafe { MessageBoxW(None, &t, w!("Uninstall WindyLand"), buttons | MB_SETFOREGROUND) }
}

/// What uninstalling would do, one step per line (also the --dry-run output).
pub fn plan(keep_data: bool) -> Vec<String> {
    let exe = exe();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut v = vec!["close WindyLand if this copy is running".to_string(), "disconnect Hermes, Claude Code and Codex".to_string()];
    if crate::tray::startup_points_here() {
        v.push("remove it from Windows startup".into());
    }
    if !keep_data {
        v.push(format!("delete data folder {}", crate::store::dir().display()));
    }
    let unins = dir.join("unins000.exe");
    if unins.exists() {
        v.push(format!("run {} /VERYSILENT", unins.display()));
    } else {
        v.push(format!("delete {}", exe.display()));
        v.push(format!("remove {} if it is empty", dir.display()));
    }
    v
}

pub fn run(args: &[String]) -> i32 {
    let quiet = args.iter().any(|a| a == "--quiet");
    let dry = args.iter().any(|a| a == "--dry-run");
    let mut keep = args.iter().any(|a| a == "--keep-data");
    if dry {
        println!("{}", plan(keep).join("\n"));
        return 0;
    }
    if !quiet {
        if ask("Remove WindyLand from this PC?\n\nIt closes WindyLand, takes it out of startup and disconnects Hermes, Claude Code and Codex.", MB_OKCANCEL | MB_ICONQUESTION) != IDOK {
            return 1;
        }
        if !keep {
            let r = ask(
                "Also delete your WindyLand data?\n\nSettings, tasks, water log, Shelf list and pinned clipboard entries. Screenshots in Pictures\\Screenshots are kept either way.",
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
            );
            keep = r != IDYES;
        }
    }
    if !close_running() {
        if !quiet {
            ask("WindyLand didn't close. Quit it from the tray icon and try again.", MB_OK | MB_ICONWARNING);
        }
        return 2;
    }
    crate::modules::agents::disconnect_all();
    if crate::tray::startup_points_here() {
        crate::tray::set_startup(false);
    }
    if !keep {
        let _ = std::fs::remove_dir_all(crate::store::dir());
    }
    let exe = exe();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let unins = dir.join("unins000.exe");
    if unins.exists() {
        // Inno's uninstaller removes the files, shortcuts and registry entry.
        let _ = std::process::Command::new(&unins).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]).spawn();
        return 0;
    }
    // No uninstaller (a copied exe): delete it once this process has exited,
    // and the folder only if nothing else is in it.
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
        exe.display(),
        dir.display()
    );
    // Run from %TEMP%: a folder that is anyone's current directory can't be removed.
    let tmp = std::env::temp_dir();
    let _ = std::env::set_current_dir(&tmp);
    let _ = std::process::Command::new("cmd.exe").raw_arg(format!("/c {script}")).current_dir(&tmp).creation_flags(CREATE_NO_WINDOW).spawn();
    if !quiet {
        ask("WindyLand was removed.", MB_OK | MB_ICONINFORMATION);
    }
    0
}
