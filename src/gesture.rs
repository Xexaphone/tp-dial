//! Pure gesture state machine: no I/O, so it can be unit-tested with synthetic time.
//!
//! Flow for one TrackPoint "burst" (continuous stream of motion reports):
//!   Watching  -> speed reaches `trigger_speed` within `rise` of the burst onset (a sharp push)
//!   Holding   -> speed must stay >= `hold_speed` for `hold` (the anti-accident delay)
//!   Open      -> dial shown, motion is swallowed; the slice you're pushing towards
//!                (over the last `speed_window`) is highlighted, joystick-style. A weak
//!                push (< `min_steer`, e.g. easing off / release rebound) keeps it.
//!   release   -> no motion for `release` => select whatever slice is highlighted
//! A burst that ramps up slowly (normal cursor use), or lets go during the hold,
//! is "Spent" and can't open the dial until the stick goes idle again.
//!
//! Mode::Any skips all of that: the TrackPoint is a dedicated dial controller.
//! The first push above `min_push` opens the dial at once; smaller drift is ignored
//! and nothing moves the cursor unless a button is held (drag / middle-scroll).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Any,
    Flick,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub mode: Mode,
    pub min_push: f64,
    pub speed_window: Duration,
    pub burst_gap: Duration,
    pub trigger_speed: f64,
    pub rise: Duration,
    pub hold_speed: f64,
    pub hold: Duration,
    pub min_steer: f64,
    pub release: Duration,
    pub slices: usize,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    ForwardMotion(i32, i32),
    ForwardButton(u16, bool),
    /// First report of a new burst (good moment to note cursor position / reload config).
    BurstStart,
    /// Sharp onset seen; waiting out the hold delay.
    Candidate,
    Open,
    /// Push direction scaled by strength (|v| <= 1), selected slice.
    Update { x: f64, y: f64, selected: Option<usize> },
    Close { selected: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Idle,
    Watching { onset: Instant },
    Holding { since: Instant },
    Spent,
    Open,
}

pub struct Engine {
    pub p: Params,
    pub phase: Phase,
    samples: VecDeque<(Instant, i32, i32)>,
    last_motion: Option<Instant>,
    held: Vec<u16>,
    suppressed: Vec<u16>,
    selected: Option<usize>,
    pub last_speed: f64,
}

