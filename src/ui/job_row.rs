//! One row of the download list.
//!
//! The row shows state and emits intent. It never touches a process, a file or
//! the queue: Cancel emits `cancel-requested` and the application decides what
//! that means for a job that is running versus one that has not started.
//!
//! Which buttons exist depends on the state, and that is on purpose. A Retry
//! button on a private video is a button that cannot work, and an Open Folder
//! button on a download that has not finished points at a file that is not
//! there yet.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;
use std::cell::{Cell, OnceCell};
use std::sync::OnceLock;

use crate::model::job::{Job, Progress, State, TranscriptState};

use super::thumbnail;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct JobRow {
        pub id: Cell<u64>,
        pub title: OnceCell<gtk::Label>,
        pub status: OnceCell<gtk::Label>,
        pub bar: OnceCell<gtk::ProgressBar>,
        pub picture: OnceCell<gtk::Picture>,
        pub placeholder: OnceCell<gtk::Image>,
        pub controls: OnceCell<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for JobRow {
        const NAME: &'static str = "MagpieJobRow";
        type Type = super::JobRow;
        type ParentType = gtk::ListBoxRow;
    }

    impl ObjectImpl for JobRow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                [
                    "pause",
                    "resume",
                    "cancel",
                    "retry",
                    "remove",
                    "open",
                    "transcript",
                    "details",
                ]
                .iter()
                .map(|name| Signal::builder(&format!("{name}-requested")).build())
                .collect()
            })
        }
    }

    impl WidgetImpl for JobRow {}
    impl ListBoxRowImpl for JobRow {}
}

