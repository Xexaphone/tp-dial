//! Settings window: edit the dial's look and trigger with a live preview, then
//! save into config.toml (comments and items are kept; see config::save_settings).
//!
//! A normal toplevel window rather than a layer surface, because GTK's colour,
//! font and file pickers open as separate windows, which would appear *under*
//! an overlay-layer surface. Hyprland floats + centres it via a window rule.

use crate::config::{self, rgba, Animation, Config, Dial, TriggerMode};
use crate::dial::{self, View, PAD};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use gtk4 as gtk;
use std::cell::RefCell;
use std::rc::Rc;

struct State {
    dial: Dial,
    mode: TriggerMode,
    min_push: f64,
    window_opacity: f64,
    /// Items as loaded (label, what it runs, is the Settings built-in) + marked for removal.
    items: Vec<(String, String, bool)>,
    removed: Vec<bool>,
}

impl State {
    fn visible_labels(&self) -> Vec<String> {
        self.items
            .iter()
            .zip(&self.removed)
            .filter(|(_, r)| !**r)
            .map(|((l, _, _), _)| l.clone())
            .collect()
    }
}

fn describe(it: &config::Item) -> String {
    it.launch().to_string()
}

type Shared = Rc<RefCell<State>>;
type Refresh = Rc<dyn Fn()>;

/// `tp-dial --settings`: show just the settings window; exits when it closes.
pub fn run_standalone() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.tpdial.Settings")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(open);
    app.run_with_args::<&str>(&[])
}