impl Engine {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            phase: Phase::Idle,
            samples: VecDeque::new(),
            last_motion: None,
            held: Vec::new(),
            suppressed: Vec::new(),
            selected: None,
            last_speed: 0.0,
        }
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.phase == Phase::Open
    }

    pub fn motion(&mut self, t: Instant, dx: i32, dy: i32) -> Vec<Action> {
        let mut out = Vec::new();
        let new_burst = self
            .last_motion
            .is_none_or(|lm| t.saturating_duration_since(lm) >= self.p.burst_gap);
        self.last_motion = Some(t);

        if self.phase == Phase::Open {
            self.speed(t, dx, dy);
            self.steer(&mut out);
            return out;
        }

        if self.p.mode == Mode::Any {
            if new_burst {
                self.samples.clear();
                self.phase = Phase::Watching { onset: t };
                out.push(Action::BurstStart);
            }
            let speed = self.speed(t, dx, dy);
            if !self.held.is_empty() || self.p.slices == 0 {
                // Dragging / middle-button scrolling still work through the TrackPoint.
                out.push(Action::ForwardMotion(dx, dy));
            } else if speed >= self.p.min_push {
                // Open immediately; the push so far already picks a slice.
                self.phase = Phase::Open;
                self.selected = None;
                out.push(Action::Open);
                self.steer(&mut out);
            }
            // Otherwise: resting drift, swallowed.
            return out;
        }

        if new_burst {
            self.samples.clear();
            self.phase = Phase::Watching { onset: t };
            out.push(Action::BurstStart);
        }
        let speed = self.speed(t, dx, dy);

        if !self.held.is_empty() || self.p.slices == 0 {
            // Button held (e.g. middle-button scrolling) or nothing to show.
            self.phase = Phase::Spent;
        }

        match self.phase {
            Phase::Watching { onset } if speed >= self.p.trigger_speed => {
                if t.saturating_duration_since(onset) <= self.p.rise {
                    self.phase = Phase::Holding { since: t };
                    out.push(Action::Candidate);
                } else {
                    self.phase = Phase::Spent;
                }
            }
            Phase::Watching { onset } if t.saturating_duration_since(onset) > self.p.rise => {
                self.phase = Phase::Spent;
            }
            Phase::Holding { .. } if speed < self.p.hold_speed => self.phase = Phase::Spent,
            Phase::Holding { since } if t.saturating_duration_since(since) >= self.p.hold => {
                self.phase = Phase::Open;
                self.selected = None;
                out.push(Action::Open);
                self.steer(&mut out);
                return out; // swallow the frame that opened the dial
            }
            _ => {}
        }
        out.push(Action::ForwardMotion(dx, dy));
        out
    }

    pub fn button(&mut self, code: u16, down: bool) -> Vec<Action> {
        if self.phase == Phase::Open {
            if down {
                // Any click while the dial is open cancels it; eat the click.
                self.suppressed.push(code);
                return self.close(None);
            }
            return vec![];
        }
        if !down {
            if let Some(i) = self.suppressed.iter().position(|&c| c == code) {
                self.suppressed.swap_remove(i);
                return vec![];
            }
            self.held.retain(|&c| c != code);
        } else if !self.held.contains(&code) {
            self.held.push(code);
        }
        vec![Action::ForwardButton(code, down)]
    }

    /// Call when the poll timeout from `deadline()` expires.
    pub fn tick(&mut self, t: Instant) -> Vec<Action> {
        match (self.phase, self.last_motion) {
            (Phase::Open, Some(lm)) if t.saturating_duration_since(lm) >= self.p.release => {
                self.close(self.selected)
            }
            _ => vec![],
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Open => self.last_motion.map(|lm| lm + self.p.release),
            _ => None,
        }
    }

    fn close(&mut self, selected: Option<usize>) -> Vec<Action> {
        self.phase = Phase::Idle;
        self.last_motion = None; // next motion is a fresh burst
        self.samples.clear();
        vec![Action::Close { selected }]
    }

    fn speed(&mut self, t: Instant, dx: i32, dy: i32) -> f64 {
        self.samples.push_back((t, dx, dy));
        while let Some(&(t0, _, _)) = self.samples.front() {
            if t.saturating_duration_since(t0) > self.p.speed_window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        let (sx, sy) = self.window_sum();
        self.last_speed = ((sx * sx + sy * sy) as f64).sqrt();
        self.last_speed
    }

    fn window_sum(&self) -> (i32, i32) {
        self.samples
            .iter()
            .fold((0, 0), |(ax, ay), &(_, x, y)| (ax + x, ay + y))
    }

    fn steer(&mut self, out: &mut Vec<Action>) {
        let (sx, sy) = self.window_sum();
        let (sx, sy) = (sx as f64, sy as f64);
        let speed = (sx * sx + sy * sy).sqrt();
        if speed >= self.p.min_steer {
            self.selected = Some(slice_at(sx, sy, self.p.slices));
        }
        // Pointer: push direction, length = push strength (full at 40/50ms).
        let scale = if speed > 0.0 { (speed / 40.0).min(1.0) / speed } else { 0.0 };
        out.push(Action::Update { x: sx * scale, y: sy * scale, selected: self.selected });
    }
}

