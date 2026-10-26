#![windows_subsystem = "windows"]
// Many helpers (gfx shapes, palette, Ctx methods) exist for future modules.
#![allow(dead_code)]
//! WindyLand: a living notch for Windows.
//!
//! Layout:
//!   app.rs      window, animation, input routing, toasts, mascot
//!   gfx.rs      Direct2D drawing into a per-pixel-alpha layered window
//!   ui.rs       immediate-mode widgets (buttons, chips, progress, hits)
//!   modules/    one file per feature, add your own here (see modules/mod.rs)
//!   tray.rs     tray icon + menu
//!   dragdrop.rs OLE drag in/out
//!   store.rs    %APPDATA%\WindyLand\*.txt persistence
//!   http.rs     WinHTTP GET (used for LRCLIB lyrics)

mod app;
mod dragdrop;
mod gfx;
mod http;
mod modules;
mod sound;
mod store;
mod time;
mod tray;
mod ui;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);

        // Docs renders: demo data, fixed clock, no window shown.
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--render-docs") {
            time::DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
            let dir = std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "docs-render".into()));
            let _ = app::App::create(false);
            let log = app::render_docs(&dir);
            let _ = std::fs::write(dir.join("render.log"), log);
            app::shutdown();
            std::process::exit(0);
        }

        // The self-test never shows a window, so it may run alongside the app.
        if std::env::args().any(|a| a == "--selftest") {
            let _ = app::App::create(false);
            let mut report = app::selftest();
            report += &format!("INFO lyrics (LRCLIB): {}\n", modules::music::selftest_lyrics());
            report += &format!("INFO data dir: {}\n", store::dir().display());
            let out = std::env::var("WINDYLAND_SELFTEST_OUT").unwrap_or_else(|_| "selftest.txt".into());
            let _ = std::fs::write(out, report);
            app::shutdown();
            std::process::exit(0);
        }

        let mutex = CreateMutexW(None, true, w!("Local\\WindyLand.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        if let Err(e) = app::App::create(true) {
            let msg = HSTRING::from(format!("WindyLand failed to start:\n{e}"));
            MessageBoxW(None, &msg, w!("WindyLand"), MB_ICONERROR);
            return;
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        app::shutdown();
        drop(mutex);
        // Exit directly. OleUninitialize() while the app's COM objects (D2D,
        // audio callback, network list) are still alive never returns, and
        // worker threads may be mid-WinRT call; the OS cleans all of it up.
        std::process::exit(0);
    }
}
