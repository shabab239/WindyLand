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
    /// Like Full but narrower; the open panel is also a little shorter.
    Slim,
    /// A smaller, shorter notch with clock and live activities.
    Compact,
    /// A thin floating bar, like the Android gesture pill. Tinted when
    /// something is live (music, timer).
    Pill,
    /// Nothing visible. Rest the mouse at the top edge to open.
    Hidden,
}

impl NotchStyle {
    pub const ALL: [NotchStyle; 5] = [NotchStyle::Full, NotchStyle::Slim, NotchStyle::Compact, NotchStyle::Pill, NotchStyle::Hidden];
    pub fn key(self) -> &'static str {
        match self {
            NotchStyle::Full => "full",
            NotchStyle::Slim => "slim",
            NotchStyle::Compact => "compact",
            NotchStyle::Pill => "pill",
            NotchStyle::Hidden => "hidden",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            NotchStyle::Full => "Full notch",
            NotchStyle::Slim => "Slim notch",
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
            NotchStyle::Full | NotchStyle::Slim => 0.0,
            NotchStyle::Compact => 0.15,
            NotchStyle::Pill => 0.3,
            NotchStyle::Hidden => 0.35,
        }
    }
    /// Full and Slim: the black notch with the clock, music and a mascot.
    pub fn full_like(self) -> bool {
        matches!(self, NotchStyle::Full | NotchStyle::Slim)
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
    /// Look for new versions on GitHub by itself (twice a day). Off: only
    /// when "Check for updates" is clicked. Also set by the installer.
    pub update_check: bool,
    /// The panel you last picked (module title); the notch opens on it.
    pub last_tab: String,
    /// Show other apps' Windows notifications in the notch (opt-in).
    pub notif_on: bool,
    /// Include the message text (off: only the app name shows).
    pub notif_text: bool,
    /// 12-hour clock ("4:20 PM"); off = 24-hour ("16:20").
    pub clock12: bool,
    /// A small toast showing what was just copied (opt-in).
    pub copy_toast: bool,
    /// Shrink to a thin, click-through bar while a maximized window is
    /// under the notch (browser tabs stay clickable).
    pub make_room: bool,
    /// What the closed notch shows, in order (Widgets tab).
    pub layout: NotchLayout,
    pub place: Place,
    /// Seconds on the clock face (Settings > Look).
    pub seconds: bool,
    /// Colour theme key (Settings > Look): default, graphite, midnight,
    /// light, glass.
    pub theme: String,
    pub extra: BTreeMap<String, String>,
}

/// One row of the closed notch's priority list: one item across both
/// sides of the clock (`wide`), or up to two one-side items. Ids come from
/// `Module::live_id` (music, timer...) and `Module::widgets` (battery...);
/// an empty string is an empty side.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NotchRow {
    pub wide: bool,
    pub left: String,
    pub right: String,
}

impl NotchRow {
    pub fn wide(id: &str) -> NotchRow {
        NotchRow { wide: true, left: id.into(), right: String::new() }
    }
    pub fn pair(l: &str, r: &str) -> NotchRow {
        NotchRow { wide: false, left: l.into(), right: r.into() }
    }
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        [&self.left, &self.right].into_iter().filter(|s| !s.is_empty())
    }
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.right.is_empty()
    }
}

/// When a widget shows on the closed notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowWhen {
    Always,
    /// Only while it has something worth showing (battery off the charger).
    Matters,
    /// For a few seconds after its value changes.
    Change,
}

impl ShowWhen {
    pub fn key(self) -> &'static str {
        match self {
            ShowWhen::Always => "a",
            ShowWhen::Matters => "m",
            ShowWhen::Change => "c",
        }
    }
    pub fn from_key(s: &str) -> Option<ShowWhen> {
        [ShowWhen::Always, ShowWhen::Matters, ShowWhen::Change].into_iter().find(|w| w.key() == s)
    }
    pub fn next(self) -> ShowWhen {
        match self {
            ShowWhen::Always => ShowWhen::Matters,
            ShowWhen::Matters => ShowWhen::Change,
            ShowWhen::Change => ShowWhen::Always,
        }
    }
}

