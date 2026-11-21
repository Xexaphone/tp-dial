use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::gesture::Params;

/// Settings part of the default config; items are appended per platform.
macro_rules! settings_text {
    () => {
        r##"# tp-dial config. Re-read automatically the next time you touch the TrackPoint.
#
# mode = "any":   any real TrackPoint push opens the dial instantly. The TrackPoint
#                 no longer moves the cursor (use the touchpad); its buttons still
#                 click, and moving while a button is held still drags / scrolls.
# mode = "flick": the TrackPoint moves the cursor as normal; a sharp push held for
#                 `hold_ms` opens the dial (settings below).
# Either way: push towards an item to highlight it, let go to run it. Pick
# "Close" or click any button to cancel. `tp-dial --debug` shows live speeds.

[trigger]
mode = "any"
min_push = 10        # "any": push strength that opens the dial (resting drift is ~1-7)

# "flick" mode only:
trigger_speed = 40   # counts per 50ms a push must reach...
rise_ms = 80         # ...within this long of the stick starting to move (sharp push, not a cursor drift)
hold_speed = 20      # then keep at least this speed...
hold_ms = 50         # ...for this long before the dial opens (accident guard; raise if it misfires)

[dial]
position = "center"  # "center" of the screen the cursor is on, or "cursor"
size = 340           # diameter in logical px
min_steer = 12       # push strength that changes the highlighted item (weaker = keep it)
release_ms = 90      # no motion this long = you let go
restore_cursor = true # put the cursor back where the flick started
# Colours: "theme" follows your desktop's colours (Omarchy theme on Omarchy, accent
# and light/dark mode on Windows; re-read every time the dial opens), or a hex colour.
# Add an alpha byte (e.g. "#16161ee6") for see-through.
accent = "theme"
background = "theme"
foreground = "theme"
font = "default"     # "default" = your monospace font on Linux (Omarchy's font there), Segoe UI on Windows
font_size = 15       # largest label size; long labels shrink, then get "…", to fit their slice

selected_text = "theme"   # label colour on the highlighted slice
center_color = "theme"    # fill of the centre circle (shows behind a transparent image)
border = "none"           # outline round each slice: "none", "theme" (accent) or a colour
border_width = 2
opacity = 1.0             # whole dial, 0.0 (invisible) - 1.0 (solid)

center_size = 0.3         # centre circle radius as a fraction of the dial radius (0.1-0.6)
center_image = ""         # PNG / JPG / SVG / animated GIF shown in the centre, e.g. "~/Pictures/cat.gif"
center_image_opacity = 1.0

animation = "pop"         # how the dial appears: "none", "fade", "pop", "spin" or "unfold"
animation_ms = 140

# Items go clockwise starting at the top. Each needs a `label` and exactly one of:
#   app     = "firefox"            an app: a command or .desktop id on Linux (started with
#                                  uwsm-app / gtk-launch when available); on Windows a program
#                                  name, path, or URI like "ms-settings:" (run with `start`)
#   script  = "~/bin/thing.sh"     executable file, run directly (.ps1/.bat/.cmd on Windows)
#   command = "notify-send hi"     any shell command line (sh on Linux, cmd on Windows)
#   action  = "add-item"           built-in: open a form that adds an item to this file
#   action  = "settings"           built-in: open the appearance / trigger settings window
#   action  = "close"              built-in: just close the dial
#   action  = "quit"               built-in: exit tp-dial (gives the TrackPoint back to the cursor)
# Items added from the dial's form are inserted before the first built-in item.

"##
    };
}

macro_rules! builtin_items {
    () => {
        r##"[[item]]
label = "Settings"
action = "settings"

[[item]]
label = "+ Add item"
action = "add-item"

[[item]]
label = "Close"
action = "close"
"##
    };
}

/// Omarchy: its launchers and menus.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const OMARCHY_ITEMS: &str = concat!(
    r##"[[item]]
label = "Terminal"
app = "xdg-terminal-exec"

