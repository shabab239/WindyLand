//! Now playing (any app in the Windows media overlay: Spotify, browsers,
//! Media Player…) with album art, seek, transport controls, and synced
//! lyrics from LRCLIB shown in a split panel.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Media::Control::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::WM_APP_WAKE;
use crate::gfx::*;
use crate::http;
use crate::modules::{Ctx, Module, Rel};
use crate::time::{filetime_to_unix, fmt_dur, unix_now};
use crate::ui::Ui;

// ------------------------------------------------------------ shared state

#[derive(Clone, Default)]
pub struct MediaInfo {
    pub has: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub playing: bool,
    /// Position (s) measured at `stamp` (unix s).
    pub pos: f64,
    pub stamp: f64,
    pub dur: f64,
    pub art: Option<Arc<(u32, u32, Vec<u8>)>>,
    pub art_ver: u64,
}

impl MediaInfo {
    fn key(&self) -> String {
        format!("{}\u{1}{}", self.title, self.artist)
    }
    pub fn position(&self) -> f64 {
        let p = if self.playing { self.pos + (unix_now() - self.stamp) } else { self.pos };
        if self.dur > 0.0 {
            p.clamp(0.0, self.dur)
        } else {
            p.max(0.0)
        }
    }
}

enum Cmd {
    Toggle,
    Next,
    Prev,
    Seek(f64),
}

#[derive(Clone)]
enum Lyrics {
    Off,
    Loading,
    Missing,
    Synced(Vec<(f64, String)>),
    Plain(Vec<String>),
}

// ------------------------------------------------------------ media thread

fn wake(h: isize) {
    unsafe {
        let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
    }
}

fn spawn_media(shared: Arc<Mutex<MediaInfo>>, rx: Receiver<Cmd>, hwnd: isize) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mgr = loop {
            match GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()) {
                Ok(m) => break m,
                Err(_) => std::thread::sleep(Duration::from_secs(5)),
            }
        };
        let mut art = ArtTrack::default();
        loop {
            match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(cmd) => {
                    if let Ok(s) = mgr.GetCurrentSession() {
                        let _ = match cmd {
                            Cmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
                            Cmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()),
                            Cmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()),
                            Cmd::Seek(t) => s.TryChangePlaybackPositionAsync((t * 1e7) as i64).and_then(|o| o.get()),
                        };
                    }
                    std::thread::sleep(Duration::from_millis(180));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let info = poll(&mgr, &shared, &mut art);
            let changed = {
                let mut cur = shared.lock().unwrap();
                let drift = (cur.position() - info.position()).abs() > 2.5;
                let changed = cur.has != info.has
                    || cur.key() != info.key()
                    || cur.playing != info.playing
                    || (cur.dur - info.dur).abs() > 0.5
                    || cur.art_ver != info.art_ver
                    || drift;
                *cur = info;
                changed
            };
            if changed {
                wake(hwnd);
            }
        }
    });
}

/// Album-art bookkeeping for the media thread.
#[derive(Default)]
struct ArtTrack {
    key: String,
    /// When the current track started (unix secs).
    since: f64,
    /// Last time the thumbnail was read.
    read_at: f64,
    /// Hash of the raw thumbnail bytes last decoded (0 = none).
    hash: u64,
}

