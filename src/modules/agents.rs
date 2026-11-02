//! AI agents: shows in the notch when Hermes, Claude Code or Codex CLI is
//! working, needs your OK, or has finished.
//!
//! How it works: "Connect" adds a hook to the agent's own config
//! (Hermes shell hooks via `hermes config set`, `~/.claude/settings.json`
//! hooks, `~/.codex/config.toml` notify). The agent
//! then runs `windyland.exe --agent-event <agent>` at each step; that short
//! process keeps only the event name, session id, project folder, tool name
//! and notice text, and hands them to the running notch with WM_COPYDATA.
//! Prompts, code and tool output are never read or kept, nothing is written
//! to disk, and nothing leaves the PC. "Disconnect" removes exactly the lines
//! it added (and the uninstaller does too).

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::*;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::time::{fmt_dur, unix_now};
use crate::ui::Ui;

pub const ACCENT: Color = hex(0xD97757);
pub const ICON: &str = "\u{E99A}";
/// WM_COPYDATA tag for agent events ("WLAG").
pub const MAGIC: usize = 0x574C_4147;

const A_CLAUDE: u32 = 1;
const A_CODEX: u32 = 2;
const A_CLEAR: u32 = 3;
const A_HERMES: u32 = 4;
const K_ROW: u32 = 1 << 24;
/// Demo video / docs renders: a few made-up sessions.
pub const A_DEMO: u32 = 990;
/// Demo video: the "needs you" moment.
pub const A_DEMO_ASK: u32 = 991;
/// Demo video: it finishes.
pub const A_DEMO_DONE: u32 = 992;

/// Claude Code hook events WindyLand listens to.
pub const CLAUDE_EVENTS: [&str; 6] = ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop", "SessionEnd"];

// ------------------------------------------------------------ hook side

/// Events handed over by `windyland.exe --agent-event` (filled from
/// WM_COPYDATA on the UI thread, read in `tick`).
static INBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// WM_COPYDATA from a hook process. Handled before the app itself so it
/// also works while the app is busy (and in the self-test).
pub fn on_copydata(hwnd: HWND, lp: LPARAM) -> Option<LRESULT> {
    // Docs/demo renders never take real events (the self-test sends its
    // own on purpose, by window handle).
    if isolated() && std::env::var("WINDYLAND_SELFTEST_OUT").is_err() && !std::env::args().any(|a| a == "--selftest") {
        return None;
    }
    let cds = unsafe { (lp.0 as *const COPYDATASTRUCT).as_ref()? };
    if cds.dwData != MAGIC || cds.lpData.is_null() || cds.cbData == 0 || cds.cbData > 65536 {
        return None;
    }
    let b = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
    push_event(String::from_utf8_lossy(b).to_string());
    unsafe {
        let _ = PostMessageW(hwnd, crate::app::WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
    Some(LRESULT(1))
}

pub fn push_event(s: String) {
    let mut q = INBOX.lock().unwrap();
    if q.len() < 500 {
        q.push(s);
    }
}

/// Keep only what the notch shows. The hook input can hold the whole prompt
/// or a file's contents; none of that is passed on.
pub fn slim(agent: &str, v: &Value) -> Value {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let cut = |t: String, n: usize| t.chars().take(n).collect::<String>();
    if agent == "hermes" {
        // Hermes shell hooks: {hook_event_name, session_id, cwd, profile, extra{...}}.
        let ex = v.get("extra").cloned().unwrap_or(Value::Null);
        let es = |k: &str| ex.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let eb = |k: &str| ex.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let surface = [es("surface"), es("platform")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
        return json!({
            "agent": "hermes",
            "event": cut(s("hook_event_name"), 40),
            "session": cut(s("session_id"), 80),
            "cwd": cut(s("cwd"), 300),
            "profile": cut(s("profile"), 60),
            "surface": cut(surface, 30),
            "model": cut(es("model"), 60),
            "kind": cut(es("kind"), 20),
            "message": cut(es("prompt"), 160),
            "interrupted": eb("interrupted"),
            "failed": eb("failed"),
        });
    }
    if agent == "codex" {
        // Codex calls its notify program once per finished turn.
        let ty = s("type");
        let event = if ty == "agent-turn-complete" { "Stop".to_string() } else { ty };
        let session = [s("thread-id"), s("thread_id"), s("session-id")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "codex".into());
        return json!({"agent": "codex", "event": event, "session": cut(session, 80), "cwd": cut(s("cwd"), 300)});
    }
    json!({
        "agent": "claude",
        "event": cut(s("hook_event_name"), 40),
        "session": cut(s("session_id"), 80),
        "cwd": cut(s("cwd"), 300),
        "tool": cut(s("tool_name"), 60),
        "message": cut(s("message"), 160),
    })
}

/// `windyland.exe --agent-event <claude|codex> [json]`: run by the agent's
/// hook. Always exits quietly with 0 so it can never block the agent.
pub fn hook_main(args: &[String]) -> i32 {
    let i = args.iter().position(|a| a == "--agent-event").unwrap_or(0);
    let agent = args.get(i + 1).map(|s| s.to_lowercase()).unwrap_or_default();
    let agent = match agent.as_str() {
        "codex" => "codex",
        "hermes" => "hermes",
        _ => "claude",
    };
    let raw = if agent == "codex" {
        args.iter().rev().find(|a| a.trim_start().starts_with('{')).cloned().unwrap_or_default()
    } else {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = std::io::stdin().lock().take(8 << 20).read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).to_string()
    };
    let v: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::Null);
    if v.is_null() {
        return 0;
    }
    let msg = slim(agent, &v).to_string();
    unsafe {
        let target = match std::env::var("WINDYLAND_AGENT_HWND").ok().and_then(|h| h.parse::<isize>().ok()) {
            Some(h) => HWND(h as *mut _),
            None => FindWindowW(w!("WindyLandNotch"), None).unwrap_or_default(),
        };
        if target.0.is_null() {
            return 0;
        }
        let cds = COPYDATASTRUCT { dwData: MAGIC, cbData: msg.len() as u32, lpData: msg.as_ptr() as *mut _ };
        let mut res = 0usize;
        let _ = SendMessageTimeoutW(target, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG | SMTO_BLOCK, 1500, Some(&mut res));
    }
    0
}

// ------------------------------------------------------- connect / disconnect

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Link {
    /// The agent isn't installed (no ~/.claude or ~/.codex folder).
    Missing,
    Off,
    On,
}

/// Set for --selftest and the docs/demo renders: never touch the real
/// agents' settings, only WINDYLAND_AGENT_HOME / WINDYLAND_HERMES_HOME.
pub static ISOLATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn isolated() -> bool {
    ISOLATED.load(std::sync::atomic::Ordering::Relaxed)
}

fn home() -> PathBuf {
    if let Ok(h) = std::env::var("WINDYLAND_AGENT_HOME") {
        return PathBuf::from(h);
    }
    if isolated() {
        return PathBuf::from(r"Z:\windyland-test-no-home");
    }
    std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// The command the hooks run (forward slashes work in bash and cmd alike).
pub fn hook_command(agent: &str) -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "windyland.exe".into());
    format!("\"{}\" --agent-event {agent}", exe.replace('\\', "/"))
}

