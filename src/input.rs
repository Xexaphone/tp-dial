//! Input thread: grabs the TrackPoint exclusively, re-emits everything through a
//! virtual clone (uinput) except motion/clicks consumed by the dial, and drives the
//! gesture engine. A crash anywhere aborts the process (panic = "abort"), which makes
//! the kernel drop the grab, so the TrackPoint can't get stuck dead.

use crate::config::{self, Config, Launch};
use crate::gesture::{Action, Engine, Phase};
use crate::hypr;
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime};

pub const VIRTUAL_SUFFIX: &str = " (tp-dial)";

pub enum UiMsg {
    Open {
        labels: Vec<String>,
        dial: config::Dial,
        monitor: Option<String>,
        center: Option<(f64, f64)>,
    },
    /// The highlighted item changed.
    Update { selected: Option<usize> },
    Close { selected: Option<usize> },
    /// Show the "add item" form.
    AddItem,
    /// Show the settings window.
    Settings,
}

pub struct Opts {
    pub device: Option<PathBuf>,
    pub debug: bool,
    pub no_grab: bool,
}

pub fn open_devices(opts: &Opts) -> Result<(Device, Option<VirtualDevice>), String> {
    let (path, mut dev) = match &opts.device {
        Some(p) => Device::open(p)
            .map(|d| (p.clone(), d))
            .map_err(|e| format!("open {}: {e}", p.display()))?,
        None => evdev::enumerate()
            .find(|(_, d)| {
                let name = d.name().unwrap_or("");
                let lower = name.to_lowercase();
                !name.ends_with(VIRTUAL_SUFFIX)
                    && d.supported_relative_axes()
                        .is_some_and(|r| r.contains(RelativeAxisCode::REL_X))
                    && (lower.contains("trackpoint") || lower.contains("pointing stick"))
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

pub fn run(
    mut dev: Device,
    mut virt: Option<VirtualDevice>,
    mut cfg: Config,
    mut cfg_mtime: Option<SystemTime>,
    opts: Opts,
    tx: async_channel::Sender<UiMsg>,
) -> ! {
    let mut engine = Engine::new(cfg.params());
    let mut burst_cursor: Option<(f64, f64)> = None;
    let mut shown_selection: Option<usize> = None;
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
                            if opts.debug && before == Phase::Open {
                                if let Some(Action::Update { x, y, selected }) = actions.last() {
                                    eprintln!(
                                        "dx={fx:+4} dy={fy:+4}  pointer=({x:+.2},{y:+.2})  {}",
                                        selected
                                            .and_then(|i| cfg.items.get(i))
                                            .map_or("(nothing yet)", |it| it.label.as_str())
                                    );
                                }
                            } else if opts.debug {
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
            match a {
                Action::ForwardMotion(dx, dy) => {
                    out.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx));
                    out.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy));
                }
                Action::ForwardButton(code, down) => {
                    out.push(InputEvent::new(EventType::KEY.0, code, down as i32));
                }
                Action::BurstStart => {
                    if let Some(m) = config::mtime().filter(|m| Some(*m) != cfg_mtime) {
                        cfg_mtime = Some(m);
                        match Config::load() {
                            Ok((c, _)) => {
                                eprintln!("config reloaded ({} items)", c.items.len());
                                engine.p = c.params();
                                cfg = c;
                            }
                            Err(e) => eprintln!("config error, keeping previous: {e}"),
                        }
                    }
                    // Where the cursor was before this push: the dial centres here.
                    burst_cursor = hypr::cursor();
                }
                Action::Candidate => {}
                Action::Open => {
                    shown_selection = None;
                    // Flush pending forwarded motion before freezing.
                    emit(&mut virt, &mut out);
                    let center = burst_cursor.or_else(hypr::cursor);
                    // In "any" mode the cursor never moved, so there's nothing to restore.
                    if cfg.dial.restore_cursor && cfg.trigger.mode == config::TriggerMode::Flick {
                        if let Some((x, y)) = burst_cursor {
                            hypr::move_cursor(x, y);
                        }
                    }
                    // Always open on the monitor the cursor is on; only place the dial
                    // at the cursor itself when configured to.
                    let (monitor, local) = match center.and_then(|(x, y)| hypr::monitor_at(x, y)) {
                        Some((name, lx, ly)) => (
                            Some(name),
                            (cfg.dial.position == config::Position::Cursor).then_some((lx, ly)),
                        ),
                        None => (None, None),
                    };
                    let _ = tx.send_blocking(UiMsg::Open {
                        labels: cfg.items.iter().map(|i| i.label.clone()).collect(),
                        dial: cfg.dial.resolved(),
                        monitor,
                        center: local,
                    });
                    if opts.debug {
                        eprintln!("== dial OPEN");
                    }
                }
                Action::Update { selected, .. } => {
                    if selected != shown_selection {
                        shown_selection = selected;
                        let _ = tx.try_send(UiMsg::Update { selected });
                    }
                }
                Action::Close { selected } => {
                    let _ = tx.send_blocking(UiMsg::Close { selected });
                    match selected.and_then(|i| cfg.items.get(i)) {
                        Some(item) => match item.launch() {
                            Launch::Builtin(config::Builtin::Quit) => {
                                eprintln!("quit selected: releasing the TrackPoint and exiting");
                                // Give the overlay a moment to hide its flash.
                                std::thread::sleep(std::time::Duration::from_millis(150));
                                std::process::exit(0);
                            }
                            Launch::Builtin(config::Builtin::Close) => eprintln!("dial closed"),
                            Launch::Builtin(config::Builtin::Settings) => {
                                eprintln!("opening settings");
                                let _ = tx.send_blocking(UiMsg::Settings);
                            }
                            Launch::Builtin(config::Builtin::AddItem) => {
                                eprintln!("opening add-item form");
                                let _ = tx.send_blocking(UiMsg::AddItem);
                            }
                            l => {
                                eprintln!("run: {}", item.label);
                                launch(l);
                            }
                        },
                        None => eprintln!("dial cancelled"),
                    }
                }
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

fn launch(l: Launch) {
    let line = match &l {
        Launch::Shell(s) => s.clone(),
        Launch::Exec(p) => format!("'{}'", p.display().to_string().replace('\'', r"'\''")),
        Launch::Builtin(_) => return,
    };
    if hypr::exec(&line) {
        return;
    }
    eprintln!("hyprland exec unavailable, spawning directly");
    let mut cmd = match l {
        Launch::Shell(s) => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(s);
            c
        }
        Launch::Exec(p) => Command::new(p),
        Launch::Builtin(_) => return,
    };
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
