#!/bin/sh
# Type-check the Windows build from Linux. Only compiles, never links, so instead
# of real Windows GTK libraries it points pkg-config at stub .pc files. A real
# Windows build is done on Windows with MSYS2 - see README.
set -eu
cd "$(dirname "$0")/.."
stubs="${CARGO_TARGET_DIR:-target}/windows-pc-stubs"
mkdir -p "$stubs"
for lib in glib-2.0 gobject-2.0 gio-2.0 gio-windows-2.0 gmodule-2.0 cairo cairo-gobject \
           cairo-win32 pango pangocairo gdk-pixbuf-2.0 graphene-1.0 graphene-gobject-1.0 \
           gtk4 gtk4-win32 harfbuzz; do
  printf 'Name: %s\nDescription: stub\nVersion: 99.0.0\nLibs:\nCflags:\n' "$lib" > "$stubs/$lib.pc"
done
export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR="$PWD/$stubs" PKG_CONFIG_PATH=
exec cargo check --target x86_64-pc-windows-gnu "$@"
