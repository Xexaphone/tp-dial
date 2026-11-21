#!/bin/sh
# Installs the udev rule that lets your desktop session read the TrackPoint and
# create the virtual pointer tp-dial forwards through. Run with sudo.
set -eu
[ "$(id -u)" = 0 ] || { echo "run with sudo"; exit 1; }
install -m 0644 "$(dirname "$0")/70-tp-dial.rules" /etc/udev/rules.d/70-tp-dial.rules
udevadm control --reload
# uinput is often a module that's only autoloaded on first open; until then
# /dev/uinput is a static placeholder udev can't act on. Load it now and at boot.
echo uinput > /etc/modules-load.d/tp-dial-uinput.conf
modprobe uinput
udevadm settle
udevadm trigger --action=change --property-match=ID_INPUT_POINTINGSTICK=1
udevadm trigger --action=change --subsystem-match=misc --sysname-match=uinput
udevadm settle
echo "installed. Session ACLs (expect a user:${SUDO_USER:-you}:rw line on each):"
for n in /dev/uinput $(udevadm trigger --dry-run --verbose --property-match=ID_INPUT_POINTINGSTICK=1 --subsystem-match=input | sed -n 's#^/sys\(.*/event[0-9]*\)$#\1#p' | xargs -r -n1 basename | sed 's#^#/dev/input/#'); do
  printf '%s: ' "$n"; getfacl -cp "$n" 2>/dev/null | grep '^user:[^:]' || echo "no user ACL"
done
