//! Plain-text persistence in %APPDATA%\WindyLand\.
//!
//! Each feature owns one small text file (settings.txt, todo.txt, water.txt,
//! shelf.txt). Files are written atomically (temp file + rename).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    // WINDYLAND_DATA_DIR overrides the location (used by `windyland --selftest`).
    let p = match std::env::var("WINDYLAND_DATA_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("WindyLand"),
    };
    let _ = fs::create_dir_all(&p);
    p
}

pub fn read(name: &str) -> String {
    fs::read_to_string(dir().join(name)).unwrap_or_default()
}

pub fn write(name: &str, content: &str) {
    let p = dir().join(name);
    let tmp = p.with_extension("tmp");
    if fs::write(&tmp, content).is_ok() {
        let _ = fs::rename(&tmp, &p);
    }
}

/// Parse `key=value` lines (ignores blanks and `#` comments).
pub fn parse_kv(s: &str) -> BTreeMap<String, String> {
    s.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn write_kv(m: &BTreeMap<String, String>) -> String {
    m.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// Escape a field for tab-separated storage.
pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

pub fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// How the notch looks when nothing is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotchStyle {
    /// Black notch with clock, live activities and the mascot.
    Full,
    /// A smaller, shorter notch with clock and live activities.
    Compact,
    /// A thin floating bar, like the Android gesture pill. Tinted when
    /// something is live (music, timer).
    Pill,
    /// Nothing visible. Rest the mouse at the top edge to open.
    Hidden,
}

impl NotchStyle {
    pub const ALL: [NotchStyle; 4] = [NotchStyle::Full, NotchStyle::Compact, NotchStyle::Pill, NotchStyle::Hidden];
    pub fn key(self) -> &'static str {
        match self {
            NotchStyle::Full => "full",
            NotchStyle::Compact => "compact",
            NotchStyle::Pill => "pill",
            NotchStyle::Hidden => "hidden",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            NotchStyle::Full => "Full notch",
            NotchStyle::Compact => "Compact notch",
            NotchStyle::Pill => "Pill (thin bar)",
            NotchStyle::Hidden => "Hidden (hover top edge)",
        }
    }
    pub fn from_key(s: &str) -> Option<NotchStyle> {
        NotchStyle::ALL.into_iter().find(|v| v.key() == s)
    }
    /// How long the mouse must rest on the collapsed notch before it opens.
    /// Smaller styles wait longer so reaching for browser tabs doesn't open it.
    pub fn hover_delay(self) -> f64 {
        match self {
            NotchStyle::Full => 0.0,
            NotchStyle::Compact => 0.15,
            NotchStyle::Pill => 0.3,
            NotchStyle::Hidden => 0.35,
        }
    }
}

/// The idle companion shown in the collapsed Full notch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mascot {
    Eyes,
    Cat,
    Ghost,
    Robot,
    Sparky,
    Sprout,
    Owl,
    Panda,
}

impl Mascot {
    pub const ALL: [Mascot; 8] =
        [Mascot::Eyes, Mascot::Cat, Mascot::Ghost, Mascot::Robot, Mascot::Sparky, Mascot::Sprout, Mascot::Owl, Mascot::Panda];
    pub fn key(self) -> &'static str {
        match self {
            Mascot::Eyes => "eyes",
            Mascot::Cat => "cat",
            Mascot::Ghost => "ghost",
            Mascot::Robot => "robot",
            Mascot::Sparky => "sparky",
            Mascot::Sprout => "sprout",
            Mascot::Owl => "owl",
            Mascot::Panda => "panda",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mascot::Eyes => "Eyes",
            Mascot::Cat => "Cat",
            Mascot::Ghost => "Ghost",
            Mascot::Robot => "Robot",
            Mascot::Sparky => "Sparky",
            Mascot::Sprout => "Sprout",
            Mascot::Owl => "Owl",
            Mascot::Panda => "Panda",
        }
    }
    pub fn from_key(s: &str) -> Option<Mascot> {
        Mascot::ALL.into_iter().find(|m| m.key() == s)
    }
}

/// Sound played with reminders and alerts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    Off,
    Chime,
    Bell,
    Pop,
    Marimba,
    Windows,
}

impl Sound {
    pub const ALL: [Sound; 6] = [Sound::Off, Sound::Chime, Sound::Bell, Sound::Pop, Sound::Marimba, Sound::Windows];
    pub fn key(self) -> &'static str {
        match self {
            Sound::Off => "off",
            Sound::Chime => "chime",
            Sound::Bell => "bell",
            Sound::Pop => "pop",
            Sound::Marimba => "marimba",
            Sound::Windows => "windows",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Sound::Off => "Off",
            Sound::Chime => "Chime",
            Sound::Bell => "Bell",
            Sound::Pop => "Pop",
            Sound::Marimba => "Marimba",
            Sound::Windows => "Windows",
        }
    }
    pub fn from_key(s: &str) -> Option<Sound> {
        Sound::ALL.into_iter().find(|m| m.key() == s)
    }
}

/// What clipboard history records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClipMode {
    Text,
    TextFiles,
    All,
}

impl ClipMode {
    pub const ALL: [ClipMode; 3] = [ClipMode::Text, ClipMode::TextFiles, ClipMode::All];
    pub fn key(self) -> &'static str {
        match self {
            ClipMode::Text => "text",
            ClipMode::TextFiles => "files",
            ClipMode::All => "all",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ClipMode::Text => "Text only",
            ClipMode::TextFiles => "Text + files",
            ClipMode::All => "Everything",
        }
    }
    pub fn from_key(s: &str) -> Option<ClipMode> {
        ClipMode::ALL.into_iter().find(|m| m.key() == s)
    }
}

