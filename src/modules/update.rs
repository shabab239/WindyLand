//! Update checker (no tab). Shortly after start and then twice a day it asks
//! GitHub for the latest release: first the API, and if that refuses (it
//! allows 60 requests an hour per internet address, and many ISPs share one
//! address between lots of customers) the normal release pages. If it's newer, it shows a toast with
//! Update / Later / Skip. "Update" downloads the installer, runs it silently
//! and quits; the installer upgrades in place and starts WindyLand again.

use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::WM_APP_WAKE;
use crate::gfx::*;
use crate::http;
use crate::modules::{Ctx, Module, Rel, Toast};
use crate::time::unix_now;

pub const REPO: &str = "shabab239/WindyLand";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Action ids. `A_CHECK_NOW` is also sent by the tray menu.
pub const A_CHECK_NOW: u32 = 1;
const A_UPDATE: u32 = 2;
const A_LATER: u32 = 3;
const A_SKIP: u32 = 4;
const A_OPEN_PAGE: u32 = 5;

const FIRST_CHECK_AFTER: f64 = 20.0;
const CHECK_EVERY: f64 = 12.0 * 3600.0;
/// After a failed automatic check, try again this soon.
const RETRY_AFTER: f64 = 3600.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub installer: Option<Installer>,
}

/// The release's installer and what to check the download against.
#[derive(Clone, Debug, PartialEq)]
pub struct Installer {
    pub url: String,
    pub size: Option<u64>,
    /// Lowercase hex SHA-256, as GitHub publishes for every release file.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug)]
enum State {
    Idle,
    Checking { manual: bool },
    Found { rel: Release, manual: bool },
    UpToDate { manual: bool },
    Failed { manual: bool, why: String },
    Downloading(Release),
    Ready(String),
    DownloadFailed(Release, String),
}

/// "v0.1.10" -> (0, 1, 10)
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Parse the GitHub "latest release" JSON.
pub fn parse_release(body: &[u8]) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) || v.get("prerelease").and_then(|d| d.as_bool()).unwrap_or(false) {
        return None;
    }
    let version = v.get("tag_name")?.as_str()?.trim_start_matches(['v', 'V']).to_string();
    let page = v.get("html_url").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let installer = v.get("assets").and_then(|a| a.as_array()).and_then(|assets| {
        assets.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?.to_lowercase();
            if name.starts_with("windylandsetup") && name.ends_with(".exe") {
                Some(Installer {
                    url: a.get("browser_download_url")?.as_str()?.to_string(),
                    size: a.get("size").and_then(|s| s.as_u64()),
                    sha256: a.get("digest").and_then(|d| d.as_str()).and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase()),
                })
            } else {
                None
            }
        })
    });
    Some(Release { version, page, installer })
}

/// Blocking: ask GitHub for the latest release (API first, then the pages).
pub fn fetch_latest() -> std::result::Result<Release, String> {
    fetch_api().or_else(|e| {
        crate::app::dlog(format!("update: API check failed ({e}), trying the release pages"));
        fetch_web().map_err(|_| e)
    })
}

pub fn fetch_api() -> std::result::Result<Release, String> {
    let r = http::get("api.github.com", &format!("/repos/{REPO}/releases/latest")).ok_or("no connection")?;
    match r.status {
        200 => parse_release(&r.body).ok_or_else(|| "unexpected reply".to_string()),
        404 => Err("no releases yet".into()),
        403 | 429 => Err("GitHub is busy, try again later".into()),
        s => Err(format!("GitHub replied {s}")),
    }
}

/// The same answer from the normal web pages, which aren't rate-limited
/// like the API: /releases/latest redirects to the newest release's tag, and
/// its asset list links the installer next to its SHA-256.
pub fn fetch_web() -> std::result::Result<Release, String> {
    let r = http::get_no_redirect("github.com", &format!("/{REPO}/releases/latest")).ok_or("no connection")?;
    let loc = r.location.ok_or_else(|| format!("GitHub replied {}", r.status))?;
    let tag = loc.split("/releases/tag/").nth(1).map(|t| t.trim().trim_end_matches('/').to_string()).filter(|t| !t.is_empty()).ok_or("no releases yet")?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    parse_version(&version).ok_or("unexpected reply")?;
    let page = format!("https://github.com/{REPO}/releases/tag/{tag}");
    let installer = http::get("github.com", &format!("/{REPO}/releases/expanded_assets/{tag}"))
        .filter(|r| r.status == 200)
        .and_then(|r| parse_assets_html(&String::from_utf8_lossy(&r.body), &tag));
    Some(Release { version, page, installer }).ok_or_else(|| "unexpected reply".into())
}

