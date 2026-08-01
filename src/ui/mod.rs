//! The half that knows a window exists.
//!
//! Widget trees are built in Rust — no `.ui` XML, no Blueprint, no GResource.
//! The structure of a pane is then readable in the same file as the behaviour
//! that drives it, which for an app this size is worth more than a designer
//! could give back.
//!
//! Widgets emit intent and nothing else. A row's Cancel button emits
//! `cancel-requested`; it does not signal a process. [`MagpieApplication`] is
//! the only object here that mutates the queue, writes a file, or spawns
//! anything.

mod add_dialog;
mod application;
mod job_row;
mod link_bar;
mod preferences;
mod process;
mod thumbnail;
mod toolbox;
mod window;

pub use add_dialog::AddDialog;
pub use application::MagpieApplication;
pub use preferences::Preferences;
pub use window::MagpieWindow;

/// What Magpie found installed. Exported so that `tests/widgets.rs` and
/// `examples/preview.rs` can build the Tools page against a machine that has
/// nothing, which is the state worth looking at.
pub use toolbox::Report as ToolReport;

/// The application stylesheet, compiled in.
pub const STYLE: &str = include_str!("style.css");

/// Load the stylesheet at application priority, above the theme and below the
/// user's own overrides.
pub fn load_stylesheet(display: &gtk::gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
