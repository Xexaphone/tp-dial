//! The dial's on-screen surface and the UI message loop. Runs on the GTK main
//! thread (which also hosts the settings window and add-item form). The surface
//! itself is per platform:
//!  - Linux: a GTK layer-shell surface (overlay/layer.rs)
//!  - Windows: a native layered, click-through, topmost window (overlay/win32.rs)
//! Both stay alive the whole time and draw nothing while the dial is closed.

use crate::config::{Animation, Dial};
use crate::dial::{load_image, View};
use crate::session::UiMsg;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[cfg(target_os = "linux")]
mod layer;
#[cfg(target_os = "linux")]
use layer::Surface;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
use win32::Surface;

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
    // No window is visible most of the time; keep the app alive regardless.
    std::mem::forget(app.hold());

    // Decode the centre image now, not on the first open (big files take a moment).
    let view = Rc::new(RefCell::new(View { image: load_image(None, &dial.center_image), ..Default::default() }));
    let surface = Rc::new(Surface::new(app, &dial, view.clone()));
    let ticking = Rc::new(Cell::new(false));
    let app = app.clone();

    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                UiMsg::Open { labels, dial, place } => {
                    surface.place(&dial, &place);
                    let needs_ticks = {
                        let mut v = view.borrow_mut();
                        let animated = dial.animation != Animation::None && dial.animation_ms > 0;
                        let image = load_image(v.image.take(), &dial.center_image);
                        let generation = v.generation + 1;
                        *v = View {
                            labels,
                            progress: if animated { 0.0 } else { 1.0 },
                            dial: Some(dial),
                            generation,
                            image,
                            ..Default::default()
                        };
                        animated || v.image.as_ref().is_some_and(|i| i.is_animated())
                    };
                    surface.redraw();
                    if needs_ticks && !ticking.get() {
                        ticking.set(true);
                        surface.start_ticks(ticking.clone());
                    }
                }
                UiMsg::Update { selected } => {
                    // Only the highlight is drawn, so only redraw when it moves.
                    let changed = {
                        let mut v = view.borrow_mut();
                        let changed = v.selected != selected;
                        v.selected = selected;
                        changed
                    };
                    if changed {
                        surface.redraw();
                    }
                }
                UiMsg::AddItem => crate::form::open(&app),
                UiMsg::Settings => crate::settings::open(&app),
                UiMsg::Close { selected } => {
                    let generation = {
                        let mut v = view.borrow_mut();
                        v.generation += 1;
                        v.progress = 1.0;
                        if selected.is_some() {
                            // Brief flash of the chosen slice before hiding.
                            v.selected = selected;
                            v.chosen = true;
                        } else {
                            v.dial = None;
                        }
                        v.generation
                    };
                    surface.redraw();
                    if selected.is_some() {
                        let (surface, view) = (surface.clone(), view.clone());
                        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                            let hide = {
                                let mut v = view.borrow_mut();
                                let hide = v.generation == generation;
                                if hide {
                                    v.dial = None;
                                }
                                hide
                            };
                            if hide {
                                surface.redraw();
                            }
                        });
                    }
                }
            }
        }
    });
}
