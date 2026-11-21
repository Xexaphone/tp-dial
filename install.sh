#!/bin/sh
# Build tp-dial, install it to ~/.local/bin, and (re)start it as a systemd user
# service tied to the graphical session. Safe to re-run after every change.
set -eu
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/tp-dial "$HOME/.local/bin/tp-dial"
install -Dm644 tp-dial.service "$HOME/.config/systemd/user/tp-dial.service"
echo "installed ~/.local/bin/tp-dial and the tp-dial user service"

# Copies started some other way (old autostart, a terminal) hold the TrackPoint grab.
if pkill -x tp-dial; then
  for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -x tp-dial >/dev/null || break; sleep 0.1; done
fi

systemctl --user daemon-reload
systemctl --user enable tp-dial.service
systemctl --user restart tp-dial.service
sleep 0.5
if systemctl --user is-active --quiet tp-dial.service; then
  echo "tp-dial running (logs: journalctl --user -t tp-dial -f)"
else
  echo "tp-dial did not start:" >&2
  journalctl --user -t tp-dial -n 10 --no-pager >&2
fi
