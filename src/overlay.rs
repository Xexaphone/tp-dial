//! GTK4 layer-shell overlay: a small, transparent, click-through surface on the
//! Overlay layer. It stays mapped the whole time and just draws nothing while the
//! dial is closed, so opening the dial costs one redraw (next frame), not a new
//! surface + buffers. While open, a frame-clock tick drives the opening animation
//! and any animated centre image, then stops.

use crate::config::{rgba, Animation, Dial};
use crate::input::UiMsg;
use gtk::cairo::{self, Context, FontSlant, FontWeight};
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gdk_pixbuf::{prelude::*, InterpType, Pixbuf, PixbufAnimation, PixbufAnimationIter};
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

pub(crate) const PAD: f64 = 8.0;
const MIN_FONT: f64 = 8.0;

/// Largest edge kept from a still image: plenty for any centre size on a 2x screen.
const MAX_STILL_PX: i32 = 1024;

/// The centre image. Big files (e.g. a 4K wallpaper) are expensive to scale, so
/// still images are shrunk once on load, and every frame paints a cached surface
/// already at the exact on-screen size; it's rebuilt only when that size changes
/// or an animated GIF moves to its next frame.
pub(crate) struct CenterImage {
    path: PathBuf,
    mtime: Option<SystemTime>,
    still: Option<Pixbuf>,
    gif: Option<(PixbufAnimation, PixbufAnimationIter)>,
    frame: Cell<u64>,
    cache: RefCell<Option<(u64, i32, cairo::ImageSurface)>>,
}

impl CenterImage {
    pub(crate) fn is_animated(&self) -> bool {
        self.gif.is_some()
    }

    /// Advance a GIF to `now`; true if the visible frame changed.
    pub(crate) fn advance(&self, now: SystemTime) -> bool {
        let changed = self.gif.as_ref().is_some_and(|(_, it)| it.advance(now));
        if changed {
            self.frame.set(self.frame.get() + 1);
        }
        changed
    }

    fn pixbuf(&self) -> Option<Pixbuf> {
        match (&self.still, &self.gif) {
            (Some(p), _) => Some(p.clone()),
            (_, Some((_, it))) => Some(it.pixbuf()),
            _ => None,
        }
    }

    /// Square, cover-fitted surface `px` device pixels across.
    fn surface(&self, px: i32) -> Option<cairo::ImageSurface> {
        let key = (self.frame.get(), px);
        if let Some((f, p, s)) = &*self.cache.borrow() {
            if (*f, *p) == key {
                return Some(s.clone());
            }
        }
        let pb = self.pixbuf()?;
        let (iw, ih) = (pb.width() as f64, pb.height() as f64);
        if iw <= 0.0 || ih <= 0.0 || px <= 0 {
            return None;
        }
        let k = (px as f64 / iw).max(px as f64 / ih);
        let (sw, sh) = (((iw * k).ceil() as i32).max(1), ((ih * k).ceil() as i32).max(1));
        let scaled = pb.scale_simple(sw, sh, InterpType::Bilinear)?;
        let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, px, px).ok()?;
        {
            let cr = Context::new(&surf).ok()?;
            cr.set_source_pixbuf(&scaled, (px - sw) as f64 / 2.0, (px - sh) as f64 / 2.0);
            let _ = cr.paint();
        }
        *self.cache.borrow_mut() = Some((key.0, key.1, surf.clone()));
        Some(surf)
    }
}

#[derive(Default)]
pub(crate) struct View {
    pub(crate) labels: Vec<String>,
    pub(crate) dial: Option<Dial>,
    pub(crate) selected: Option<usize>,
    pub(crate) chosen: bool, // release confirmed a choice: brief flash before hiding
    pub(crate) generation: u64,
    /// Opening animation progress 0..1, and the frame time it started at.
    pub(crate) progress: f64,
    pub(crate) anim_start: Option<i64>,
    pub(crate) image: Option<CenterImage>,
}

pub fn run(rx: async_channel::Receiver<UiMsg>, dial: Dial) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.tpdial.TpDial")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| build(app, rx.clone(), dial.clone()));
    // Don't let GTK see our CLI flags.
    app.run_with_args::<&str>(&[])
}

