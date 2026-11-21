// No console window when started normally on Windows; CLI output still reaches
// the terminal it was run from (see platform::init).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod dial;
mod form;
mod gesture;
#[cfg(target_os = "linux")]
mod input_linux;
#[cfg(windows)]
mod input_windows;
mod overlay;
mod platform;
mod session;
mod settings;

use session::Session;
use std::process::ExitCode;

const HELP: &str = "tp-dial: push the TrackPoint to open a radial launcher.

usage: tp-dial [--debug] [--no-grab] [--device DEVICE]
       tp-dial --add-item          open the form that adds an item to the dial
       tp-dial --settings          open the appearance / trigger settings window
       tp-dial --check             validate the config and list items
       tp-dial --default-config    print the default config for this computer
       tp-dial --demo              show the dial with a scripted sweep (no TrackPoint needed)
Windows only:
       tp-dial --find-device       push the TrackPoint to identify it, then save it to the config
       tp-dial --install-autostart start tp-dial when you sign in
       tp-dial --remove-autostart

  --debug     print per-report speed and gesture phase (for tuning [trigger])
  --no-grab   (Linux) don't take exclusive control of the TrackPoint
  --device    Linux: /dev/input/eventN or part of the device name.
              Windows: part of the device path (see --find-device).
              Also settable as `device = \"...\"` under [trigger] in the config.

config: ~/.config/tp-dial/config.toml (Windows: %APPDATA%\\tp-dial\\config.toml),
created on first run and re-read whenever it changes.";

pub struct Opts {
    pub device: Option<String>,
    pub debug: bool,
    pub no_grab: bool,
}

/// Keep GTK off the GPU. tp-dial draws everything with cairo, so GTK's GL/Vulkan
/// renderers buy nothing, and initialising them makes the process load and hold
/// a discrete GPU (e.g. NVIDIA via glvnd/Vulkan ICD enumeration), which stops it
/// from runtime-suspending. The cairo renderer uses plain shared-memory buffers.
/// Set TP_DIAL_GPU=1 to let GTK pick its renderer as usual.
fn stay_off_the_gpu() {
    if std::env::var_os("TP_DIAL_GPU").is_some() {
        return;
    }
    // SAFETY: called first thing in main, before any other thread exists.
    unsafe {
        if std::env::var_os("GSK_RENDERER").is_none() {
            std::env::set_var("GSK_RENDERER", "cairo");
        }
        if std::env::var_os("GDK_DISABLE").is_none() {
            std::env::set_var("GDK_DISABLE", "gl,vulkan");
        }
    }
}

fn main() -> ExitCode {
    stay_off_the_gpu();
    platform::init();
    let mut opts = Opts { device: None, debug: false, no_grab: false };
    let mut demo = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--debug" => opts.debug = true,
            "--demo" => demo = true,
            "--no-grab" => opts.no_grab = true,
            "--device" => opts.device = args.next(),
            "--settings" => {
                platform::apply_rules(); // so the window floats even if the daemon isn't running
                settings::run_standalone();
                return ExitCode::SUCCESS;
            }
            "--add-item" => {
                return match config::Config::load() {
                    Ok(_) => {
                        form::run_standalone();
                        ExitCode::SUCCESS
                    }
                    Err(e) => fail(&e),
                };
            }
            "--default-config" => {
                print!("{}", config::default_config());
                return ExitCode::SUCCESS;
            }
            "--check" => {
                return match config::Config::load() {
                    Ok((c, _)) => {
                        println!("{}: ok", config::Config::path().display());
                        for (i, it) in c.items.iter().enumerate() {
                            println!("  {}. {:<14} {}", i + 1, it.label, it.launch());
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => fail(&e),
                };
            }
            #[cfg(windows)]
            "--find-device" => return to_exit(input_windows::find_device()),
            #[cfg(windows)]
            "--install-autostart" => return to_exit(platform::set_autostart(true)),
            #[cfg(windows)]
            "--remove-autostart" => return to_exit(platform::set_autostart(false)),
            "-h" | "--help" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }

    if platform::running_as_root() {
        return fail(
            "refusing to run as root: launched apps would run as root and the overlay can't \
             reach your desktop session. Set up device permissions instead (see README).",
        );
    }

    let (cfg, mtime) = match config::Config::load() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if opts.device.is_none() && !cfg.trigger.device.is_empty() {
        opts.device = Some(cfg.trigger.device.clone());
    }
    let (tx, rx) = async_channel::bounded(64);
    let dial = cfg.dial.resolved();

    if demo {
        std::thread::spawn(move || demo_sweep(cfg, tx));
        overlay::run(rx, dial);
        return ExitCode::SUCCESS;
    }

    let ready = format!(
        "ready: {} items. {}",
        cfg.items.len(),
        match cfg.trigger.mode {
            config::TriggerMode::Any => "push the TrackPoint to open the dial (it no longer moves the cursor).",
            config::TriggerMode::Flick => "flick + hold the TrackPoint to open the dial.",
        }
    );
    let session = Session::new(cfg, mtime, tx, opts.debug);

    #[cfg(target_os = "linux")]
    {
        let (dev, virt) = match input_linux::open_devices(&opts) {
            Ok(d) => d,
            Err(e) => return fail(&e),
        };
        eprintln!("{ready}");
        std::thread::Builder::new()
            .name("input".into())
            .spawn(move || input_linux::run(dev, virt, session))
            .expect("spawn input thread");
    }
    #[cfg(windows)]
    {
        if let Err(e) = input_windows::start(&opts, session) {
            return fail(&e);
        }
        eprintln!("{ready}");
    }

    platform::keep_rules_applied();
    if overlay::run(rx, dial) == gtk4::glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    platform::report_error(msg);
    ExitCode::FAILURE
}

#[cfg(windows)]
fn to_exit(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// Open the dial, sweep the highlight once around, select item 2, exit.
fn demo_sweep(cfg: config::Config, tx: async_channel::Sender<session::UiMsg>) {
    use std::time::Duration;
    let place = platform::placement(cfg.dial.position, platform::cursor());
    std::thread::sleep(Duration::from_millis(400));
    // Debug aid: open the settings window inside the overlay process, as the
    // dial's Settings slice does (that's where overlay styles could leak in).
    if std::env::var_os("TP_DIAL_DEMO_SETTINGS").is_some() {
        let _ = tx.send_blocking(session::UiMsg::Settings);
        std::thread::sleep(Duration::from_secs(4));
        std::process::exit(0);
    }
    let _ = tx.send_blocking(session::UiMsg::Open {
        labels: cfg.items.iter().map(|i| i.label.clone()).collect(),
        dial: cfg.dial.resolved(),
        place,
    });
    let n = cfg.items.len().max(1);
    let p = cfg.params();
    let step_ms = std::env::var("TP_DIAL_DEMO_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(25);
    for step in 0..=120 {
        let a = step as f64 / 120.0 * std::f64::consts::TAU;
        let r = (step as f64 / 20.0).min(1.0);
        let (x, y) = (a.sin() * r, -a.cos() * r);
        let sel = (r * 40.0 >= p.min_steer).then(|| gesture::slice_at(x, y, n));
        let _ = tx.send_blocking(session::UiMsg::Update { selected: sel });
        std::thread::sleep(Duration::from_millis(step_ms));
    }
    let _ = tx.send_blocking(session::UiMsg::Close { selected: Some(1 % n) });
    std::thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}
