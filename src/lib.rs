//! Magpie: a video downloader for GNOME, pointed at whatever `yt-dlp` is
//! installed.
//!
//! Two halves. `model/` links no GTK and spawns no process: it turns a request
//! into an argument vector and a line of output back into an event, which is
//! why `cargo test` can exercise the interesting parts with no display and no
//! network. `ui/` is the only half that knows a window exists, and
//! `ui::MagpieApplication` is the only thing that runs a subprocess or writes a
//! file.

pub mod model;
pub mod ui;

pub const APP_ID: &str = "us.hagreli.Magpie";