[[item]]
label = "Browser"
command = "omarchy-launch-browser"

[[item]]
label = "Files"
app = "nautilus --new-window"

[[item]]
label = "Screenshot"
command = "omarchy-capture-screenshot"

[[item]]
label = "Menu"
command = "omarchy-menu"

"##,
    builtin_items!()
);

/// Any other Linux desktop: freedesktop defaults.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const LINUX_ITEMS: &str = concat!(
    r##"[[item]]
label = "Terminal"
command = "for t in xdg-terminal-exec x-terminal-emulator foot kitty alacritty wezterm ghostty konsole gnome-terminal; do command -v $t >/dev/null && exec $t; done"

[[item]]
label = "Browser"
command = "b=$(xdg-settings get default-web-browser 2>/dev/null) && [ -n \"$b\" ] && exec gtk-launch \"$b\"; exec xdg-open https://"

[[item]]
label = "Files"
command = "xdg-open \"$HOME\""

"##,
    builtin_items!()
);

/// Windows 11.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOWS_ITEMS: &str = concat!(
    r##"[[item]]
label = "Terminal"
app = "wt"

[[item]]
label = "Files"
app = "explorer"

[[item]]
label = "Snip"
app = "ms-screenclip:"

[[item]]
label = "Task Manager"
app = "taskmgr"

"##,
    builtin_items!()
);

/// The default config for the platform this is running on.
pub fn default_config() -> String {
    format!("{}{}", settings_text!(), crate::platform::default_items())
}

/// Fixed default (Omarchy flavour) so tests don't depend on the machine.
#[cfg(test)]
pub const DEFAULT_CONFIG: &str = concat!(settings_text!(), "[[item]]\nlabel = \"Terminal\"\napp = \"xdg-terminal-exec\"\n\n[[item]]\nlabel = \"Browser\"\ncommand = \"omarchy-launch-browser\"\n\n[[item]]\nlabel = \"Files\"\napp = \"nautilus --new-window\"\n\n[[item]]\nlabel = \"Screenshot\"\ncommand = \"omarchy-capture-screenshot\"\n\n[[item]]\nlabel = \"Menu\"\ncommand = \"omarchy-menu\"\n\n", builtin_items!());


#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub trigger: Trigger,
    pub dial: Dial,
    #[serde(default, rename = "item")]
    pub items: Vec<Item>,
    #[serde(default)]
    pub settings_window: SettingsWindow,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SettingsWindow {
    /// Background opacity of the settings window, 0.0-1.0.
    #[serde(default = "default_window_opacity")]
    pub opacity: f64,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self { opacity: default_window_opacity() }
    }
}

fn default_window_opacity() -> f64 {
    1.0
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(default = "default_mode")]
    pub mode: TriggerMode,
    /// Which pointing device is the TrackPoint, if auto-detection picks the wrong
    /// one: Linux /dev/input/eventN or part of its name; Windows part of its
    /// device path (`tp-dial --find-device` fills this in).
    #[serde(default)]
    pub device: String,
    #[serde(default = "default_min_push")]
    pub min_push: f64,
    pub trigger_speed: f64,
    pub rise_ms: u64,
    pub hold_speed: f64,
    pub hold_ms: u64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Dial {
    #[serde(default)]
    pub position: Position,
    pub size: f64,
    #[serde(default = "default_min_steer")]
    pub min_steer: f64,
    /// Unused since direct steering; still accepted so older configs load.
    #[serde(default)]
    #[allow(dead_code)]
    pub radius: f64,
    #[serde(default)]
    #[allow(dead_code)]
    pub deadzone: f64,
    pub release_ms: u64,
    pub restore_cursor: bool,
    pub accent: String,
    pub background: String,
    pub foreground: String,
    #[serde(default = "default_font")]
    pub font: String,
    #[serde(default = "default_font_size")]
    pub font_size: f64,

    // Appearance (all optional, so older configs keep loading).
    #[serde(default = "theme")]
    pub selected_text: String,
    #[serde(default = "theme")]
    pub center_color: String,
    #[serde(default = "none")]
    pub border: String,
    #[serde(default = "default_border_width")]
    pub border_width: f64,
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default = "default_center_size")]
    pub center_size: f64,
    #[serde(default)]
    pub center_image: String,
    #[serde(default = "one")]
    pub center_image_opacity: f64,
    #[serde(default)]
    pub animation: Animation,
    #[serde(default = "default_animation_ms")]
    pub animation_ms: u64,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Animation {
    None,
    Fade,
    #[default]
    Pop,
    Spin,
    Unfold,
}