/// Find the installer link (and its SHA-256) in a release's asset list HTML.
pub fn parse_assets_html(html: &str, tag: &str) -> Option<Installer> {
    let prefix = format!("/{REPO}/releases/download/{tag}/");
    let mut rest = html;
    while let Some(i) = rest.find(&prefix) {
        let after = &rest[i..];
        let end = after.find('"')?;
        let path = &after[..end];
        let name = path.rsplit('/').next().unwrap_or("").to_lowercase();
        if name.starts_with("windylandsetup") && name.ends_with(".exe") {
            // The digest sits in the same row, before the next file's link.
            let row = &after[end..];
            let row = &row[..row[1..].find(&prefix).map_or(row.len(), |n| n + 1)];
            let sha256 = row.find("sha256:").map(|k| row[k + 7..].chars().take_while(|c| c.is_ascii_hexdigit()).collect::<String>().to_lowercase()).filter(|h| h.len() == 64);
            return Some(Installer { url: format!("https://github.com{path}"), size: None, sha256 });
        }
        rest = &after[prefix.len()..];
    }
    None
}

/// Lowercase hex SHA-256 (Windows' own implementation).
pub fn sha256_hex(data: &[u8]) -> String {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Blocking: download the release installer to %TEMP% and check it: the
/// SHA-256 GitHub publishes for it (or the exact size when that's all we
/// have), and a Windows exe header.
pub fn download_installer(rel: &Release) -> std::result::Result<std::path::PathBuf, String> {
    let inst = rel.installer.clone().ok_or("release has no installer")?;
    let (host, path) = split_url(&inst.url).ok_or("bad download link")?;
    let r = http::get(&host, &path).ok_or("download failed")?;
    if r.status != 200 {
        return Err(format!("download failed ({})", r.status));
    }
    if !r.body.starts_with(b"MZ") {
        return Err("downloaded file isn't an installer".into());
    }
    match (&inst.sha256, inst.size) {
        (Some(want), _) if sha256_hex(&r.body) != *want => return Err("downloaded file doesn't match the release".into()),
        (None, Some(size)) if r.body.len() as u64 != size => return Err(format!("download incomplete ({} of {size} bytes)", r.body.len())),
        (None, None) if r.body.len() < 500_000 => return Err("download incomplete".into()),
        _ => {}
    }
    let p = std::env::temp_dir().join(format!("WindyLandSetup-{}.exe", rel.version));
    std::fs::write(&p, &r.body).map_err(|e| e.to_string())?;
    Ok(p)
}

fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    Some((host.to_string(), format!("/{path}")))
}