fn is_ours(entry: &Value) -> bool {
    entry.get("hooks").and_then(|h| h.as_array()).map_or(false, |hs| {
        hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains("--agent-event")))
    })
}

fn claude_path() -> PathBuf {
    home().join(".claude").join("settings.json")
}

pub fn claude_link() -> Link {
    if !home().join(".claude").is_dir() {
        return Link::Missing;
    }
    let v: Value = std::fs::read_to_string(claude_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let on = v.get("hooks").and_then(|h| h.as_object()).map_or(false, |h| h.values().any(|a| a.as_array().map_or(false, |a| a.iter().any(is_ours))));
    if on {
        Link::On
    } else {
        Link::Off
    }
}

/// Add (on) or remove (off) WindyLand's hooks in ~/.claude/settings.json,
/// leaving everything else in the file as it was.
pub fn claude_connect(on: bool) -> std::result::Result<(), String> {
    if !home().join(".claude").is_dir() {
        return Err("Claude Code isn't installed for this user".into());
    }
    let path = claude_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut v: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).map_err(|_| "Couldn't read ~/.claude/settings.json, so it was left alone".to_string())?
    };
    let Some(root) = v.as_object_mut() else { return Err("~/.claude/settings.json isn't a JSON object; left alone".into()) };
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else { return Err("\"hooks\" in ~/.claude/settings.json isn't an object; left alone".into()) };
    // Remove ours first (also updates an old exe path).
    for list in hooks.values_mut() {
        if let Some(a) = list.as_array_mut() {
            a.retain(|e| !is_ours(e));
        }
    }
    hooks.retain(|_, list| list.as_array().map_or(true, |a| !a.is_empty()));
    if on {
        let cmd = hook_command("claude");
        for ev in CLAUDE_EVENTS {
            let mut entry = json!({"hooks": [{"type": "command", "command": cmd, "timeout": 10}]});
            if ev == "PostToolUse" {
                entry["matcher"] = json!("*");
            }
            let list = hooks.entry(ev).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.push(entry);
            }
        }
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    let backup = path.with_extension("json.windyland-backup");
    if !text.is_empty() && !backup.exists() {
        let _ = std::fs::write(&backup, &text);
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&path, out).map_err(|e| format!("Couldn't save ~/.claude/settings.json: {e}"))
}

fn codex_path() -> PathBuf {
    home().join(".codex").join("config.toml")
}