impl Animation {
    pub const ALL: [Animation; 5] =
        [Animation::None, Animation::Fade, Animation::Pop, Animation::Spin, Animation::Unfold];
    pub fn name(self) -> &'static str {
        match self {
            Animation::None => "none",
            Animation::Fade => "fade",
            Animation::Pop => "pop",
            Animation::Spin => "spin",
            Animation::Unfold => "unfold",
        }
    }
}

fn theme() -> String {
    "theme".into()
}
fn none() -> String {
    "none".into()
}
fn one() -> f64 {
    1.0
}
fn default_font_size() -> f64 {
    15.0
}
fn default_border_width() -> f64 {
    2.0
}
fn default_center_size() -> f64 {
    0.3
}
fn default_animation_ms() -> u64 {
    140
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Any,
    Flick,
}

fn default_mode() -> TriggerMode {
    TriggerMode::Any
}

fn default_font() -> String {
    "default".into()
}

fn default_min_steer() -> f64 {
    12.0
}

fn default_min_push() -> f64 {
    10.0
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Center,
    Cursor,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Builtin {
    AddItem,
    Settings,
    Close,
    Quit,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub label: String,
    pub app: Option<String>,
    pub script: Option<String>,
    pub command: Option<String>,
    pub action: Option<Builtin>,
}

pub enum Launch {
    /// A shell command line (sh on Linux, cmd on Windows).
    Shell(String),
    /// An executable / script file.
    Exec(PathBuf),
    /// An app; each platform decides how to start it (uwsm-app, gtk-launch, `start`).
    App(String),
    Builtin(Builtin),
}

impl std::fmt::Display for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Launch::Shell(s) => write!(f, "{s}"),
            Launch::Exec(p) => write!(f, "{}", p.display()),
            Launch::App(a) => write!(f, "app: {a}"),
            Launch::Builtin(b) => write!(f, "built-in: {b:?}"),
        }
    }
}

impl Item {
    pub fn launch(&self) -> Launch {
        match (&self.app, &self.script, &self.command, self.action) {
            (Some(a), ..) => Launch::App(a.clone()),
            (_, Some(s), ..) => Launch::Exec(expand_tilde(s)),
            (_, _, Some(c), _) => Launch::Shell(c.clone()),
            (.., Some(b)) => Launch::Builtin(b),
            _ => unreachable!("validated on load"),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".config"))
        };
        base.join("tp-dial").join("config.toml")
    }

    /// Load, writing the default file first if none exists.
    pub fn load() -> Result<(Self, Option<SystemTime>), String> {
        let path = Self::path();
        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&path, default_config()).map_err(|e| e.to_string())?;
            eprintln!("wrote default config to {}", path.display());
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let cfg = Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((cfg, mtime()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        for (i, it) in cfg.items.iter().enumerate() {
            let n = [&it.app, &it.script, &it.command].iter().filter(|x| x.is_some()).count()
                + it.action.is_some() as usize;
            if n != 1 {
                return Err(format!(
                    "item {} ({:?}) needs exactly one of app / script / command / action",
                    i + 1,
                    it.label
                ));
            }
        }
        if cfg.items.len() > MAX_ITEMS {
            return Err(format!("at most {MAX_ITEMS} items"));
        }
        Ok(cfg)
    }

    pub fn params(&self) -> Params {
        Params {
            mode: match self.trigger.mode {
                TriggerMode::Any => crate::gesture::Mode::Any,
                TriggerMode::Flick => crate::gesture::Mode::Flick,
            },
            min_push: self.trigger.min_push,
            speed_window: Duration::from_millis(50),
            burst_gap: Duration::from_millis(80),
            trigger_speed: self.trigger.trigger_speed,
            rise: Duration::from_millis(self.trigger.rise_ms),
            hold_speed: self.trigger.hold_speed,
            hold: Duration::from_millis(self.trigger.hold_ms),
            min_steer: self.dial.min_steer,
            release: Duration::from_millis(self.dial.release_ms),
            slices: self.items.len(),
        }
    }
}