pub fn open(app: &gtk::Application) {
    let cfg = match Config::load() {
        Ok((c, _)) => c,
        Err(e) => {
            eprintln!("settings: {e}");
            return;
        }
    };
    let labels: Vec<String> = cfg.items.iter().map(|i| i.label.clone()).collect();
    let state: Shared = Rc::new(RefCell::new(State {
        dial: cfg.dial.clone(),
        mode: cfg.trigger.mode,
        min_push: cfg.trigger.min_push,
        window_opacity: cfg.settings_window.opacity,
        items: cfg
            .items
            .iter()
            .map(|i| (i.label.clone(), describe(i), i.action == Some(config::Builtin::Settings)))
            .collect(),
        removed: vec![false; cfg.items.len()],
    }));

    let win = gtk::ApplicationWindow::new(app);
    win.set_title(Some("tp-dial settings"));
    win.set_default_size(875, 600);

    // Window background: the theme's background colour at the chosen opacity.
    win.add_css_class("tp-settings");
    let win_css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &win_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
    );
    let apply_window_bg: Rc<dyn Fn()> = {
        let (state, win_css) = (state.clone(), win_css.clone());
        Rc::new(move || {
            let s = state.borrow();
            let d = s.dial.resolved();
            let (r, g, b, _) = rgba(&d.background);
            let (fr, fg, fb, _) = rgba(&d.foreground);
            // Match GTK's own widget styling (entries, dropdowns, buttons) to the
            // background, or e.g. on Windows its default light theme puts dark grey
            // text on a dark window.
            let dark = 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
            if let Some(settings) = gtk::Settings::default() {
                // GTK 4.20+ has gtk-interface-color-scheme; older GTK only the prefer-dark flag.
                let scheme = settings
                    .find_property("gtk-interface-color-scheme")
                    .and_then(|p| glib::EnumClass::with_type(p.value_type()))
                    .and_then(|c| c.to_value_by_nick(if dark { "dark" } else { "light" }));
                match scheme {
                    Some(v) => settings.set_property_from_value("gtk-interface-color-scheme", &v),
                    None => settings.set_gtk_application_prefer_dark_theme(dark),
                }
            }
            let rgb = |r: f64, g: f64, b: f64| format!("{}, {}, {}", (r * 255.0).round(), (g * 255.0).round(), (b * 255.0).round());
            win_css.load_from_string(&format!(
                "window.tp-settings, window.tp-settings.background {{ background-color: rgba({}, {:.2}); color: rgb({}); }}
                 window.tp-settings label {{ color: rgb({}); }}
                 window.tp-settings label.dim-label {{ color: rgba({}, 0.7); }}",
                rgb(r, g, b),
                s.window_opacity.clamp(0.2, 1.0),
                rgb(fr, fg, fb),
                rgb(fr, fg, fb),
                rgb(fr, fg, fb),
            ));
        })
    };
    apply_window_bg();

    // ---- live preview -------------------------------------------------------
    let view = Rc::new(RefCell::new(View {
        labels,
        selected: Some(1),
        progress: 1.0,
        ..Default::default()
    }));
    let preview = gtk::DrawingArea::new();
    preview.set_size_request(320, 320);
    preview.set_vexpand(true);
    {
        let view = view.clone();
        preview.set_draw_func(move |_, cr, w, h| {
            let v = view.borrow();
            let Some(d) = &v.dial else { return };
            let k = (w.min(h) as f64 / (d.size + 2.0 * PAD)).min(1.0);
            cr.scale(k, k);
            dial::draw(&v, cr, w as f64 / k, h as f64 / k);
        });
    }
    let refresh: Refresh = {
        let (state, view, preview) = (state.clone(), view.clone(), preview.clone());
        Rc::new(move || {
            let (d, labels) = {
                let s = state.borrow();
                (s.dial.resolved(), s.visible_labels())
            };
            let mut v = view.borrow_mut();
            v.image = dial::load_image(v.image.take(), &d.center_image);
            v.selected = (!labels.is_empty()).then_some(1.min(labels.len() - 1));
            v.labels = labels;
            v.dial = Some(d);
            preview.queue_draw();
        })
    };
    refresh();
    // Drives the "replay" animation and animated centre images in the preview.
    {
        let view = view.clone();
        preview.add_tick_callback(move |area, clock| {
            let mut v = view.borrow_mut();
            let Some(ms) = v.dial.as_ref().map(|d| d.animation_ms.max(1)) else {
                return glib::ControlFlow::Continue;
            };
            let mut redraw = false;
            if v.progress < 1.0 {
                let now = clock.frame_time();
                let start = *v.anim_start.get_or_insert(now);
                v.progress = ((now - start) as f64 / 1000.0 / ms as f64).min(1.0);
                redraw = true;
            }
            if let Some(img) = &v.image {
                if img.is_animated() {
                    redraw |= img.advance(std::time::SystemTime::now());
                }
            }
            if redraw {
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }
    let replay = gtk::Button::with_label("Replay animation");
    {
        let view = view.clone();
        replay.connect_clicked(move |_| {
            let mut v = view.borrow_mut();
            v.progress = 0.0;
            v.anim_start = None;
        });
    }

    // ---- form ---------------------------------------------------------------
    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let mut row = 0;
    let section = |grid: &gtk::Grid, row: &mut i32, title: &str| {
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
        l.set_halign(gtk::Align::Start);
        if *row > 0 {
            l.set_margin_top(10);
        }
        grid.attach(&l, 0, *row, 3, 1);
        *row += 1;
    };
    let label = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.set_halign(gtk::Align::Start);
        l
    };

    section(&grid, &mut row, "Settings window");
    grid.attach(&label("Window opacity"), 0, row, 1, 1);
    let win_opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.2, 1.0, 0.01);
    win_opacity.set_digits(2);
    win_opacity.set_draw_value(true);
    win_opacity.set_hexpand(true);
    win_opacity.set_value(state.borrow().window_opacity);
    {
        let (state, apply_window_bg) = (state.clone(), apply_window_bg.clone());
        win_opacity.connect_value_changed(move |s| {
            state.borrow_mut().window_opacity = s.value();
            apply_window_bg();
        });
    }
    grid.attach(&win_opacity, 1, row, 2, 1);
    row += 1;

    section(&grid, &mut row, "Dial items");
    let items_hint = label("Remove takes effect when you press Save. Add items from the dial's \"+ Add item\".");
    items_hint.add_css_class("dim-label");
    items_hint.set_wrap(true);
    grid.attach(&items_hint, 0, row, 3, 1);
    row += 1;
    let n_items = state.borrow().items.len();
    for i in 0..n_items {
        let (name, what, is_settings) = state.borrow().items[i].clone();
        let name_l = label(&name);
        let what_l = label(&what);
        what_l.add_css_class("dim-label");
        what_l.set_ellipsize(pango::EllipsizeMode::End);
        what_l.set_hexpand(true);
        what_l.set_tooltip_text(Some(&what));
        let btn = gtk::ToggleButton::with_label("Remove");
        if is_settings {
            btn.set_sensitive(false);
            btn.set_tooltip_text(Some("Kept so you can always get back to these settings"));
        }
        {
            let (state, refresh, name_l, what_l) = (state.clone(), refresh.clone(), name_l.clone(), what_l.clone());
            btn.connect_toggled(move |b| {
                let gone = b.is_active();
                b.set_label(if gone { "Undo" } else { "Remove" });
                name_l.set_opacity(if gone { 0.4 } else { 1.0 });
                what_l.set_text(if gone { "will be removed on Save" } else { &what });
                state.borrow_mut().removed[i] = gone;
                refresh();
            });
        }
        grid.attach(&name_l, 0, row, 1, 1);
        grid.attach(&what_l, 1, row, 1, 1);
        grid.attach(&btn, 2, row, 1, 1);
        row += 1;
    }

    section(&grid, &mut row, "Colours");
    type Get = fn(&Dial) -> &String;
    type Set = fn(&mut Dial, String);
    let colours: [(&str, Get, Set, bool); 6] = [
        ("Highlight", |d| &d.accent, |d, v| d.accent = v, false),
        ("Slices", |d| &d.background, |d, v| d.background = v, false),
        ("Text", |d| &d.foreground, |d, v| d.foreground = v, false),
        ("Highlighted text", |d| &d.selected_text, |d, v| d.selected_text = v, false),
        ("Centre", |d| &d.center_color, |d, v| d.center_color = v, false),
        ("Border", |d| &d.border, |d, v| d.border = v, true),
    ];
    for (name, get, set, allow_none) in colours {
        grid.attach(&label(name), 0, row, 1, 1);
        let (dd, btn) = colour_picker(&state, &refresh, get, set, allow_none);
        grid.attach(&dd, 1, row, 1, 1);
        grid.attach(&btn, 2, row, 1, 1);
        row += 1;
    }
    grid.attach(&label("Border width"), 0, row, 1, 1);
    grid.attach(&spin(&state, &refresh, 0.0, 8.0, 0.5, |d| d.border_width, |d, v| d.border_width = v), 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Opacity"), 0, row, 1, 1);
    grid.attach(&slider(&state, &refresh, 0.1, 1.0, |d| d.opacity, |d, v| d.opacity = v), 1, row, 2, 1);
    row += 1;

    section(&grid, &mut row, "Size and text");
    grid.attach(&label("Dial size (px)"), 0, row, 1, 1);
    grid.attach(&spin(&state, &refresh, 200.0, 800.0, 10.0, |d| d.size, |d, v| d.size = v), 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Centre size"), 0, row, 1, 1);
    grid.attach(&slider(&state, &refresh, 0.1, 0.6, |d| d.center_size, |d, v| d.center_size = v), 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Font"), 0, row, 1, 1);
    let (font_dd, font_btn) = font_picker(&state, &refresh);
    grid.attach(&font_dd, 1, row, 1, 1);
    grid.attach(&font_btn, 2, row, 1, 1);
    row += 1;
    grid.attach(&label("Largest font size"), 0, row, 1, 1);
    grid.attach(&spin(&state, &refresh, 8.0, 32.0, 1.0, |d| d.font_size, |d, v| d.font_size = v), 1, row, 2, 1);
    row += 1;

    section(&grid, &mut row, "Centre image");
    let img_label = gtk::Label::new(None);
    img_label.set_halign(gtk::Align::Start);
    img_label.set_ellipsize(pango::EllipsizeMode::Start);
    img_label.set_hexpand(true);
    let set_img_label = {
        let img_label = img_label.clone();
        move |p: &str| img_label.set_text(if p.is_empty() { "None" } else { p })
    };
    set_img_label(&state.borrow().dial.center_image);
    let choose = gtk::Button::with_label("Choose…");
    let clear = gtk::Button::with_label("Clear");
    {
        let (state, refresh, win, set_img_label) = (state.clone(), refresh.clone(), win.clone(), set_img_label.clone());
        choose.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images and GIFs"));
            filter.add_pixbuf_formats();
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder().title("Centre image").filters(&filters).build();
            let (state, refresh, set_img_label) = (state.clone(), refresh.clone(), set_img_label.clone());
            dialog.open(Some(&win), None::<&gio::Cancellable>, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    let p = path.display().to_string();
                    set_img_label(&p);
                    state.borrow_mut().dial.center_image = p;
                    refresh();
                }
            });
        });
    }
    {
        let (state, refresh) = (state.clone(), refresh.clone());
        clear.connect_clicked(move |_| {
            set_img_label("");
            state.borrow_mut().dial.center_image.clear();
            refresh();
        });
    }
    let img_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    img_row.append(&img_label);
    img_row.append(&choose);
    img_row.append(&clear);
    grid.attach(&label("Image / GIF"), 0, row, 1, 1);
    grid.attach(&img_row, 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Image opacity"), 0, row, 1, 1);
    grid.attach(
        &slider(&state, &refresh, 0.0, 1.0, |d| d.center_image_opacity, |d, v| d.center_image_opacity = v),
        1,
        row,
        2,
        1,
    );
    row += 1;

    section(&grid, &mut row, "Animation");
    let names: Vec<&str> = Animation::ALL.iter().map(|a| a.name()).collect();
    let anim = gtk::DropDown::from_strings(&names);
    anim.set_selected(Animation::ALL.iter().position(|a| *a == state.borrow().dial.animation).unwrap_or(0) as u32);
    {
        let (state, refresh, view) = (state.clone(), refresh.clone(), view.clone());
        anim.connect_selected_notify(move |dd| {
            state.borrow_mut().dial.animation = Animation::ALL[dd.selected() as usize];
            refresh();
            let mut v = view.borrow_mut(); // show it straight away
            v.progress = 0.0;
            v.anim_start = None;
        });
    }
    grid.attach(&label("Style"), 0, row, 1, 1);
    grid.attach(&anim, 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Duration (ms)"), 0, row, 1, 1);
    grid.attach(
        &spin(&state, &refresh, 0.0, 1000.0, 10.0, |d| d.animation_ms as f64, |d, v| d.animation_ms = v as u64),
        1,
        row,
        2,
        1,
    );
    row += 1;

    section(&grid, &mut row, "Opening the dial");
    let mode = gtk::DropDown::from_strings(&["Any TrackPoint push", "Flick and hold"]);
    mode.set_selected(if state.borrow().mode == TriggerMode::Any { 0 } else { 1 });
    let push = gtk::SpinButton::with_range(4.0, 40.0, 1.0);
    push.set_value(state.borrow().min_push);
    push.set_sensitive(state.borrow().mode == TriggerMode::Any);
    {
        let (state, push) = (state.clone(), push.clone());
        mode.connect_selected_notify(move |dd| {
            let m = if dd.selected() == 0 { TriggerMode::Any } else { TriggerMode::Flick };
            state.borrow_mut().mode = m;
            push.set_sensitive(m == TriggerMode::Any);
        });
    }
    {
        let state = state.clone();
        push.connect_value_changed(move |s| state.borrow_mut().min_push = s.value());
    }
    grid.attach(&label("Opens on"), 0, row, 1, 1);
    grid.attach(&mode, 1, row, 2, 1);
    row += 1;
    grid.attach(&label("Push needed"), 0, row, 1, 1);
    grid.attach(&push, 1, row, 2, 1);

    // ---- layout + save ------------------------------------------------------
    let scroller = gtk::ScrolledWindow::builder()
        .child(&grid)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .build();
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_margin_top(16);
    right.set_margin_end(16);
    right.set_margin_bottom(16);
    right.append(&preview);
    right.append(&replay);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&scroller);
    body.append(&right);

    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_hexpand(true);
    error.set_halign(gtk::Align::Start);
    error.set_wrap(true);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.set_margin_start(16);
    bar.set_margin_end(16);
    bar.set_margin_bottom(12);
    bar.append(&error);
    bar.append(&cancel);
    bar.append(&save);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&body);
    root.append(&bar);
    win.set_child(Some(&root));

    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, state, error) = (win.clone(), state.clone(), error.clone());
        save.connect_clicked(move |_| {
            let s = state.borrow();
            let removed: Vec<(usize, String)> = s
                .items
                .iter()
                .enumerate()
                .filter(|(i, _)| s.removed[*i])
                .map(|(i, (l, _, _))| (i, l.clone()))
                .collect();
            match config::save_settings_to_file(&s.dial, s.mode, s.min_push, s.window_opacity, &removed) {
                Ok(()) => {
                    eprintln!("settings saved");
                    let _ = std::process::Command::new("notify-send")
                        .args(["-a", "tp-dial", "Dial settings saved", "They apply the next time you open the dial."])
                        .spawn()
                        .map(|mut c| std::thread::spawn(move || c.wait()));
                    win.close();
                }
                Err(e) => error.set_text(&e),
            }
        });
    }
    let keys = gtk::EventControllerKey::new();
    {
        let win = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                win.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(keys);
    win.present();
}