const CODEX_NOTE: &str = "# WindyLand: shows in the notch when Codex finishes (remove both lines to stop)";

/// Top-level `notify = ...` lines (before the first [table]).
fn codex_notify_lines(text: &str) -> Vec<(usize, bool)> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let t = l.trim_start();
        if t.starts_with('[') {
            break;
        }
        if t.starts_with("notify") && t["notify".len()..].trim_start().starts_with('=') {
            out.push((i, l.contains("--agent-event")));
        }
    }
    out
}

pub fn codex_link() -> Link {
    if !home().join(".codex").is_dir() {
        return Link::Missing;
    }
    let text = std::fs::read_to_string(codex_path()).unwrap_or_default();
    if codex_notify_lines(&text).iter().any(|(_, ours)| *ours) {
        Link::On
    } else {
        Link::Off
    }
}

pub fn codex_connect(on: bool) -> std::result::Result<(), String> {
    if !home().join(".codex").is_dir() {
        return Err("Codex isn't installed for this user".into());
    }
    let path = codex_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let found = codex_notify_lines(&text);
    if on && found.iter().any(|(_, ours)| !*ours) {
        return Err("Codex already runs its own notify program, so WindyLand left it alone".into());
    }
    let drop: Vec<usize> = found.iter().filter(|(_, o)| *o).map(|(i, _)| *i).collect();
    let mut lines: Vec<&str> = text.lines().enumerate().filter(|(i, l)| !drop.contains(i) && l.trim() != CODEX_NOTE).map(|(_, l)| l).collect();
    let line;
    if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string().replace('\\', "/")).unwrap_or_default();
        line = format!("notify = [\"{exe}\", \"--agent-event\", \"codex\"]");
        lines.insert(0, &line);
        lines.insert(0, CODEX_NOTE);
    }
    let mut out = lines.join(nl);
    if !out.is_empty() {
        out += nl;
    }
    std::fs::write(&path, out).map_err(|e| format!("Couldn't save ~/.codex/config.toml: {e}"))
}

// ------------------------------------------------------------------ Hermes

/// Hermes shell hooks WindyLand registers (all observers: they can't block
/// or change anything in Hermes).
pub const HERMES_EVENTS: [&str; 6] = ["on_session_start", "on_stream_start", "on_human_input_request", "on_human_input_resolved", "on_session_end", "on_session_finalize"];

/// Hermes' home: HERMES_HOME, else %LOCALAPPDATA%\hermes, else ~/.hermes
/// (whichever has a config.yaml).
pub fn hermes_home() -> Option<PathBuf> {
    if isolated() {
        return std::env::var("WINDYLAND_HERMES_HOME").ok().map(PathBuf::from).filter(|p| p.join("config.yaml").is_file());
    }
    let mut c: Vec<PathBuf> = Vec::new();
    for k in ["WINDYLAND_HERMES_HOME", "HERMES_HOME"] {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                c.push(PathBuf::from(v));
                break;
            }
        }
    }
    if std::env::var("WINDYLAND_AGENT_HOME").is_err() {
        if let Ok(l) = std::env::var("LOCALAPPDATA") {
            c.push(PathBuf::from(l).join("hermes"));
        }
    }
    c.push(home().join(".hermes"));
    c.into_iter().find(|p| p.join("config.yaml").is_file())
}

/// The `hermes` command: the one installed with Hermes, else on PATH.
pub fn hermes_cli() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("WINDYLAND_HERMES_CLI") {
        return Some(PathBuf::from(v));
    }
    if isolated() {
        // The real hermes command, but pointed at the scratch home by run_hermes.
        let real = std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join("hermes").join("hermes-agent").join(".hermes").join("bin").join("hermes.exe")).ok();
        return real.filter(|p| p.is_file());
    }
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(h) = hermes_home() {
        c.push(h.join("hermes-agent").join(".hermes").join("bin").join("hermes.exe"));
    }
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            c.push(d.join("hermes.exe"));
        }
    }
    c.into_iter().find(|p| p.is_file())
}

pub fn hermes_link() -> Link {
    match hermes_home() {
        None => Link::Missing,
        Some(h) => {
            let cfg = std::fs::read_to_string(h.join("config.yaml")).unwrap_or_default();
            if cfg.contains("--agent-event hermes") {
                Link::On
            } else {
                Link::Off
            }
        }
    }
}

fn entry_is_ours(e: &Value) -> bool {
    e.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains("--agent-event"))
}

/// Hermes' `hooks:` section with WindyLand's entries removed, then added
/// back (on) for each event. Everything else is kept as it was.
pub fn merge_hermes_hooks(cur: &Value, cmd: &str, on: bool) -> Value {
    let mut m = cur.as_object().cloned().unwrap_or_default();
    for (_, v) in m.iter_mut() {
        if let Some(a) = v.as_array_mut() {
            a.retain(|e| !entry_is_ours(e));
        }
    }
    m.retain(|_, v| v.as_array().map_or(true, |a| !a.is_empty()));
    if on {
        for ev in HERMES_EVENTS {
            let list = m.entry(ev).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.push(json!({"command": cmd, "timeout": 10}));
            }
        }
    }
    Value::Object(m)
}