/// How an alert (volume, Caps Lock, copied...) appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertMode {
    Off,
    /// A few seconds inside the closed notch, nothing opens.
    Small,
    /// The card that drops out of the notch.
    Popup,
}

impl AlertMode {
    pub const ALL: [AlertMode; 3] = [AlertMode::Off, AlertMode::Small, AlertMode::Popup];
    pub fn key(self) -> &'static str {
        match self {
            AlertMode::Off => "o",
            AlertMode::Small => "s",
            AlertMode::Popup => "p",
        }
    }
    pub fn from_key(s: &str) -> Option<AlertMode> {
        AlertMode::ALL.into_iter().find(|w| w.key() == s)
    }
}

/// The alerts that have a mode: (toast key, name).
pub const ALERTS: [(&str, &str); 8] = [
    ("volume", "Volume"),
    ("copy", "Copied"),
    ("power", "Battery"),
    ("net", "Network"),
    ("caps", "Caps Lock"),
    ("lang", "Language"),
    ("bt", "Bluetooth"),
    ("privacy", "Mic & camera"),
];

/// What the closed notch shows. Simple: `picked` is what is switched on and
/// WindyLand orders it. Custom (`custom`): `rows` is the order, top first;
/// the first row with something to show wins, and a one-side row with an
/// empty (or idle) side borrows the next one-side item below.
#[derive(Clone, Debug, PartialEq)]
pub struct NotchLayout {
    pub rows: Vec<NotchRow>,
    /// The rule for widgets without one of their own: Matters, else Always.
    pub smart: bool,
    pub custom: bool,
    /// Everything switched on, in the order it was switched on.
    pub picked: Vec<String>,
    pub when: BTreeMap<String, ShowWhen>,
    pub alerts: BTreeMap<String, AlertMode>,
}

const DEFAULT_PICKED: [&str; 8] = ["pomodoro", "timer", "prayer", "music", "agents", "due", "battery", "bluetooth"];

impl Default for NotchLayout {
    /// Today's order: timers above prayer above music above agents above
    /// due tasks, then the battery and a Bluetooth device's battery.
    fn default() -> NotchLayout {
        let mut rows: Vec<NotchRow> = DEFAULT_PICKED[..6].iter().map(|id| NotchRow::wide(id)).collect();
        rows.push(NotchRow::pair("battery", "bluetooth"));
        NotchLayout { rows, smart: true, custom: false, picked: DEFAULT_PICKED.iter().map(|s| s.to_string()).collect(), when: BTreeMap::new(), alerts: BTreeMap::new() }
    }
}

impl NotchLayout {
    /// For tests and renders: exactly these rows, as a custom order.
    pub fn custom_rows(rows: Vec<NotchRow>, smart: bool) -> NotchLayout {
        let picked = rows.iter().flat_map(|r| r.ids().cloned().collect::<Vec<_>>()).collect();
        NotchLayout { rows, smart, custom: true, picked, when: BTreeMap::new(), alerts: BTreeMap::new() }
    }

    /// Switched on.
    pub fn contains(&self, id: &str) -> bool {
        self.picked.iter().any(|x| x == id)
    }

    pub fn when_of(&self, id: &str) -> ShowWhen {
        self.when.get(id).copied().unwrap_or(if self.smart { ShowWhen::Matters } else { ShowWhen::Always })
    }

    /// Switch an item off (its row goes when it empties).
    pub fn remove(&mut self, id: &str) {
        self.picked.retain(|x| x != id);
        for r in &mut self.rows {
            if r.left == id {
                r.left.clear();
            }
            if r.right == id {
                r.right.clear();
            }
        }
        self.rows.retain(|r| !r.is_empty());
    }

    /// Switch an item on: last, as a one-side item (beside the last one
    /// when that row has room).
    pub fn add(&mut self, id: &str) {
        if !self.contains(id) {
            self.picked.push(id.into());
        }
        if self.rows.iter().any(|r| r.ids().any(|x| x == id)) {
            return;
        }
        match self.rows.last_mut() {
            Some(r) if !r.wide && r.right.is_empty() && !r.left.is_empty() => r.right = id.into(),
            _ => self.rows.push(NotchRow::pair(id, "")),
        }
    }

