# Patch

An always-on todo list, kanban board and calendar — native Rust on **Linux** and
**Android**, matte black and glassy, laid out like the macOS Calendar, and synced
between your devices **over Tailscale** only.

![Month view, Matte Black](docs/screenshots/desktop-matte-month.jpg)

| Week | Board (Liquid Glass) | Editor with range picker (Light) |
|---|---|---|
| ![](docs/screenshots/desktop-matte-week.jpg) | ![](docs/screenshots/desktop-glass-board.jpg) | ![](docs/screenshots/desktop-light-editor.jpg) |

**Android** — phone layout, plus a floating panel/bubble that sits *over other apps*:

![Phone](docs/screenshots/phone.jpg)
![Overlay](docs/screenshots/overlay.jpg)

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

## Features

- **Calendar like macOS**: Day, Week and Month views. Multi-day items are bars that keep their lane across a week. The week grid lays overlapping items side by side and shows a red "now" line. You can drag to move and drag the bottom edge to resize, snapping to 15 minutes.
- **Kanban board**: drag cards between columns and reorder them; on phones, long-press to drag. You can add, rename and delete columns. Dropping a card in **Done** completes it.
- **Todo list**: grouped into Overdue, Today, Tomorrow, Next 7 days, Later, Someday and Completed.
- **Task inspector**:
  - list, status, priority, checklist and notes
  - repeats (daily, weekdays, weekly, monthly, yearly) and reminders
  - the date-range picker from the design reference: tap a start, tap an end, set times, then Reset or ✓
- **Natural-language quick add**: `Dentist tomorrow 3pm #work !!!`, `Conference 13-15 nov`, `Standup every weekday 9:30`, `Gym tmrw 7am for 1h30m`, `Call back in 2 hours`.
- **Three looks**: Matte Black (default), Liquid Glass and Light.
- **Always on**:
  - **Linux**: a frameless always-on-top widget (Today / Upcoming / Board with quick add), a tray icon, start at login, and desktop notifications.
  - **Android**: a **Display over other apps** floating bubble that expands into the same panel, with system notifications for reminders.
- **Offline first**: every device keeps a full copy and edits merge field by field, so nothing is lost when two devices change the same task.

## Sync over Tailscale

One machine runs the **hub**: the Linux app (Settings → *Host the sync hub on this
computer*) or the headless `patch-hub` on any always-on box such as a home server
or Raspberry Pi. Every other device connects to it.

1. Install [Tailscale](https://tailscale.com) on each device and sign in to the same tailnet.
2. On the hub machine, turn on hosting. Settings shows the addresses (e.g. `http://my-laptop:4747`) and a pairing token like `K7QF-2MZD-9XPA-T4RH`.
3. On the phone (or another computer): Settings → *Sync over Tailscale* → enter the address and the token → **Connect**.

Edits show up on the other devices within about a second (long-polling), and a
device that was offline catches up when it reconnects.

Security model:

- The hub only answers loopback and Tailscale peers (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`). Add `--allow-lan` to also accept your LAN.
- Every request needs the pairing token.
- Traffic is plain HTTP *inside* the WireGuard tunnel Tailscale provides, so it is encrypted end to end without a TLS stack on the phone.

How it merges: every field of every task carries a hybrid-logical-clock stamp.
Merging keeps the newest write per field, which is commutative and idempotent,
so devices converge regardless of the order they sync in
([`crates/core`](crates/core/src)).

## Install

### Linux

```sh
scripts/install-linux.sh            # builds `patch` + `patch-hub`, installs to ~/.local
patch                               # or launch "Patch" from your app menu
```

The headless hub as a systemd user service:

```sh
cargo build --release -p patch-sync --bin patch-hub
install -Dm755 target/release/patch-hub ~/.local/bin/patch-hub
install -Dm644 packaging/linux/patch-hub.service ~/.config/systemd/user/patch-hub.service
systemctl --user enable --now patch-hub
journalctl --user -u patch-hub      # shows the pairing token and URLs
```

Build dependencies (Debian/Ubuntu): `build-essential pkg-config libfontconfig1-dev libxkbcommon-dev libwayland-dev libdbus-1-dev`.

### Android

Download the APK from the latest CI run (*Actions → build → android-apk*) or build it:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
cd android && ./gradlew assembleRelease    # needs ANDROID_HOME and NDK 28
```

On first launch, open Settings → *Display over other apps* to enable the floating
bubble. Android will ask you to grant the "Display over other apps" permission.

## Using it

| Quick add | Result |
|---|---|
| `Pay rent 1st #personal !!!` | all-day on the 1st, Personal list, high priority |
| `Team sync fri 10-11am` | Friday 10:00–11:00 |
| `Conference 13-15 nov` | all-day range |
| `Water plants every sunday` | weekly repeat |
| `Check oven in 20 min` | 20 minutes from now |

Desktop shortcuts: `N` new task · `T` today · `D/W/M/B/L` or `1–5` views ·
`←/→` previous/next · `/` or `Ctrl+F` search · `Ctrl+,` settings · `Esc` close.

## Project layout

```
crates/core      data model, hybrid logical clock, CRDT merge, recurrence, quick-add parser
crates/sync      Tailscale-only hub (tiny HTTP + long-poll), sync client, `patch-hub` binary
crates/ui        the shared Slint UI (ui/*.slint) and the App controller
apps/linux       native Linux shell: winit window, tray, always-on-top widget, hub hosting
crates/android   Rust cdylib: custom Slint platform rendering into Android Surfaces
android/         Gradle project: Activity, overlay service, IME bridge (thin Java glue)
packaging/linux  .desktop entry, icons, AppStream metadata, systemd unit
```

On Android one Rust process renders every surface — the full-screen app and the
floating overlay — through a custom Slint platform (software renderer → `ANativeWindow`).
Java only hosts the Activity, the overlay window and the keyboard bridge.

## Development

```sh
cargo test                                   # core, sync, ui (incl. a UI interaction test)
cargo clippy --all-targets
cargo run -p patch-ui --example shots -- screenshots/   # render every screen in all looks
```

## License

MIT for Patch itself (see [LICENSE](LICENSE)). The UI is built with
[Slint](https://slint.dev) under its royalty-free license (attribution in
Settings → About and the badge above). Inter font under the SIL OFL 1.1.