fn iso_utc(t: std::time::SystemTime) -> String {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    let micros = d.subsec_micros();
    let frac = if micros == 0 { String::new() } else { format!(".{micros:06}") };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{frac}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Hermes asks before running a new shell hook; switching Hermes on in
/// WindyLand is that yes, so record it in Hermes' own allowlist (the same
/// entry `hermes` writes after a "yes" at its prompt).
fn hermes_allowlist(home: &std::path::Path, cmd: &str, on: bool) -> std::result::Result<(), String> {
    let path = home.join("shell-hooks-allowlist.json");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut v: Value = if text.trim().is_empty() { json!({"approvals": []}) } else { serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| "Hermes' hook allowlist is unreadable; left alone".to_string())? };
    let list = v.get_mut("approvals").and_then(|a| a.as_array_mut()).ok_or("Hermes' hook allowlist has an unexpected shape; left alone")?;
    list.retain(|e| !entry_is_ours(e));
    if on {
        let exe = std::env::current_exe().ok();
        let mtime = exe.and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()).map(iso_utc);
        let now = iso_utc(std::time::SystemTime::now());
        for ev in HERMES_EVENTS {
            list.push(json!({"approved_at": now, "command": cmd, "event": ev, "script_mtime_at_approval": mtime}));
        }
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| format!("Couldn't save Hermes' hook allowlist: {e}"))
}