unsafe fn poll(mgr: &GlobalSystemMediaTransportControlsSessionManager, shared: &Arc<Mutex<MediaInfo>>, art_st: &mut ArtTrack) -> MediaInfo {
    let prev = shared.lock().unwrap().clone();
    let Ok(s) = mgr.GetCurrentSession() else {
        art_st.key = String::new();
        return MediaInfo::default();
    };
    let mut info = MediaInfo { has: true, ..Default::default() };
    info.app = s.SourceAppUserModelId().map(|h| pretty_app(&h.to_string())).unwrap_or_default();
    let props = s.TryGetMediaPropertiesAsync().and_then(|o| o.get());
    if let Ok(p) = &props {
        info.title = p.Title().map(|h| h.to_string()).unwrap_or_default();
        info.artist = p.Artist().map(|h| h.to_string()).unwrap_or_default();
        info.album = p.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
    }
    if info.title.is_empty() {
        return MediaInfo::default();
    }
    if let Ok(tl) = s.GetTimelineProperties() {
        let pos = tl.Position().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        let end = tl.EndTime().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        let upd = tl.LastUpdatedTime().map(|t| filetime_to_unix(t.UniversalTime)).unwrap_or(0.0);
        info.pos = pos;
        info.dur = end;
        let now = unix_now();
        info.stamp = if upd > 0.0 && upd <= now + 1.0 && now - upd < 3600.0 { upd } else { now };
    }
    if let Ok(pb) = s.GetPlaybackInfo() {
        info.playing = pb.PlaybackStatus().map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing).unwrap_or(false);
    }
    // Some players report a stale timeline; keep extrapolating smoothly.
    if info.playing && prev.key() == info.key() && (prev.position() - info.position()).abs() < 2.5 {
        info.pos = prev.pos;
        info.stamp = prev.stamp;
    }
    // Album art. Players often publish the new title before the new cover,
    // so a read right at the track change can return the previous song's
    // art. Re-read every second for 15 s after a change, then every 5 s, and
    // only decode/redraw when the raw bytes actually differ.
    let key = info.key();
    let now = unix_now();
    let new_track = key != art_st.key;
    if new_track {
        art_st.key = key;
        art_st.since = now;
    }
    let every = if now - art_st.since < 15.0 { 0.9 } else { 5.0 };
    info.art = prev.art.clone();
    info.art_ver = prev.art_ver;
    if new_track || now - art_st.read_at >= every {
        art_st.read_at = now;
        let raw = props.ok().and_then(|p| p.Thumbnail().ok()).and_then(|t| read_stream_bytes(&t));
        let hash = raw.as_ref().map_or(0, |b| fnv1a(b));
        if hash != art_st.hash || (new_track && hash == 0) {
            art_st.hash = hash;
            info.art = raw.and_then(|b| decode_square(&b, 160)).map(Arc::new);
            info.art_ver = prev.art_ver + 1;
        }
    }
    info
}

fn fnv1a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x100000001b3);
    }
    h | 1
}

