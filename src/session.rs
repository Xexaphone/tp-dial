//! What happens around a gesture, independent of where the input came from:
//! reloading the config, placing the dial, telling the UI, and running the chosen
//! item. Each platform's input backend feeds engine actions through `Session`
//! and only deals with forwarding motion/buttons itself.

use crate::config::{self, Builtin, Config, Launch};
use crate::gesture::{Action, Engine};
use crate::platform::{self, Placement};
use std::time::SystemTime;

pub enum UiMsg {
    Open { labels: Vec<String>, dial: config::Dial, place: Placement },
    /// The highlighted item changed.
    Update { selected: Option<usize> },
    Close { selected: Option<usize> },
    /// Show the "add item" form.
    AddItem,
    /// Show the settings window.
    Settings,
}

/// Motion / buttons the platform should pass on to the cursor as usual. (On
/// Windows the mouse hook lets them through instead, so the values go unused.)
#[cfg_attr(windows, allow(dead_code))]
pub enum Forward {
    Motion(i32, i32),
    Button(u16, bool),
}

pub struct Session {
    pub cfg: Config,
    cfg_mtime: Option<SystemTime>,
    burst_cursor: Option<(f64, f64)>,
    shown_selection: Option<usize>,
    tx: async_channel::Sender<UiMsg>,
    pub debug: bool,
}

impl Session {
    pub fn new(cfg: Config, cfg_mtime: Option<SystemTime>, tx: async_channel::Sender<UiMsg>, debug: bool) -> Self {
        Self { cfg, cfg_mtime, burst_cursor: None, shown_selection: None, tx, debug }
    }

    /// Handle one engine action; returns what (if anything) to forward.
    pub fn handle(&mut self, engine: &mut Engine, action: Action) -> Option<Forward> {
        match action {
            Action::ForwardMotion(dx, dy) => return Some(Forward::Motion(dx, dy)),
            Action::ForwardButton(code, down) => return Some(Forward::Button(code, down)),
            Action::BurstStart => {
                self.reload_if_changed(engine);
                // Where the cursor was before this push: the dial can centre here.
                self.burst_cursor = platform::cursor();
            }
            Action::Candidate => {}
            Action::Open => self.open(),
            Action::Update { selected, .. } => {
                if selected != self.shown_selection {
                    self.shown_selection = selected;
                    let _ = self.tx.try_send(UiMsg::Update { selected });
                }
            }
            Action::Close { selected } => self.close(selected),
        }
        None
    }

    fn reload_if_changed(&mut self, engine: &mut Engine) {
        let Some(m) = config::mtime().filter(|m| Some(*m) != self.cfg_mtime) else { return };
        self.cfg_mtime = Some(m);
        match Config::load() {
            Ok((c, _)) => {
                eprintln!("config reloaded ({} items)", c.items.len());
                engine.p = c.params();
                self.cfg = c;
            }
            Err(e) => eprintln!("config error, keeping previous: {e}"),
        }
    }

    fn open(&mut self) {
        self.shown_selection = None;
        let cursor = self.burst_cursor.or_else(platform::cursor);
        // In "any" mode the cursor never moved, so there's nothing to restore.
        if self.cfg.dial.restore_cursor && self.cfg.trigger.mode == config::TriggerMode::Flick {
            if let Some((x, y)) = self.burst_cursor {
                platform::restore_cursor(x, y);
            }
        }
        let _ = self.tx.send_blocking(UiMsg::Open {
            labels: self.cfg.items.iter().map(|i| i.label.clone()).collect(),
            dial: self.cfg.dial.resolved(),
            place: platform::placement(self.cfg.dial.position, cursor),
        });
        if self.debug {
            eprintln!("== dial OPEN");
        }
    }

    fn close(&mut self, selected: Option<usize>) {
        let _ = self.tx.send_blocking(UiMsg::Close { selected });
        let Some(item) = selected.and_then(|i| self.cfg.items.get(i)) else {
            eprintln!("dial cancelled");
            return;
        };
        match item.launch() {
            Launch::Builtin(Builtin::Quit) => {
                eprintln!("quit selected: releasing the TrackPoint and exiting");
                // Give the overlay a moment to hide its flash.
                std::thread::sleep(std::time::Duration::from_millis(150));
                std::process::exit(0);
            }
            Launch::Builtin(Builtin::Close) => eprintln!("dial closed"),
            Launch::Builtin(Builtin::Settings) => {
                eprintln!("opening settings");
                let _ = self.tx.send_blocking(UiMsg::Settings);
            }
            Launch::Builtin(Builtin::AddItem) => {
                eprintln!("opening add-item form");
                let _ = self.tx.send_blocking(UiMsg::AddItem);
            }
            l => {
                eprintln!("run: {}", item.label);
                platform::launch(&l);
            }
        }
    }
}
