//! Linux input: grabs the TrackPoint exclusively (evdev), re-emits everything
//! through a virtual clone (uinput) except motion/clicks consumed by the dial, and
//! drives the gesture engine. A crash anywhere aborts the process (panic = "abort"),
//! which makes the kernel drop the grab, so the TrackPoint can't get stuck dead.

use crate::gesture::{Action, Engine, Phase};
use crate::session::{Forward, Session};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use crate::Opts;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Instant;

pub const VIRTUAL_SUFFIX: &str = " (tp-dial)";

/// `device` is a /dev/input/eventN path, or a substring of the device name.
pub fn open_devices(opts: &Opts) -> Result<(Device, Option<VirtualDevice>), String> {
    let wanted = opts.device.as_deref().filter(|d| !d.is_empty());
    let (path, mut dev) = match wanted {
        Some(p) if p.starts_with('/') => Device::open(p)
            .map(|d| (PathBuf::from(p), d))
            .map_err(|e| format!("open {p}: {e}"))?,
        _ => evdev::enumerate()
            .find(|(_, d)| {
                let name = d.name().unwrap_or("");
                let lower = name.to_lowercase();
                !name.ends_with(VIRTUAL_SUFFIX)
                    && d.supported_relative_axes()
                        .is_some_and(|r| r.contains(RelativeAxisCode::REL_X))
                    && match wanted {
                        Some(w) => lower.contains(&w.to_lowercase()),
                        None => lower.contains("trackpoint") || lower.contains("pointing stick"),
                    }
            })
            .ok_or("no readable TrackPoint in /dev/input (see README: permissions)")?,
    };
    eprintln!("trackpoint: {} ({})", dev.name().unwrap_or("?"), path.display());

    if opts.no_grab {
        eprintln!("--no-grab: cursor will keep moving while the dial is open");
        return Ok((dev, None));
    }

    // Clone the device's identity so libinput treats the copy as the same TrackPoint
    // (pointing-stick acceleration, middle-button scrolling, quirks).
    let mut keys = AttributeSet::<KeyCode>::new();
    for k in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE] {
        keys.insert(k);
    }
    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    rel.insert(RelativeAxisCode::REL_X);
    rel.insert(RelativeAxisCode::REL_Y);
    let mut props = AttributeSet::<PropType>::new();
    props.insert(PropType::POINTER);
    props.insert(PropType::POINTING_STICK);
    let name = format!("{}{VIRTUAL_SUFFIX}", dev.name().unwrap_or("TrackPoint"));

    let virt = VirtualDevice::builder()
        .map_err(|e| format!("/dev/uinput: {e} (see README: permissions)"))?
        .name(&name)
        .input_id(dev.input_id())
        .with_keys(&keys)
        .and_then(|b| b.with_relative_axes(&rel))
        .and_then(|b| b.with_properties(&props))
        .and_then(|b| b.build())
        .map_err(|e| format!("create virtual device: {e}"))?;

    dev.grab().map_err(|e| match e.raw_os_error() {
        Some(libc::EBUSY) => "TrackPoint is already grabbed — another tp-dial is probably running \
                              (check: pgrep -a tp-dial)"
            .to_string(),
        _ => format!("grab TrackPoint: {e}"),
    })?;
    Ok((dev, Some(virt)))
}

pub fn run(mut dev: Device, mut virt: Option<VirtualDevice>, mut session: Session) -> ! {
    let mut engine = Engine::new(session.cfg.params());
    let debug = session.debug;
    let (mut fx, mut fy) = (0i32, 0i32);
    let mut frame_buttons: Vec<(u16, bool)> = Vec::new();
    let fd = dev.as_raw_fd();
    dev.set_nonblocking(true).expect("set_nonblocking");

    loop {
        // Sleep until input arrives or the engine's release timer fires.
        let timeout_ms = engine
            .deadline()
            .map(|d| d.saturating_duration_since(Instant::now()).as_millis() as i32 + 1)
            .unwrap_or(-1);
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            fatal(&format!("poll: {e}"));
        }

        let mut actions = Vec::new();
        if r == 0 {
            actions.extend(engine.tick(Instant::now()));
        } else {
            let events: Vec<InputEvent> = match dev.fetch_events() {
                Ok(it) => it.collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => fatal(&format!("read TrackPoint: {e}")),
            };
            for ev in events {
                match ev.destructure() {
                    EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, v) => fx += v,
                    EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, v) => fy += v,
                    EventSummary::Key(_, code, v) if v != 2 => frame_buttons.push((code.0, v != 0)),
                    EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                        let now = Instant::now();
                        for (code, down) in frame_buttons.drain(..) {
                            actions.extend(engine.button(code, down));
                        }
                        if fx != 0 || fy != 0 {
                            let before = engine.phase;
                            actions.extend(engine.motion(now, fx, fy));
                            if debug && before == Phase::Open {
                                if let Some(Action::Update { x, y, selected }) = actions.last() {
                                    eprintln!(
                                        "dx={fx:+4} dy={fy:+4}  pointer=({x:+.2},{y:+.2})  {}",
                                        selected
                                            .and_then(|i| session.cfg.items.get(i))
                                            .map_or("(nothing yet)", |it| it.label.as_str())
                                    );
                                }
                            } else if debug {
                                debug_line(fx, fy, engine.last_speed, before, engine.phase);
                            }
                        }
                        fx = 0;
                        fy = 0;
                    }
                    _ => {}
                }
            }
            // Release timer may also be due if events trickled in.
            actions.extend(engine.tick(Instant::now()));
        }

        let mut out: Vec<InputEvent> = Vec::new();
        for a in actions {
            if a == Action::Open {
                // Flush pending forwarded motion before the dial freezes the cursor.
                emit(&mut virt, &mut out);
            }
            match session.handle(&mut engine, a) {
                Some(Forward::Motion(dx, dy)) => {
                    out.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx));
                    out.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy));
                }
                Some(Forward::Button(code, down)) => {
                    out.push(InputEvent::new(EventType::KEY.0, code, down as i32));
                }
                None => {}
            }
        }
        emit(&mut virt, &mut out);
    }
}

fn emit(virt: &mut Option<VirtualDevice>, out: &mut Vec<InputEvent>) {
    if out.is_empty() {
        return;
    }
    if let Some(v) = virt {
        // VirtualDevice::emit appends the SYN_REPORT.
        if let Err(e) = v.emit(out) {
            fatal(&format!("uinput write: {e}"));
        }
    }
    out.clear();
}

fn debug_line(dx: i32, dy: i32, speed: f64, before: Phase, after: Phase) {
    let name = |p: Phase| match p {
        Phase::Idle => "idle",
        Phase::Watching { .. } => "watching",
        Phase::Holding { .. } => "HOLDING",
        Phase::Spent => "spent",
        Phase::Open => "OPEN",
    };
    let change = if std::mem::discriminant(&before) != std::mem::discriminant(&after) {
        format!("  -> {}", name(after))
    } else {
        String::new()
    };
    eprintln!("dx={dx:+4} dy={dy:+4}  speed={speed:5.1}  {}{change}", name(before));
}

fn fatal(msg: &str) -> ! {
    eprintln!("fatal: {msg}");
    std::process::exit(1);
}