fn run_hermes(cli: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::result::Result<String, String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(cli)
        .args(args)
        .env("HERMES_HOME", home)
        .env("PYTHONIOENCODING", "utf-8")
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("Couldn't run hermes: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
        let line = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("hermes reported an error").to_string();
        return Err(format!("hermes config: {}", line.chars().take(120).collect::<String>()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Add or remove WindyLand's Hermes hooks, through Hermes' own
/// `hermes config get/set` (never by editing config.yaml directly). Takes
/// a few seconds: run it off the UI thread.
pub fn hermes_connect(on: bool) -> std::result::Result<(), String> {
    let home = hermes_home().ok_or("Hermes isn't installed for this user")?;
    let cli = hermes_cli().ok_or("Couldn't find the hermes command")?;
    let raw = run_hermes(&cli, &home, &["config", "get", "hooks", "--json", "--raw"])?;
    let raw = raw.trim();
    let cur: Value = if raw.is_empty() || raw == "null" { json!({}) } else { serde_json::from_str(raw).map_err(|_| "Couldn't read Hermes' hooks, so nothing was changed".to_string())? };
    if !cur.is_object() {
        return Err("Hermes' hooks setting has an unexpected shape; nothing was changed".into());
    }
    let cmd = hook_command("hermes");
    let new = merge_hermes_hooks(&cur, &cmd, on);
    if new != cur {
        if new.as_object().map_or(false, |m| m.is_empty()) {
            run_hermes(&cli, &home, &["config", "unset", "hooks"])?;
        } else {
            // --force: replace the whole section with the merged one (Hermes refuses
            // to overwrite a section otherwise).
            run_hermes(&cli, &home, &["config", "set", "--force", "hooks", &new.to_string()])?;
        }
    }
    hermes_allowlist(&home, &cmd, on)
}

/// Result of the background connect: (switched on?, outcome).
static HERMES_DONE: Mutex<Option<(bool, std::result::Result<(), String>)>> = Mutex::new(None);

/// Uninstall: remove WindyLand's hooks from every agent.
pub fn disconnect_all() {
    if hermes_link() == Link::On {
        let _ = hermes_connect(false);
    }
    if claude_link() == Link::On {
        let _ = claude_connect(false);
    }
    if codex_link() == Link::On {
        let _ = codex_connect(false);
    }
}

// --------------------------------------------------------------- module

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Ready,
    Working,
    NeedsYou,
    Done,
}

#[derive(Clone, Debug)]
struct Session {
    agent: &'static str,
    id: String,
    project: String,
    state: State,
    /// When the current state began / the current task started.
    since: f64,
    started: f64,
    last: f64,
    detail: String,
}

impl Session {
    fn name(&self) -> &'static str {
        match self.agent {
            "codex" => "Codex",
            "hermes" => "Hermes",
            _ => "Claude Code",
        }
    }
}

pub struct Agents {
    sessions: Vec<Session>,
    hermes: Link,
    /// Hermes connect/disconnect running in the background.
    hermes_busy: bool,
    claude: Link,
    codex: Link,
    checked: f64,
    status: Option<(String, f64)>,
}

fn project_of(cwd: &str) -> String {
    let p = cwd.trim_end_matches(['\\', '/']);
    let name = p.rsplit(['\\', '/']).next().unwrap_or(p);
    if name.is_empty() {
        "terminal".into()
    } else {
        name.to_string()
    }
}

fn state_color(s: State) -> Color {
    match s {
        State::Ready => col::DIM,
        State::Working => col::BLUE,
        State::NeedsYou => col::ACCENT,
        State::Done => col::GREEN,
    }
}

fn state_label(s: State) -> &'static str {
    match s {
        State::Ready => "Ready",
        State::Working => "Working",
        State::NeedsYou => "Needs you",
        State::Done => "Done",
    }
}

/// How long ago a "working" session last reported before we stop trusting
/// it (an interrupted turn sends no Stop).
const STALE: f64 = 20.0 * 60.0;

impl Agents {
    pub fn new() -> Agents {
        // After an update the exe is newer than Hermes' approval: refresh it
        // so `hermes hooks list` doesn't warn about a changed script.
        if hermes_link() == Link::On && std::env::var("WINDYLAND_SELFTEST_OUT").is_err() {
            if let Some(h) = hermes_home() {
                let _ = hermes_allowlist(&h, &hook_command("hermes"), true);
            }
        }
        Agents { sessions: Vec::new(), hermes: hermes_link(), hermes_busy: false, claude: claude_link(), codex: codex_link(), checked: unix_now(), status: None }
    }

    /// Apply one event (the slimmed JSON from the hook).
    pub fn apply(&mut self, v: &Value, now: f64, ctx: &mut Ctx) {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let agent: &'static str = match s("agent").as_str() {
            "codex" => "codex",
            "hermes" => "hermes",
            _ => "claude",
        };
        let mut event = s("event");
        let mut hermes_msg = String::new();
        if agent == "hermes" {
            event = match event.as_str() {
                "on_session_start" => "SessionStart",
                "on_stream_start" | "on_human_input_resolved" => "PostToolUse",
                "on_human_input_request" => {
                    hermes_msg = match s("kind").as_str() {
                        "approval" => "Wants your OK to run a command",
                        "clarify" => "Has a question for you",
                        "sudo" => "Needs your password",
                        _ => "Waiting for you",
                    }
                    .to_string();
                    "Notification"
                }
                "on_session_end" if v.get("interrupted").and_then(|x| x.as_bool()).unwrap_or(false) => "Interrupted",
                "on_session_end" => "Stop",
                "on_session_finalize" => "SessionEnd",
                _ => "",
            }
            .to_string();
        }
        // Hermes runs from your home folder: name it by where it runs instead.
        let cwd_project = |s: &dyn Fn(&str) -> String| {
            let cwd = s("cwd");
            let p = project_of(&cwd);
            // A home folder (C:\Users\<anyone>, whatever USERNAME says) or
            // Hermes' own folder isn't a project.
            let parent = std::path::Path::new(cwd.trim_end_matches(['\\', '/'])).parent().and_then(|x| x.file_name()).map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            let user = std::env::var("USERNAME").unwrap_or_default();
            let is_home = parent.eq_ignore_ascii_case("Users") || p.eq_ignore_ascii_case(&user);
            if agent == "hermes" && (cwd.is_empty() || is_home || p.eq_ignore_ascii_case("hermes-agent")) {
                let surface = s("surface");
                if surface.is_empty() { "chat".to_string() } else { surface }
            } else {
                p
            }
        };
        let id = s("session");
        let id = if id.is_empty() { agent.to_string() } else { id };
        if event == "SessionEnd" {
            self.sessions.retain(|x| x.id != id);
            return;
        }
        let i = match self.sessions.iter().position(|x| x.id == id) {
            Some(i) => i,
            None => {
                self.sessions.insert(0, Session { agent, id: id.clone(), project: cwd_project(&s), state: State::Ready, since: now, started: now, last: now, detail: String::new() });
                0
            }
        };
        let sess = &mut self.sessions[i];
        sess.last = now;
        if !s("cwd").is_empty() {
            sess.project = cwd_project(&s);
        }
        let set = |sess: &mut Session, st: State| {
            if sess.state != st {
                sess.state = st;
                sess.since = now;
            }
        };
        match event.as_str() {
            "SessionStart" => set(sess, State::Ready),
            "UserPromptSubmit" => {
                set(sess, State::Working);
                sess.started = now;
                sess.detail = "Thinking".into();
            }
            "PreToolUse" | "PostToolUse" => {
                if sess.state != State::Working {
                    if sess.state != State::NeedsYou {
                        sess.started = now;
                    }
                    set(sess, State::Working);
                }
                let tool = s("tool");
                if !tool.is_empty() {
                    sess.detail = format!("Used {tool}");
                } else if agent == "hermes" {
                    let model = s("model");
                    sess.detail = if model.is_empty() { "Working".into() } else { model.rsplit('/').next().unwrap_or(&model).to_string() };
                }
            }
            "Notification" => {
                let msg = if agent == "hermes" { hermes_msg.clone() } else { s("message") };
                let asks = agent == "hermes" || msg.to_lowercase().contains("permission") || msg.to_lowercase().contains("approv");
                // The "waiting for your input" nudge after a finished turn
                // is not news.
                if asks || sess.state == State::Working {
                    set(sess, State::NeedsYou);
                    sess.detail = if msg.is_empty() { "Waiting for you".into() } else { msg.clone() };
                    let title = format!("{} needs you", sess.name());
                    let sub = format!("{} · {}", sess.project, if msg.is_empty() { "waiting for your answer" } else { msg.as_str() });
                    ctx.toast(Toast::new("agent", ICON, col::ACCENT, title, sub).secs(6.0).chime().click(K_ROW));
                }
            }
            "Interrupted" => {
                set(sess, State::Ready);
                sess.detail = "Stopped".into();
            }
            "Stop" => {
                let took = now - sess.started;
                let was_busy = matches!(sess.state, State::Working | State::NeedsYou) || agent == "codex";
                set(sess, State::Done);
                sess.detail = if was_busy && took >= 1.0 && agent != "codex" { format!("Finished in {}", fmt_dur(took)) } else { "Finished".into() };
                if was_busy {
                    let title = format!("{} finished", sess.name());
                    let sub = if agent == "codex" || took < 1.0 { sess.project.clone() } else { format!("{} · took {}", sess.project, fmt_dur(took)) };
                    ctx.toast(Toast::new("agent", ICON, col::GREEN, title, sub).secs(5.0).chime().click(K_ROW));
                }
            }
            _ => {}
        }
        // Newest activity first.
        let s = self.sessions.remove(i);
        self.sessions.insert(0, s);
        self.sessions.truncate(8);
    }

    fn top(&self, now: f64) -> Option<&Session> {
        let pick = |st: State| self.sessions.iter().find(|s| s.state == st);
        pick(State::NeedsYou).or_else(|| pick(State::Working)).or_else(|| self.sessions.iter().find(|s| s.state == State::Done && now - s.since < 12.0))
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), unix_now()));
    }

    fn draw_link(&self, ui: &mut Ui, r: Rect, name: &str, link: Link, id: u32) {
        match link {
            Link::Missing => {
                ui.fill_rr(r, 13.0, col::BTN);
                ui.text(&format!("{name}: not installed"), r.inset_xy(10.0, 0.0), 11.0, col::FAINT, Weight::Regular, Align::Center);
                ui.hit_tip(r, id, "Install it, then connect it here");
            }
            _ => ui.toggle(r, name, link == Link::On, id, ACCENT),
        }
    }
}

