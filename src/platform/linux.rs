//! Linux: any Wayland compositor with wlr-layer-shell (Hyprland, Sway, niri,
//! river, Wayfire...). Hyprland gets extras through its IPC socket: dial on the
//! monitor under the cursor, cursor restore, launching via Hyprland, and window
//! rules. Elsewhere the compositor picks the output (usually the focused one) and
//! items are started directly. Omarchy is detected for its theme and defaults.

use super::hypr;
use crate::config::{Launch, Position};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the overlay should appear.
#[derive(Clone, Debug, Default)]
pub struct Placement {
    /// Output connector name (e.g. "eDP-1"); None = let the compositor choose.
    pub monitor: Option<String>,
    /// Centre in that monitor's logical coordinates, for `position = "cursor"`.
    pub center: Option<(f64, f64)>,
}

fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn sway() -> bool {
    std::env::var_os("SWAYSOCK").is_some()
}

/// Is `cmd` an executable on PATH?
pub fn have(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn omarchy_colors() -> PathBuf {
    home().join(".local/state/omarchy/current/theme/colors.toml")
}

pub fn is_omarchy() -> bool {
    omarchy_colors().exists() || Path::new("/usr/share/omarchy").is_dir()
}

/// Global cursor position (only known on Hyprland).
pub fn cursor() -> Option<(f64, f64)> {
    if hyprland() { hypr::cursor() } else { None }
}

pub fn placement(position: Position, cursor: Option<(f64, f64)>) -> Placement {
    match cursor.and_then(|(x, y)| hypr::monitor_at(x, y)) {
        Some((name, lx, ly)) => Placement {
            monitor: Some(name),
            center: (position == Position::Cursor).then_some((lx, ly)),
        },
        // Unknown cursor: centred on whichever output the compositor gives us.
        None => Placement::default(),
    }
}

pub fn restore_cursor(x: f64, y: f64) {
    if hyprland() {
        hypr::move_cursor(x, y);
    }
}

pub fn keep_rules_applied() {
    if hyprland() {
        hypr::keep_rules_applied();
    } else {
        apply_rules();
    }
}

/// Make the settings window float (the dial itself is a layer surface and
/// needs no rules outside Hyprland).
pub fn apply_rules() {
    if hyprland() {
        hypr::apply_rules();
    } else if sway() && have("swaymsg") {
        let _ = Command::new("swaymsg")
            .arg(r#"for_window [app_id="^dev\.tpdial\."] floating enable"#)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Shell command line for an item.
fn command_line(l: &Launch) -> Option<String> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    Some(match l {
        Launch::Shell(s) => s.clone(),
        Launch::Exec(p) => quote(&p.display().to_string()),
        Launch::App(a) => {
            if have("uwsm-app") {
                // uwsm sessions (Omarchy and others): start it as its own scope.
                format!("uwsm-app -- {a}")
            } else if a.ends_with(".desktop") && have("gtk-launch") {
                format!("gtk-launch {}", quote(a))
            } else {
                a.clone()
            }
        }
        Launch::Builtin(_) => return None,
    })
}

pub fn launch(l: &Launch) {
    let Some(line) = command_line(l) else { return };
    // On Hyprland, start it through the compositor so it gets the session's
    // environment and isn't our child.
    if hyprland() && hypr::exec(&line) {
        return;
    }
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(&line);
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::null()).process_group(0);
    // Don't pass tp-dial's own "no GPU" GTK settings on to the apps it launches.
    if std::env::var_os("TP_DIAL_GPU").is_none() {
        cmd.env_remove("GSK_RENDERER").env_remove("GDK_DISABLE");
    }
    match cmd.spawn() {
        // Reap in the background so finished launches don't linger as zombies.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("launch failed: {e}"),
    }
}

/// The desktop theme's palette as a TOML table with `accent`, `background`,
/// `foreground` keys (Omarchy's colors.toml already has exactly that shape).
pub fn theme_palette() -> Option<toml::Table> {
    std::fs::read_to_string(omarchy_colors()).ok()?.parse().ok()
}

/// fontconfig's "monospace" is the user's chosen monospace font (on Omarchy,
/// the one `omarchy font set` picks).
pub fn default_font() -> &'static str {
    "monospace"
}

pub fn default_items() -> &'static str {
    if is_omarchy() { crate::config::OMARCHY_ITEMS } else { crate::config::LINUX_ITEMS }
}

pub fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Process setup before anything else runs (nothing needed on Linux).
pub fn init() {}

/// Errors already go to stderr / the journal on Linux.
pub fn report_error(_msg: &str) {}