glib::wrapper! {
    pub struct JobRow(ObjectSubclass<imp::JobRow>)
        @extends gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl JobRow {
    pub fn new(id: u64) -> Self {
        let row: Self = glib::Object::builder()
            .property("activatable", false)
            .property("selectable", false)
            .build();
        row.imp().id.set(id);
        row
    }

    pub fn id(&self) -> u64 {
        self.imp().id.get()
    }

    fn build(&self) {
        // 16:9 at a size that reads as a still rather than an icon, and small
        // enough that eight rows fit in the default window.
        let (poster, picture, placeholder) = thumbnail::poster(80, 45);

        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .lines(1)
            .build();
        title.add_css_class("heading");

        let status = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("caption");
        status.add_css_class("dimmed");
        status.add_css_class("job-status");

        let bar = gtk::ProgressBar::builder().visible(false).build();
        bar.add_css_class("job-progress");

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(&title);
        text.append(&status);
        text.append(&bar);

        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        row.append(&poster);
        row.append(&text);
        row.append(&controls);
        self.set_child(Some(&row));

        let imp = self.imp();
        let _ = imp.title.set(title);
        let _ = imp.status.set(status);
        let _ = imp.bar.set(bar);
        let _ = imp.picture.set(picture);
        let _ = imp.placeholder.set(placeholder);
        let _ = imp.controls.set(controls);
    }

    /// Show this job's current state.
    pub fn bind(&self, job: &Job, progress: Option<&Progress>) {
        let imp = self.imp();

        let title = imp.title.get().expect("built");
        title.set_label(&job.title);
        // The list is the only place the full title is available, and it is
        // ellipsised, so the tooltip is the way to read a long one.
        title.set_tooltip_text(Some(&job.title));

        let status = imp.status.get().expect("built");
        status.set_label(&job.status_line(progress));
        status.remove_css_class("error");
        status.remove_css_class("warning");
        match &job.state {
            State::Failed(_) => status.add_css_class("error"),
            State::Paused => status.add_css_class("warning"),
            _ => {}
        }
        if matches!(job.transcript_state, TranscriptState::Failed(_)) {
            status.add_css_class("warning");
        }

        let bar = imp.bar.get().expect("built");
        bar.set_visible(job.shows_progress());
        match job.fraction(progress) {
            Some(fraction) => {
                bar.set_fraction(fraction);
                bar.set_pulse_step(0.1);
            }
            // No total to divide by, or a post-processing step with no byte
            // count. A bar that pulses says "working"; one parked at a made-up
            // number says "stuck".
            None => bar.pulse(),
        }

        self.rebuild_controls(job);
        self.update_accessible_description(job, progress);
    }

    /// Put a poster in place, or leave the placeholder showing.
    pub fn set_poster(&self, texture: Option<&gtk::gdk::Texture>) {
        let (Some(picture), Some(placeholder)) =
            (self.imp().picture.get(), self.imp().placeholder.get())
        else {
            return;
        };
        picture.set_paintable(texture.map(|t| t.upcast_ref::<gtk::gdk::Paintable>()));
        picture.set_visible(texture.is_some());
        placeholder.set_visible(texture.is_none());
    }

    /// The controls for the row's current state.
    ///
    /// The last button is always the one that makes the row go away, whatever
    /// that means for the state — cancel a running download, drop a waiting one,
    /// forget a finished one. Same glyph, same position, one habit to learn.
    fn rebuild_controls(&self, job: &Job) {
        let controls = self.imp().controls.get().expect("built");
        while let Some(child) = controls.first_child() {
            controls.remove(&child);
        }

        match &job.state {
            State::Waiting => {
                self.add_button(
                    controls,
                    "window-close-symbolic",
                    "Remove",
                    "remove-requested",
                );
            }
            State::Running => {
                self.add_button(
                    controls,
                    "media-playback-pause-symbolic",
                    "Pause",
                    "pause-requested",
                );
                self.add_button(
                    controls,
                    "window-close-symbolic",
                    "Cancel",
                    "cancel-requested",
                );
            }
            State::Paused => {
                self.add_button(
                    controls,
                    "media-playback-start-symbolic",
                    "Resume",
                    "resume-requested",
                );
                self.add_button(
                    controls,
                    "window-close-symbolic",
                    "Cancel",
                    "cancel-requested",
                );
            }
            State::Done => {
                if job.transcript_path().is_some() {
                    self.add_button(
                        controls,
                        "text-x-generic-symbolic",
                        "Open Transcript",
                        "transcript-requested",
                    );
                }
                if !job.outputs.is_empty() {
                    self.add_button(
                        controls,
                        "folder-open-symbolic",
                        "Show in Files",
                        "open-requested",
                    );
                }
                self.add_button(
                    controls,
                    "window-close-symbolic",
                    "Remove from List",
                    "remove-requested",
                );
            }
            State::Failed(failure) => {
                if failure.detail().is_some() {
                    self.add_button(
                        controls,
                        "dialog-information-symbolic",
                        "What Went Wrong",
                        "details-requested",
                    );
                }
                // Withheld when trying again cannot help. See
                // `Failure::is_retryable`.
                if failure.is_retryable() {
                    self.add_button(
                        controls,
                        "view-refresh-symbolic",
                        "Try Again",
                        "retry-requested",
                    );
                }
                self.add_button(
                    controls,
                    "window-close-symbolic",
                    "Remove from List",
                    "remove-requested",
                );
            }
        }
    }

    fn add_button(&self, controls: &gtk::Box, icon: &str, tooltip: &str, signal: &'static str) {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .build();
        button.add_css_class("flat");
        button.add_css_class("circular");
        // Icon-only, so the tooltip is not the only thing naming it: a screen
        // reader gets the same words.
        button.update_property(&[gtk::accessible::Property::Label(tooltip)]);

        button.connect_clicked(glib::clone!(
            #[weak(rename_to = row)]
            self,
            move |_| row.emit_by_name::<()>(signal, &[])
        ));
        controls.append(&button);
    }

    /// The row's whole state as one sentence, for a screen reader.
    ///
    /// Without this the reader gets a title and then a separate progress bar
    /// percentage with nothing tying them together.
    fn update_accessible_description(&self, job: &Job, progress: Option<&Progress>) {
        let description = format!("{}. {}", job.title, job.status_line(progress));
        self.update_property(&[gtk::accessible::Property::Description(&description)]);
    }
}