impl Module for Agents {
    fn tip(&self, id: u32) -> &'static str {
        match id {
            A_HERMES => "Show Hermes' status in the notch (adds a hook to Hermes)",
            A_CLAUDE => "Show Claude Code's status in the notch (adds a hook)",
            A_CODEX => "Show when Codex finishes (adds a notify line)",
            _ => "",
        }
    }
    fn title(&self) -> &'static str {
        "AI Agents"
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

    fn tick(&mut self, ctx: &mut Ctx) {
        let now = unix_now();
        let events: Vec<String> = std::mem::take(&mut *INBOX.lock().unwrap());
        for e in events {
            if let Ok(v) = serde_json::from_str::<Value>(&e) {
                self.apply(&v, now, ctx);
            }
        }
        for s in &mut self.sessions {
            if matches!(s.state, State::Working | State::NeedsYou) && now - s.last > STALE {
                s.state = State::Ready;
                s.since = now;
                s.detail = "No news for a while".into();
            }
        }
        self.sessions.retain(|s| now - s.last < 6.0 * 3600.0);
        if let Some((on, res)) = HERMES_DONE.lock().unwrap().take() {
            self.hermes_busy = false;
            self.hermes = hermes_link();
            match res {
                Ok(()) if on => self.set_status("Hermes connected. Its next reply shows up here (within a minute)"),
                Ok(()) => self.set_status("Hermes disconnected"),
                Err(e) => self.set_status(e),
            }
        }
        // Someone may connect from a terminal or edit the files: re-read now and then.
        if now - self.checked > 30.0 && !self.hermes_busy {
            self.checked = now;
            self.hermes = hermes_link();
            self.claude = claude_link();
            self.codex = codex_link();
        }
    }

    fn draw(&mut self, ui: &mut Ui, mut r: Rect) {
        let now = unix_now();
        let mut head = r.cut_top(26.0, 10.0);
        let codex = head.cut_right(TOGGLE_W[2], 6.0);
        let claude = head.cut_right(TOGGLE_W[1], 6.0);
        let hermes = head.cut_right(TOGGLE_W[0], 10.0);
        if self.hermes_busy {
            ui.fill_rr(hermes, 13.0, col::BTN);
            ui.text("Hermes...", hermes.inset_xy(10.0, 0.0), 11.0, col::DIM, Weight::Regular, Align::Center);
        } else {
            self.draw_link(ui, hermes, "Hermes", self.hermes, A_HERMES);
        }
        self.draw_link(ui, claude, "Claude Code", self.claude, A_CLAUDE);
        self.draw_link(ui, codex, "Codex", self.codex, A_CODEX);
        let msg = match &self.status {
            Some((m, at)) if now - at < 6.0 => Some(m.clone()),
            _ => None,
        };
        let working = self.sessions.iter().filter(|s| s.state == State::Working).count();
        let line = match (self.sessions.len(), working) {
            (0, _) => "No agents running".to_string(),
            (n, 0) => format!("{n} session{}", if n == 1 { "" } else { "s" }),
            (_, w) => format!("{w} working"),
        };
        ui.text(&line, head, 12.0, col::DIM, Weight::Regular, Align::Left);
        // Connect results get their own full-width line.
        if let Some(m) = &msg {
            let row = r.cut_top(18.0, 6.0);
            ui.fill_rr(row, 6.0, col::ACCENT.with_a(0.12));
            ui.text(m, row.inset_xy(8.0, 0.0), 11.5, col::ACCENT, Weight::Regular, Align::Left);
        }

        if self.sessions.is_empty() {
            let connected = self.hermes == Link::On || self.claude == Link::On || self.codex == Link::On;
            let text = if connected {
                "Give your agent a task. The notch shows when it's working, when it needs your OK, and when it's done."
            } else {
                "Switch on Hermes, Claude Code or Codex above, and the notch shows when they're working, need your OK, or finish. Only the status passes through; nothing leaves your PC."
            };
            ui.icon(ICON, r.cut_top(30.0, 6.0), 22.0, ACCENT.with_a(0.8));
            ui.text_wrap(text, r.inset_xy(30.0, 0.0), 12.0, col::DIM, Weight::Regular, Align::Center);
            return;
        }
        let foot = r.cut_bottom(16.0, 4.0);
        ui.text("Click a session to clear it · times come from the agent's own hooks", foot, 10.5, col::FAINT, Weight::Regular, Align::Left);
        let rows = 3;
        let row_h = ((r.h - 6.0 * (rows as f32 - 1.0)) / rows as f32).min(32.0);
        for (k, s) in self.sessions.clone().iter().enumerate().take(rows) {
            let row = r.cut_top(row_h, 6.0);
            let c = state_color(s.state);
            let hov = ui.hovered(row);
            ui.fill_rr(row, 9.0, if hov { col::BTN_HOVER } else { col::BTN });
            let mut rr = row.inset_xy(10.0, 0.0);
            let dot = rr.cut_left(10.0, 8.0);
            ui.circle(dot.cx(), dot.cy(), 4.5, c);
            if s.state == State::Working {
                ui.stroke_circle(dot.cx(), dot.cy(), 7.5, c.with_a(0.35), 1.2);
            }
            let chip_w = ui.measure(state_label(s.state), 10.5, Weight::Semi) + 16.0;
            let time = rr.cut_right(54.0, 6.0);
            let chip = rr.cut_right(chip_w, 8.0).centered(chip_w, 18.0);
            ui.fill_rr(chip, 9.0, c.with_a(0.2));
            ui.text(state_label(s.state), chip, 10.5, c, Weight::Semi, Align::Center);
            let el = match s.state {
                State::Working | State::NeedsYou => fmt_dur(now - s.started),
                _ => ago(now - s.since),
            };
            ui.text(&el, time, 11.0, col::DIM, Weight::Regular, Align::Right);
            let name = format!("{} · {}", s.name(), s.project);
            let nw = ui.measure(&name, 12.0, Weight::Semi).min(rr.w * 0.6);
            ui.text(&name, rr.cut_left(nw + 2.0, 10.0), 12.0, col::TEXT, Weight::Semi, Align::Left);
            ui.text(&s.detail, rr, 11.0, col::DIM, Weight::Regular, Align::Left);
            ui.hit_tip(row, K_ROW | (k as u32 + 1), "Clear this session");
        }
        let more = self.sessions.len().saturating_sub(rows);
        if more > 0 {
            ui.text(&format!("+{more} more"), r, 10.5, col::FAINT, Weight::Regular, Align::Right);
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        match id {
            A_CLAUDE | A_CODEX => {
                let (link, name) = if id == A_CLAUDE { (self.claude, "Claude Code") } else { (self.codex, "Codex") };
                if link == Link::Missing {
                    self.set_status(format!("{name} isn't installed for this user"));
                    return;
                }
                let on = link != Link::On;
                let res = if id == A_CLAUDE { claude_connect(on) } else { codex_connect(on) };
                self.claude = claude_link();
                self.codex = codex_link();
                match res {
                    Ok(()) if on => self.set_status(format!("{name} connected. New sessions report here")),
                    Ok(()) => self.set_status(format!("{name} disconnected")),
                    Err(e) => self.set_status(e),
                }
            }
            A_HERMES => {
                if self.hermes_busy {
                    return;
                }
                if self.hermes == Link::Missing {
                    self.set_status("Hermes isn't installed for this user");
                    return;
                }
                let on = self.hermes != Link::On;
                self.hermes_busy = true;
                self.set_status(if on { "Connecting Hermes..." } else { "Disconnecting Hermes..." });
                let hwnd = ctx.hwnd.0 as isize;
                std::thread::spawn(move || {
                    let res = hermes_connect(on);
                    *HERMES_DONE.lock().unwrap() = Some((on, res));
                    unsafe {
                        let _ = PostMessageW(HWND(hwnd as *mut _), crate::app::WM_APP_WAKE, WPARAM(0), LPARAM(0));
                    }
                });
            }
            A_CLEAR => self.sessions.clear(),
            K_ROW => ctx.open_my_tab(),
            i if i > K_ROW && i <= K_ROW + 8 => {
                let k = (i - K_ROW - 1) as usize;
                if k < self.sessions.len() {
                    self.sessions.remove(k);
                }
            }
            A_DEMO => {
                let now = unix_now();
                self.hermes = Link::On;
                self.claude = Link::On;
                self.codex = Link::Off;
                self.sessions = vec![
                    Session { agent: "hermes", id: "d0".into(), project: "desktop".into(), state: State::Working, since: now - 212.0, started: now - 212.0, last: now, detail: "claude-opus".into() },
                    Session { agent: "claude", id: "d1".into(), project: "WindyLand".into(), state: State::Working, since: now - 95.0, started: now - 95.0, last: now, detail: "Used Edit".into() },
                    Session { agent: "codex", id: "d2".into(), project: "shop-api".into(), state: State::Done, since: now - 240.0, started: now - 600.0, last: now - 240.0, detail: "Finished".into() },
                ];
            }
            A_DEMO_ASK => {
                let v = json!({"agent": "hermes", "event": "on_human_input_request", "session": "d0", "kind": "approval", "surface": "desktop"});
                self.apply(&v, unix_now(), ctx);
            }
            A_DEMO_DONE => {
                let v = json!({"agent": "hermes", "event": "on_session_end", "session": "d0", "surface": "desktop"});
                self.apply(&v, unix_now(), ctx);
            }
            _ => {}
        }
    }

    fn live(&self) -> Option<u8> {
        let s = self.top(unix_now())?;
        Some(match s.state {
            State::NeedsYou => 75,
            State::Done => 50,
            _ => 45,
        })
    }

    fn live_color(&self) -> Color {
        self.top(unix_now()).map_or(ACCENT, |s| if s.state == State::Working { ACCENT } else { state_color(s.state) })
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let now = unix_now();
        let Some(s) = self.top(now).cloned() else { return };
        let c = if s.state == State::Working { ACCENT } else { state_color(s.state) };
        let mut l = left;
        let ic = l.cut_left(left.h, 4.0);
        ui.icon(ICON, ic, (left.h * 0.5).min(13.0), c);
        let size = (left.h * 0.46).clamp(9.0, 12.0);
        let text = match s.state {
            State::Working => fmt_dur(now - s.started),
            other => state_label(other).to_string(),
        };
        ui.text(&text, l, size, c, Weight::Semi, Align::Left);
        let mut rr = right;
        if s.state == State::Working {
            let d = rr.cut_right(rr.h * 0.5, 2.0);
            let pulse = 0.45 + 0.4 * ((now * 1.0).fract() < 0.5) as i32 as f32;
            ui.circle(d.cx(), d.cy(), 3.0, c.with_a(pulse));
        }
        ui.text(&s.project, rr, size, col::DIM, Weight::Regular, Align::Right);
    }

    fn debug_state(&self) -> String {
        let s: Vec<String> = self.sessions.iter().map(|s| format!("{}:{}:{:?}", s.agent, s.project, s.state)).collect();
        format!("hermes {:?}, claude {:?}, codex {:?}, sessions [{}]", self.hermes, self.claude, self.codex, s.join(", "))
    }
}

/// Header switch widths: Hermes, Claude Code, Codex.
pub const TOGGLE_W: [f32; 3] = [104.0, 132.0, 98.0];

fn ago(secs: f64) -> String {
    if secs < 60.0 {
        "now".into()
    } else if secs < 3600.0 {
        format!("{}m ago", (secs / 60.0) as u64)
    } else {
        format!("{}h ago", (secs / 3600.0) as u64)
    }
}
