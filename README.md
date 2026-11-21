# tp-dial

Push the TrackPoint: a radial dial opens in the centre of the screen. Push towards an item and let go to run it.

## Gesture

`[trigger] mode` chooses how the dial opens:

- **`any`** (default): any real push opens the dial straight away. Resting drift below `min_push` is ignored. The TrackPoint no longer moves the cursor; use the touchpad for that. Its buttons still click, and moving it while a button is held still drags or middle-scrolls.
- **`flick`**: the TrackPoint moves the cursor as normal. A sharp push (`trigger_speed` reached within `rise_ms`) held for `hold_ms` opens the dial.

While the dial is open, whichever item you're pushing towards is highlighted. Easing off keeps the highlight, so letting go runs it. Pick **Close** or click a TrackPoint button to cancel.

## Setup (once)

```sh
sudo ./setup-permissions.sh
```

This installs `/etc/udev/rules.d/70-tp-dial.rules`, which gives your logged-in session access to the TrackPoint event device and `/dev/uinput` through `uaccess` ACLs. No group change or re-login is needed. tp-dial refuses to run as root.

## Run

```sh
cargo build --release
./target/release/tp-dial --debug   # shows the speed of each report and the gesture phase, for tuning
./target/release/tp-dial --demo    # draws the dial with a scripted sweep, no TrackPoint needed
./target/release/tp-dial --check   # validates the config and lists items
```

tp-dial grabs the TrackPoint exclusively and re-emits its events through a virtual clone, `TPPS/2 Elan TrackPoint (tp-dial)`. That clone is how motion gets swallowed while the dial is open. If tp-dial exits or crashes, the kernel releases the grab and the real device works again.

## Custom items

Edit `~/.config/tp-dial/config.toml`. Changes are picked up the next time you touch the stick. Items run clockwise from the top, up to 12:

```toml
[[item]]
label = "Obsidian"
app = "obsidian"                # uwsm-app -- obsidian

[[item]]
label = "Backup"
script = "~/bin/backup.sh"      # must be executable

[[item]]
label = "Mute"
command = "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
```

## Install and autostart

```sh
./install.sh     # build, copy to ~/.local/bin/tp-dial, restart the running copy
```

It runs as the systemd user service `tp-dial.service`, started with the graphical
session and restarted if it crashes. Nothing goes in `~/.config/hypr`, so
`omarchy refresh hyprland` can't remove it. tp-dial registers its own two Hyprland
rules over IPC at startup and after every config reload: the dial shows without a
fade, and the settings window floats. Its log is in the journal:

```sh
journalctl --user -t tp-dial -f
```

Re-run `./install.sh` after changing the code. Config changes don't need it.

## Appearance

Easiest: pick **Settings** on the dial (or run `tp-dial --settings`). It has a live preview and saves into the config file, keeping your comments and items.

Or edit `[dial]` in `~/.config/tp-dial/config.toml` directly; changes apply the next time the dial opens.

| Option | What it does |
|---|---|
| `accent`, `background`, `foreground`, `selected_text`, `center_color` | Colours: `"theme"` or a hex colour (`#rrggbb` or `#rrggbbaa`) |
| `border`, `border_width` | Outline round slices and the centre: `"none"`, `"theme"` or a colour |
| `opacity` | Transparency of the whole dial, 0.0–1.0 |
| `font`, `font_size` | Label font and largest size. Labels that don't fit their slice shrink, then end in "…" |
| `size`, `center_size` | Dial diameter in px; centre circle as a fraction of the radius |
| `center_image`, `center_image_opacity` | PNG, JPG, SVG or animated GIF in the centre, cropped to the circle |
| `animation`, `animation_ms` | How the dial appears: `none`, `fade`, `pop`, `spin` or `unfold` |

## Theme

`accent`, `background` and `foreground` set to `"theme"` use the current Omarchy
theme's `colors.toml`, re-read each time the dial opens, so `omarchy theme set`
applies immediately. `font = "monospace"` is your Omarchy font. Any of these can
be a hex colour or font family instead.