pub const MAX_ITEMS: usize = 12;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    App,
    Script,
    Command,
}

/// Insert a new `[[item]]` into the config text before the first built-in item
/// (so Add/Quit stay last), keeping all comments. Fails without touching the
/// file if the result wouldn't load.
pub fn insert_item(text: &str, kind: Kind, label: &str, value: &str) -> Result<String, String> {
    let (label, value) = (label.trim(), value.trim());
    if label.is_empty() || value.is_empty() {
        return Err("label and value are both required".into());
    }
    let key = match kind {
        Kind::App => "app",
        Kind::Script => "script",
        Kind::Command => "command",
    };
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let block = format!("[[item]]\nlabel = {}\n{key} = {}\n\n", q(label), q(value));

    let lines: Vec<&str> = text.lines().collect();
    let first_builtin = lines
        .iter()
        .position(|l| l.trim_start().starts_with("action") && l.contains('='));
    let at = first_builtin
        .and_then(|i| (0..=i).rev().find(|&j| lines[j].trim() == "[[item]]"))
        .map(|j| lines[..j].iter().map(|l| l.len() + 1).sum::<usize>().min(text.len()));
    let out = match at {
        Some(off) => format!("{}{block}{}", &text[..off], &text[off..]),
        None => format!("{}\n\n{}", text.trim_end(), block.trim_end()) + "\n",
    };
    Config::parse(&out)?;
    Ok(out)
}

/// insert_item + atomic write to the real config file.
pub fn add_item_to_file(kind: Kind, label: &str, value: &str) -> Result<(), String> {
    let path = Config::path();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let out = insert_item(&text, kind, label, value)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

impl Dial {
    /// Copy with any "theme" colours replaced by the desktop's (Omarchy theme,
    /// Windows accent), falling back to a neutral dark palette; "default" font
    /// replaced by the platform's.
    pub fn resolved(&self) -> Dial {
        let theme = crate::platform::theme_palette();
        Dial {
            accent: resolve(&self.accent, theme.as_ref(), "accent", "#7aa2f7"),
            background: resolve(&self.background, theme.as_ref(), "background", "#16161e"),
            foreground: resolve(&self.foreground, theme.as_ref(), "foreground", "#c0caf5"),
            selected_text: resolve(&self.selected_text, theme.as_ref(), "background", "#16161e"),
            center_color: resolve(&self.center_color, theme.as_ref(), "background", "#16161e"),
            border: resolve(&self.border, theme.as_ref(), "accent", "#7aa2f7"),
            font: if self.font.eq_ignore_ascii_case("default") {
                crate::platform::default_font().to_string()
            } else {
                self.font.clone()
            },
            opacity: self.opacity.clamp(0.0, 1.0),
            center_image_opacity: self.center_image_opacity.clamp(0.0, 1.0),
            center_size: self.center_size.clamp(0.1, 0.6),
            center_image: if self.center_image.is_empty() {
                String::new()
            } else {
                expand_tilde(&self.center_image).display().to_string()
            },
            ..self.clone()
        }
    }
}

fn resolve(value: &str, theme: Option<&toml::Table>, key: &str, fallback: &str) -> String {
    if !value.eq_ignore_ascii_case("theme") {
        return value.to_string();
    }
    theme
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with('#'))
        .unwrap_or(fallback)
        .to_string()
}

