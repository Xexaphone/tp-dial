# tp-dial

Push the TrackPoint: a radial dial opens in the centre of the screen. Push towards an item and let go to run it.

## Where it runs

| Platform | Status |
|---|---|
| **Linux, Hyprland** (incl. Omarchy) | Full support: dial on the monitor under the cursor, cursor restore, Hyprland window rules, Omarchy theme and defaults |
| **Linux, other wlroots-style Wayland compositors** (Sway, niri, river, Wayfire, labwc) | Supported. The compositor picks the output (usually the focused one); `position = "cursor"` falls back to centred. KDE Plasma also implements layer-shell, so it should work, but it's untested |
| **Linux, GNOME** | Not supported: GNOME has no layer-shell for the overlay |
| **Windows 11** | Supported, untested on hardware so far (see [Windows](#windows)) |

## Gesture

`[trigger] mode` chooses how the dial opens:

- **`any`** (default): any real push opens the dial straight away. Resting drift below `min_push` is ignored. The TrackPoint no longer moves the cursor; use the touchpad for that. Its buttons still click, and moving it while a button is held still drags or middle-scrolls.
- **`flick`**: the TrackPoint moves the cursor as normal. A sharp push (`trigger_speed` reached within `rise_ms`) held for `hold_ms` opens the dial.

While the dial is open, whichever item you're pushing towards is highlighted. Easing off keeps the highlight, so letting go runs it. Pick **Close** or click a TrackPoint button to cancel.

## Linux

### Build

Needs Rust, GTK 4.12+ and gtk4-layer-shell:

| Distro | Packages |
|---|---|
| Arch / Omarchy | `sudo pacman -S rust gtk4 gtk4-layer-shell` |
| Fedora | `sudo dnf install cargo gtk4-devel gtk4-layer-shell-devel` |
| Debian 13 / Ubuntu 24.10+ | `sudo apt install cargo libgtk-4-dev libgtk4-layer-shell-dev` |

### Permissions (once)

```sh
sudo ./setup-permissions.sh
```

This installs `/etc/udev/rules.d/70-tp-dial.rules`, which gives your logged-in session access to the TrackPoint event device and `/dev/uinput` through `uaccess` ACLs (systemd-logind). No group change or re-login is needed. tp-dial refuses to run as root.

tp-dial grabs the TrackPoint exclusively and re-emits its events through a virtual clone, `<name> (tp-dial)`. That clone is how motion gets swallowed while the dial is open. If tp-dial exits or crashes, the kernel releases the grab and the real device works again. It finds the TrackPoint by name; if it picks the wrong device, set `device = "part of its name"` under `[trigger]`.

### Install and autostart

```sh
./install.sh     # build, copy to ~/.local/bin/tp-dial, enable + restart the user service
```

It runs as the systemd user service `tp-dial.service`, started with the graphical session (your compositor must start `graphical-session.target`, as uwsm and most Sway/niri setups do) and restarted if it crashes. On Hyprland it registers its own window rules over IPC, so `omarchy refresh hyprland` can't remove anything. On Sway it makes the settings window float with `swaymsg`.

```sh
journalctl --user -t tp-dial -f     # its log
```

Re-run `./install.sh` after changing the code. Config changes don't need it.

## Windows

### Install

1. Download **`tp-dial-setup.exe`** from the [latest release](https://github.com/Xexaphone/tp-dial/releases/latest).
2. Run it. It installs for your user only, so there's no administrator prompt.
3. At the end, leave **Find my TrackPoint now** ticked and push the TrackPoint when asked. That's how tp-dial knows which device it is.

That's it: tp-dial starts straight away, and (if you kept the option ticked) whenever you sign in. The Start Menu gets **tp-dial**, **tp-dial Settings** and **Find my TrackPoint**. Uninstall it from *Settings → Apps* like any other app; your config in `%APPDATA%\tp-dial` is kept.

Windows may warn that the installer is from an unknown publisher, because it isn't code-signed. Choose *More info → Run anyway*.

Prefer no installer? The release also has **`tp-dial-windows-portable.zip`**: unzip it anywhere and run `bin\tp-dial.exe`. Run `bin\tp-dial.exe --find-device` once, and `--install-autostart` if you want it at sign-in.

### How it works on Windows

- **Input:** Raw Input identifies the TrackPoint's own movement. A low-level mouse hook stops it moving the cursor, but Windows doesn't tell the hook which device moved, so this is best effort: the first report of a push can still nudge the cursor slightly. Your touchpad and mouse are unaffected.
- **Overlay:** a topmost, click-through layered window, drawn with the same renderer as Linux and scaled for the monitor's DPI.
- **Items:** `app` items go through `start` (program names, paths, or URIs like `ms-settings:`), `command` items through `cmd`, and `.ps1` scripts through PowerShell.
- **Theme:** `"theme"` colours follow your Windows accent colour and light/dark mode.

### Build from source

Only needed if you're changing the code.

1. Install [MSYS2](https://www.msys2.org/), open the **UCRT64** shell and run:
   ```sh
   pacman -S mingw-w64-ucrt-x86_64-rust mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-gtk4
   ```
2. In that shell, run `cargo build --release` in this folder.
3. To make a self-contained folder like the release's, run `packaging/windows/bundle.sh` (output in `dist/tp-dial`).

GitHub builds, checks and packages every push to `master` / `cross-platform` (see *Actions*); pushing a tag like `v0.2.0` publishes a release. To check the Windows code compiles from Linux (no linking, no Windows GTK needed): `./scripts/check-windows.sh`.

## Using it

```sh
tp-dial --debug      # shows the speed of each report and the gesture phase, for tuning
tp-dial --demo       # draws the dial with a scripted sweep, no TrackPoint needed
tp-dial --check      # validates the config and lists items
tp-dial --settings   # appearance, trigger and item settings, with a live preview
tp-dial --add-item   # add an item
```

## Custom items

Easiest: **+ Add item** on the dial. Remove items under **Dial items** in Settings.

Or edit the config file (`~/.config/tp-dial/config.toml`, or `%APPDATA%\tp-dial\config.toml` on Windows). Changes are picked up the next time you touch the stick. Items run clockwise from the top, up to 12:

```toml
[[item]]
label = "Obsidian"
app = "obsidian"                # Linux: uwsm-app / gtk-launch when available; Windows: `start obsidian`

[[item]]
label = "Backup"
script = "~/bin/backup.sh"      # must be executable (.ps1 / .bat / .cmd on Windows)

[[item]]
label = "Mute"
command = "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
```

The first config is written for the platform it's created on: Omarchy launchers on Omarchy, freedesktop defaults (terminal, default browser, files) on other Linux desktops, and Windows Terminal, Explorer, Snipping Tool and Task Manager on Windows.

## Appearance

Easiest: pick **Settings** on the dial (or run `tp-dial --settings`). It has a live preview and saves into the config file, keeping your comments and items.

Or edit `[dial]` in the config directly; changes apply the next time the dial opens.

| Option | What it does |
|---|---|
| `accent`, `background`, `foreground`, `selected_text`, `center_color` | Colours: `"theme"` or a hex colour (`#rrggbb` or `#rrggbbaa`) |
| `border`, `border_width` | Outline round slices and the centre: `"none"`, `"theme"` or a colour |
| `opacity` | Transparency of the whole dial, 0.0–1.0 |
| `font`, `font_size` | Label font (`"default"`: your monospace font on Linux, Segoe UI on Windows) and largest size. Labels that don't fit shrink, then end in "…" |
| `size`, `center_size` | Dial diameter in px; centre circle as a fraction of the radius |
| `center_image`, `center_image_opacity` | PNG, JPG, SVG or animated GIF in the centre, cropped to the circle |
| `animation`, `animation_ms` | How the dial appears: `none`, `fade`, `pop`, `spin` or `unfold` |

`"theme"` colours come from the desktop and are re-read each time the dial opens: the Omarchy theme on Omarchy (`omarchy theme set` applies straight away), your accent colour and light/dark mode on Windows, and a neutral dark palette elsewhere.