fn build(app: &gtk::Application, rx: async_channel::Receiver<UiMsg>, dial: Dial) {
    std::mem::forget(app.hold());

    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("tp-dial-transparent");
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    // No anchors = centred on its monitor. Cursor mode anchors top-left + margins.
    win.set_exclusive_zone(-1);
    win.set_keyboard_mode(KeyboardMode::None);
    win.set_namespace(Some("tp-dial"));

    let css = gtk::CssProvider::new();
    // Only our overlay-style windows; the settings window keeps a real background.
    css.load_from_string("window.tp-dial-transparent, window.tp-dial-transparent.background { background: transparent; }");
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Empty input region: clicks and hover go straight through to whatever is below.
    win.connect_map(|w| {
        if let Some(s) = w.surface() {
            s.set_input_region(Some(&cairo::Region::create()));
        }
    });

    // Decode the centre image now, not on the first open (big files take a moment).
    let view = Rc::new(RefCell::new(View { image: load_image(None, &dial.center_image), ..Default::default() }));
    let area = gtk::DrawingArea::new();
    let side = |d: &Dial| (d.size + 2.0 * PAD).ceil() as i32;
    area.set_size_request(side(&dial), side(&dial));
    {
        let view = view.clone();
        // TP_DIAL_TRACE=1 logs each frame's draw time (debugging animation smoothness).
        let trace = std::env::var_os("TP_DIAL_TRACE").is_some();
        area.set_draw_func(move |_, cr, w, h| {
            let t = std::time::Instant::now();
            draw(&view.borrow(), cr, w as f64, h as f64);
            if trace {
                eprintln!("frame drawn in {:.2}ms", t.elapsed().as_secs_f64() * 1000.0);
            }
        });
    }
    win.set_child(Some(&area));
    win.present(); // mapped for good; draws nothing until a dial opens

    let ticking = Rc::new(RefCell::new(false));
    let app = app.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            let mut v = view.borrow_mut();
            match msg {
                UiMsg::Open { labels, dial, monitor, center } => {
                    let px = side(&dial);
                    if area.size_request() != (px, px) {
                        area.set_size_request(px, px);
                    }
                    let target = monitor.as_deref().and_then(find_monitor);
                    if target.is_some() && target != win.monitor() {
                        win.set_monitor(target.as_ref());
                    }
                    match center {
                        Some((x, y)) => {
                            let half = px as f64 / 2.0;
                            win.set_anchor(Edge::Top, true);
                            win.set_anchor(Edge::Left, true);
                            win.set_margin(Edge::Left, (x - half).max(0.0) as i32);
                            win.set_margin(Edge::Top, (y - half).max(0.0) as i32);
                        }
                        None => {
                            win.set_anchor(Edge::Top, false);
                            win.set_anchor(Edge::Left, false);
                        }
                    }
                    let animated = dial.animation != Animation::None && dial.animation_ms > 0;
                    let image = load_image(v.image.take(), &dial.center_image);
                    v.generation += 1;
                    *v = View {
                        labels,
                        progress: if animated { 0.0 } else { 1.0 },
                        dial: Some(dial),
                        generation: v.generation,
                        image,
                        ..Default::default()
                    };
                    let needs_ticks = animated || v.image.as_ref().is_some_and(|i| i.is_animated());
                    area.queue_draw();
                    if needs_ticks && !*ticking.borrow() {
                        *ticking.borrow_mut() = true;
                        start_ticker(&area, view.clone(), ticking.clone());
                    }
                }
                UiMsg::Update { selected } => {
                    // Only the highlight is drawn, so only redraw when it moves.
                    if v.selected != selected {
                        v.selected = selected;
                        area.queue_draw();
                    }
                }
                UiMsg::AddItem => {
                    drop(v);
                    crate::form::open(&app);
                }
                UiMsg::Settings => {
                    drop(v);
                    crate::settings::open(&app);
                }
                UiMsg::Close { selected } => {
                    v.generation += 1;
                    v.progress = 1.0;
                    if selected.is_some() {
                        v.selected = selected;
                        v.chosen = true;
                        area.queue_draw();
                        let (area, view, generation_at_close) = (area.clone(), view.clone(), v.generation);
                        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                            let mut v = view.borrow_mut();
                            if v.generation == generation_at_close {
                                v.dial = None;
                                area.queue_draw();
                            }
                        });
                    } else {
                        v.dial = None;
                        area.queue_draw();
                    }
                }
            }
        }
    });
}

