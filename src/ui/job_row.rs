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
//!
//! A playlist is one row and a hundred and seven files, so its row opens: the
//! disclosure reveals a line per item saying which have landed, which is being
//! fetched, and which are still to come. Collapsed by default, and built only
//! once opened — a hundred rows of widgets nobody has asked to see is a hundred
//! rows of widgets to lay out on every progress tick.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;
use std::cell::{Cell, OnceCell, RefCell};
use std::sync::OnceLock;

use crate::model::collection::{self, Line, Stage, Words};
use crate::model::job::{Job, Progress, State, TranscriptState};
use crate::model::media;

use super::thumbnail;

/// One built line of the expanded view, kept so a redraw can change the words
/// rather than the widgets.
#[derive(Clone)]
pub struct ItemRow {
    index: usize,
    row: gtk::ListBoxRow,
    title: gtk::Label,
    detail: gtk::Label,
    done: gtk::Image,
    running: adw::Spinner,
    transcribing: adw::Spinner,
    transcript: gtk::Button,
}

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
        pub disclosure: OnceCell<gtk::ToggleButton>,
        pub revealer: OnceCell<gtk::Revealer>,
        pub items_list: OnceCell<gtk::ListBox>,
        pub items: RefCell<Vec<ItemRow>>,
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
                let mut signals: Vec<Signal> = [
                    "pause",
                    "resume",
                    "cancel",
                    "retry",
                    "remove",
                    "open",
                    "transcript",
                    "details",
                    // Emitted when the items are first shown, so the application
                    // can go and find out what they are called.
                    "expand",
                    // Make the words for what has been downloaded and has none,
                    // and stop doing that.
                    "transcribe",
                    "stop-transcript",
                ]
                .iter()
                .map(|name| Signal::builder(&format!("{name}-requested")).build())
                .collect();

                // The two that carry anything: which item of the collection to
                // show in Files, and which one's transcript to open.
                for name in ["item-open-requested", "item-transcript-requested"] {
                    signals.push(
                        Signal::builder(name)
                            .param_types([u64::static_type()])
                            .build(),
                    );
                }
                signals
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

        // Last, where AdwExpanderRow puts its own: the actions belong to the
        // row, the chevron belongs to what is underneath it.
        let disclosure = gtk::ToggleButton::builder()
            .icon_name("pan-end-symbolic")
            .tooltip_text("Show Items")
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        disclosure.add_css_class("flat");
        disclosure.add_css_class("circular");
        disclosure.add_css_class("job-disclosure");
        disclosure.update_property(&[gtk::accessible::Property::Label("Show Items")]);
        disclosure.connect_toggled(glib::clone!(
            #[weak(rename_to = row)]
            self,
            move |button| row.apply_expanded(button.is_active())
        ));

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        header.append(&poster);
        header.append(&text);
        header.append(&controls);
        header.append(&disclosure);

        let items_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        items_list.add_css_class("job-items");
        items_list.connect_row_activated(glib::clone!(
            #[weak(rename_to = row)]
            self,
            move |_, activated| row.item_activated(activated)
        ));

        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .child(&items_list)
            .build();

        let outer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        outer.append(&header);
        outer.append(&revealer);
        self.set_child(Some(&outer));

        let imp = self.imp();
        let _ = imp.title.set(title);
        let _ = imp.status.set(status);
        let _ = imp.bar.set(bar);
        let _ = imp.picture.set(picture);
        let _ = imp.placeholder.set(placeholder);
        let _ = imp.controls.set(controls);
        let _ = imp.disclosure.set(disclosure);
        let _ = imp.revealer.set(revealer);
        let _ = imp.items_list.set(items_list);
    }

    /// Show this job's current state.
    pub fn bind(&self, job: &Job, progress: Option<&Progress>, whisper: bool) {
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

        self.rebuild_controls(job, whisper);

        // Only a collection has anything under it. A row that grew a chevron
        // when yt-dlp turned out to have found a playlist would be a row that
        // changes shape while you are reading it, so the button is placed from
        // the job rather than from whether the items happen to be known yet.
        let disclosure = self.imp().disclosure.get().expect("built");
        disclosure.set_visible(job.collection.is_some());
        if job.collection.is_none() && disclosure.is_active() {
            disclosure.set_active(false);
        }
        if self.is_expanded() {
            self.set_items(&collection::lines(job, progress));
        }

        self.update_accessible_description(job, progress);
    }

    pub fn is_expanded(&self) -> bool {
        self.imp()
            .disclosure
            .get()
            .is_some_and(gtk::ToggleButton::is_active)
    }

    /// Open or close the item list.
    ///
    /// Driven by the disclosure button in the window; called directly by
    /// `tests/widgets.rs` and `examples/preview.rs`, which have no pointer.
    pub fn set_expanded(&self, expanded: bool) {
        if let Some(disclosure) = self.imp().disclosure.get() {
            // The button is the state, so going through it keeps one source of
            // truth and lets this be the whole of the handler.
            if disclosure.is_active() != expanded {
                disclosure.set_active(expanded);
                return;
            }
        }
        self.apply_expanded(expanded);
    }

    fn apply_expanded(&self, expanded: bool) {
        let imp = self.imp();
        let disclosure = imp.disclosure.get().expect("built");
        disclosure.set_icon_name(if expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
        let label = if expanded { "Hide Items" } else { "Show Items" };
        disclosure.set_tooltip_text(Some(label));
        disclosure.update_property(&[gtk::accessible::Property::Label(label)]);
        imp.revealer
            .get()
            .expect("built")
            .set_reveal_child(expanded);

        if expanded {
            // The items may never have been fetched — a job queued before Magpie
            // kept them, or one added without the dialog. Asking now is what
            // gets the names in; until they arrive the list shows numbers.
            self.emit_by_name::<()>("expand-requested", &[]);
        }
    }

    /// Show one line per item of the collection.
    ///
    /// The widgets are reused. A hundred-item playlist redrawn four times a
    /// second is four hundred rows of construction a second if it is rebuilt,
    /// and the only thing that actually changes between two of those ticks is
    /// one line's icon.
    fn set_items(&self, lines: &[Line]) {
        let imp = self.imp();
        let list = imp.items_list.get().expect("built");
        let mut items = imp.items.borrow_mut();

        let same = items.len() == lines.len()
            && items
                .iter()
                .zip(lines)
                .all(|(item, line)| item.index == line.index);
        if !same {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            *items = lines
                .iter()
                .map(|line| build_item(self, list, line))
                .collect();
        }

        for (item, line) in items.iter().zip(lines) {
            item.update(line);
        }
    }

    fn item_activated(&self, activated: &gtk::ListBoxRow) {
        let index = self
            .imp()
            .items
            .borrow()
            .iter()
            .find(|item| item.row == *activated)
            .map(|item| item.index);
        if let Some(index) = index {
            self.emit_by_name::<()>("item-open-requested", &[&(index as u64)]);
        }
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
    fn rebuild_controls(&self, job: &Job, whisper: bool) {
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
                // A pass over a playlist is an hour of CPU or three, so it has
                // to be stoppable by something better than removing the row.
                if job.transcript_is_running() {
                    self.add_button(
                        controls,
                        "media-playback-stop-symbolic",
                        "Stop Transcribing",
                        "stop-transcript-requested",
                    );
                } else if job.can_transcribe() {
                    // The catch-up: files on disk with no words beside them,
                    // whatever was asked for when they were downloaded.
                    let button = self.add_button(
                        controls,
                        "format-text-rich-symbolic",
                        "Transcribe",
                        "transcribe-requested",
                    );
                    // Present but insensitive on a machine without whisper,
                    // rather than a button that looks ready and then explains
                    // itself in a toast.
                    button.set_sensitive(whisper);
                    if !whisper {
                        button.set_tooltip_text(Some(
                            "Transcripts need whisper.cpp — see Preferences",
                        ));
                    }
                }
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

    fn add_button(
        &self,
        controls: &gtk::Box,
        icon: &str,
        tooltip: &str,
        signal: &'static str,
    ) -> gtk::Button {
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
        button
    }

    /// How many item lines are built. For `tests/widgets.rs`, which has no other
    /// way to tell an expanded playlist from a collapsed one.
    pub fn item_count(&self) -> usize {
        self.imp().items.borrow().len()
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

impl ItemRow {
    /// The line's current words and marks.
    ///
    /// Each label is compared before it is set: a hundred rows told to display
    /// the text they are already displaying is a hundred needless relayouts, and
    /// this runs on the redraw tick.
    fn update(&self, line: &Line) {
        if self.title.label() != line.title {
            self.title.set_label(&line.title);
            self.title.set_tooltip_text(Some(&line.title));
        }

        let detail = detail(line);
        if self.detail.label() != detail {
            self.detail.set_label(&detail);
        }

        // Two marks, each in its own slot: what the file is doing, and what its
        // words are doing. Sharing one slot put a tick and a spinner on top of
        // each other the moment an item was both saved and being transcribed.
        let done = matches!(line.stage, Stage::Done(_));
        self.done.set_visible(done);
        self.running.set_visible(line.stage == Stage::Running);
        self.transcribing.set_visible(line.words == Words::Running);
        self.transcript.set_visible(line.transcript().is_some());
        // Only a file that exists can be shown in Files.
        self.row.set_activatable(done);
        self.row.set_tooltip_text(done.then_some("Show in Files"));
    }
}

/// The right-hand column: what the item is doing, or how long it is.
///
/// A waiting item has nothing to report, so it reports its duration instead —
/// which is the fact someone scrolling a hundred-item playlist is actually
/// after. An item with words has a button for them, so the column does not need
/// to say so twice.
fn detail(line: &Line) -> String {
    match (&line.stage, &line.words, line.duration) {
        (Stage::Done(_), Words::Running, _) => "Transcribing".to_string(),
        (Stage::Done(_), Words::Failed, _) => "No transcript".to_string(),
        (Stage::Done(_), _, _) => "Saved".to_string(),
        (Stage::Running, _, _) => "Downloading".to_string(),
        (Stage::Waiting, _, Some(seconds)) => media::format_duration(seconds),
        (Stage::Waiting, _, None) => String::new(),
    }
}

fn build_item(owner: &JobRow, list: &gtk::ListBox, line: &Line) -> ItemRow {
    // Right-aligned and tabular, so a hundred and seven of them make a column
    // rather than a ragged edge.
    let number = gtk::Label::builder()
        .label(line.index.to_string())
        .xalign(1.0)
        .width_chars(3)
        .build();
    number.add_css_class("caption");
    number.add_css_class("dimmed");
    number.add_css_class("numeric");

    let title = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .lines(1)
        .build();

    let detail = gtk::Label::builder().xalign(1.0).build();
    detail.add_css_class("caption");
    detail.add_css_class("dimmed");
    detail.add_css_class("numeric");

    let done = gtk::Image::builder()
        .icon_name("object-select-symbolic")
        .visible(false)
        .build();
    done.add_css_class("success");

    // AdwSpinner rather than GtkSpinner: it keeps turning when the user has
    // reduced motion on, where GtkSpinner simply stops.
    let running = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .visible(false)
        .build();

    let transcribing = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .visible(false)
        .build();

    // The item's own transcript, on the item. A collection has as many as it has
    // items, so there is nothing sensible for the row above to open.
    let transcript = gtk::Button::builder()
        .icon_name("text-x-generic-symbolic")
        .tooltip_text("Open Transcript")
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    transcript.add_css_class("flat");
    transcript.add_css_class("circular");
    transcript.update_property(&[gtk::accessible::Property::Label("Open Transcript")]);
    let index = line.index as u64;
    transcript.connect_clicked(glib::clone!(
        #[weak]
        owner,
        move |_| owner.emit_by_name::<()>("item-transcript-requested", &[&index])
    ));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    // A slot each, sized whether or not anything is showing in them, so the
    // detail column stays a column as items finish. The two inside a slot are
    // mutually exclusive; the slots are not.
    let slot = |a: &gtk::Widget, b: &gtk::Widget| {
        let slot = gtk::Box::builder()
            .width_request(16)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        slot.append(a);
        slot.append(b);
        slot
    };

    content.append(&number);
    content.append(&title);
    content.append(&detail);
    content.append(&slot(done.upcast_ref(), running.upcast_ref()));
    content.append(&slot(transcribing.upcast_ref(), transcript.upcast_ref()));

    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .selectable(false)
        .build();
    list.append(&row);

    let item = ItemRow {
        index: line.index,
        row,
        title,
        detail,
        done,
        running,
        transcribing,
        transcript,
    };
    item.update(line);
    item
}
