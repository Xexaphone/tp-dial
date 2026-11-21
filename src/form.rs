//! "Add item" form, opened from the dial's built-in `action = "add-item"` slice.
//! A centred layer-shell surface with exclusive keyboard focus, so it floats
//! above everything without needing Hyprland window rules.

use crate::config::{self, rgba, Config, Kind};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

const KINDS: [(&str, Kind, &str, &str); 3] = [
    ("App", Kind::App, "", "Pick one of your installed apps (type to search)."),
    ("Script", Kind::Script, "e.g. ~/bin/backup.sh", "Path to an executable file."),
    ("Command", Kind::Command, "e.g. wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle", "Any shell command line."),
];

/// `tp-dial --add-item`: show just the form; exits when it closes.
pub fn run_standalone() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("dev.tpdial.AddItem")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(open);
    app.run_with_args::<&str>(&[])
}

pub fn open(app: &gtk::Application) {
    // Colours follow the dial's config.
    let (accent, bg, fg) = match Config::load() {
        Ok((c, _)) => {
            let d = c.dial.resolved();
            (d.accent, d.background, d.foreground)
        }
        Err(_) => ("#7aa2f7".into(), "#16161ee6".into(), "#c0caf5".into()),
    };
    let css_rgba = |hex: &str, force_opaque: bool| {
        let (r, g, b, a) = rgba(hex);
        let a = if force_opaque { 1.0 } else { a };
        format!("rgba({},{},{},{a})", (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    };
    let css = gtk::CssProvider::new();
    css.load_from_string(&format!(
        "window.tp-dial-transparent, window.tp-dial-transparent.background {{ background: transparent; }}
         .tp-form {{ background: {bg}; color: {fg}; border-radius: 14px; padding: 20px; border: 1px solid {acc}; }}
         .tp-form label.title {{ font-size: 16pt; font-weight: bold; }}
         .tp-form label.hint {{ opacity: 0.7; font-size: 9pt; }}
         .tp-form label.error {{ color: #f7768e; }}
         .tp-form button.suggested-action {{ background: {acc}; color: {bg_solid}; }}",
        bg = css_rgba(&bg, true),
        fg = css_rgba(&fg, false),
        acc = css_rgba(&accent, false),
        bg_solid = css_rgba(&bg, true),
    ));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );

    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("tp-dial-transparent"); // only the rounded .tp-form box is drawn
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_keyboard_mode(KeyboardMode::Exclusive);
    win.set_namespace(Some("tp-dial-form"));
    win.set_default_size(460, -1);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("tp-form");

    let title = gtk::Label::new(Some("Add dial item"));
    title.add_css_class("title");
    title.set_halign(gtk::Align::Start);

    let label = gtk::Entry::builder().placeholder_text("Label shown on the dial").build();
    let kind = gtk::DropDown::from_strings(&KINDS.map(|k| k.0));
    let value = gtk::Entry::new();

    // Installed apps, by display name -> desktop id (what uwsm-app needs).
    let mut apps: Vec<(String, String)> = gtk::gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|a| Some((a.display_name().to_string(), a.id()?.to_string())))
        .collect();
    apps.sort_by_key(|(n, _)| n.to_lowercase());
    apps.dedup_by(|a, b| a.1 == b.1);
    // Index 0 is a placeholder (GTK dropdowns always select something).
    let names: Vec<&str> = std::iter::once("Choose an app…")
        .chain(apps.iter().map(|(n, _)| n.as_str()))
        .collect();
    let app_dd = gtk::DropDown::from_strings(&names);
    app_dd.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<&gtk::Expression>,
        "string",
    )));
    app_dd.set_enable_search(true);
    app_dd.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    app_dd.set_selected(0);
    app_dd.set_hexpand(true);
    let apps = std::rc::Rc::new(apps);

    // Fill the label from the chosen app unless the user typed their own.
    let auto_label = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    {
        let (label, apps, auto_label) = (label.clone(), apps.clone(), auto_label.clone());
        app_dd.connect_selected_notify(move |dd| {
            if let Some((name, _)) = (dd.selected() as usize).checked_sub(1).and_then(|i| apps.get(i)) {
                let current = label.text().to_string();
                if current.is_empty() || current == *auto_label.borrow() {
                    label.set_text(name);
                    *auto_label.borrow_mut() = name.clone();
                }
            }
        });
    }
    let hint = gtk::Label::new(None);
    hint.add_css_class("hint");
    hint.set_halign(gtk::Align::Start);
    hint.set_wrap(true);
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_halign(gtk::Align::Start);
    error.set_wrap(true);

    let sync_kind = {
        let (value, app_dd, hint) = (value.clone(), app_dd.clone(), hint.clone());
        move |dd: &gtk::DropDown| {
            let k = &KINDS[dd.selected() as usize];
            let is_app = k.1 == Kind::App;
            value.set_visible(!is_app);
            app_dd.set_visible(is_app);
            value.set_placeholder_text(Some(k.2));
            hint.set_text(k.3);
        }
    };
    sync_kind(&kind);
    kind.connect_selected_notify(sync_kind);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&kind);
    value.set_hexpand(true);
    row.append(&value);
    row.append(&app_dd);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let add = gtk::Button::with_label("Add to dial");
    add.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&add);

    for w in [title.upcast_ref::<gtk::Widget>(), label.upcast_ref(), row.upcast_ref(), hint.upcast_ref(), error.upcast_ref(), buttons.upcast_ref()] {
        root.append(w);
    }
    win.set_child(Some(&root));

    // Warn up front if the dial is already full.
    if let Ok((c, _)) = Config::load() {
        if c.items.len() >= config::MAX_ITEMS {
            error.set_text(&format!(
                "The dial already has the maximum {} items. Remove one in {} first.",
                config::MAX_ITEMS,
                Config::path().display()
            ));
        }
    }

    let submit = {
        let (win, label, kind, value, error) = (win.clone(), label.clone(), kind.clone(), value.clone(), error.clone());
        let (app_dd, apps) = (app_dd.clone(), apps.clone());
        move || {
            let k = KINDS[kind.selected() as usize].1;
            let v = if k == Kind::App {
                match (app_dd.selected() as usize).checked_sub(1).and_then(|i| apps.get(i)) {
                    Some((_, id)) => id.clone(),
                    None => return error.set_text("Pick an app from the list."),
                }
            } else {
                value.text().to_string()
            };
            let name = label.text().trim().to_string();
            if let Ok((c, _)) = Config::load() {
                if c.items.iter().any(|i| i.label.eq_ignore_ascii_case(&name)) {
                    return error.set_text(&format!("There's already an item called \"{name}\". Pick another label."));
                }
            }
            match config::add_item_to_file(k, &name, &v) {
                Ok(()) => {
                    eprintln!("added item {name:?} ({v})");
                    let _ = std::process::Command::new("notify-send")
                        .args(["-a", "tp-dial", "Added to the dial", &format!("\"{name}\" will show next time you open it.")])
                        .spawn()
                        .map(|mut c| std::thread::spawn(move || c.wait()));
                    win.close();
                }
                Err(e) => error.set_text(&e),
            }
        }
    };
    {
        let submit = submit.clone();
        add.connect_clicked(move |_| submit());
    }
    {
        let submit = submit.clone();
        value.connect_activate(move |_| submit());
    }
    {
        let value = value.clone();
        label.connect_activate(move |_| {
            value.grab_focus();
        });
    }
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let keys = gtk::EventControllerKey::new();
    {
        let win = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                win.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(keys);

    win.present();
    label.grab_focus();
}

use gtk::glib;
