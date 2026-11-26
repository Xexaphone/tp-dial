#!/usr/bin/env bash
# Build a self-contained tp-dial folder for Windows. Run in an MSYS2 UCRT64 shell
# after `cargo build --release`. Produces dist/tp-dial/ laid out the way GTK
# expects on Windows (it finds lib/ and share/ relative to bin/):
#
#   bin/tp-dial.exe + every DLL it (and the image loaders) need
#   lib/gdk-pixbuf-2.0/2.10.0/    image loaders (PNG/JPEG/GIF/SVG for the centre image)
#   share/glib-2.0/schemas/       compiled GSettings schemas (GTK's dialogs use them)
#   share/icons/                  icon theme index + symbolic icons for GTK widgets
set -euo pipefail
cd "$(dirname "$0")/../.."
prefix=${MINGW_PREFIX:?run this in an MSYS2 UCRT64 shell}
out=dist/tp-dial
rm -rf "$out" && mkdir -p "$out/bin"

cp target/release/tp-dial.exe "$out/bin/"

# Copy the MSYS2 DLLs a binary depends on (recursively, via ldd).
copy_deps() {
  ldd "$1" | awk -v p="$prefix" 'index($3, p) == 1 { print $3 }' | while read -r dll; do
    [ -f "$out/bin/$(basename "$dll")" ] || cp "$dll" "$out/bin/"
  done
}
copy_deps "$out/bin/tp-dial.exe"

# gdk-pixbuf image loaders (+ their DLLs) and the loader cache.
pixbuf=lib/gdk-pixbuf-2.0/2.10.0
mkdir -p "$out/$pixbuf"
cp -r "$prefix/$pixbuf/loaders" "$out/$pixbuf/"
for loader in "$out/$pixbuf"/loaders/*.dll; do copy_deps "$loader"; done
# Write the cache with loader paths relative to the install folder (passing the
# loaders as relative paths makes query-loaders record them that way), so it
# works wherever tp-dial is installed.
(cd "$out" && "$prefix/bin/gdk-pixbuf-query-loaders.exe" "$pixbuf"/loaders/*.dll > "$pixbuf/loaders.cache")
grep -q "$(cygpath -m "$PWD" | cut -c1-3)" "$out/$pixbuf/loaders.cache" && { echo "loaders.cache has absolute paths" >&2; exit 1; } || true

# GSettings schemas (compiled).
mkdir -p "$out/share/glib-2.0/schemas"
cp "$prefix"/share/glib-2.0/schemas/*.xml "$out/share/glib-2.0/schemas/" 2>/dev/null || true
glib-compile-schemas "$out/share/glib-2.0/schemas"

# Icons GTK's widgets use (dropdown arrows, spin buttons...): theme indexes plus
# Adwaita's symbolic icons, which is all GTK4 widgets need.
mkdir -p "$out/share/icons"
cp -r "$prefix/share/icons/hicolor" "$out/share/icons/"
if [ -d "$prefix/share/icons/Adwaita" ]; then
  mkdir -p "$out/share/icons/Adwaita"
  cp "$prefix/share/icons/Adwaita/index.theme" "$out/share/icons/Adwaita/"
  [ -d "$prefix/share/icons/Adwaita/symbolic" ] && cp -r "$prefix/share/icons/Adwaita/symbolic" "$out/share/icons/Adwaita/"
fi

cp README.md "$out/"
cp packaging/windows/tp-dial.ico "$out/"
echo "bundled $(find "$out" -type f | wc -l) files, $(du -sh "$out" | cut -f1), in $out"