/// Write the settings window's values into the config file, keeping every
/// comment and all other content. Fails without touching the file if the result
/// wouldn't load.
pub fn save_settings(
    text: &str,
    dial: &Dial,
    mode: TriggerMode,
    min_push: f64,
    window_opacity: f64,
) -> Result<String, String> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;

    // Replace a value but keep its trailing "# comment"; append the key if new.
    fn put(t: &mut Table, key: &str, v: Item) {
        let mut v = v;
        if let (Some(old), Some(new)) = (t.get(key).and_then(Item::as_value), v.as_value_mut()) {
            *new.decor_mut() = old.decor().clone();
        }
        t.insert(key, v);
    }
    let round = |x: f64| (x * 100.0).round() / 100.0;

    let dial_t = doc
        .entry("dial")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("[dial] is not a table")?;
    for (k, v) in [
        ("accent", &dial.accent),
        ("background", &dial.background),
        ("foreground", &dial.foreground),
        ("selected_text", &dial.selected_text),
        ("center_color", &dial.center_color),
        ("border", &dial.border),
        ("font", &dial.font),
        ("center_image", &dial.center_image),
    ] {
        put(dial_t, k, value(v.as_str()));
    }
    for (k, v) in [
        ("border_width", round(dial.border_width)),
        ("opacity", round(dial.opacity)),
        ("size", dial.size.round()),
        ("center_size", round(dial.center_size)),
        ("font_size", round(dial.font_size)),
        ("center_image_opacity", round(dial.center_image_opacity)),
    ] {
        put(dial_t, k, value(v));
    }
    put(dial_t, "animation", value(dial.animation.name()));
    put(dial_t, "animation_ms", value(dial.animation_ms as i64));

    let trig = doc
        .entry("trigger")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("[trigger] is not a table")?;
    put(trig, "mode", value(match mode {
        TriggerMode::Any => "any",
        TriggerMode::Flick => "flick",
    }));
    put(trig, "min_push", value(round(min_push)));

    let win_t = doc
        .entry("settings_window")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("[settings_window] is not a table")?;
    put(win_t, "opacity", value(round(window_opacity.clamp(0.2, 1.0))));

    let out = doc.to_string();
    Config::parse(&out)?;
    Ok(out)
}

/// Delete `[[item]]` entries by (index, label) as they were when the settings
/// window opened. The label must still match, so a config edited in the meantime
/// can't lose the wrong item; the Settings built-in can't be removed (it's the
/// way back into this window). Comments elsewhere are kept.
pub fn remove_items(text: &str, removed: &[(usize, String)]) -> Result<String, String> {
    use toml_edit::{DocumentMut, Item};
    if removed.is_empty() {
        return Ok(text.to_string());
    }
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let items = doc
        .get_mut("item")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or("the config has no [[item]] entries")?;
    let mut order: Vec<&(usize, String)> = removed.iter().collect();
    order.sort_by(|a, b| b.0.cmp(&a.0)); // highest index first, so earlier ones don't shift
    order.dedup_by_key(|r| r.0);
    for (i, label) in order {
        let t = items
            .get(*i)
            .ok_or_else(|| format!("item {} no longer exists; reopen settings", i + 1))?;
        if t.get("label").and_then(|v| v.as_str()) != Some(label.as_str()) {
            return Err("the config changed since settings opened; reopen settings and try again".into());
        }
        if t.get("action").and_then(|v| v.as_str()) == Some("settings") {
            return Err("the Settings item can't be removed (it's the way back here)".into());
        }
        items.remove(*i);
    }
    let out = doc.to_string();
    Config::parse(&out)?;
    Ok(out)
}