fn pretty_app(aumid: &str) -> String {
    let l = aumid.to_lowercase();
    for (k, v) in [
        ("spotify", "Spotify"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("zunemusic", "Media Player"),
        ("applemusic", "Apple Music"),
        ("vlc", "VLC"),
        ("brave", "Brave"),
        ("opera", "Opera"),
    ] {
        if l.contains(k) {
            return v.to_string();
        }
    }
    aumid.rsplit(['\\', '!', '.']).next().unwrap_or(aumid).trim_end_matches(".exe").to_string()
}

pub(crate) unsafe fn read_stream_bytes(r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = r.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 20_000_000 {
        return None;
    }
    let input = stream.GetInputStreamAt(0).ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

/// Decode any image WIC understands, centre-crop to a square, scale to `side`.
pub(crate) unsafe fn decode_square(bytes: &[u8], side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let stream = f.CreateStream().ok()?;
    stream.InitializeFromMemory(bytes).ok()?;
    let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    let (mut w, mut h) = (0u32, 0u32);
    frame.GetSize(&mut w, &mut h).ok()?;
    let s = w.min(h);
    if s == 0 {
        return None;
    }
    let clip = f.CreateBitmapClipper().ok()?;
    let rect = WICRect { X: ((w - s) / 2) as i32, Y: ((h - s) / 2) as i32, Width: s as i32, Height: s as i32 };
    clip.Initialize(&frame, &rect).ok()?;
    let scaler = f.CreateBitmapScaler().ok()?;
    scaler.Initialize(&clip, side, side, WICBitmapInterpolationModeFant).ok()?;
    let conv = f.CreateFormatConverter().ok()?;
    conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
        .ok()?;
    let mut px = vec![0u8; (side * side * 4) as usize];
    conv.CopyPixels(std::ptr::null(), side * 4, &mut px).ok()?;
    Some((side, side, px))
}

// ------------------------------------------------------------------ lyrics

/// Clean up titles from video sites: "Artist - Song (Official Video) [4K]".
fn clean_query(title: &str, artist: &str) -> (String, String) {
    let mut t = title.to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (t.find(open), t.find(close)) {
            if b > a {
                let inner = t[a + 1..b].to_lowercase();
                if ["official", "video", "audio", "lyric", "visualizer", "hd", "4k", "mv", "live", "remaster"]
                    .iter()
                    .any(|k| inner.contains(k))
                {
                    t.replace_range(a..=b, "");
                    continue;
                }
            }
            break;
        }
    }
    let mut a = artist.trim().trim_end_matches(" - Topic").to_string();
    if let Some((x, y)) = t.split_once(" - ") {
        if a.is_empty() || x.trim().eq_ignore_ascii_case(&a) {
            a = x.trim().to_string();
            t = y.to_string();
        }
    }
    (t.trim().to_string(), a)
}

fn parse_lrc(s: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for line in s.lines() {
        let mut rest = line;
        let mut stamps = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            if let Some((m, sec)) = tag.split_once(':') {
                if let (Ok(m), Ok(sec)) = (m.parse::<f64>(), sec.parse::<f64>()) {
                    stamps.push(m * 60.0 + sec);
                }
            }
            rest = &r[end + 1..];
        }
        let text = rest.trim().to_string();
        for t in stamps {
            out.push((t, text.clone()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out
}

fn fetch_lyrics(title: &str, artist: &str, album: &str, dur: f64) -> Lyrics {
    let (t, a) = clean_query(title, artist);
    let from_json = |v: &serde_json::Value| -> Option<Lyrics> {
        // LRCLIB has some junk entries (e.g. "[00:00.00]probe"); require a few real lines.
        if let Some(s) = v.get("syncedLyrics").and_then(|x| x.as_str()) {
            let l = parse_lrc(s);
            if l.iter().filter(|(_, t)| !t.is_empty()).count() >= 3 {
                return Some(Lyrics::Synced(l));
            }
        }
        if let Some(s) = v.get("plainLyrics").and_then(|x| x.as_str()) {
            if s.lines().filter(|l| !l.trim().is_empty()).count() >= 3 {
                return Some(Lyrics::Plain(s.lines().map(|l| l.to_string()).collect()));
            }
        }
        None
    };
    let mut path = format!("/api/get?track_name={}&artist_name={}", http::enc(&t), http::enc(&a));
    if !album.is_empty() {
        path.push_str(&format!("&album_name={}", http::enc(album)));
    }
    if dur > 0.0 {
        path.push_str(&format!("&duration={}", dur.round() as u64));
    }
    if let Some(r) = http::get("lrclib.net", &path) {
        if r.status == 200 {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&r.body) {
                if let Some(l) = from_json(&v) {
                    return l;
                }
            }
        }
    }
    // Fallback: fuzzy search, prefer synced lyrics with the closest duration.
    let q = if a.is_empty() {
        format!("/api/search?q={}", http::enc(&t))
    } else {
        format!("/api/search?track_name={}&artist_name={}", http::enc(&t), http::enc(&a))
    };
    if let Some(r) = http::get("lrclib.net", &q) {
        if let Ok(serde_json::Value::Array(list)) = serde_json::from_slice::<serde_json::Value>(&r.body) {
            let mut best: Option<(f64, &serde_json::Value)> = None;
            for v in &list {
                let synced = v.get("syncedLyrics").and_then(|x| x.as_str()).map_or(false, |s| s.lines().count() >= 3);
                let d = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let score = (if dur > 0.0 { (d - dur).abs() } else { 0.0 }) + if synced { 0.0 } else { 1000.0 };
                if best.map_or(true, |(s, _)| score < s) {
                    best = Some((score, v));
                }
            }
            if let Some((_, v)) = best {
                if let Some(l) = from_json(v) {
                    return l;
                }
            }
        }
    }
    Lyrics::Missing
}

// ------------------------------------------------------------------ module

const A_TOGGLE: u32 = 1;
const A_PREV: u32 = 2;
const A_NEXT: u32 = 3;
const A_SEEK: u32 = 4;
const A_LYRICS: u32 = 5;
const A_LINE: u32 = 1000; // + line index
/// `--render-docs` hook: load the demo lyrics and open the lyrics view.
pub const A_DEMO_LYRICS: u32 = 990;

pub struct Music {
    shared: Arc<Mutex<MediaInfo>>,
    tx: Sender<Cmd>,
    info: MediaInfo,
    art: Option<(u64, u32, ID2D1Bitmap)>,
    show_lyrics: bool,
    lyrics: Arc<Mutex<(String, Lyrics)>>,
    lyric_scroll: f32,
    manual_scroll: Option<(f32, f64)>,
    hwnd: isize,
}

impl Music {
    pub fn new(hwnd: HWND) -> Music {
        let shared = Arc::new(Mutex::new(MediaInfo::default()));
        let (tx, rx) = mpsc::channel();
        let h = hwnd.0 as isize;
        if crate::time::demo() {
            *shared.lock().unwrap() = demo_media();
        } else {
            spawn_media(shared.clone(), rx, h);
        }
        let show_lyrics = crate::store::Settings::load().extra.get("lyrics").map_or(false, |v| v == "1");
        Music {
            shared,
            tx,
            info: MediaInfo::default(),
            art: None,
            show_lyrics,
            lyrics: Arc::new(Mutex::new((String::new(), Lyrics::Off))),
            lyric_scroll: 0.0,
            manual_scroll: None,
            hwnd: h,
        }
    }

    /// Fetch lyrics for the current track if the lyrics view is on.
    fn ensure_lyrics(&mut self) {
        if !self.show_lyrics || !self.info.has {
            return;
        }
        let key = self.info.key();
        {
            let mut l = self.lyrics.lock().unwrap();
            if l.0 == key {
                return;
            }
            *l = (key.clone(), Lyrics::Loading);
        }
        self.lyric_scroll = 0.0;
        let (title, artist, album, dur) = (self.info.title.clone(), self.info.artist.clone(), self.info.album.clone(), self.info.dur);
        let slot = self.lyrics.clone();
        let hwnd = self.hwnd;
        std::thread::spawn(move || {
            let res = fetch_lyrics(&title, &artist, &album, dur);
            let mut l = slot.lock().unwrap();
            if l.0 == key {
                l.1 = res;
            }
            drop(l);
            wake(hwnd);
        });
    }

    fn art_bitmap(&mut self, ui: &mut Ui) -> Option<ID2D1Bitmap> {
        let art = self.info.art.clone()?;
        let ok = matches!(&self.art, Some((v, g, _)) if *v == self.info.art_ver && *g == ui.gen);
        if !ok {
            let (w, h, px) = &*art;
            let bmp = ui.make_bitmap(*w, *h, px)?;
            self.art = Some((self.info.art_ver, ui.gen, bmp));
        }
        self.art.as_ref().map(|(_, _, b)| b.clone())
    }

    fn draw_player(&mut self, ui: &mut Ui, mut r: Rect) {
        if !self.info.has {
            let mut c = r;
            let ic = c.cut_top(c.h * 0.5, 0.0);
            ui.icon(glyph::MUSIC, Rect::new(ic.x, ic.b() - 30.0, ic.w, 28.0), 22.0, col::FAINT);
            ui.text("Nothing playing", Rect::new(c.x, c.y + 4.0, c.w, 18.0), 13.0, col::DIM, Weight::Semi, Align::Center);
            ui.text("Play something in Spotify, YouTube or any media app", Rect::new(c.x, c.y + 22.0, c.w, 16.0), 11.0, col::FAINT, Weight::Regular, Align::Center);
            return;
        }
        let art_r = r.cut_left(r.h.min(96.0), 16.0);
        let art_r = Rect::new(art_r.x, art_r.y + (art_r.h - art_r.w) / 2.0, art_r.w, art_r.w);
        match self.art_bitmap(ui) {
            Some(b) => ui.image_rr(&b, art_r, 14.0),
            None => {
                ui.fill_rr(art_r, 14.0, col::TEXT.with_a(0.08));
                ui.icon(glyph::MUSIC, art_r, 26.0, col::FAINT);
            }
        }
        let info = self.info.clone();
        let mut meta = r.cut_top(44.0, 4.0);
        ui.text(&info.title, meta.cut_top(22.0, 0.0), 15.0, col::TEXT, Weight::Semi, Align::Left);
        let sub = if info.artist.is_empty() { info.app.clone() } else { format!("{} · {}", info.artist, info.app) };
        ui.text(&sub, meta, 12.0, col::DIM, Weight::Regular, Align::Left);

        let mut prog = r.cut_top(22.0, 4.0);
        let pos = info.position();
        let lt = prog.cut_left(40.0, 6.0);
        let rt = prog.cut_right(44.0, 6.0);
        ui.text(&fmt_dur(pos), lt, 10.5, col::DIM, Weight::Regular, Align::Left);
        let remain = if info.dur > 0.0 { format!("-{}", fmt_dur(info.dur - pos)) } else { String::new() };
        ui.text(&remain, rt, 10.5, col::DIM, Weight::Regular, Align::Right);
        let frac = if info.dur > 0.0 { (pos / info.dur) as f32 } else { 0.0 };
        ui.progress(prog.centered(prog.w, 4.0), frac, col::TEXT, if info.dur > 0.0 { Some(A_SEEK) } else { None });

        let mut ctl = r.cut_top(32.0, 0.0);
        let lyr = ctl.cut_right(32.0, 0.0);
        let fg = if self.show_lyrics { col::ACCENT } else { col::DIM };
        let bg = if self.show_lyrics { Some(col::ACCENT.with_a(0.18)) } else { None };
        ui.icon_btn(lyr, glyph::LYRICS, A_LYRICS, 14.0, fg, bg, if self.show_lyrics { "Hide lyrics" } else { "Show lyrics" });
        let mid = ctl.centered(32.0 * 3.0 + 24.0, 32.0);
        let cells = mid.columns(3, 12.0);
        ui.icon_btn(cells[0], glyph::PREV, A_PREV, 15.0, col::TEXT, None, "Previous");
        let (g, tip) = if info.playing { (glyph::PAUSE, "Pause") } else { (glyph::PLAY, "Play") };
        ui.icon_btn(cells[1], g, A_TOGGLE, 17.0, col::TEXT, None, tip);
        ui.icon_btn(cells[2], glyph::NEXT, A_NEXT, 15.0, col::TEXT, None, "Next");
    }

    fn draw_lyrics(&mut self, ui: &mut Ui, r: Rect) {
        let state = self.lyrics.lock().unwrap().1.clone();
        let mut r = r;
        let foot = r.cut_bottom(14.0, 4.0);
        ui.text("Lyrics provided by LRCLIB", foot, 9.5, col::FAINT, Weight::Regular, Align::Right);
        match state {
            Lyrics::Off | Lyrics::Loading => {
                ui.text("Loading lyrics…", r, 12.0, col::DIM, Weight::Regular, Align::Center);
            }
            Lyrics::Missing => {
                ui.text("Lyrics not available", r, 12.0, col::DIM, Weight::Regular, Align::Center);
            }
            Lyrics::Synced(lines) => {
                let pos = self.info.position() + 0.25;
                let cur = lines.iter().rposition(|(t, _)| *t <= pos);
                let lh = 26.0;
                let target = cur.map_or(0.0, |c| c as f32 * lh - r.h / 2.0 + lh / 2.0);
                if let Some((_, at)) = &mut self.manual_scroll {
                    if *at < 0.0 {
                        *at = ui.t;
                    }
                }
                let auto = match self.manual_scroll {
                    Some((_, at)) if ui.t - at < 4.0 => false,
                    _ => {
                        self.manual_scroll = None;
                        true
                    }
                };
                if auto {
                    self.lyric_scroll += (target - self.lyric_scroll) * 0.18;
                    if (target - self.lyric_scroll).abs() < 0.5 {
                        self.lyric_scroll = target;
                    }
                } else if let Some((s, _)) = self.manual_scroll {
                    self.lyric_scroll = s;
                }
                ui.push_clip(r);
                for (i, (_, text)) in lines.iter().enumerate() {
                    let y = r.y + i as f32 * lh - self.lyric_scroll;
                    if y + lh < r.y || y > r.b() {
                        continue;
                    }
                    let lr = Rect::new(r.x, y, r.w, lh);
                    let is_cur = Some(i) == cur;
                    // Fade lines toward the edges.
                    let d = ((lr.cy() - r.cy()).abs() / (r.h / 2.0)).min(1.0);
                    let a = ui.alpha;
                    ui.alpha = a * (1.0 - d * 0.75);
                    let txt = if text.is_empty() { "♪" } else { text.as_str() };
                    if is_cur {
                        ui.text(txt, lr, 15.0, col::TEXT, Weight::Semi, Align::Left);
                    } else {
                        let c = if ui.hovered(lr) { col::TEXT.with_a(0.8) } else { col::DIM };
                        ui.text(txt, lr, 13.0, c, Weight::Regular, Align::Left);
                    }
                    ui.alpha = a;
                    ui.hit(lr, A_LINE + i as u32);
                }
                ui.pop_clip();
            }
            Lyrics::Plain(lines) => {
                let lh = 20.0;
                let max = (lines.len() as f32 * lh - r.h).max(0.0);
                let s = self.manual_scroll.map_or(0.0, |(s, _)| s).clamp(0.0, max);
                ui.push_clip(r);
                for (i, text) in lines.iter().enumerate() {
                    let y = r.y + i as f32 * lh - s;
                    if y + lh < r.y || y > r.b() {
                        continue;
                    }
                    ui.text(text, Rect::new(r.x, y, r.w, lh), 12.0, col::DIM, Weight::Regular, Align::Left);
                }
                ui.pop_clip();
            }
        }
    }

    fn save_pref(&self, ctx: &mut Ctx) {
        ctx.settings.extra.insert("lyrics".into(), (self.show_lyrics as u8).to_string());
        ctx.settings.save();
    }
}

impl Module for Music {
    fn title(&self) -> &'static str {
        "Now Playing"
    }
    fn accent(&self) -> Color {
        col::RED
    }
    fn icon(&self) -> &'static str {
        glyph::MUSIC
    }
    fn size(&self) -> (f32, f32) {
        if self.show_lyrics && self.info.has {
            (720.0, crate::modules::PANEL.1)
        } else {
            (440.0, 112.0)
        }
    }

    fn tick(&mut self, _ctx: &mut Ctx) {
        let fresh = self.shared.lock().unwrap().clone();
        if fresh.key() != self.info.key() {
            self.manual_scroll = None;
        }
        self.info = fresh;
        self.ensure_lyrics();
    }

    fn draw(&mut self, ui: &mut Ui, r: Rect) {
        // Keep the progress bar and lyric highlight fresh between ticks.
        let s = self.shared.lock().unwrap().clone();
        if s.key() == self.info.key() {
            self.info = s;
        }
        if self.show_lyrics && self.info.has {
            let mut r = r;
            let left = r.cut_left(330.0, 0.0);
            let div = r.cut_left(1.0, 20.0);
            ui.fill_rect(Rect::new(div.x, div.y + 6.0, 1.0, div.h - 12.0), col::TEXT.with_a(0.1));
            self.draw_player(ui, Rect::new(left.x, left.y, left.w, 112.0));
            self.draw_lyrics(ui, r);
        } else {
            self.draw_player(ui, r);
        }
    }

    fn action(&mut self, id: u32, rel: Rel, ctx: &mut Ctx) {
        let send = |c: Cmd| {
            let _ = self.tx.send(c);
        };
        match id {
            A_TOGGLE => {
                send(Cmd::Toggle);
                // Optimistic UI.
                let mut s = self.shared.lock().unwrap();
                let p = s.position();
                s.pos = p;
                s.stamp = unix_now();
                s.playing = !s.playing;
                self.info = s.clone();
            }
            A_PREV => send(Cmd::Prev),
            A_NEXT => send(Cmd::Next),
            A_SEEK => {
                let t = rel.0.clamp(0.0, 1.0) as f64 * self.info.dur;
                send(Cmd::Seek(t));
                let mut s = self.shared.lock().unwrap();
                s.pos = t;
                s.stamp = unix_now();
                self.info = s.clone();
            }
            A_DEMO_LYRICS => {
                self.info = self.shared.lock().unwrap().clone();
                self.demo_lyrics();
            }
            A_LYRICS => {
                self.show_lyrics = !self.show_lyrics;
                self.save_pref(ctx);
                self.ensure_lyrics();
            }
            i if i >= A_LINE => {
                let idx = (i - A_LINE) as usize;
                let t = match &self.lyrics.lock().unwrap().1 {
                    Lyrics::Synced(l) => l.get(idx).map(|x| x.0),
                    _ => None,
                };
                if let Some(t) = t {
                    send(Cmd::Seek(t));
                    let mut s = self.shared.lock().unwrap();
                    s.pos = t;
                    s.stamp = unix_now();
                    self.info = s.clone();
                    self.manual_scroll = None;
                }
            }
            _ => {}
        }
    }

    fn scroll(&mut self, delta: f32) {
        if self.show_lyrics {
            let base = self.manual_scroll.map_or(self.lyric_scroll, |(s, _)| s);
            // Timestamp is filled in on the next draw (we don't have the clock here).
            self.manual_scroll = Some(((base - delta * 40.0).max(-40.0), -1.0));
        }
    }

    fn live(&self) -> Option<u8> {
        if self.info.has && self.info.playing {
            Some(50)
        } else {
            None
        }
    }

    fn draw_live(&mut self, ui: &mut Ui, left: Rect, right: Rect) {
        let s = left.h.min(22.0);
        let a = Rect::new(left.x, left.cy() - s / 2.0, s, s);
        match self.art_bitmap(ui) {
            Some(b) => ui.image_rr(&b, a, 6.0),
            None => {
                ui.fill_rr(a, 6.0, col::TEXT.with_a(0.1));
                ui.icon(glyph::MUSIC, a, 11.0, col::DIM);
            }
        }
        // Little equaliser.
        let n = 4;
        let bw = 3.0;
        let gap = 3.0;
        let total = n as f32 * bw + (n - 1) as f32 * gap;
        let x0 = right.r() - total - 2.0;
        for i in 0..n {
            let ph = ui.t * (5.0 + i as f64 * 1.7) + i as f64 * 1.3;
            let h = 3.0 + (ph.sin() * 0.5 + 0.5) as f32 * (right.h * 0.6).min(12.0);
            let x = x0 + i as f32 * (bw + gap);
            ui.fill_rr(Rect::new(x, right.cy() - h / 2.0, bw, h), 1.5, col::ACCENT);
        }
    }

    fn animating(&self) -> bool {
        self.info.has && self.info.playing
    }

    fn debug_state(&self) -> String {
        let i = &self.info;
        if !i.has {
            return "no media session".into();
        }
        format!(
            "app={} playing={} pos={:.1}/{:.1}s art={} title_len={} artist_len={}",
            i.app,
            i.playing,
            i.position(),
            i.dur,
            i.art.as_ref().map_or("none".to_string(), |a| format!("{}x{}", a.0, a.1)),
            i.title.chars().count(),
            i.artist.chars().count()
        )
    }
}

/// Made-up track used by `--render-docs` (art from WINDYLAND_DEMO_ART).
fn demo_media() -> MediaInfo {
    let art = std::env::var("WINDYLAND_DEMO_ART")
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| unsafe { decode_square(&b, 160) })
        .map(Arc::new);
    MediaInfo {
        has: true,
        title: "Paper Sky".into(),
        artist: "Lumen Coast".into(),
        album: "Summer Flows".into(),
        app: "Spotify".into(),
        playing: true,
        pos: 134.0,
        stamp: unix_now(),
        dur: 213.0,
        art,
        art_ver: 1,
    }
}

