#!/usr/bin/env bash
# Builds Patch and installs it for the current user (no root needed):
#   ~/.local/bin/patch, ~/.local/bin/patch-hub, the launcher entry, icons,
#   AppStream metadata and a systemd user unit for the headless hub.
#
#   scripts/install-linux.sh              build (release) and install
#   scripts/install-linux.sh --no-build   install already-built binaries
#   scripts/install-linux.sh --uninstall  remove everything installed (keeps your data)
#   scripts/install-linux.sh --prefix DIR install under DIR instead of ~/.local
set -euo pipefail

APP_ID=com.mdhaarith.Patch
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PKG=$ROOT/packaging/linux
PREFIX=$HOME/.local
BUILD=1
ACTION=do_install

usage() { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case $1 in
        --uninstall) ACTION=do_uninstall ;;
        --no-build) BUILD=0 ;;
        --prefix) PREFIX=${2:?--prefix needs a directory}; shift ;;
        --prefix=*) PREFIX=${1#--prefix=} ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install-linux.sh: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

BIN=$PREFIX/bin
SHARE=$PREFIX/share
CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}
UNIT_DIR=$CONFIG/systemd/user
SIZES=(16 22 24 32 48 64 128 256 512)

say() { printf '\033[1m%s\033[0m\n' "$*"; }

refresh_caches() {
    # Only refresh an existing cache: a stale one in ~/.local would hide icons
    # other apps install later without updating it.
    if [[ -f $SHARE/icons/hicolor/icon-theme.cache ]] && command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t -f "$SHARE/icons/hicolor" 2>/dev/null || true
    fi
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$SHARE/applications" 2>/dev/null || true
    fi
}

do_uninstall() {
    if [[ -x $BIN/patch ]]; then
        "$BIN/patch" --quit >/dev/null 2>&1 || true
    fi
    if command -v systemctl >/dev/null && systemctl --user is-enabled patch-hub.service >/dev/null 2>&1; then
        systemctl --user disable --now patch-hub.service || true
    fi
    rm -f "$BIN/patch" "$BIN/patch-hub" \
        "$SHARE/applications/$APP_ID.desktop" \
        "$SHARE/metainfo/$APP_ID.metainfo.xml" \
        "$UNIT_DIR/patch-hub.service" \
        "$CONFIG/autostart/$APP_ID.desktop"
    for size in "${SIZES[@]}"; do
        rm -f "$SHARE/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
    done
    command -v systemctl >/dev/null && systemctl --user daemon-reload 2>/dev/null || true
    refresh_caches
    say "Patch is uninstalled."
    echo "Your data is still in ${XDG_DATA_HOME:-$HOME/.local/share}/patch (and .../patch-hub); delete it to start over."
}

do_install() {
    local target=${CARGO_TARGET_DIR:-$ROOT/target}
    if [[ $BUILD == 1 ]]; then
        command -v cargo >/dev/null || { echo "cargo not found: install Rust from https://rustup.rs" >&2; exit 1; }
        say "Building Patch (release)…"
        (cd "$ROOT" && cargo build --release -p patch-linux --bin patch -p patch-sync --bin patch-hub)
    fi
    for bin in patch patch-hub; do
        [[ -x $target/release/$bin ]] || { echo "$target/release/$bin is missing; run without --no-build" >&2; exit 1; }
    done

    # A running Patch keeps its old binary open; ask it to exit so the new one starts fresh.
    # (`--quit` prints "Patch isn't running." when there was nothing to stop.)
    local was_running=0
    if [[ -x $BIN/patch ]] && [[ -z $("$BIN/patch" --quit 2>/dev/null || echo failed) ]]; then
        was_running=1
    fi

    say "Installing to $PREFIX…"
    install -Dm755 "$target/release/patch" "$BIN/patch"
    install -Dm755 "$target/release/patch-hub" "$BIN/patch-hub"
    for size in "${SIZES[@]}"; do
        install -Dm644 "$PKG/icons/hicolor/${size}x${size}/apps/$APP_ID.png" \
            "$SHARE/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
    done
    # Absolute paths: the desktop session's PATH doesn't always include ~/.local/bin.
    mkdir -p "$SHARE/applications" "$SHARE/metainfo" "$UNIT_DIR"
    sed -e "s|^Exec=patch --quit$|Exec=$BIN/patch --quit|" -e "s|^Exec=patch$|Exec=$BIN/patch|" \
        "$PKG/$APP_ID.desktop" >"$SHARE/applications/$APP_ID.desktop"
    chmod 644 "$SHARE/applications/$APP_ID.desktop"
    install -Dm644 "$PKG/$APP_ID.metainfo.xml" "$SHARE/metainfo/$APP_ID.metainfo.xml"
    sed -e "s|^ExecStart=%h/.local/bin/patch-hub$|ExecStart=$BIN/patch-hub|" \
        "$PKG/patch-hub.service" >"$UNIT_DIR/patch-hub.service"
    chmod 644 "$UNIT_DIR/patch-hub.service"
    command -v systemctl >/dev/null && systemctl --user daemon-reload 2>/dev/null || true
    refresh_caches

    say "Patch is installed."
    echo "  Open it from your apps, or run: $BIN/patch"
    case ":$PATH:" in *":$BIN:"*) ;; *) echo "  (add $BIN to your PATH to run 'patch' from a terminal)" ;; esac
    echo "  Headless hub on an always-on machine: systemctl --user enable --now patch-hub"
    if [[ $was_running == 1 ]]; then
        (setsid "$BIN/patch" --background >/dev/null 2>&1 &)
        echo "  Restarted the running Patch in the background."
    fi
}

"$ACTION"
