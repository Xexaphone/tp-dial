//! Windows input.
//!
//! Windows has no way to grab one pointing device, so this combines two APIs on
//! one thread:
//!  - Raw Input (WM_INPUT, RIDEV_INPUTSINK) reports per-device motion/buttons. The
//!    TrackPoint's reports drive the gesture engine.
//!  - A low-level mouse hook (WH_MOUSE_LL) decides whether each cursor movement
//!    happens. The hook can't tell which device moved, so it blocks movement while
//!    the TrackPoint is the device currently reporting (within the last 50ms) and
//!    the engine is swallowing its motion, plus all movement / clicks while the
//!    dial is open. Best effort: the first report of a push can still nudge the
//!    cursor, and other devices are unaffected otherwise.
//! The hook only reads atomics, so it always answers instantly (Windows drops
//! hooks that stall).

use crate::config::TriggerMode;
use crate::gesture::{Action, Engine, Phase};
use crate::session::Session;
use crate::Opts;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE,
    RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDI_DEVICENAME, RID_INPUT,
    RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, RegisterClassW,
    SetTimer, SetWindowsHookExW, TranslateMessage, HWND_MESSAGE, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT,
    RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP,
    RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSW,
};

// Button codes as on Linux (BTN_LEFT/RIGHT/MIDDLE) so the engine is identical.
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

/// How recently the TrackPoint must have reported for the hook to treat a
/// cursor movement as the TrackPoint's.
const TP_ACTIVE_MS: u64 = 50;
const TIMER_ID: usize = 1;

// --- state the hook reads ---------------------------------------------------
static TP_DEVICE: AtomicIsize = AtomicIsize::new(0);
static DIAL_OPEN: AtomicBool = AtomicBool::new(false);
static SWALLOW_TP: AtomicBool = AtomicBool::new(false);
static LAST_WAS_TP: AtomicBool = AtomicBool::new(false);
static TP_LAST_MS: AtomicU64 = AtomicU64::new(0);
/// Buttons whose press was eaten while the dial was open: eat the release too.
static EATEN_DOWN: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

struct State {
    engine: Engine,
    session: Session,
    hwnd: HWND,
    any_mode: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Every mouse-class raw input device: (handle, device path).
fn mouse_devices() -> Vec<(HANDLE, String)> {
    unsafe {
        let size = size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut n = 0u32;
        GetRawInputDeviceList(None, &mut n, size);
        let mut list = vec![RAWINPUTDEVICELIST::default(); n as usize];
        let got = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut n, size);
        if got == u32::MAX {
            return vec![];
        }
        list.truncate(got as usize);
        list.into_iter()
            .filter(|d| d.dwType == RIM_TYPEMOUSE)
            .map(|d| (d.hDevice, device_name(d.hDevice)))
            .collect()
    }
}

fn device_name(h: HANDLE) -> String {
    unsafe {
        let mut len = 0u32; // in characters
        GetRawInputDeviceInfoW(Some(h), RIDI_DEVICENAME, None, &mut len);
        let mut buf = vec![0u16; len as usize + 1];
        GetRawInputDeviceInfoW(Some(h), RIDI_DEVICENAME, Some(buf.as_mut_ptr() as *mut _), &mut len);
        String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string()
    }
}

/// Pick the TrackPoint: the configured substring, else a ThinkPad pointing stick
/// on the PS/2 (ACPI) bus.
fn choose_device(wanted: Option<&str>) -> Result<(HANDLE, String), String> {
    let devs = mouse_devices();
    let lower = |s: &str| s.to_ascii_lowercase();
    if let Some(w) = wanted.filter(|w| !w.is_empty()) {
        return devs
            .into_iter()
            .find(|(_, n)| lower(n).contains(&lower(w)))
            .ok_or_else(|| format!("no pointing device matches {w:?}; run `tp-dial --find-device`"));
    }
    for hint in ["acpi#len", "acpi#ibm", "acpi#etd", "trackpoint"] {
        if let Some(d) = devs.iter().find(|(_, n)| lower(n).contains(hint)) {
            return Ok(d.clone());
        }
    }
    Err("tp-dial couldn't identify the TrackPoint automatically.\n\nRun \"Find my TrackPoint\" from the Start Menu \
         (or `tp-dial --find-device`) and push the TrackPoint when asked."
        .into())
}