fn to_hex(c: &gdk::RGBA) -> String {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut s = format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()));
    if c.alpha() < 0.999 {
        s += &format!("{:02x}", b(c.alpha()));
    }
    s
}

fn to_rgba(hex: &str) -> gdk::RGBA {
    let (r, g, b, a) = rgba(hex);
    gdk::RGBA::new(r as f32, g as f32, b as f32, a as f32)
}

/// [Theme | Custom (| None)] dropdown + colour button for one colour setting.
fn colour_picker(
    state: &Shared,
    refresh: &Refresh,
    get: fn(&Dial) -> &String,
    set: fn(&mut Dial, String),
    allow_none: bool,
) -> (gtk::DropDown, gtk::ColorDialogButton) {
    let options: &[&str] = if allow_none { &["Theme", "Custom", "None"] } else { &["Theme", "Custom"] };
    let dd = gtk::DropDown::from_strings(options);
    let dialog = gtk::ColorDialog::new();
    dialog.set_with_alpha(true);
    let btn = gtk::ColorDialogButton::new(Some(dialog));

    let current = get(&state.borrow().dial).clone();
    let resolved = {
        // What "theme" currently means, so switching to Custom starts from it.
        let mut d = state.borrow().dial.clone();
        set(&mut d, "theme".into());
        get(&d.resolved()).clone()
    };
    let kind = match current.to_ascii_lowercase().as_str() {
        "theme" => 0,
        "none" if allow_none => 2,
        _ => 1,
    };
    dd.set_selected(kind);
    btn.set_rgba(&to_rgba(if kind == 1 { &current } else { &resolved }));
    btn.set_sensitive(kind == 1);

    {
        let (state, refresh, btn) = (state.clone(), refresh.clone(), btn.clone());
        dd.connect_selected_notify(move |dd| {
            let v = match dd.selected() {
                0 => "theme".to_string(),
                2 => "none".to_string(),
                _ => to_hex(&btn.rgba()),
            };
            btn.set_sensitive(dd.selected() == 1);
            set(&mut state.borrow_mut().dial, v);
            refresh();
        });
    }
    {
        let (state, refresh, dd) = (state.clone(), refresh.clone(), dd.clone());
        btn.connect_rgba_notify(move |b| {
            if dd.selected() == 1 {
                set(&mut state.borrow_mut().dial, to_hex(&b.rgba()));
                refresh();
            }
        });
    }
    (dd, btn)
}