    /// After the rows were edited (drag, remove): `picked` follows them.
    pub fn sync_picked(&mut self) {
        let in_rows: Vec<String> = self.rows.iter().flat_map(|r| r.ids().cloned().collect::<Vec<_>>()).collect();
        self.picked.retain(|p| in_rows.contains(p));
        for id in in_rows {
            if !self.picked.contains(&id) {
                self.picked.push(id);
            }
        }
    }

    /// `*music;battery,bluetooth;cpu,` (wide rows start with *).
    pub fn to_text(&self) -> String {
        self.rows
            .iter()
            .map(|r| if r.wide { format!("*{}", r.left) } else { format!("{},{}", r.left, r.right) })
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn from_text(s: &str) -> Vec<NotchRow> {
        let clean = |x: &str| x.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>();
        s.split(';')
            .filter_map(|part| {
                let row = match part.trim().strip_prefix('*') {
                    Some(id) => NotchRow::wide(&clean(id)),
                    None => {
                        let (l, r) = part.split_once(',').unwrap_or((part, ""));
                        NotchRow::pair(&clean(l), &clean(r))
                    }
                };
                (!row.is_empty()).then_some(row)
            })
            .collect()
    }

    fn load(kv: &BTreeMap<String, String>) -> NotchLayout {
        let mut l = NotchLayout::default();
        l.smart = kv.get("widgets_smart").map_or(true, |v| v == "1");
        if let Some(t) = kv.get("notch_layout") {
            let rows = NotchLayout::from_text(t);
            // Saved by a build without the simple mode: an edited order
            // stays a custom order.
            l.custom = kv.get("notch_custom").map_or(rows != l.rows, |v| v == "1");
            l.rows = rows;
            l.picked = match kv.get("notch_picked") {
                Some(p) => p.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                None => l.rows.iter().flat_map(|r| r.ids().cloned().collect::<Vec<_>>()).collect(),
            };
        } else if kv.get("monitor_live").map_or(false, |v| v == "1") {
            // The monitor's old "Show on closed notch" switch.
            l.add("cpu");
            l.add("net");
        }
        let pairs = |k: &str| -> Vec<(String, String)> {
            kv.get(k).map_or(Vec::new(), |v| v.split(',').filter_map(|p| p.split_once(':')).map(|(a, b)| (a.to_string(), b.to_string())).collect())
        };
        l.when = pairs("notch_when").into_iter().filter_map(|(a, b)| Some((a, ShowWhen::from_key(&b)?))).collect();
        l.alerts = pairs("notch_alerts").into_iter().filter_map(|(a, b)| Some((a, AlertMode::from_key(&b)?))).collect();
        l
    }

    fn save(&self, kv: &mut BTreeMap<String, String>) {
        kv.insert("notch_layout".into(), self.to_text());
        kv.insert("notch_custom".into(), (self.custom as u8).to_string());
        kv.insert("notch_picked".into(), self.picked.join(","));
        kv.insert("widgets_smart".into(), (self.smart as u8).to_string());
        kv.insert("notch_when".into(), self.when.iter().map(|(a, b)| format!("{a}:{}", b.key())).collect::<Vec<_>>().join(","));
        kv.insert("notch_alerts".into(), self.alerts.iter().map(|(a, b)| format!("{a}:{}", b.key())).collect::<Vec<_>>().join(","));
        kv.remove("monitor_live");
    }
}

/// Where you are, for the weather (and prayer times, if you choose).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceMode {
    /// The main city of Windows' time zone (offline, no permission).
    Auto,
    /// Windows' location service.
    Windows,
    /// A city you picked.
    Manual,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub mode: PlaceMode,
    /// Manual: the city. Windows: the last position it gave.
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    /// Prayer times use this place instead of their own city.
    pub prayer: bool,
    /// Weather in °F (default °C).
    pub fahrenheit: bool,
}