pub fn start(opts: &Opts, session: Session) -> Result<(), String> {
    let (dev, name) = choose_device(opts.device.as_deref())?;
    eprintln!("trackpoint: {name}");
    TP_DEVICE.store(dev.0 as isize, Relaxed);
    let any_mode = session.cfg.trigger.mode == TriggerMode::Any;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            let hwnd = match create_sink() {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            STATE.with(|s| {
                *s.borrow_mut() =
                    Some(State { engine: Engine::new(session.cfg.params()), session, hwnd, any_mode })
            });
            update_flags();
            let _ = ready_tx.send(Ok(()));
            message_loop();
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|e| e.to_string())?
}

/// Message-only window receiving raw input from every mouse, plus the hook.
fn create_sink() -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).map_err(|e| e.to_string())?;
        let class = w!("tp-dial-input");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("tp-dial input"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("create input window: {e}"))?;
        let rid = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        RegisterRawInputDevices(&[rid], size_of::<RAWINPUTDEVICE>() as u32)
            .map_err(|e| format!("register raw input: {e}"))?;
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(hinst.into()), 0)
            .map_err(|e| format!("install mouse hook: {e}"))?;
        Ok(hwnd)
    }
}

fn message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => on_raw_input(HRAWINPUT(lp.0 as _)),
        WM_TIMER if wp.0 == TIMER_ID => with_state(|st| {
            let _ = unsafe { KillTimer(Some(st.hwnd), TIMER_ID) };
            let actions = st.engine.tick(Instant::now());
            run_actions(st, actions);
        }),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            f(st);
        }
    });
}

fn on_raw_input(h: HRAWINPUT) {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let got = unsafe {
        GetRawInputData(h, RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, size_of::<RAWINPUTHEADER>() as u32)
    };
    if got == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let m = unsafe { raw.data.mouse };
    let is_tp = raw.header.hDevice.0 as isize == TP_DEVICE.load(Relaxed);
    let (dx, dy) = (m.lLastX, m.lLastY);
    let absolute = m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0;
    if (dx != 0 || dy != 0) && !absolute {
        LAST_WAS_TP.store(is_tp, Relaxed);
        if is_tp {
            TP_LAST_MS.store(now_ms(), Relaxed);
        }
    }
    let flags = unsafe { m.Anonymous.Anonymous.usButtonFlags } as u32;

    with_state(|st| {
        let mut actions = Vec::new();
        // Any click cancels an open dial (the hook has already eaten it); otherwise
        // only the TrackPoint's own buttons matter to the engine.
        if is_tp || st.engine.phase == Phase::Open {
            for (bit, code, down) in [
                (RI_MOUSE_LEFT_BUTTON_DOWN, BTN_LEFT, true),
                (RI_MOUSE_LEFT_BUTTON_UP, BTN_LEFT, false),
                (RI_MOUSE_RIGHT_BUTTON_DOWN, BTN_RIGHT, true),
                (RI_MOUSE_RIGHT_BUTTON_UP, BTN_RIGHT, false),
                (RI_MOUSE_MIDDLE_BUTTON_DOWN, BTN_MIDDLE, true),
                (RI_MOUSE_MIDDLE_BUTTON_UP, BTN_MIDDLE, false),
            ] {
                if flags & bit != 0 {
                    actions.extend(st.engine.button(code, down));
                }
            }
        }
        if is_tp && !absolute && (dx != 0 || dy != 0) {
            let before = st.engine.phase;
            actions.extend(st.engine.motion(Instant::now(), dx, dy));
            if st.session.debug {
                eprintln!("dx={dx:+4} dy={dy:+4}  speed={:5.1}  {before:?} -> {:?}", st.engine.last_speed, st.engine.phase);
            }
        }
        actions.extend(st.engine.tick(Instant::now()));
        run_actions(st, actions);
    });
}

/// Forwarded motion/buttons need no action here: the hook let them through.
fn run_actions(st: &mut State, actions: Vec<Action>) {
    for a in actions {
        let _ = st.session.handle(&mut st.engine, a);
    }
    // Re-read every time: a config reload (e.g. switching to "flick" in Settings)
    // happens inside session.handle, and the hook must follow it straight away.
    st.any_mode = st.session.cfg.trigger.mode == TriggerMode::Any;
    update_flags_for(st);
    match st.engine.deadline() {
        Some(d) => {
            let ms = d.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)).as_millis() as u32;
            unsafe { SetTimer(Some(st.hwnd), TIMER_ID, ms, None) };
        }
        None => {
            let _ = unsafe { KillTimer(Some(st.hwnd), TIMER_ID) };
        }
    }
}

