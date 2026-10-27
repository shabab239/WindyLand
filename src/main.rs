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
mod mascot;
mod modules;
mod sound;
mod store;
mod time;
mod tray;
mod ui;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::{CreateMutexW, CreateWaitableTimerExW, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The message loop, which also paces animation frames: transitions draw
/// once per display refresh (`app::wait_refresh`), slow ambient animations
/// wake on a precise high-resolution timer, and when nothing moves the
/// thread sleeps until a message arrives. (A plain Windows timer ticks in
/// ~15.6 ms steps and is delayed freely, which made animations choppy.)
unsafe fn run_loop() {
    const TIMER_ALL_ACCESS: u32 = 0x1F0003;
    let timer = CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS)
        .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS))
        .ok();
    let mut msg = MSG::default();
    loop {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let wake = |handles: &[HANDLE]| {
            MsgWaitForMultipleObjectsEx(Some(handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) == WAIT_OBJECT_0 && !handles.is_empty()
        };
        match app::with_app(|a| a.pacing()) {
            Some(app::Pacing::Refresh) => {
                if let Some(h) = app::with_app(|a| {
                    a.frame();
                    a.hwnd
                }) {
                    app::wait_refresh(h);
                }
            }
            Some(app::Pacing::After(secs)) if secs < 0.001 => {
                app::with_app(|a| a.frame());
            }
            Some(app::Pacing::After(secs)) => match timer {
                Some(t) => {
                    let due = -((secs * 1e7) as i64).max(1);
                    let _ = SetWaitableTimer(t, &due, 0, None, None, false);
                    if wake(&[t]) {
                        app::with_app(|a| a.frame());
                    }
                }
                None => {
                    std::thread::sleep(std::time::Duration::from_secs_f64(secs.min(0.05)));
                }
            },
            _ => {
                wake(&[]);
            }
        }
    }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);

        // Docs renders: demo data, fixed clock, no window shown.
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--render-demo") {
            time::DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
            let dir = std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "demo-render".into()));
            let _ = app::App::create(false);
            let log = app::render_demo(&dir);
            let _ = std::fs::write(dir.join("render.log"), log);
            app::shutdown();
            std::process::exit(0);
        }
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
        if let Ok(h) = &mutex {
            app::SINGLE_INSTANCE.store(h.0 as isize, std::sync::atomic::Ordering::SeqCst);
        }

        if let Err(e) = app::App::create(true) {
            let msg = HSTRING::from(format!("WindyLand failed to start:\n{e}"));
            MessageBoxW(None, &msg, w!("WindyLand"), MB_ICONERROR);
            return;
        }

        run_loop();
        app::shutdown();
        drop(mutex);
        // Exit directly. OleUninitialize() while the app's COM objects (D2D,
        // audio callback, network list) are still alive never returns, and
        // worker threads may be mid-WinRT call; the OS cleans all of it up.
        std::process::exit(0);
    }
}