impl Music {
    /// Demo lyrics (original lines, not a real song).
    pub fn demo_lyrics(&mut self) {
        let lines = [
            (112.0, "Wind across the open fields"),
            (118.0, "carries every word I kept"),
            (124.0, "Hold the light a little longer"),
            (129.0, "till the evening finds us here"),
            (134.0, "Every road we never took"),
            (139.0, "folds into a paper sky"),
            (144.0, "Sing it softly, let it fade"),
            (149.0, "the wind will bring you home"),
            (155.0, "Wind across the open fields"),
        ];
        let l = lines.iter().map(|(t, s)| (*t, s.to_string())).collect();
        *self.lyrics.lock().unwrap() = (self.info.key(), Lyrics::Synced(l));
        self.show_lyrics = true;
        self.lyric_scroll = 0.0;
    }
}

/// Self-test: fetch lyrics for a well-known song and summarise.
pub fn selftest_lyrics() -> String {
    match fetch_lyrics("Never Gonna Give You Up", "Rick Astley", "", 213.0) {
        Lyrics::Synced(l) => format!("synced, {} lines, first at {:.1}s", l.len(), l.first().map_or(0.0, |x| x.0)),
        Lyrics::Plain(l) => format!("plain only, {} lines", l.len()),
        Lyrics::Missing => "missing (network or LRCLIB problem)".into(),
        _ => "unexpected state".into(),
    }
}
