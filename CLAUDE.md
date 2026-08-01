# magpie

A video downloader (crate/binary name is `magpie`, not the repo name). Shells out to yt-dlp.

## Stack

GTK 4.22 + libadwaita 1.9 via gtk4-rs 0.11 / libadwaita-rs 0.9, Rust edition 2021 (MSRV 1.80). `gio` is a direct dependency purely to raise the API level to v2_80 — leave it.

Crate is a lib + bin so integration tests and `examples/` can drive the real application rather than a copy of it.

## Commands

- `./test.sh` — fmt check, clippy with `-D warnings`, then `cargo test --all-targets`. Add `--headless` to run under Xvfb + a private D-Bus session. This is the gate; run it, not bare `cargo test`.
- `./install.sh` — release build, installs under `~/.local`. `./uninstall.sh` reverses it.
- `packaging/build-flatpak.sh` and `packaging/build-deb.sh` — distribution artifacts.
- `cargo run --example preview -- /tmp/preview [dark]` — paints the real widget tree
  offscreen to PNGs. This is how a UI change gets looked at; GNOME will not give a
  screenshot to a non-interactive caller.

Widget tests need a display; model tests do not and are the bulk of the suite. `test.sh` sets `GTK_A11Y=none` and `GSETTINGS_BACKEND=memory` so tests never touch real user state — keep that true for anything new.

## Layout

`src/model/` is pure logic with no GTK types. `src/ui/` is widgets and the application. Read `DESIGN.md` and `README.md` before proposing structural changes; both are current.

The seam that makes the tests possible: **`model/` builds argument vectors and parses lines; `ui/` runs the process.** Nothing under `model/` spawns anything, so every flag combination and every shape of yt-dlp output is checkable with no display and no network. `ui/process.rs` is the only file that launches a child. Widgets emit intent signals; `ui/application.rs` is the only object that mutates the queue, writes a file, or spawns anything.

## Conventions

- Use the `developing-gtk-apps` and `designing-gnome-ui` skills for widget, threading, and HIG decisions rather than deriving them again.
- Edit files with the Edit tool. Do not rewrite Rust sources through `python3 - <<PY` heredocs or `sed -i`.
- The sibling apps (brain, familiar, planner, stickies) share this layout and these scripts; a pattern established in one is the pattern here.
