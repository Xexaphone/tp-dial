//! Minimal Hyprland IPC over its request socket (no hyprctl process spawn).

use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn request(cmd: &str) -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let mut s = UnixStream::connect(format!("{dir}/hypr/{sig}/.socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(250))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(250))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

#[derive(Deserialize)]
struct Pos {
    x: f64,
    y: f64,
}

/// Global cursor position in logical layout coordinates.
pub fn cursor() -> Option<(f64, f64)> {
    let p: Pos = serde_json::from_str(&request("j/cursorpos")?).ok()?;
    Some((p.x, p.y))
}

pub fn move_cursor(x: f64, y: f64) {
    let (x, y) = (x.round() as i64, y.round() as i64);
    // Lua-config Hyprland first; fall back to the classic dispatcher syntax.
    let lua = request(&format!("dispatch hl.dsp.cursor.move({{x={x}, y={y}}})"));
    if lua.as_deref().map(str::trim) != Some("ok") {
        request(&format!("dispatch movecursor {x} {y}"));
    }
}

/// Run a shell command line through Hyprland, so it gets the session's environment
/// and isn't a child of tp-dial. Returns false if Hyprland didn't accept it.
pub fn exec(cmd: &str) -> bool {
    // Lua long-bracket string: no escaping needed unless the command contains the closer.
    let lua_ok = !cmd.contains("]==]")
        && request(&format!("dispatch hl.dsp.exec_cmd([==[{cmd}]==])"))
            .is_some_and(|r| r.trim() == "ok");
    lua_ok
        || request(&format!("dispatch exec {cmd}")).is_some_and(|r| r.trim() == "ok")
}

#[derive(Deserialize)]
struct Monitor {
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    transform: u32,
}

/// The monitor containing (x, y), and the point relative to that monitor's top-left.
pub fn monitor_at(x: f64, y: f64) -> Option<(String, f64, f64)> {
    let mons: Vec<Monitor> = serde_json::from_str(&request("j/monitors")?).ok()?;
    mons.into_iter().find_map(|m| {
        let (mut w, mut h) = (m.width / m.scale, m.height / m.scale);
        if m.transform % 2 == 1 {
            std::mem::swap(&mut w, &mut h);
        }
        let (lx, ly) = (x - m.x, y - m.y);
        (lx >= 0.0 && ly >= 0.0 && lx < w && ly < h).then_some((m.name, lx, ly))
    })
}

/// tp-dial's own Hyprland rules, registered over IPC instead of living in
/// ~/.config/hypr (which `omarchy refresh hyprland` resets to defaults):
///  - the dial and add-item form appear without Hyprland's layer fade
///  - the settings window floats, centred, like Omarchy's utility windows
const RULES: [&str; 2] = [
    r#"hl.layer_rule({ match = { namespace = "^(tp-dial|tp-dial-form)$" }, no_anim = true, animation = "none" })"#,
    r#"o.window("dev\\.tpdial\\..*", { tag = "+floating-window" })"#,
];

pub fn apply_rules() {
    for r in RULES {
        if request(&format!("eval {r}")).map(|s| s.trim().to_string()).as_deref() != Some("ok") {
            eprintln!("hyprland rule not applied: {r}");
        }
    }
}

/// Re-apply the rules whenever Hyprland reloads its config (a reload drops
/// rules added at runtime). Runs for the life of the process.
pub fn keep_rules_applied() {
    apply_rules();
    std::thread::Builder::new()
        .name("hypr-events".into())
        .spawn(|| {
            use std::io::BufRead;
            let (Ok(dir), Ok(sig)) =
                (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE"))
            else {
                return;
            };
            let path = format!("{dir}/hypr/{sig}/.socket2.sock");
            loop {
                if let Ok(s) = UnixStream::connect(&path) {
                    for line in std::io::BufReader::new(s).lines().map_while(Result::ok) {
                        if line.starts_with("configreloaded>>") {
                            apply_rules();
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(2)); // Hyprland restarting; retry
            }
        })
        .expect("spawn hypr-events thread");
}