/// [Default font | Custom] dropdown + font family button. "default" is the
/// platform's font (monospace / Omarchy's font on Linux, Segoe UI on Windows).
fn font_picker(state: &Shared, refresh: &Refresh) -> (gtk::DropDown, gtk::FontDialogButton) {
    let dd = gtk::DropDown::from_strings(&["Default font", "Custom"]);
    let btn = gtk::FontDialogButton::new(Some(gtk::FontDialog::new()));
    btn.set_level(gtk::FontLevel::Family);
    let current = state.borrow().dial.font.clone();
    let is_default = |f: &str| f.eq_ignore_ascii_case("default") || f == "monospace";
    let custom = !is_default(&current);
    dd.set_selected(custom as u32);
    let shown = if custom { current.clone() } else { crate::platform::default_font().to_string() };
    btn.set_font_desc(&pango::FontDescription::from_string(&shown));
    btn.set_sensitive(custom);
    let family = |b: &gtk::FontDialogButton| {
        b.font_desc().and_then(|f| f.family()).map(|f| f.to_string()).unwrap_or_else(|| "monospace".into())
    };
    {
        let (state, refresh, btn) = (state.clone(), refresh.clone(), btn.clone());
        dd.connect_selected_notify(move |dd| {
            let custom = dd.selected() == 1;
            btn.set_sensitive(custom);
            state.borrow_mut().dial.font = if custom { family(&btn) } else { "default".into() };
            refresh();
        });
    }
    {
        let (state, refresh, dd) = (state.clone(), refresh.clone(), dd.clone());
        btn.connect_font_desc_notify(move |b| {
            if dd.selected() == 1 {
                state.borrow_mut().dial.font = family(b);
                refresh();
            }
        });
    }
    (dd, btn)
}

fn slider(
    state: &Shared,
    refresh: &Refresh,
    min: f64,
    max: f64,
    get: fn(&Dial) -> f64,
    set: fn(&mut Dial, f64),
) -> gtk::Scale {
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 0.01);
    s.set_digits(2);
    s.set_draw_value(true);
    s.set_hexpand(true);
    s.set_value(get(&state.borrow().dial));
    let (state, refresh) = (state.clone(), refresh.clone());
    s.connect_value_changed(move |s| {
        set(&mut state.borrow_mut().dial, s.value());
        refresh();
    });
    s
}

fn spin(
    state: &Shared,
    refresh: &Refresh,
    min: f64,
    max: f64,
    step: f64,
    get: fn(&Dial) -> f64,
    set: fn(&mut Dial, f64),
) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, step);
    s.set_value(get(&state.borrow().dial));
    let (state, refresh) = (state.clone(), refresh.clone());
    s.connect_value_changed(move |s| {
        set(&mut state.borrow_mut().dial, s.value());
        refresh();
    });
    s
}