fn update_flags() {
    with_state(update_flags_for);
}

fn update_flags_for(st: &mut State) {
    let open = st.engine.phase == Phase::Open;
    DIAL_OPEN.store(open, Relaxed);
    // "any" mode: the TrackPoint never moves the cursor, except while a button is
    // held (drag / middle-button scroll).
    SWALLOW_TP.store(open || (st.any_mode && !st.engine.buttons_held()), Relaxed);
}

fn button_bit(msg: u32) -> Option<(u32, bool)> {
    Some(match msg {
        WM_LBUTTONDOWN => (1, true),
        WM_LBUTTONUP => (1, false),
        WM_RBUTTONDOWN => (2, true),
        WM_RBUTTONUP => (2, false),
        WM_MBUTTONDOWN => (4, true),
        WM_MBUTTONUP => (4, false),
        _ => return None,
    })
}

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        if info.flags & LLMHF_INJECTED == 0 {
            let msg = wp.0 as u32;
            if msg == WM_MOUSEMOVE {
                let tp_active = LAST_WAS_TP.load(Relaxed)
                    && now_ms().saturating_sub(TP_LAST_MS.load(Relaxed)) < TP_ACTIVE_MS;
                if DIAL_OPEN.load(Relaxed) || (SWALLOW_TP.load(Relaxed) && tp_active) {
                    return LRESULT(1);
                }
            } else if let Some((bit, down)) = button_bit(msg) {
                if down && DIAL_OPEN.load(Relaxed) {
                    EATEN_DOWN.fetch_or(bit, Relaxed);
                    return LRESULT(1);
                }
                if !down && EATEN_DOWN.load(Relaxed) & bit != 0 {
                    EATEN_DOWN.fetch_and(!bit, Relaxed);
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// `tp-dial --find-device`: watch every mouse for a few seconds, pick the one
/// that moved most while the user pushes the TrackPoint, and save it.
pub fn find_device() -> Result<(), String> {
    use std::collections::HashMap;
    thread_local! {
        static MOVED: RefCell<HashMap<isize, u64>> = RefCell::new(HashMap::new());
    }
    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_INPUT {
            let mut raw = RAWINPUT::default();
            let mut size = size_of::<RAWINPUT>() as u32;
            let got = unsafe {
                GetRawInputData(
                    HRAWINPUT(lp.0 as _),
                    RID_INPUT,
                    Some(&mut raw as *mut _ as *mut _),
                    &mut size,
                    size_of::<RAWINPUTHEADER>() as u32,
                )
            };
            if got != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE.0 {
                let m = unsafe { raw.data.mouse };
                if m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0 {
                    let amount = (m.lLastX.unsigned_abs() + m.lLastY.unsigned_abs()) as u64;
                    MOVED.with(|mv| *mv.borrow_mut().entry(raw.header.hDevice.0 as isize).or_default() += amount);
                }
            }
        }
        if msg == WM_TIMER {
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).map_err(|e| e.to_string())?;
        let class = w!("tp-dial-find");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        let rid = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        RegisterRawInputDevices(&[rid], size_of::<RAWINPUTDEVICE>() as u32).map_err(|e| e.to_string())?;
        crate::platform::tell(
            "Let's find your TrackPoint.\n\nAfter you click OK, push the TrackPoint around for 6 seconds. \
             Don't touch the touchpad or mouse meanwhile.",
        );
        SetTimer(Some(hwnd), 1, 6000, None);
        message_loop();
    }

    let best = MOVED.with(|mv| mv.borrow().iter().max_by_key(|(_, n)| **n).map(|(h, n)| (*h, *n)));
    let Some((handle, amount)) = best.filter(|(_, n)| *n > 50) else {
        return Err("no pointing-device movement seen; try again and push the TrackPoint".into());
    };
    let name = mouse_devices()
        .into_iter()
        .find(|(h, _)| h.0 as isize == handle)
        .map(|(_, n)| n)
        .ok_or("that device disappeared")?;
    crate::config::set_device_in_file(&name)?;
    crate::platform::tell(&format!(
        "Found it, and saved it to the settings.\n\n{name}\n(moved {amount} counts)\n\nIf the dial doesn't respond, run \
         \"Find my TrackPoint\" again from the Start Menu."
    ));
    Ok(())
}
