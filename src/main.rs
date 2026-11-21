mod config;
mod form;
mod gesture;
mod hypr;
mod input;
mod overlay;
mod settings;

use std::process::ExitCode;

const HELP: &str = "tp-dial: flick-and-hold the TrackPoint to open a radial launcher.

usage: tp-dial [--debug] [--no-grab] [--device /dev/input/eventN]
       tp-dial --add-item       open the form that adds an item to the dial
       tp-dial --settings       open the appearance / trigger settings window
       tp-dial --check          validate the config and list items
       tp-dial --default-config print the default config

  --debug     print per-report speed and gesture phase (for tuning [trigger])
  --demo      show the dial with a scripted pointer sweep (no TrackPoint access needed)
  --no-grab   don't take exclusive control (cursor moves while dial is open; no uinput needed)

config: ~/.config/tp-dial/config.toml (created on first run, re-read on change)";

/// Keep GTK off the GPU. tp-dial draws everything with cairo, so GTK's GL/Vulkan
/// renderers buy nothing, and initialising them makes the process load and hold
/// the discrete NVIDIA GPU (via glvnd/Vulkan ICD enumeration), which stops it from
/// runtime-suspending. The cairo renderer uses plain shared-memory buffers.
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
    let mut opts = input::Opts { device: None, debug: false, no_grab: false };
    let mut demo = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--debug" => opts.debug = true,
            "--demo" => demo = true,
            "--no-grab" => opts.no_grab = true,
            "--device" => opts.device = args.next().map(Into::into),
            "--settings" => {
                hypr::apply_rules(); // so the window floats even if the daemon isn't running
                settings::run_standalone();
                return ExitCode::SUCCESS;
            }
            "--add-item" => {
                return match config::Config::load() {
                    Ok(_) => {
                        form::run_standalone();
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        ExitCode::FAILURE
                    }
                };
            }
            "--default-config" => {
                print!("{}", config::DEFAULT_CONFIG);
                return ExitCode::SUCCESS;
            }
            "--check" => {
                return match config::Config::load() {
                    Ok((c, _)) => {
                        println!("{}: ok", config::Config::path().display());
                        for (i, it) in c.items.iter().enumerate() {
                            let what = match it.launch() {
                                config::Launch::Shell(s) => s,
                                config::Launch::Exec(p) => p.display().to_string(),
                                config::Launch::Builtin(b) => format!("(built-in {b:?})"),
                            };
                            println!("  {}. {:<14} {what}", i + 1, it.label);
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        ExitCode::FAILURE
                    }
                };
            }
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

    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "refusing to run as root: launched apps would run as root and the overlay can't \
             reach your Wayland session. Set up device permissions instead (see README)."
        );
        return ExitCode::FAILURE;
    }

    let (cfg, mtime) = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if demo {
        let (tx, rx) = async_channel::bounded(64);
        let dial = cfg.dial.resolved();
        std::thread::spawn(move || demo_sweep(cfg, tx));
        overlay::run(rx, dial);
        return ExitCode::SUCCESS;
    }

    let (dev, virt) = match input::open_devices(&opts) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "ready: {} items. {}",
        cfg.items.len(),
        match cfg.trigger.mode {
            config::TriggerMode::Any => "push the TrackPoint to open the dial (it no longer moves the cursor).",
            config::TriggerMode::Flick => "flick + hold the TrackPoint to open the dial.",
        }
    );

    hypr::keep_rules_applied();
    let (tx, rx) = async_channel::bounded(64);
    let dial = cfg.dial.resolved();
    std::thread::Builder::new()
        .name("input".into())
        .spawn(move || input::run(dev, virt, cfg, mtime, opts, tx))
        .expect("spawn input thread");

    if overlay::run(rx, dial) == gtk4::glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Open the dial at the cursor, sweep the pointer once around, select item 1, exit.
fn demo_sweep(cfg: config::Config, tx: async_channel::Sender<input::UiMsg>) {
    use std::time::Duration;
    let center = hypr::cursor().and_then(|(x, y)| hypr::monitor_at(x, y));
    std::thread::sleep(Duration::from_millis(400));
    // Debug aid: open the settings window inside the overlay process, as the
    // dial's Settings slice does (that's where overlay styles could leak in).
    if std::env::var_os("TP_DIAL_DEMO_SETTINGS").is_some() {
        let _ = tx.send_blocking(input::UiMsg::Settings);
        std::thread::sleep(Duration::from_secs(4));
        std::process::exit(0);
    }
    let _ = tx.send_blocking(input::UiMsg::Open {
        labels: cfg.items.iter().map(|i| i.label.clone()).collect(),
        dial: cfg.dial.resolved(),
        monitor: center.as_ref().map(|c| c.0.clone()),
        center: center
            .filter(|_| cfg.dial.position == config::Position::Cursor)
            .map(|c| (c.1, c.2)),
    });
    let n = cfg.items.len().max(1);
    let p = cfg.params();
    for step in 0..=120 {
        let a = step as f64 / 120.0 * std::f64::consts::TAU;
        let r = (step as f64 / 20.0).min(1.0);
        let (x, y) = (a.sin() * r, -a.cos() * r);
        let sel = (r * 40.0 >= p.min_steer).then(|| gesture::slice_at(x, y, n));
        let _ = tx.send_blocking(input::UiMsg::Update { selected: sel });
        std::thread::sleep(Duration::from_millis(std::env::var("TP_DIAL_DEMO_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(25)));
    }
    let _ = tx.send_blocking(input::UiMsg::Close { selected: Some(1 % n) });
    std::thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}