/// Which screen edge the notch sits on (like the taskbar).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right];
    pub fn key(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Edge::Top => "Top",
            Edge::Bottom => "Bottom",
            Edge::Left => "Left",
            Edge::Right => "Right",
        }
    }
    pub fn from_key(s: &str) -> Option<Edge> {
        Edge::ALL.into_iter().find(|e| e.key() == s)
    }
    /// Left/right edges: the collapsed tab is vertical.
    pub fn vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

/// App-wide settings shared by every module (settings.txt).
#[derive(Clone, Debug)]
pub struct Settings {
    /// Show the mascot when idle (off = clock only).
    pub personality: bool,
    /// Which mascot to show.
    pub mascot: Mascot,
    /// Collapsed look: full, compact, pill or hidden.
    pub style: NotchStyle,
    /// Screen edge, position along it (0..1 of the work area), and a point
    /// inside the chosen monitor (None = primary monitor).
    pub edge: Edge,
    pub pos: f32,
    pub monitor: Option<(i32, i32)>,
    /// Pop up overlays for volume changes.
    pub volume_overlay: bool,
    /// Show dots while the microphone or camera is in use.
    pub privacy_dots: bool,
    /// Clipboard history: recording on (off until the user turns it on) and
    /// what it records.
    pub clip_on: bool,
    pub clip_mode: ClipMode,
    /// The welcome tour has been shown (or skipped).
    pub onboarded: bool,
    /// Sound for reminders and alerts.
    pub sound: Sound,
    /// Show the mascot all the time (Full style), not only when idle.
    pub mascot_always: bool,
    /// Where the mascot sits in the notch: 0 left, 1 centre, 2 right.
    pub mascot_pos: u8,
    /// Panels turned off in Settings > Panels (module titles).
    pub hidden_panels: Vec<String>,
    /// Show other apps' Windows notifications in the notch (opt-in).
    pub notif_on: bool,
    /// Include the message text (off: only the app name shows).
    pub notif_text: bool,
    pub extra: BTreeMap<String, String>,
}

impl Settings {
    /// The panel with this title was turned off in Settings > Panels.
    pub fn panel_hidden(&self, title: &str) -> bool {
        self.hidden_panels.iter().any(|t| t == title)
    }

    pub fn load() -> Settings {
        let kv = parse_kv(&read("settings.txt"));
        let b = |k: &str, d: bool| kv.get(k).map(|v| v == "1" || v == "true").unwrap_or(d);
        Settings {
            personality: b("personality", true),
            mascot: kv.get("mascot").and_then(|s| Mascot::from_key(s)).unwrap_or(Mascot::Eyes),
            // Older builds stored `minimal=1`; map it to Compact.
            style: kv
                .get("style")
                .and_then(|s| NotchStyle::from_key(s))
                .unwrap_or(if b("minimal", false) { NotchStyle::Compact } else { NotchStyle::Full }),
            edge: kv.get("edge").and_then(|s| Edge::from_key(s)).unwrap_or(Edge::Top),
            pos: kv.get("pos").and_then(|s| s.parse().ok()).unwrap_or(0.5f32).clamp(0.0, 1.0),
            monitor: kv.get("monitor").and_then(|s| {
                let (x, y) = s.split_once(',')?;
                Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
            }),
            volume_overlay: b("volume_overlay", true),
            privacy_dots: b("privacy_dots", true),
            clip_on: b("clip_on", false),
            clip_mode: kv.get("clip_mode").and_then(|s| ClipMode::from_key(s)).unwrap_or(ClipMode::Text),
            // Anyone who hasn't finished the tour sees it once, including
            // people updating from a version that didn't have it.
            onboarded: kv.get("onboarded").map_or(false, |v| v == "1"),
            sound: kv.get("sound").and_then(|s| Sound::from_key(s)).unwrap_or(Sound::Chime),
            mascot_always: kv.get("mascot_always").map_or(false, |v| v == "1"),
            mascot_pos: kv.get("mascot_pos").and_then(|v| v.parse().ok()).filter(|p| *p <= 2).unwrap_or(1),
            hidden_panels: kv.get("hidden_panels").map_or(Vec::new(), |v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect()),
            notif_on: kv.get("notif_on").map_or(false, |v| v == "1"),
            notif_text: kv.get("notif_text").map_or(true, |v| v == "1"),
            extra: kv,
        }
    }
    pub fn save(&self) {
        let mut kv = self.extra.clone();
        kv.insert("personality".into(), (self.personality as u8).to_string());
        kv.insert("mascot".into(), self.mascot.key().into());
        kv.remove("minimal");
        kv.insert("style".into(), self.style.key().into());
        kv.insert("edge".into(), self.edge.key().into());
        kv.insert("pos".into(), format!("{:.4}", self.pos));
        match self.monitor {
            Some((x, y)) => kv.insert("monitor".into(), format!("{x},{y}")),
            None => kv.remove("monitor"),
        };
        kv.insert("volume_overlay".into(), (self.volume_overlay as u8).to_string());
        kv.insert("privacy_dots".into(), (self.privacy_dots as u8).to_string());
        kv.insert("clip_on".into(), (self.clip_on as u8).to_string());
        kv.insert("clip_mode".into(), self.clip_mode.key().into());
        kv.insert("onboarded".into(), (self.onboarded as u8).to_string());
        kv.insert("sound".into(), self.sound.key().into());
        kv.insert("mascot_always".into(), (self.mascot_always as u8).to_string());
        kv.insert("mascot_pos".into(), self.mascot_pos.to_string());
        kv.insert("hidden_panels".into(), self.hidden_panels.join(","));
        kv.insert("notif_on".into(), (self.notif_on as u8).to_string());
        kv.insert("notif_text".into(), (self.notif_text as u8).to_string());
        write("settings.txt", &write_kv(&kv));
    }
}
