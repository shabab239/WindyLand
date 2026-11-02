//! Tobi, the AI Agent, chatting right inside the notch: type in the field on the Tobi tab, see his answers and what he is doing,
//! approve or deny what he asks permission for, drop files on the tab to give them to him. The bigger chat window is one
//! button away, and his reminders pop out of the notch as toasts.
//!
//! Tobi is a small Python program installed next to windyland.exe (`agent\`, with its own `venv\`).
//! This module only starts it, shows whether it is running and relays notifications.
//!
//! Start:   `agent\venv\Scripts\pythonw.exe -m windyagent.launcher --rect l,t,r,b`
//! Attach:  `... -m windyagent.launcher --rect l,t,r,b --attach <file> [<file>...]`
//! Stop:    `... -m windyagent.launcher --stop`
//! Status:  a TCP connect to the port Tobi writes to `%APPDATA%\WindyLand\agent\port.txt`.
//! Notices: Tobi appends `title<TAB>message` lines to `%APPDATA%\WindyLand\agent\notify.txt`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, PostMessageW};

use crate::app::WM_APP_WAKE;

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::store;
use crate::ui::Ui;

const A_OPEN: u32 = 1;
const A_STOP: u32 = 2;
const A_SETTINGS: u32 = 3;
const A_FIELD: u32 = 4;
const A_SEND: u32 = 5;
const A_ALLOW: u32 = 6;
const A_DENY: u32 = 7;
const A_HALT: u32 = 8;
const A_NEW: u32 = 9;
const A_ATTACH: u32 = 10;
const A_KNOW: u32 = 11;
const A_UNATTACH: u32 = 100;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const DEFAULT_PORT: u16 = 47615;

/// `<folder of windyland.exe>\agent`, if Tobi was installed.
pub fn agent_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("agent");
    dir.join("windyagent").is_dir().then_some(dir)
}

fn python() -> Option<PathBuf> {
    let p = agent_dir()?.join("venv").join("Scripts").join("pythonw.exe");
    p.is_file().then_some(p)
}

pub fn port() -> u16 {
    store::read("agent/port.txt").trim().parse().unwrap_or(DEFAULT_PORT)
}

fn running() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// "l,t,r,b" of the notch window in screen pixels, so the chat window opens next to it.
fn rect_arg(hwnd: HWND) -> Option<String> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    Some(format!("{},{},{},{}", r.left, r.top, r.right, r.bottom))
}

fn launch(args: &[String]) -> Result<(), String> {
    let dir = agent_dir().ok_or("Tobi is not installed (run the WindyLand setup again).")?;
    let py = python().ok_or("Tobi's Python is missing (run the WindyLand setup again).")?;
    Command::new(py)
        .args(["-m", "windyagent.launcher"])
        .args(args)
        .current_dir(dir)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start Tobi: {e}"))
}

fn with_rect(hwnd: HWND, mut args: Vec<String>) -> Vec<String> {
    if let Some(r) = rect_arg(hwnd) {
        args.push("--rect".into());
        args.push(r);
    }
    args
}

/// Open Tobi's chat window (starting Tobi first if needed). Also used by the tray menu.
pub fn open_agent(hwnd: HWND) -> Result<(), String> {
    launch(&with_rect(hwnd, vec![]))
}


// ------------------------------------------------------------ talking to Tobi's local server

/// What the notch shows of the conversation.
#[derive(Default, Clone)]
struct Chat {
    /// (is_user, text), oldest first, only the last few.
    msgs: Vec<(bool, String)>,
    busy: bool,
    status: String,
    /// (id, title, detail) of the first thing Tobi is waiting for permission to do.
    pending: Option<(String, String, String)>,
    /// Texts of the two buttons when Tobi asks a question with named answers (yes, no).
    pending_labels: Option<(String, String)>,
    configured: bool,
    error: String,
    /// A message just sent that the server has not listed yet.
    sent: Option<(String, f64)>,
    /// Files chosen with the paperclip, waiting to be attached by the UI thread.
    picked: Vec<String>,
    picking: bool,
}

enum Cmd {
    Send(String),
    Confirm(String, bool),
    Halt,
    New,
    /// Open the file window on the PC (through Tobi's server) and attach what the user picks.
    Pick,
}