fn wake(h: isize) {
    unsafe {
        let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

pub struct Updater {
    hwnd: isize,
    state: Arc<Mutex<State>>,
    started: f64,
    next_check: f64,
    /// Version the user chose to skip (settings `update_skip`).
    skip: String,
    shown: Option<String>,
    /// The release offered in the last toast (what Update/Skip act on).
    offer: Option<Release>,
}

impl Updater {
    pub fn new(hwnd: HWND) -> Updater {
        let skip = crate::store::Settings::load().extra.get("update_skip").cloned().unwrap_or_default();
        Updater {
            hwnd: hwnd.0 as isize,
            state: Arc::new(Mutex::new(State::Idle)),
            started: unix_now(),
            next_check: unix_now() + FIRST_CHECK_AFTER,
            skip,
            shown: None,
            offer: None,
        }
    }

    fn check(&mut self, manual: bool) {
        {
            let mut s = self.state.lock().unwrap();
            if matches!(*s, State::Checking { .. } | State::Downloading(_)) {
                return;
            }
            *s = State::Checking { manual };
        }
        self.next_check = unix_now() + CHECK_EVERY;
        let (slot, hwnd) = (self.state.clone(), self.hwnd);
        std::thread::spawn(move || {
            let next = match fetch_latest() {
                Ok(rel) if is_newer(&rel.version, VERSION) => State::Found { rel, manual },
                Ok(_) => State::UpToDate { manual },
                Err(why) => State::Failed { manual, why },
            };
            *slot.lock().unwrap() = next;
            wake(hwnd);
        });
    }

    fn download(&mut self, rel: Release) {
        if rel.installer.is_none() {
            return;
        }
        *self.state.lock().unwrap() = State::Downloading(rel.clone());
        let (slot, hwnd) = (self.state.clone(), self.hwnd);
        std::thread::spawn(move || {
            *slot.lock().unwrap() = match download_installer(&rel) {
                Ok(p) => State::Ready(p.display().to_string()),
                Err(e) => State::DownloadFailed(rel, e),
            };
            wake(hwnd);
        });
    }

    fn open_page(&self) {
        let url = HSTRING::from(format!("https://github.com/{REPO}/releases/latest"));
        unsafe {
            ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
        }
    }
}

impl Module for Updater {
    fn title(&self) -> &'static str {
        "Updates"
    }
    fn has_tab(&self) -> bool {
        false
    }

    fn tick(&mut self, ctx: &mut Ctx) {
        if crate::time::demo() {
            return;
        }
        if unix_now() >= self.next_check && unix_now() - self.started >= FIRST_CHECK_AFTER {
            self.check(false);
        }
        let state = self.state.lock().unwrap().clone();
        match state {
            State::Found { rel, manual } => {
                *self.state.lock().unwrap() = State::Idle;
                let fresh = self.shown.as_deref() != Some(rel.version.as_str());
                if manual || (rel.version != self.skip && fresh) {
                    self.shown = Some(rel.version.clone());
                    let mut t = Toast::new("update", "\u{E895}", col::GREEN, "Update available", format!("WindyLand {} is out (you have {VERSION})", rel.version))
                        .secs(30.0);
                    t = if rel.installer.is_some() { t.action("Update", A_UPDATE) } else { t.action("Open page", A_OPEN_PAGE) };
                    ctx.toast(t.action("Later", A_LATER).action("Skip", A_SKIP));
                    self.offer = Some(rel);
                }
            }
            State::UpToDate { manual: true } => {
                *self.state.lock().unwrap() = State::Idle;
                ctx.toast(Toast::new("update", glyph::CHECK, col::GREEN, "You're up to date", format!("WindyLand {VERSION} is the latest")));
            }
            State::Failed { manual, why } => {
                *self.state.lock().unwrap() = State::Idle;
                // Offline or GitHub busy: try again within the hour rather
                // than waiting half a day.
                self.next_check = self.next_check.min(unix_now() + RETRY_AFTER);
                if manual {
                    ctx.toast(Toast::new("update", "\u{E895}", col::YELLOW, "Couldn't check for updates", why));
                }
            }
            State::UpToDate { manual: false } => *self.state.lock().unwrap() = State::Idle,
            State::Ready(path) => {
                *self.state.lock().unwrap() = State::Idle;
                // Run the installer silently; it waits for us to exit (AppMutex),
                // upgrades in place and starts the new version.
                let file = HSTRING::from(path.as_str());
                crate::app::release_single_instance();
                let ok = unsafe {
                    ShellExecuteW(None, w!("open"), &file, w!("/SILENT /SUPPRESSMSGBOXES /NORESTART"), None, SW_SHOWNORMAL).0 as isize > 32
                };
                if ok {
                    unsafe {
                        let _ = DestroyWindow(ctx.hwnd);
                    }
                } else {
                    ctx.toast(Toast::new("update", "\u{E895}", col::YELLOW, "Couldn't start the installer", "Opening the download page instead"));
                    self.open_page();
                }
            }
            State::DownloadFailed(rel, why) => {
                *self.state.lock().unwrap() = State::Idle;
                self.offer = Some(rel);
                ctx.toast(Toast::new("update", "\u{E895}", col::YELLOW, "Update failed", why).action("Open page", A_OPEN_PAGE));
            }
            _ => {}
        }
    }

    fn action(&mut self, id: u32, _rel: Rel, ctx: &mut Ctx) {
        let offered = self.offer.clone();
        match id {
            A_CHECK_NOW => self.check(true),
            A_UPDATE => {
                if let Some(rel) = offered {
                    ctx.toast(Toast::new("update", "\u{E896}", col::GREEN, "Downloading update", format!("WindyLand {}", rel.version)).secs(60.0));
                    self.download(rel);
                }
            }
            A_SKIP => {
                if let Some(rel) = offered {
                    self.skip = rel.version.clone();
                    ctx.settings.extra.insert("update_skip".into(), rel.version);
                    ctx.settings.save();
                }
                self.offer = None;
            }
            A_LATER => self.offer = None,
            A_OPEN_PAGE => self.open_page(),
            _ => {}
        }
    }

    fn debug_state(&self) -> String {
        format!("version {VERSION}, skip='{}'", self.skip)
    }
}
