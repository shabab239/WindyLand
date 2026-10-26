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
    /// Show the mascot eyes when idle ("Personality" in the tray menu).
    pub personality: bool,
    /// Collapsed look: full, compact, pill or hidden.
    pub style: NotchStyle,
    /// Screen edge, position along it (0..1 of the work area), and a point
    /// inside the chosen monitor (None = primary monitor).
    pub edge: Edge,
    pub pos: f32,
    pub monitor: Option<(i32, i32)>,
    /// Pop up overlays for volume changes.
    pub volume_overlay: bool,
    pub extra: BTreeMap<String, String>,
}

impl Settings {
    pub fn load() -> Settings {
        let kv = parse_kv(&read("settings.txt"));
        let b = |k: &str, d: bool| kv.get(k).map(|v| v == "1" || v == "true").unwrap_or(d);
        Settings {
            personality: b("personality", true),
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
            extra: kv,
        }
    }
    pub fn save(&self) {
        let mut kv = self.extra.clone();
        kv.insert("personality".into(), (self.personality as u8).to_string());
        kv.remove("minimal");
        kv.insert("style".into(), self.style.key().into());
        kv.insert("edge".into(), self.edge.key().into());
        kv.insert("pos".into(), format!("{:.4}", self.pos));
        match self.monitor {
            Some((x, y)) => kv.insert("monitor".into(), format!("{x},{y}")),
            None => kv.remove("monitor"),
        };
        kv.insert("volume_overlay".into(), (self.volume_overlay as u8).to_string());
        write("settings.txt", &write_kv(&kv));
    }
}