/// Slice 0 is centred straight up; indices increase clockwise.
pub fn slice_at(x: f64, y: f64, n: usize) -> usize {
    let ang = x.atan2(-y).to_degrees().rem_euclid(360.0);
    let step = 360.0 / n as f64;
    (((ang + step / 2.0) / step) as usize) % n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params {
            mode: Mode::Flick,
            min_push: 10.0,
            speed_window: Duration::from_millis(50),
            burst_gap: Duration::from_millis(80),
            trigger_speed: 40.0,
            rise: Duration::from_millis(100),
            hold_speed: 20.0,
            hold: Duration::from_millis(150),
            min_steer: 12.0,
            release: Duration::from_millis(90),
            slices: 4,
        }
    }

    struct Clock(Instant);
    impl Clock {
        fn at(&self, ms: u64) -> Instant {
            self.0 + Duration::from_millis(ms)
        }
    }

    fn run(e: &mut Engine, c: &Clock, from: u64, to: u64, dx: i32, dy: i32) -> Vec<Action> {
        (from..to)
            .step_by(8)
            .flat_map(|ms| e.motion(c.at(ms), dx, dy))
            .collect()
    }

    #[test]
    fn slices_clockwise_from_top() {
        assert_eq!(slice_at(0.0, -1.0, 4), 0); // up
        assert_eq!(slice_at(1.0, 0.0, 4), 1); // right
        assert_eq!(slice_at(0.0, 1.0, 4), 2); // down
        assert_eq!(slice_at(-1.0, 0.0, 4), 3); // left
        assert_eq!(slice_at(-0.1, -1.0, 4), 0);
    }

    #[test]
    fn slow_ramp_never_opens() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        // Cursor-style ramp: 1 count/report growing to 10 over ~250ms.
        let acts: Vec<_> = (0..40u64)
            .flat_map(|i| e.motion(c.at(i * 8), -((1 + i / 4) as i32), 0))
            .collect();
        assert!(!acts.contains(&Action::Open));
        assert!(acts.iter().all(|a| !matches!(a, Action::Candidate)));
    }

    #[test]
    fn short_flick_without_hold_does_not_open() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        let acts = run(&mut e, &c, 0, 80, 0, -9); // sharp, but only 80ms
        assert!(acts.contains(&Action::Candidate));
        assert!(!acts.contains(&Action::Open));
    }

    #[test]
    fn flick_hold_steer_release_selects() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        let acts = run(&mut e, &c, 0, 300, 9, 0); // sharp push right, held
        assert!(acts.contains(&Action::Open));
        // motion after open is swallowed
        let open_at = acts.iter().position(|a| *a == Action::Open).unwrap();
        assert!(acts[open_at..]
            .iter()
            .all(|a| !matches!(a, Action::ForwardMotion(..))));
        assert_eq!(e.deadline().is_some(), true);
        let last = c.at(296);
        assert_eq!(e.tick(last + Duration::from_millis(50)), vec![]);
        assert_eq!(
            e.tick(last + Duration::from_millis(95)),
            vec![Action::Close { selected: Some(1) }]
        );
        assert!(!e.is_open());
    }

    #[test]
    fn direction_change_switches_slice_immediately_and_easing_off_keeps_it() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        run(&mut e, &c, 0, 260, 9, 0); // flick right, held: opens on "right" (1)
        assert!(e.is_open());
        // Turn the push downwards: within ~50ms the window points down (2).
        let acts = run(&mut e, &c, 264, 330, 0, 9);
        assert!(matches!(acts.last(), Some(Action::Update { selected: Some(2), .. })));
        // Ease off + small opposite rebound: selection holds.
        let acts = run(&mut e, &c, 400, 440, 0, -1);
        assert!(matches!(acts.last(), Some(Action::Update { selected: Some(2), .. })));
        assert_eq!(e.tick(c.at(440 + 100)), vec![Action::Close { selected: Some(2) }]);
    }

    #[test]
    fn click_while_open_cancels_and_is_eaten() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        run(&mut e, &c, 0, 260, 0, -9);
        assert!(e.is_open());
        assert_eq!(e.button(0x110, true), vec![Action::Close { selected: None }]);
        assert_eq!(e.button(0x110, false), vec![]); // release swallowed too
        assert_eq!(e.button(0x110, true), vec![Action::ForwardButton(0x110, true)]);
    }

    #[test]
    fn held_button_blocks_trigger() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(params());
        e.button(0x112, true); // middle (scroll)
        let acts = run(&mut e, &c, 0, 300, 0, -9);
        assert!(!acts.contains(&Action::Open));
    }

    fn any_params() -> Params {
        Params { mode: Mode::Any, ..params() }
    }

    #[test]
    fn any_mode_ignores_resting_drift_and_never_moves_cursor() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(any_params());
        // 1 count per report ~= 6-7 per 50ms window: below min_push 10.
        let acts = run(&mut e, &c, 0, 400, 1, 0);
        assert!(!acts.contains(&Action::Open));
        assert!(acts.iter().all(|a| !matches!(a, Action::ForwardMotion(..))));
    }

    #[test]
    fn any_mode_opens_on_second_report_of_a_push_and_seeds_direction() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(any_params());
        assert!(!e.motion(c.at(0), 5, -1).contains(&Action::Open)); // 5.1
        let acts = e.motion(c.at(8), 9, -3); // window 14.6 -> open, already pointing right
        assert_eq!(acts[0], Action::Open);
        match acts[1] {
            Action::Update { x, selected: Some(1), .. } => assert!(x > 0.0),
            ref a => panic!("unexpected {a:?}"),
        }
        let acts = run(&mut e, &c, 16, 60, 12, 0);
        assert!(matches!(acts.last(), Some(Action::Update { selected: Some(1), .. })));
        // release -> select, then the post-release rebound doesn't reopen
        let r = e.tick(c.at(60 + 100));
        assert_eq!(r, vec![Action::Close { selected: Some(1) }]);
        assert!(!e.motion(c.at(170), -2, 0).contains(&Action::Open));
    }

    #[test]
    fn any_mode_forwards_motion_while_button_held() {
        let c = Clock(Instant::now());
        let mut e = Engine::new(any_params());
        e.button(0x112, true);
        let acts = run(&mut e, &c, 0, 100, 0, -9);
        assert!(!acts.contains(&Action::Open));
        assert!(acts.contains(&Action::ForwardMotion(0, -9)));
    }
}
