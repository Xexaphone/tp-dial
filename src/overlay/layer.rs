//! Linux: a GTK4 layer-shell surface on the Overlay layer (any wlroots-style
//! compositor). Small, transparent and click-through; it stays mapped and just
//! draws nothing while the dial is closed, so opening costs one redraw. While
//! open, the frame clock drives the opening animation and animated images.

use crate::config::Dial;
use crate::dial::{self, View, PAD};
use crate::platform::Placement;
use gtk::cairo;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct Surface {
    win: gtk::ApplicationWindow,
    area: gtk::DrawingArea,
    view: Rc<RefCell<View>>,
}

fn side(d: &Dial) -> i32 {
    (d.size + 2.0 * PAD).ceil() as i32
}

impl Surface {
    pub fn new(app: &gtk::Application, dial: &Dial, view: Rc<RefCell<View>>) -> Self {
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
        css.load_from_string(
            "window.tp-dial-transparent, window.tp-dial-transparent.background { background: transparent; }",
        );
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

        let area = gtk::DrawingArea::new();
        area.set_size_request(side(dial), side(dial));
        {
            let view = view.clone();
            // TP_DIAL_TRACE=1 logs each frame's draw time (debugging animation smoothness).
            let trace = std::env::var_os("TP_DIAL_TRACE").is_some();
            area.set_draw_func(move |_, cr, w, h| {
                let t = std::time::Instant::now();
                dial::draw(&view.borrow(), cr, w as f64, h as f64);
                if trace {
                    eprintln!("frame drawn in {:.2}ms", t.elapsed().as_secs_f64() * 1000.0);
                }
            });
        }
        win.set_child(Some(&area));
        win.present(); // mapped for good; draws nothing until a dial opens
        Self { win, area, view }
    }

    pub fn place(&self, dial: &Dial, place: &Placement) {
        let px = side(dial);
        if self.area.size_request() != (px, px) {
            self.area.set_size_request(px, px);
        }
        let target = place.monitor.as_deref().and_then(find_monitor);
        if target.is_some() && target != self.win.monitor() {
            self.win.set_monitor(target.as_ref());
        }
        match place.center {
            Some((x, y)) => {
                let half = px as f64 / 2.0;
                self.win.set_anchor(Edge::Top, true);
                self.win.set_anchor(Edge::Left, true);
                self.win.set_margin(Edge::Left, (x - half).max(0.0) as i32);
                self.win.set_margin(Edge::Top, (y - half).max(0.0) as i32);
            }
            None => {
                self.win.set_anchor(Edge::Top, false);
                self.win.set_anchor(Edge::Left, false);
            }
        }
    }

    pub fn redraw(&self) {
        self.area.queue_draw();
    }

    /// Frame-clock driver; stops itself once nothing is moving or the dial closed.
    pub fn start_ticks(&self, ticking: Rc<Cell<bool>>) {
        let view = self.view.clone();
        self.area.add_tick_callback(move |area, clock| {
            let (redraw, keep) = dial::tick(&mut view.borrow_mut(), clock.frame_time());
            if redraw {
                area.queue_draw();
            }
            if keep {
                glib::ControlFlow::Continue
            } else {
                ticking.set(false);
                glib::ControlFlow::Break
            }
        });
    }
}

fn find_monitor(connector: &str) -> Option<gdk::Monitor> {
    let mons = gdk::Display::default()?.monitors();
    (0..mons.n_items())
        .filter_map(|i| mons.item(i).and_downcast::<gdk::Monitor>())
        .find(|m| m.connector().as_deref() == Some(connector))
}