/// Frame-clock driver for the opening animation and animated centre images.
/// Stops itself once the dial is closed (or nothing is moving any more).
fn start_ticker(area: &gtk::DrawingArea, view: Rc<RefCell<View>>, ticking: Rc<RefCell<bool>>) {
    area.add_tick_callback(move |area, clock| {
        let mut v = view.borrow_mut();
        let Some(d) = v.dial.clone() else {
            *ticking.borrow_mut() = false;
            return glib::ControlFlow::Break;
        };
        let now = clock.frame_time();
        let mut redraw = false;
        if v.progress < 1.0 {
            let start = *v.anim_start.get_or_insert(now);
            v.progress = ((now - start) as f64 / 1000.0 / d.animation_ms.max(1) as f64).min(1.0);
            redraw = true;
        }
        let gif = v.image.as_ref().is_some_and(|i| i.is_animated());
        if gif {
            redraw |= v.image.as_ref().unwrap().advance(SystemTime::now());
        }
        if redraw {
            area.queue_draw();
        }
        if v.progress >= 1.0 && !gif {
            *ticking.borrow_mut() = false;
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

/// Reuse the previously loaded image if it's the same unchanged file.
pub(crate) fn load_image(prev: Option<CenterImage>, path: &str) -> Option<CenterImage> {
    if path.is_empty() {
        return None;
    }
    let path = PathBuf::from(path);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Some(mut p) = prev.filter(|p| p.path == path && p.mtime == mtime) {
        if let Some((anim, it)) = &mut p.gif {
            *it = anim.iter(Some(SystemTime::now())); // restart the GIF each open
            p.frame.set(p.frame.get() + 1);
        }
        return Some(p);
    }
    match PixbufAnimation::from_file(&path) {
        Ok(anim) if anim.is_static_image() => {
            let full = anim.static_image()?;
            let (w, h) = (full.width(), full.height());
            let k = (MAX_STILL_PX as f64 / w.max(h) as f64).min(1.0);
            let still = if k < 1.0 {
                full.scale_simple((w as f64 * k) as i32, (h as f64 * k) as i32, InterpType::Bilinear)
            } else {
                Some(full)
            };
            Some(CenterImage { path, mtime, still, gif: None, frame: Cell::new(0), cache: RefCell::new(None) })
        }
        Ok(anim) => {
            let iter = anim.iter(Some(SystemTime::now()));
            Some(CenterImage {
                path,
                mtime,
                still: None,
                gif: Some((anim, iter)),
                frame: Cell::new(0),
                cache: RefCell::new(None),
            })
        }
        Err(e) => {
            eprintln!("center_image {}: {e}", path.display());
            None
        }
    }
}

fn find_monitor(connector: &str) -> Option<gdk::Monitor> {
    let mons = gdk::Display::default()?.monitors();
    (0..mons.n_items())
        .filter_map(|i| mons.item(i).and_downcast::<gdk::Monitor>())
        .find(|m| m.connector().as_deref() == Some(connector))
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_back(t: f64) -> f64 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

pub(crate) fn draw(v: &View, cr: &Context, w: f64, h: f64) {
    let Some(d) = &v.dial else { return };
    let n = v.labels.len().max(1);
    let r_out = d.size / 2.0;
    let r_in = r_out * d.center_size;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let p = v.progress.clamp(0.0, 1.0);
    let e = ease_out_cubic(p);

    // Whole-dial transform + fade for the opening animation.
    let (alpha, scale, rot) = match d.animation {
        Animation::None => (1.0, 1.0, 0.0),
        Animation::Fade => (e, 1.0, 0.0),
        Animation::Pop => (e, 0.6 + 0.4 * ease_out_back(p), 0.0),
        Animation::Spin => (e, 0.85 + 0.15 * e, -(1.0 - e) * FRAC_PI_2),
        Animation::Unfold => (1.0, 1.0, 0.0),
    };
    let alpha = alpha * d.opacity;
    if alpha <= 0.0 {
        return;
    }
    cr.push_group();
    cr.translate(cx, cy);
    cr.rotate(rot);
    cr.scale(scale, scale);
    // From here on the dial is drawn around (0, 0).

    let (bg, accent, fg) = (rgba(&d.background), rgba(&d.accent), rgba(&d.foreground));
    let (sel_fg, centre) = (rgba(&d.selected_text), rgba(&d.center_color));
    let border = (!d.border.eq_ignore_ascii_case("none")).then(|| rgba(&d.border));
    let set = |c: (f64, f64, f64, f64), a: f64| cr.set_source_rgba(c.0, c.1, c.2, c.3 * a);

    let step = TAU / n as f64;
    let gap = if n > 1 { 0.025 } else { 0.0 };
    let ring_in = r_in + 4.0;
    for (i, label) in v.labels.iter().enumerate() {
        // "unfold": slices grow outwards one after another.
        let slice_p = if d.animation == Animation::Unfold {
            ease_out_cubic(((p * (n as f64 + 3.0) - i as f64) / 4.0).clamp(0.0, 1.0))
        } else {
            1.0
        };
        if slice_p <= 0.0 {
            continue;
        }
        let r_slice = ring_in + (r_out - ring_in) * slice_p;
        let mid = -FRAC_PI_2 + i as f64 * step;
        let (a0, a1) = (mid - step / 2.0 + gap, mid + step / 2.0 - gap);
        let sel = v.selected == Some(i);

        cr.new_path();
        cr.arc(0.0, 0.0, r_slice, a0, a1);
        cr.arc_negative(0.0, 0.0, ring_in, a1, a0);
        cr.close_path();
        if sel {
            set(accent, if v.chosen { 1.0 } else { 0.92 });
        } else {
            set(bg, slice_p);
        }
        if let Some(b) = border {
            let _ = cr.fill_preserve();
            set(b, slice_p);
            cr.set_line_width(d.border_width);
            let _ = cr.stroke();
        } else {
            let _ = cr.fill();
        }
        if slice_p < 1.0 {
            continue; // labels appear once their slice has unfolded
        }

        let weight = if sel { FontWeight::Bold } else { FontWeight::Normal };
        cr.select_font_face(&d.font, FontSlant::Normal, weight);
        let half_angle = if n > 1 { step / 2.0 - gap - 0.03 } else { f64::INFINITY };
        let lr = ring_in + (r_out - ring_in) * 0.5;
        let (size, text) = fit_label(cr, label, d.font_size, lr, mid, half_angle, ring_in + 5.0, r_out - 5.0);
        cr.set_font_size(size);
        if sel {
            set(sel_fg, 1.0);
        } else {
            set(fg, 1.0);
        }
        centered_text(cr, &text, lr * mid.cos(), lr * mid.sin());
    }

    // Centre circle: colour, then image (cover-fit, clipped), then optional ring.
    cr.new_path();
    cr.arc(0.0, 0.0, r_in, 0.0, TAU);
    set(centre, 1.0);
    let _ = cr.fill_preserve();
    if let Some(img) = &v.image {
        // Size in device pixels at the dial's final size (HiDPI-aware); the
        // opening animation then just scales this small cached surface.
        let (dsx, _) = cr.group_target().device_scale();
        let px = (2.0 * r_in * dsx).ceil() as i32;
        if let Some(surf) = img.surface(px) {
            let _ = cr.save();
            cr.clip();
            cr.translate(-r_in, -r_in);
            cr.scale(2.0 * r_in / px as f64, 2.0 * r_in / px as f64);
            let _ = cr.set_source_surface(&surf, 0.0, 0.0);
            let _ = cr.paint_with_alpha(d.center_image_opacity);
            let _ = cr.restore();
        }
    } else if v.selected.is_none() {
        cr.new_path();
        set(fg, 0.6);
        cr.select_font_face(&d.font, FontSlant::Normal, FontWeight::Normal);
        let (size, text) = fit_label(cr, "push to pick", 11.0, 0.0, 0.0, f64::INFINITY, 0.0, r_in - 6.0);
        cr.set_font_size(size);
        centered_text(cr, &text, 0.0, 0.0);
    }
    cr.new_path();
    if let Some(b) = border {
        cr.arc(0.0, 0.0, r_in, 0.0, TAU);
        set(b, 1.0);
        cr.set_line_width(d.border_width);
        let _ = cr.stroke();
    }

    let _ = cr.pop_group_to_source();
    let _ = cr.paint_with_alpha(alpha);
}

/// Largest font size (<= `max`) at which `text`, centred at radius `lr` / angle
/// `mid`, fits inside the ring sector [r_lo, r_hi] x [mid ± half_angle]. Below
/// MIN_FONT the text is shortened with "…" instead.
#[allow(clippy::too_many_arguments)]
fn fit_label(
    cr: &Context,
    text: &str,
    max: f64,
    lr: f64,
    mid: f64,
    half_angle: f64,
    r_lo: f64,
    r_hi: f64,
) -> (f64, String) {
    let (lx, ly) = (lr * mid.cos(), lr * mid.sin());
    let fits = |s: &str, size: f64| -> bool {
        cr.set_font_size(size);
        let Ok(e) = cr.text_extents(s) else { return false };
        let (hw, hh) = (e.width() / 2.0, e.height() / 2.0);
        [(-hw, -hh), (hw, -hh), (-hw, hh), (hw, hh)].iter().all(|&(dx, dy)| {
            let (x, y) = (lx + dx, ly + dy);
            let r = (x * x + y * y).sqrt();
            let off = (y.atan2(x) - mid + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
            r >= r_lo && r <= r_hi && off.abs() <= half_angle
        })
    };
    let mut size = max.max(MIN_FONT);
    while size >= MIN_FONT {
        if fits(text, size) {
            return (size, text.to_string());
        }
        size -= 0.5;
    }
    let chars: Vec<char> = text.chars().collect();
    for k in (1..chars.len()).rev() {
        let t: String = chars[..k].iter().collect::<String>().trim_end().to_string() + "…";
        if fits(&t, MIN_FONT) {
            return (MIN_FONT, t);
        }
    }
    (MIN_FONT, "…".into())
}

fn centered_text(cr: &Context, s: &str, x: f64, y: f64) {
    if let Ok(e) = cr.text_extents(s) {
        cr.move_to(x - (e.width() / 2.0 + e.x_bearing()), y - (e.height() / 2.0 + e.y_bearing()));
        let _ = cr.show_text(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        let s = cairo::ImageSurface::create(cairo::Format::ARgb32, 10, 10).unwrap();
        let cr = Context::new(&s).unwrap();
        cr.select_font_face("monospace", FontSlant::Normal, FontWeight::Bold);
        cr
    }

    // Geometry of an 8-slice, 340px dial (the default look).
    const R_OUT: f64 = 170.0;
    const RING_IN: f64 = 170.0 * 0.3 + 4.0;
    const STEP: f64 = TAU / 8.0;

    fn fit(text: &str, slice: usize) -> (f64, String) {
        let mid = -FRAC_PI_2 + slice as f64 * STEP;
        let lr = RING_IN + (R_OUT - RING_IN) * 0.5;
        fit_label(&ctx(), text, 15.0, lr, mid, STEP / 2.0 - 0.055, RING_IN + 5.0, R_OUT - 5.0)
    }

    #[test]
    fn short_labels_keep_full_size() {
        assert_eq!(fit("Menu", 0), (15.0, "Menu".into()));
    }

    #[test]
    fn long_label_shrinks_to_fit_its_slice() {
        // "Screenshot" in slice 3 (bottom-right) was drawn past the slice edge at 15px.
        let (size, text) = fit("Screenshot", 3);
        assert_eq!(text, "Screenshot");
        assert!(size < 15.0 && size >= MIN_FONT, "size {size}");
    }

    #[test]
    fn very_long_label_is_ellipsized() {
        let (size, text) = fit("An extremely long label that cannot possibly fit", 2);
        assert_eq!(size, MIN_FONT);
        assert!(text.ends_with('…') && text.len() < 40, "{text}");
    }

    #[test]
    fn eases_end_at_one() {
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-9);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-9);
        assert!(ease_out_back(0.7) > 1.0, "pop overshoots slightly");
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::config::Config;

    /// cargo test --release bench_draw -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_draw() {
        let (cfg, _) = Config::load().unwrap();
        for anim in Animation::ALL {
            let mut d = cfg.dial.resolved();
            d.animation = anim;
            let mut v = View {
                labels: cfg.items.iter().map(|i| i.label.clone()).collect(),
                image: load_image(None, &d.center_image),
                dial: Some(d.clone()),
                selected: Some(1),
                ..Default::default()
            };
            let px = ((d.size + 2.0 * PAD) * 2.0) as i32; // 2x HiDPI
            let rec = std::env::var_os("BENCH_RECORDING").is_some();
            let surf: cairo::Surface = if rec {
                (*cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None).unwrap()).clone()
            } else {
                (*cairo::ImageSurface::create(cairo::Format::ARgb32, px, px).unwrap()).clone()
            };
            surf.set_device_scale(2.0, 2.0); // like GTK on this 2x screen
            let n = 30;
            let t0 = std::time::Instant::now();
            for f in 0..n {
                v.progress = f as f64 / (n - 1) as f64;
                let cr = Context::new(&surf).unwrap();
                draw(&v, &cr, px as f64 / 2.0, px as f64 / 2.0);
            }
            let per = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("{:>7}: {per:6.2} ms/frame", anim.name());
        }
    }
}