fn token() -> String {
    serde_json::from_str::<serde_json::Value>(&store::read("agent/config.json"))
        .ok()
        .and_then(|v| v.get("token").and_then(|t| t.as_str().map(String::from)))
        .unwrap_or_default()
}

/// One small HTTP request to 127.0.0.1 (no TLS, no crates). Returns (status, body).
fn http(method: &str, path: &str, body: Option<&str>) -> Option<(u16, String)> {
    http_t(method, path, body, 8)
}

fn http_t(method: &str, path: &str, body: Option<&str>, read_secs: u64) -> Option<(u16, String)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(read_secs)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: wl_agent={}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        port(),
        token(),
        body.len()
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n")?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, rest.to_string()))
}

fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

fn wake(h: isize) {
    unsafe {
        let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

fn clean(s: &str) -> String {
    // "[Attached file: C:\x\y.docx]" -> "(file) y.docx"
    let mut out = String::new();
    for line in s.lines() {
        let t = line.trim();
        if let Some(p) = t.strip_prefix("[Attached file:").and_then(|x| x.strip_suffix(']')) {
            let name = p.trim().rsplit(['\\', '/']).next().unwrap_or("");
            out.push_str(&format!("(file) {name}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn poll(shared: &Arc<Mutex<Chat>>) -> bool {
    let Some((code, body)) = http("GET", "/api/state", None) else {
        let mut c = shared.lock().unwrap();
        let changed = c.busy;
        c.busy = false;
        return changed;
    };
    if code != 200 {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return false };
    let mut msgs: Vec<(bool, String)> = Vec::new();
    let mut last_tool = String::new();
    if let Some(items) = v.get("history").and_then(|h| h.as_array()) {
        for it in items {
            let kind = it.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let text = it.get("text").and_then(|t| t.as_str()).unwrap_or("");
            match kind {
                "user" => msgs.push((true, clean(text))),
                "assistant" if !text.trim().is_empty() => msgs.push((false, text.trim().to_string())),
                "tool" => last_tool = it.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                _ => {}
            }
        }
    }
    let skip = msgs.len().saturating_sub(12);
    msgs.drain(..skip);
    let busy = v.get("busy").and_then(|b| b.as_bool()).unwrap_or(false);
    let mut status = v.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string();
    if busy && status.is_empty() && !last_tool.is_empty() {
        status = format!("Using {last_tool}...");
    }
    let pending = v.get("pending").and_then(|p| p.as_array()).and_then(|a| a.first()).map(|p| {
        let g = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        (g("id"), g("title"), g("detail"))
    });
    let pending_labels = v.get("pending").and_then(|p| p.as_array()).and_then(|a| a.first()).and_then(|p| p.get("labels")).and_then(|l| l.as_array()).and_then(|l| {
        let a = l.first()?.as_str()?.to_string();
        let b = l.get(1)?.as_str()?.to_string();
        Some((a, b))
    });
    let configured = v.pointer("/settings/configured").and_then(|b| b.as_bool()).unwrap_or(true);
    let mut c = shared.lock().unwrap();
    let now = crate::time::unix_now();
    // keep showing a message that was just sent until the server lists it
    let mut sent = c.sent.take();
    if let Some((text, at)) = &sent {
        if msgs.iter().any(|(u, t)| *u && t.ends_with(text.as_str())) || now - *at > 6.0 {
            sent = None;
        }
    }
    let changed = c.msgs != msgs || c.busy != busy || c.status != status || c.pending != pending || c.pending_labels != pending_labels || c.configured != configured;
    c.msgs = msgs;
    c.busy = busy;
    c.status = status;
    c.pending = pending;
    c.pending_labels = pending_labels;
    c.configured = configured;
    c.sent = sent;
    changed
}

fn worker(hwnd: isize, shared: Arc<Mutex<Chat>>, rx: Receiver<Cmd>, active: Arc<AtomicU64>) {
    loop {
        let recent = {
            let last = active.load(Ordering::Relaxed);
            (crate::time::unix_now() * 1000.0) as u64 - last < 4000
        };
        let busy = shared.lock().unwrap().busy;
        let wait = if busy { 400 } else if recent { 700 } else { 3000 };
        let mut force = false;
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(cmd) => {
                force = true;
                match cmd {
                    Cmd::Send(text) => {
                        if !running() {
                            let _ = launch(&["--no-window".to_string()]);
                            for _ in 0..80 {
                                if running() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(250));
                            }
                        }
                        let r = http("POST", "/api/send", Some(&format!("{{\"text\":{}}}", json_str(&text))));
                        let mut c = shared.lock().unwrap();
                        c.error = match r {
                            Some((200, _)) => String::new(),
                            Some((409, _)) => "Tobi is busy. Press stop first.".into(),
                            Some((code, b)) => format!("Tobi said no ({code}): {}", b.chars().take(80).collect::<String>()),
                            None => "Could not reach Tobi.".into(),
                        };
                    }
                    Cmd::Confirm(id, ok) => {
                        let _ = http("POST", "/api/confirm", Some(&format!("{{\"id\":{},\"approve\":{ok}}}", json_str(&id))));
                    }
                    Cmd::Halt => {
                        let _ = http("POST", "/api/stop", Some("{}"));
                    }
                    Cmd::New => {
                        let _ = http("POST", "/api/new", Some("{}"));
                    }
                    Cmd::Pick => {
                        shared.lock().unwrap().picking = true;
                        wake(hwnd);
                        if !running() {
                            let _ = launch(&["--no-window".to_string()]);
                            for _ in 0..80 {
                                if running() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(250));
                            }
                        }
                        // the file window stays open until the user chooses (up to 10 minutes)
                        let r = http_t("POST", "/api/pick-file", Some("{}"), 600);
                        let path = r
                            .and_then(|(_, b)| serde_json::from_str::<serde_json::Value>(&b).ok())
                            .and_then(|v| v.get("path").and_then(|p| p.as_str().map(String::from)))
                            .unwrap_or_default();
                        let mut c = shared.lock().unwrap();
                        c.picking = false;
                        if !path.trim().is_empty() {
                            c.picked.push(path.trim().to_string());
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if (recent || busy || force) && poll(&shared) || force {
            wake(hwnd);
        }
    }
}

// ------------------------------------------------------------ the tab

pub struct Agent {
    up: bool,
    installed: bool,
    last_poll: f64,
    /// Time of the last click on Open, to show "Starting..." for a few seconds.
    opened_at: f64,
    chat: Arc<Mutex<Chat>>,
    tx: Sender<Cmd>,
    active: Arc<AtomicU64>,
    input: String,
    focused: bool,
    attached: Vec<String>,
    /// Messages typed while Tobi was busy; sent one by one when he is free.
    queued: std::collections::VecDeque<String>,
    /// How many lines the chat is scrolled up from the newest.
    scroll: usize,
    max_scroll: usize,
}

impl Agent {
    pub fn new(hwnd: HWND) -> Agent {
        let installed = agent_dir().is_some();
        // Tobi keeps reminders; if any are waiting, start him quietly so they fire on time.
        if installed && !crate::time::demo() {
            let pending = store::read("agent/reminders.json");
            if pending.contains("\"due\"") {
                let _ = launch(&["--no-window".to_string()]);
            }
        }
        let chat = Arc::new(Mutex::new(Chat::default()));
        let (tx, rx) = channel();
        let active = Arc::new(AtomicU64::new(0));
        if installed && !crate::time::demo() {
            let (c, a, h) = (chat.clone(), active.clone(), hwnd.0 as isize);
            std::thread::spawn(move || worker(h, c, rx, a));
        }
        Agent { up: false, installed, last_poll: 0.0, opened_at: 0.0, chat, tx, active, input: String::new(), focused: false,
                attached: Vec::new(), queued: Default::default(), scroll: 0, max_scroll: 0 }
    }

    fn dispatch(&mut self, full: String) {
        {
            let mut c = self.chat.lock().unwrap();
            c.sent = Some((full.trim().to_string(), crate::time::unix_now()));
            c.busy = true;
            c.error.clear();
        }
        let _ = self.tx.send(Cmd::Send(full));
    }

    fn send(&mut self, ctx: &mut Ctx) {
        let text = self.input.trim().to_string();
        if text.is_empty() && self.attached.is_empty() {
            return;
        }
        let mut full = String::new();
        for p in self.attached.drain(..) {
            full.push_str(&format!("[Attached file: {p}]\n"));
        }
        full.push_str(&text);
        let busy = self.chat.lock().unwrap().busy;
        if busy {
            // Tobi is working: keep the message and send it when he is free (instead of "Tobi is busy")
            if self.queued.len() < 5 {
                self.queued.push_back(full);
            }
        } else {
            self.dispatch(full);
        }
        self.input.clear();
        self.scroll = 0;
        self.focused = true;
        ctx.want_keyboard(true);
    }
}

/// Greedy word wrap to `width` DIPs.
fn wrap(ui: &mut Ui, text: &str, width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if ui.measure(&cand, size, Weight::Regular) <= width {
                cur = cand;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            // a single word wider than the line: cut it by characters
            let mut piece = String::new();
            for ch in word.chars() {
                let mut t = piece.clone();
                t.push(ch);
                if ui.measure(&t, size, Weight::Regular) > width && !piece.is_empty() {
                    lines.push(std::mem::take(&mut piece));
                }
                piece.push(ch);
            }
            cur = piece;
        }
        lines.push(cur);
    }
    lines
}

impl Module for Agent {
    fn title(&self) -> &'static str {
        "Tobi"
    }
    fn icon(&self) -> &'static str {
        "\u{E8BD}"
    }
    fn size(&self) -> (f32, f32) {
        (500.0, 164.0)
    }
    fn accent(&self) -> Color {
        col::ACCENT
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        if ctx.now.unix - self.last_poll >= 2.0 {
            self.last_poll = ctx.now.unix;
            self.up = self.installed && running();
        }
        if !self.installed || crate::time::demo() {
            return;
        }
        // Reminders and messages from Tobi: one "title<TAB>message" per line.
        let file = store::dir().join("agent").join("notify.txt");
        if file.exists() {
            let taken = file.with_extension("reading");
            if std::fs::rename(&file, &taken).is_ok() {
                if let Ok(text) = std::fs::read_to_string(&taken) {
                    for line in text.lines().filter(|l| !l.trim().is_empty()).take(3) {
                        let (title, msg) = line.split_once('\t').unwrap_or(("Tobi", line));
                        ctx.toast(Toast::new("tobi", glyph::BELL, col::ACCENT, title, msg).secs(12.0).chime());
                    }
                }
                let _ = std::fs::remove_file(&taken);
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        self.active.store((crate::time::unix_now() * 1000.0) as u64, Ordering::Relaxed);
        if !self.installed {
            let l = r.cut_top(24.0, 4.0);
            ui.text("Tobi is not installed. Run the WindyLand setup.", l, 13.0, col::RED, Weight::Semi, Align::Left);
            return;
        }
        let picked: Vec<String> = std::mem::take(&mut self.chat.lock().unwrap().picked);
        for p in picked {
            if !self.attached.contains(&p) {
                self.attached.push(p);
            }
            self.focused = true;
        }
        {
            let c = self.chat.lock().unwrap();
            let free = !c.busy && c.pending.is_none();
            drop(c);
            if free {
                if let Some(next) = self.queued.pop_front() {
                    self.dispatch(next);
                }
            }
        }
        let chat = self.chat.lock().unwrap().clone();
        let starting = crate::time::unix_now() - self.opened_at < 6.0 && !self.up;
        let queued = if self.queued.is_empty() { String::new() } else { format!("  ({} waiting)", self.queued.len()) };
        let (dot, state) = if chat.picking {
            (col::YELLOW, "Choose the file in the window that opened...".to_string())
        } else if chat.busy {
            (col::YELLOW, format!("{}{queued}", if chat.status.is_empty() { "Working..." } else { chat.status.as_str() }))
        } else if !chat.error.is_empty() {
            (col::RED, chat.error.clone())
        } else if self.up {
            (col::GREEN, "Awake".to_string())
        } else if starting {
            (col::YELLOW, "Waking up...".to_string())
        } else {
            (col::DIM, "Asleep. Send a message to wake him.".to_string())
        };

        // ---- header: dot, name, what he is doing, small buttons
        let mut head = r.cut_top(22.0, 6.0);
        let mut btns = head;
        let sleep = if self.up { Some(btns.cut_right(48.0, 6.0)) } else { None };
        let gear = btns.cut_right(22.0, 6.0);
        let window = btns.cut_right(62.0, 6.0);
        let newc = btns.cut_right(40.0, 6.0);
        let d = head.cut_left(14.0, 4.0);
        ui.circle(d.cx(), d.cy(), 4.0, dot);
        let name = head.cut_left(36.0, 6.0);
        ui.text("Tobi", name, 15.0, col::TEXT, Weight::Semi, Align::Left);
        head.w = (btns.r() - head.x - 4.0).max(0.0);
        ui.text(&state, head, 11.5, col::DIM, Weight::Regular, Align::Left);
        ui.button(newc, "New", A_NEW);
        ui.button(window, "Window", A_OPEN);
        ui.icon_btn(gear, glyph::SETTINGS, A_SETTINGS, 12.0, col::DIM, Some(col::BTN), "Settings");
        if let Some(s) = sleep {
            ui.button(s, "Sleep", A_STOP);
        }

        // ---- input row
        let mut row = r.cut_bottom(28.0, 6.0);
        let send = row.cut_right(28.0, 6.0);
        let attach = row.cut_left(26.0, 4.0);
        let know = row.cut_left(26.0, 6.0);
        ui.icon_btn(attach, "\u{E723}", A_ATTACH, 12.0, col::DIM, Some(col::BTN), "Attach a file");
        ui.icon_btn(know, glyph::FOLDER, A_KNOW, 12.0, col::DIM, Some(col::BTN), "Tobi's knowledge folder");
        let field = row;
        let hot = self.focused || ui.hovered(field);
        ui.fill_rr(field, 14.0, if self.focused { col::TEXT.with_a(0.14) } else { col::BTN });
        if self.focused {
            ui.stroke_rr(field, 14.0, col::ACCENT.with_a(0.8), 1.2);
        }
        let inner = field.inset_xy(12.0, 0.0);
        if self.input.is_empty() {
            let hint = if self.attached.is_empty() { "Ask Tobi, or tell him what to do..." } else { "Say what to do with the file..." };
            ui.text(hint, inner, 12.5, if hot { col::DIM } else { col::FAINT }, Weight::Regular, Align::Left);
        } else {
            let one_line: String = self.input.replace('\n', " ");
            let overflow = ui.measure(&one_line, 12.5, Weight::Regular) > inner.w - 6.0;
            ui.text(&one_line, inner, 12.5, col::TEXT, Weight::Regular, if overflow { Align::Right } else { Align::Left });
        }
        if self.focused && ((ui.t * 2.0) as i64) % 2 == 0 {
            let tw = ui.measure(&self.input.replace('\n', " "), 12.5, Weight::Regular);
            let cx = if self.input.is_empty() { inner.x } else { (inner.x + tw.min(inner.w - 6.0) + 1.0).min(inner.r()) };
            ui.line(cx, field.y + 7.0, cx, field.b() - 7.0, col::ACCENT, 1.4);
        }
        ui.hit(field, A_FIELD);
        if chat.busy {
            let hv = ui.hovered(send);
            ui.fill_rr(send, 14.0, if hv { col::RED.with_a(0.9) } else { col::RED.with_a(0.75) });
            ui.fill_rr(send.centered(9.0, 9.0), 2.0, hex(0x000000));
            ui.hit_tip(send, A_HALT, "Stop");
        } else {
            let on = !self.input.trim().is_empty() || !self.attached.is_empty();
            let hv = ui.hovered(send);
            ui.fill_rr(send, 14.0, if on { if hv { col::ACCENT.with_a(0.85) } else { col::ACCENT } } else { col::BTN });
            ui.icon("\u{E724}", send, 12.0, if on { hex(0x000000) } else { col::FAINT });
            ui.hit_tip(send, A_SEND, "Send");
        }

        // ---- attached files
        if !self.attached.is_empty() {
            let mut chips = r.cut_bottom(20.0, 4.0);
            for (i, p) in self.attached.iter().enumerate().take(3) {
                let name = p.rsplit(['\\', '/']).next().unwrap_or(p);
                let short: String = if name.chars().count() > 26 { format!("{}...", name.chars().take(24).collect::<String>()) } else { name.to_string() };
                let chip = chips.cut_left(ui.measure(&short, 11.0, Weight::Semi) + 38.0, 6.0);
                ui.fill_rr(chip, 10.0, col::ACCENT.with_a(0.2));
                ui.text(&short, chip.inset_xy(10.0, 0.0), 11.0, col::ACCENT, Weight::Semi, Align::Left);
                ui.icon(glyph::CLOSE, Rect::new(chip.r() - 22.0, chip.y, 18.0, chip.h), 8.0, col::ACCENT);
                ui.hit_tip(chip, A_UNATTACH + i as u32, "Remove");
            }
        }

        // ---- permission card
        if let Some((id, title, detail)) = &chat.pending {
            if let Some((yes, no)) = &chat.pending_labels {
                // a question with named answers: the question on top, one button per answer below
                let card = r.cut_bottom(66.0, 6.0);
                ui.fill_rr(card, 10.0, col::YELLOW.with_a(0.14));
                let mut c = card.inset_xy(10.0, 5.0);
                let mut row = c.cut_bottom(24.0, 0.0);
                let half = (row.w - 8.0) / 2.0;
                let left = row.cut_left(half, 8.0);
                ui.button_accent(left, yes, A_ALLOW, col::GREEN);
                ui.button(row, no, A_DENY);
                let t = c.cut_top(16.0, 2.0);
                ui.text(title, t, 12.0, col::YELLOW, Weight::Semi, Align::Left);
                let d = detail.replace('\n', " ");
                ui.text(&d, c, 10.0, col::TEXT, Weight::Regular, Align::Left);
                let _ = id;
            } else {
                let card = r.cut_bottom(44.0, 6.0);
                ui.fill_rr(card, 10.0, col::YELLOW.with_a(0.14));
                let mut c = card.inset_xy(10.0, 4.0);
                let mut b = c.cut_right(100.0, 8.0);
                let allow = b.cut_left(48.0, 4.0);
                ui.button_accent(allow, "Allow", A_ALLOW, col::GREEN);
                ui.button(b, "No", A_DENY);
                let t = c.cut_top(16.0, 0.0);
                ui.text(&format!("Allow: {title}"), t, 12.0, col::YELLOW, Weight::Semi, Align::Left);
                let d = detail.replace('\n', " ");
                ui.text(&d, c, 11.0, col::TEXT, Weight::Regular, Align::Left);
                let _ = id;
            }
        }

        // ---- the conversation: the newest lines, scrollable with the wheel
        let area = r;
        let w = area.w - 4.0;
        let lh = 16.0;
        let mut lines: Vec<(String, Color, Weight)> = Vec::new();
        let mut shown = chat.msgs.clone();
        if let Some((t, _)) = &chat.sent {
            if !shown.iter().any(|(u, x)| *u && x.ends_with(t.as_str())) {
                shown.push((true, clean(t)));
            }
        }
        if shown.is_empty() {
            for l in wrap(ui, "Hi! I am Tobi. Ask me anything or tell me what to do: \"Humanize this file\" (drop a Word file here), \"Play Naruto\", \"Open Spotify\", \"Remind me in 20 minutes to ...\"", w, 12.5) {
                lines.push((l, col::DIM, Weight::Regular));
            }
        }
        for (user, text) in &shown {
            let (prefix, color, weight) = if *user { ("You: ", col::CYAN, Weight::Regular) } else { ("", col::TEXT, Weight::Regular) };
            let wrapped = wrap(ui, &format!("{prefix}{text}"), w, 12.5);
            for l in wrapped {
                lines.push((l, color, weight));
            }
            lines.push((String::new(), col::TEXT, Weight::Regular));
        }
        while lines.last().map_or(false, |l| l.0.is_empty()) {
            lines.pop();
        }
        let visible = ((area.h / lh).floor() as usize).max(1);
        self.max_scroll = lines.len().saturating_sub(visible);
        self.scroll = self.scroll.min(self.max_scroll);
        let end = lines.len() - self.scroll;
        let start = end.saturating_sub(visible);
        ui.push_clip(area);
        let mut y = area.y + (area.h - (end - start) as f32 * lh).max(0.0);
        for (text, color, weight) in &lines[start..end] {
            if !text.is_empty() {
                ui.text(text, Rect::new(area.x + 2.0, y, area.w - 4.0, lh), 12.5, *color, *weight, Align::Left);
            }
            y += lh;
        }
        ui.pop_clip();
        if self.scroll > 0 {
            ui.text("scrolled up - wheel down for newest", Rect::new(area.x, area.b() - 14.0, area.w, 14.0), 10.0, col::FAINT, Weight::Regular, Align::Right);
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        let res = match id {
            A_OPEN => {
                self.opened_at = crate::time::unix_now();
                open_agent(ctx.hwnd)
            }
            A_SETTINGS => launch(&with_rect(ctx.hwnd, vec!["--settings".to_string()])),
            A_STOP => {
                self.up = false;
                launch(&["--stop".to_string()])
            }
            A_FIELD => {
                self.focused = true;
                ctx.want_keyboard(true);
                return;
            }
            A_SEND => {
                self.send(ctx);
                return;
            }
            A_HALT => {
                self.queued.clear();
                let _ = self.tx.send(Cmd::Halt);
                return;
            }
            A_ATTACH => {
                let _ = self.tx.send(Cmd::Pick);
                return;
            }
            A_KNOW => launch(&["--knowledge".to_string()]),
            A_NEW => {
                let _ = self.tx.send(Cmd::New);
                self.attached.clear();
                self.queued.clear();
                self.scroll = 0;
                return;
            }
            A_ALLOW | A_DENY => {
                let pending = self.chat.lock().unwrap().pending.clone();
                if let Some((pid, _, _)) = pending {
                    let _ = self.tx.send(Cmd::Confirm(pid, id == A_ALLOW));
                    self.chat.lock().unwrap().pending = None;
                }
                return;
            }
            i if i >= A_UNATTACH && ((i - A_UNATTACH) as usize) < self.attached.len() => {
                self.attached.remove((i - A_UNATTACH) as usize);
                return;
            }
            _ => return,
        };
        if let Err(e) = res {
            ctx.toast(Toast::new("tobi", glyph::CLOSE, col::RED, "Tobi", e).secs(6.0));
        }
    }

    fn scroll(&mut self, delta: f32) -> bool {
        if self.max_scroll == 0 {
            return false;
        }
        if delta > 0.0 {
            self.scroll = (self.scroll + 2).min(self.max_scroll);
        } else {
            self.scroll = self.scroll.saturating_sub(2);
        }
        true
    }

    fn key(&mut self, vk: u16, ctrl: bool, ctx: &mut Ctx) -> bool {
        if !self.focused {
            return false;
        }
        match vk {
            0x0D => self.send(ctx),
            0x1B => {
                self.focused = false;
                ctx.want_keyboard(false);
            }
            0x08 => {
                if ctrl {
                    let t = self.input.trim_end().to_string();
                    let cut = t.rfind(' ').map(|i| i + 1).unwrap_or(0);
                    self.input.truncate(cut);
                } else {
                    self.input.pop();
                }
            }
            0x56 if ctrl => {
                if let Some(s) = crate::modules::todo::clipboard_text() {
                    self.input.push_str(&s.replace(['\r', '\n'], " "));
                    self.input.truncate(self.input.char_indices().nth(2000).map_or(self.input.len(), |(i, _)| i));
                }
            }
            _ => return false,
        }
        true
    }

    fn chr(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        if !self.focused {
            return false;
        }
        if !c.is_control() && self.input.chars().count() < 2000 {
            self.input.push(c);
        }
        true
    }

    fn blur(&mut self) {
        self.focused = false;
    }

    fn animating(&self) -> bool {
        self.focused
    }

    /// Files dropped on the notch while this tab is showing are given to Tobi with the next message.
    /// (`wants_drops` stays false so the File shelf keeps taking drops on other tabs.)
    fn drop_files(&mut self, paths: Vec<String>, ctx: &mut Ctx) -> bool {
        if !self.installed || paths.is_empty() {
            return false;
        }
        for p in paths.into_iter().take(5) {
            if !self.attached.contains(&p) {
                self.attached.push(p);
            }
        }
        self.focused = true;
        ctx.want_keyboard(true);
        true
    }

    fn debug_state(&self) -> String {
        format!("installed={} up={} port={}", self.installed, self.up, port())
    }
}