pub fn save_settings_to_file(
    dial: &Dial,
    mode: TriggerMode,
    min_push: f64,
    window_opacity: f64,
    removed: &[(usize, String)],
) -> Result<(), String> {
    let path = Config::path();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let out = remove_items(&save_settings(&text, dial, mode, min_push, window_opacity)?, removed)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Set `[trigger] device`, keeping the rest of the file as it is.
pub fn set_device(text: &str, device: &str) -> Result<String, String> {
    use toml_edit::{value, DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let t = doc
        .entry("trigger")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("[trigger] is not a table")?;
    t.insert("device", value(device));
    let out = doc.to_string();
    Config::parse(&out)?;
    Ok(out)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn set_device_in_file(device: &str) -> Result<(), String> {
    Config::load()?; // creates the file if needed
    let path = Config::path();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let out = set_device(&text, device)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(Config::path()).and_then(|m| m.modified()).ok()
}

fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).unwrap_or_default()
}

fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        Some(rest) => home().join(rest),
        None => PathBuf::from(s),
    }
}

/// "#rrggbb" or "#rrggbbaa" -> (r, g, b, a) in 0..1
pub fn rgba(hex: &str) -> (f64, f64, f64, f64) {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .map(|b| b as f64 / 255.0)
    };
    match (byte(0), byte(2), byte(4)) {
        (Some(r), Some(g), Some(b)) => (r, g, b, byte(6).unwrap_or(1.0)),
        _ => (1.0, 0.0, 1.0, 1.0), // loud magenta = bad colour string
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.items.len(), 8);
        assert_eq!(c.params().slices, 8);
        assert_eq!(c.dial.position, Position::Center);
        assert_eq!(c.items[7].action, Some(Builtin::Close));
        assert_eq!(c.trigger.mode, TriggerMode::Any);
    }

    #[test]
    fn item_needs_exactly_one_action() {
        let bad = DEFAULT_CONFIG.to_string() + "\n[[item]]\nlabel = \"x\"\n";
        assert!(Config::parse(&bad).unwrap_err().contains("exactly one"));
        let two = DEFAULT_CONFIG.to_string() + "\n[[item]]\nlabel = \"x\"\napp=\"a\"\ncommand=\"b\"\n";
        assert!(Config::parse(&two).is_err());
    }

    #[test]
    fn insert_goes_before_builtins_and_escapes() {
        let out = insert_item(DEFAULT_CONFIG, Kind::Command, "Say \"hi\"", "notify-send 'a\\b'").unwrap();
        let c = Config::parse(&out).unwrap();
        assert_eq!(c.items.len(), 9);
        assert_eq!(c.items[5].label, "Say \"hi\"");
        assert_eq!(c.items[5].command.as_deref(), Some("notify-send 'a\\b'"));
        assert_eq!(c.items[6].action, Some(Builtin::Settings));
        assert!(out.contains("# tp-dial config"), "comments preserved");
    }

    #[test]
    fn insert_appends_when_no_builtins_and_enforces_limit() {
        let base = "[trigger]\nmode=\"flick\"\ntrigger_speed=40\nrise_ms=80\nhold_speed=20\nhold_ms=50\n[dial]\nsize=1.0\nradius=1.0\ndeadzone=1.0\nrelease_ms=1\nrestore_cursor=true\naccent=\"#fff\"\nbackground=\"#000\"\nforeground=\"#fff\"\n";
        let mut t = base.to_string();
        for i in 0..MAX_ITEMS {
            t = insert_item(&t, Kind::App, &format!("a{i}"), "x").unwrap();
        }
        assert_eq!(Config::parse(&t).unwrap().items[0].label, "a0");
        assert!(insert_item(&t, Kind::App, "one too many", "x").is_err());
        assert!(insert_item(base, Kind::App, "  ", "x").is_err());
    }

    #[test]
    fn theme_colours_resolve_with_fallback() {
        let t: toml::Table = "accent = \"#ac637e\"\nbackground = \"#030000\"".parse().unwrap();
        assert_eq!(resolve("theme", Some(&t), "accent", "#000000"), "#ac637e");
        assert_eq!(resolve("THEME", Some(&t), "foreground", "#c0caf5"), "#c0caf5");
        assert_eq!(resolve("#123456", Some(&t), "accent", "#000000"), "#123456");
        assert_eq!(resolve("theme", None, "accent", "#7aa2f7"), "#7aa2f7");
    }

    #[test]
    fn save_settings_keeps_comments_and_items() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        let mut d = c.dial.clone();
        d.accent = "#ff0000cc".into();
        d.opacity = 0.756;
        d.animation = Animation::Spin;
        d.center_image = "~/Pictures/a \"b\".gif".into();
        let out = save_settings(DEFAULT_CONFIG, &d, TriggerMode::Flick, 14.0, 0.8).unwrap();
        let c2 = Config::parse(&out).unwrap();
        assert_eq!(c2.dial.accent, "#ff0000cc");
        assert_eq!(c2.dial.opacity, 0.76);
        assert_eq!(c2.dial.animation, Animation::Spin);
        assert_eq!(c2.dial.center_image, "~/Pictures/a \"b\".gif");
        assert_eq!(c2.trigger.mode, TriggerMode::Flick);
        assert_eq!(c2.settings_window.opacity, 0.8);
        assert_eq!(c.settings_window.opacity, 1.0, "missing table -> default");
        assert_eq!(c2.items.len(), c.items.len());
        assert!(out.contains("# tp-dial config"));
        // trailing comment on a replaced value survives
        assert!(out.lines().any(|l| l.starts_with("opacity = 0.76") && l.contains("# whole dial")), "{out}");
    }

    #[test]
    fn remove_items_by_index_and_label() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        // Files (index 2) and Menu (index 4)
        let out = remove_items(DEFAULT_CONFIG, &[(2, "Files".into()), (4, "Menu".into())]).unwrap();
        let labels: Vec<String> = Config::parse(&out).unwrap().items.into_iter().map(|i| i.label).collect();
        assert_eq!(labels.len(), c.items.len() - 2);
        assert!(!labels.contains(&"Files".to_string()) && !labels.contains(&"Menu".to_string()));
        assert!(labels.contains(&"Screenshot".to_string()));
        assert!(out.contains("# tp-dial config"), "comments kept");
    }

    #[test]
    fn remove_items_refuses_stale_or_settings() {
        assert!(remove_items(DEFAULT_CONFIG, &[(2, "Not Files".into())]).unwrap_err().contains("changed"));
        let settings = Config::parse(DEFAULT_CONFIG)
            .unwrap()
            .items
            .iter()
            .position(|i| i.action == Some(Builtin::Settings))
            .unwrap();
        assert!(remove_items(DEFAULT_CONFIG, &[(settings, "Settings".into())]).is_err());
        assert_eq!(remove_items(DEFAULT_CONFIG, &[]).unwrap(), DEFAULT_CONFIG);
    }

    #[test]
    fn every_platform_default_parses() {
        for items in [OMARCHY_ITEMS, LINUX_ITEMS, WINDOWS_ITEMS] {
            let c = Config::parse(&format!("{}{}", settings_text!(), items)).unwrap();
            assert!(c.items.iter().any(|i| i.action == Some(Builtin::Settings)));
            assert_eq!(c.dial.font, "default");
        }
        let linux = Config::parse(&format!("{}{}", settings_text!(), LINUX_ITEMS)).unwrap();
        let browser = linux.items.iter().find(|i| i.label == "Browser").unwrap();
        assert!(browser.command.as_deref().unwrap().contains(r#"gtk-launch "$b""#));
    }

    #[test]
    fn set_device_writes_windows_paths_safely() {
        let dev = r"\\?\ACPI#LEN0312#4&2f3c&0#{378de44c-56ef-11d1-bc8c-00a0c91405dd}";
        let out = set_device(DEFAULT_CONFIG, dev).unwrap();
        assert_eq!(Config::parse(&out).unwrap().trigger.device, dev);
        assert!(out.contains("# tp-dial config"));
    }

    #[test]
    fn colours() {
        assert_eq!(rgba("#ff0000"), (1.0, 0.0, 0.0, 1.0));
        assert_eq!(rgba("#00000080").3, 128.0 / 255.0);
    }
}