impl Place {
    fn load(kv: &BTreeMap<String, String>) -> Place {
        let f = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        Place {
            mode: match kv.get("place_mode").map(String::as_str) {
                Some("windows") => PlaceMode::Windows,
                Some("manual") => PlaceMode::Manual,
                _ => PlaceMode::Auto,
            },
            lat: f("place_lat").clamp(-90.0, 90.0),
            lon: f("place_lon").clamp(-180.0, 180.0),
            name: kv.get("place_name").cloned().unwrap_or_default(),
            prayer: kv.get("place_prayer").map_or(false, |v| v == "1"),
            fahrenheit: kv.get("weather_f").map_or(false, |v| v == "1"),
        }
    }

    fn save(&self, kv: &mut BTreeMap<String, String>) {
        let mode = match self.mode {
            PlaceMode::Auto => "auto",
            PlaceMode::Windows => "windows",
            PlaceMode::Manual => "manual",
        };
        kv.insert("place_mode".into(), mode.into());
        kv.insert("place_lat".into(), format!("{:.4}", self.lat));
        kv.insert("place_lon".into(), format!("{:.4}", self.lon));
        kv.insert("place_name".into(), self.name.replace(['\n', '\r'], " "));
        kv.insert("place_prayer".into(), (self.prayer as u8).to_string());
        kv.insert("weather_f".into(), (self.fahrenheit as u8).to_string());
    }
}

impl Settings {
    /// The panel with this title was turned off in Settings > Panels.
    /// How an alert shows. Volume and Copied also have their own switch
    /// (Settings): off there is Off here.
    pub fn alert_mode(&self, key: &str) -> AlertMode {
        let m = self.layout.alerts.get(key).copied().unwrap_or(AlertMode::Popup);
        let switch = match key {
            "volume" => Some(self.volume_overlay),
            "copy" => Some(self.copy_toast),
            _ => None,
        };
        match switch {
            Some(false) => AlertMode::Off,
            // Switched back on in Settings: a pop-up again.
            Some(true) if m == AlertMode::Off => AlertMode::Popup,
            _ => m,
        }
    }

    pub fn set_alert_mode(&mut self, key: &str, mode: AlertMode) {
        match key {
            "volume" => self.volume_overlay = mode != AlertMode::Off,
            "copy" => self.copy_toast = mode != AlertMode::Off,
            _ => {}
        }
        self.layout.alerts.insert(key.to_string(), mode);
    }

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
            update_check: kv.get("update_check").map_or(true, |v| v != "0"),
            last_tab: kv.get("last_tab").cloned().unwrap_or_default(),
            hidden_panels: kv.get("hidden_panels").map_or(Vec::new(), |v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect()),
            notif_on: kv.get("notif_on").map_or(false, |v| v == "1"),
            notif_text: kv.get("notif_text").map_or(true, |v| v == "1"),
            clock12: kv.get("clock12").map_or(true, |v| v != "0"),
            copy_toast: kv.get("copy_toast").map_or(false, |v| v == "1"),
            make_room: kv.get("make_room").map_or(true, |v| v != "0"),
            layout: NotchLayout::load(&kv),
            place: Place::load(&kv),
            seconds: kv.get("seconds").map_or(false, |v| v == "1"),
            theme: kv.get("theme").cloned().unwrap_or_else(|| "default".into()),
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
        kv.insert("update_check".into(), (self.update_check as u8).to_string());
        if !self.last_tab.is_empty() {
            kv.insert("last_tab".into(), self.last_tab.clone());
        }
        kv.insert("notif_on".into(), (self.notif_on as u8).to_string());
        kv.insert("notif_text".into(), (self.notif_text as u8).to_string());
        kv.insert("clock12".into(), (self.clock12 as u8).to_string());
        kv.insert("copy_toast".into(), (self.copy_toast as u8).to_string());
        kv.insert("make_room".into(), (self.make_room as u8).to_string());
        self.layout.save(&mut kv);
        self.place.save(&mut kv);
        kv.insert("seconds".into(), (self.seconds as u8).to_string());
        kv.insert("theme".into(), self.theme.clone());
        write("settings.txt", &write_kv(&kv));
    }
}
