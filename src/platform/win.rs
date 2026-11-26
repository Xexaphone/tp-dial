//! Windows 11: dial placement by monitor + DPI, launching through `start`/cmd,
//! theme colours from the Windows accent and light/dark mode, and autostart via
//! the per-user Run key.

use crate::config::{Launch, Position};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_SUCCESS, POINT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD,
};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, MessageBoxW, SetCursorPos, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
};

/// Where the overlay should appear, in physical screen pixels.
#[derive(Clone, Debug, Default)]
pub struct Placement {
    pub center: (i32, i32),
    /// Monitor DPI / 96.
    pub scale: f64,
    /// The monitor's work area (left, top, right, bottom), to keep the dial on it.
    pub work: (i32, i32, i32, i32),
}

static HAS_CONSOLE: AtomicBool = AtomicBool::new(false);

/// Per-monitor DPI awareness (so sizes and cursor coordinates are real pixels), and
/// reuse the console we were started from, if any, for CLI output.
pub fn init() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Output goes somewhere if we're in a console, or if the parent redirected
        // our output (a pipe / file, e.g. a script or CI) - then never pop dialogs.
        let attached = AttachConsole(ATTACH_PARENT_PROCESS).is_ok();
        let redirected = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        HAS_CONSOLE.store(attached || redirected, Ordering::Relaxed);
    }
}

/// Started without a console (autostart, double-click): show errors in a dialog.
pub fn report_error(msg: &str) {
    if !HAS_CONSOLE.load(Ordering::Relaxed) {
        unsafe {
            MessageBoxW(None, &HSTRING::from(msg), &HSTRING::from("tp-dial"), MB_OK | MB_ICONERROR);
        }
    }
}

/// Tell the user something: the console if we were run from one, else a dialog
/// (Start Menu shortcuts and the installer have no console). Blocks until the
/// dialog is closed.
pub fn tell(msg: &str) {
    if HAS_CONSOLE.load(Ordering::Relaxed) {
        println!("{msg}");
    } else {
        unsafe {
            MessageBoxW(None, &HSTRING::from(msg), &HSTRING::from("tp-dial"), MB_OK | MB_ICONINFORMATION);
        }
    }
}

pub fn cursor() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x as f64, p.y as f64))
}

pub fn placement(position: Position, cursor: Option<(f64, f64)>) -> Placement {
    let pt = cursor.map(|(x, y)| POINT { x: x as i32, y: y as i32 }).unwrap_or_default();
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let r = info.rcWork;
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let center = match (position, cursor) {
            (Position::Cursor, Some(_)) => (pt.x, pt.y),
            _ => ((r.left + r.right) / 2, (r.top + r.bottom) / 2),
        };
        Placement { center, scale: dx.max(96) as f64 / 96.0, work: (r.left, r.top, r.right, r.bottom) }
    }
}

pub fn restore_cursor(x: f64, y: f64) {
    let _ = unsafe { SetCursorPos(x as i32, y as i32) };
}

/// No compositor rules on Windows.
pub fn keep_rules_applied() {}
pub fn apply_rules() {}

pub fn launch(l: &Launch) {
    let mut cmd = Command::new("cmd");
    match l {
        // cmd's own quoting rules apply, so pass the line through untouched.
        Launch::Shell(s) => {
            cmd.raw_arg(format!("/D /C {s}"));
        }
        // `start` handles program names, paths, App Paths entries and URIs (ms-settings:).
        Launch::App(a) => {
            cmd.raw_arg(format!("/D /C start \"\" {a}"));
        }
        Launch::Exec(p) => match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("ps1") => {
                cmd = Command::new("powershell");
                cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(p);
            }
            Some("bat") | Some("cmd") => {
                cmd.raw_arg(format!("/D /C \"{}\"", p.display()));
            }
            _ => {
                cmd.raw_arg(format!("/D /C start \"\" \"{}\"", p.display()));
            }
        },
        Launch::Builtin(_) => return,
    }
    cmd.creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if std::env::var_os("TP_DIAL_GPU").is_none() {
        cmd.env_remove("GSK_RENDERER").env_remove("GDK_DISABLE");
    }
    match cmd.spawn() {
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("launch failed: {e}"),
    }
}

fn reg_dword(subkey: &str, name: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

/// Accent colour + light/dark app mode, as `accent` / `background` / `foreground`.
pub fn theme_palette() -> Option<toml::Table> {
    let mut t = toml::Table::new();
    if let Some(c) = reg_dword(r"Software\Microsoft\Windows\DWM", "AccentColor") {
        // Stored as 0xAABBGGRR.
        let (r, g, b) = (c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff);
        t.insert("accent".into(), format!("#{r:02x}{g:02x}{b:02x}").into());
    }
    let light = reg_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(1);
    let (bg, fg) = if light { ("#f3f3f3", "#1b1b1b") } else { ("#202020", "#ffffff") };
    t.insert("background".into(), bg.into());
    t.insert("foreground".into(), fg.into());
    Some(t)
}

pub fn default_font() -> &'static str {
    "Segoe UI"
}

pub fn default_items() -> &'static str {
    crate::config::WINDOWS_ITEMS
}

pub fn running_as_root() -> bool {
    false
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Start (or stop starting) tp-dial at sign-in, via HKCU\...\Run.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let (key, name) = (HSTRING::from(RUN_KEY), HSTRING::from("tp-dial"));
    let r = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                REG_SZ.0,
                Some(value.as_ptr() as *const _),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) }
    };
    if r != ERROR_SUCCESS {
        return Err(format!("registry update failed ({r:?})"));
    }
    tell(if on { "tp-dial will start when you sign in." } else { "tp-dial will no longer start when you sign in." });
    Ok(())
}
