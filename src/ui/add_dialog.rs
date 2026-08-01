//! The dialog that turns a pasted link into a queued download.
//!
//! An `AdwDialog` rather than a window: it presents as a centred sheet on a
//! desktop and a bottom sheet on a narrow screen, and it attaches with
//! `present(parent)` instead of `transient_for`.
//!
//! The dialog does not fetch anything. It opens in its Looking-up state, and the
//! application — which is the half that knows where yt-dlp is — calls
//! [`AddDialog::show_media`] or [`AddDialog::show_failure`] when it knows. That
//! keeps a widget out of the business of running processes, and it is what lets
//! `tests/widgets.rs` drive every state of this dialog with no network.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;

use crate::model::failure::Failure;
use crate::model::media::{self, Info, Media, Playlist};
use crate::model::quality::{AudioFormat, Quality};
use crate::model::request::{self, Collection, Selection};
use crate::model::settings::Settings;
use crate::model::transcript;

use super::thumbnail;

/// What the user settled on.
#[derive(Debug, Clone)]
pub struct Choice {
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub destination: PathBuf,
    pub selection: Selection,
    pub collection: Option<Collection>,
    pub transcribe: Option<transcript::Wish>,
}

/// The extra entry at the end of the quality list.
///
/// Only for a single video: the formats a playlist's items offer differ from one
/// item to the next, so a specific format id chosen from the first one is
/// meaningless for the rest.
const CHOOSE_A_FORMAT: &str = "Choose a specific format…";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AddDialog {
        pub url: RefCell<String>,
        pub settings: RefCell<Settings>,
        pub destination: RefCell<PathBuf>,
        pub info: RefCell<Option<Info>>,
        pub choice: RefCell<Option<Choice>>,
        pub whisper_available: std::cell::Cell<bool>,

        pub stack: OnceCell<gtk::Stack>,
        pub download: OnceCell<gtk::Button>,
        pub retry: OnceCell<gtk::Button>,

        pub poster: OnceCell<gtk::Picture>,
        pub poster_placeholder: OnceCell<gtk::Image>,
        pub heading: OnceCell<gtk::Label>,
        pub subheading: OnceCell<gtk::Label>,
        pub duration: OnceCell<gtk::Label>,

        pub audio_only: OnceCell<adw::SwitchRow>,
        pub quality: OnceCell<adw::ComboRow>,
        pub audio_format: OnceCell<adw::ComboRow>,
        pub exact_format: OnceCell<adw::ComboRow>,
        pub transcribe: OnceCell<adw::SwitchRow>,
        pub folder: OnceCell<adw::ActionRow>,

        pub items_group: OnceCell<adw::PreferencesGroup>,
        pub items_list: OnceCell<gtk::ListBox>,
        pub checks: RefCell<Vec<(usize, gtk::CheckButton)>>,

        pub failure_page: OnceCell<adw::StatusPage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AddDialog {
        const NAME: &'static str = "MagpieAddDialog";
        type Type = super::AddDialog;
        type ParentType = adw::Dialog;
    }

    impl ObjectImpl for AddDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("confirmed").build(),
                    Signal::builder("retry-requested").build(),
                ]
            })
        }
    }

    impl WidgetImpl for AddDialog {}
    impl AdwDialogImpl for AddDialog {}
}

glib::wrapper! {
    pub struct AddDialog(ObjectSubclass<imp::AddDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AddDialog {
    /// A dialog for `url`, showing the Looking-up state.
    pub fn new(url: &str, settings: &Settings, destination: PathBuf, whisper: bool) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.url.replace(url.to_string());
        imp.settings.replace(settings.clone());
        imp.destination.replace(destination);
        imp.whisper_available.set(whisper);
        dialog.apply_defaults();
        dialog
    }

    /// The choice, available once `confirmed` has fired.
    pub fn choice(&self) -> Option<Choice> {
        self.imp().choice.borrow().clone()
    }

    pub fn url(&self) -> String {
        self.imp().url.borrow().clone()
    }

    // -- construction -------------------------------------------------------

    fn build(&self) {
        self.set_title("Add Download");
        self.set_content_width(520);
        // No content height: the dialog takes its natural one, which is bounded
        // because the item list scrolls at 220px. Fixing it would leave a band of
        // empty sheet under a single video's four rows.

        let cancel = gtk::Button::with_label("Cancel");
        cancel.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                dialog.close();
            }
        ));

        let download = gtk::Button::with_label("Download");
        download.add_css_class("suggested-action");
        download.set_sensitive(false);
        download.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.confirm()
        ));

        let retry = gtk::Button::with_label("Try Again");
        retry.set_visible(false);
        retry.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                dialog.show_looking_up();
                dialog.emit_by_name::<()>("retry-requested", &[]);
            }
        ));

        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .show_start_title_buttons(false)
            // Named explicitly rather than left to the dialog's own title: the
            // automatic binding is only established once the dialog is
            // presented, so an unpresented one — a preview, or a widget test —
            // would show a header bar with nothing in it.
            .title_widget(&adw::WindowTitle::new("Add Download", ""))
            .build();
        header.pack_start(&cancel);
        header.pack_end(&download);
        header.pack_end(&retry);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vhomogeneous(false)
            .build();
        stack.add_named(&self.build_looking_up(), Some("looking-up"));
        stack.add_named(&self.build_ready(), Some("ready"));
        stack.add_named(&self.build_failure(), Some("failure"));
        stack.set_visible_child_name("looking-up");

        let view = adw::ToolbarView::builder().content(&stack).build();
        view.add_top_bar(&header);
        self.set_child(Some(&view));

        let imp = self.imp();
        let _ = imp.stack.set(stack);
        let _ = imp.download.set(download);
        let _ = imp.retry.set(retry);
    }

    fn build_looking_up(&self) -> gtk::Widget {
        // AdwSpinner rather than GtkSpinner: it still animates when the user has
        // reduced motion turned on, where GtkSpinner simply stops.
        let page = adw::StatusPage::builder()
            .title("Looking up the link")
            .description("Reading the title and the available formats")
            .build();
        page.set_paintable(Some(&adw::SpinnerPaintable::new(gtk::Widget::NONE)));
        page.upcast()
    }

    fn build_failure(&self) -> gtk::Widget {
        let page = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .title("Could not read the link")
            .build();
        let _ = self.imp().failure_page.set(page.clone());
        page.upcast()
    }

    fn build_ready(&self) -> gtk::Widget {
        let page = adw::PreferencesPage::new();
        page.add(&self.build_preview());
        page.add(&self.build_items());
        page.add(&self.build_options());
        page.add(&self.build_destination());

        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&page)
            .build()
            .upcast()
    }

    fn build_preview(&self) -> adw::PreferencesGroup {
        let (poster, picture, placeholder) = thumbnail::poster(160, 90);

        let duration = gtk::Label::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .visible(false)
            .build();
        duration.add_css_class("caption");
        duration.add_css_class("duration-badge");

        let framed = gtk::Overlay::builder().child(&poster).build();
        framed.add_overlay(&duration);

        let heading = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        heading.add_css_class("title-4");

        let subheading = gtk::Label::builder().xalign(0.0).build();
        subheading.add_css_class("caption");
        subheading.add_css_class("dimmed");

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&heading);
        text.append(&subheading);

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        row.append(&framed);
        row.append(&text);

        let group = adw::PreferencesGroup::new();
        group.add(&row);

        let imp = self.imp();
        let _ = imp.poster.set(picture);
        let _ = imp.poster_placeholder.set(placeholder);
        let _ = imp.heading.set(heading);
        let _ = imp.subheading.set(subheading);
        let _ = imp.duration.set(duration);
        group
    }

    fn build_items(&self) -> adw::PreferencesGroup {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            // Tall enough to show that it scrolls, short enough that the buttons
            // below stay in view on a laptop screen.
            .max_content_height(220)
            .propagate_natural_height(true)
            .child(&list)
            .build();

        let all = gtk::Button::with_label("Select All");
        all.add_css_class("flat");
        all.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.set_all_items(true)
        ));

        let none = gtk::Button::with_label("Select None");
        none.add_css_class("flat");
        none.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.set_all_items(false)
        ));

        let group = adw::PreferencesGroup::builder().title("Items").build();
        group.set_header_suffix(Some(&{
            let box_ = gtk::Box::builder().spacing(6).build();
            box_.append(&none);
            box_.append(&all);
            box_
        }));
        group.add(&scroller);
        group.set_visible(false);

        let imp = self.imp();
        let _ = imp.items_group.set(group.clone());
        let _ = imp.items_list.set(list);
        group
    }

    fn build_options(&self) -> adw::PreferencesGroup {
        let audio_only = adw::SwitchRow::builder()
            .title("Audio only")
            .subtitle("Save the sound and skip the video")
            .build();

        let quality = adw::ComboRow::builder().title("Quality").build();
        quality.set_model(Some(&string_list(
            Quality::ALL.iter().map(|q| q.label()).collect::<Vec<_>>(),
        )));

        let audio_format = adw::ComboRow::builder()
            .title("Format")
            .visible(false)
            .build();
        audio_format.set_model(Some(&string_list(
            AudioFormat::ALL
                .iter()
                .map(|f| f.label())
                .collect::<Vec<_>>(),
        )));

        let exact_format = adw::ComboRow::builder()
            .title("Format")
            .subtitle("Passed to yt-dlp exactly as listed")
            .visible(false)
            .build();

        let transcribe = adw::SwitchRow::builder()
            .title("Transcribe")
            .subtitle("Write a text transcript next to the file")
            .build();

        audio_only.connect_active_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.refresh_format_rows()
        ));
        quality.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.refresh_format_rows()
        ));

        let group = adw::PreferencesGroup::new();
        group.add(&audio_only);
        group.add(&quality);
        group.add(&audio_format);
        group.add(&exact_format);
        group.add(&transcribe);

        let imp = self.imp();
        let _ = imp.audio_only.set(audio_only);
        let _ = imp.quality.set(quality);
        let _ = imp.audio_format.set(audio_format);
        let _ = imp.exact_format.set(exact_format);
        let _ = imp.transcribe.set(transcribe);
        group
    }

    fn build_destination(&self) -> adw::PreferencesGroup {
        let choose = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Choose Folder")
            .valign(gtk::Align::Center)
            .build();
        choose.add_css_class("flat");

        let folder = adw::ActionRow::builder().title("Save to").build();
        folder.add_suffix(&choose);
        folder.set_activatable_widget(Some(&choose));

        choose.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.choose_folder()
        ));

        let group = adw::PreferencesGroup::new();
        group.add(&folder);
        let _ = self.imp().folder.set(folder);
        group
    }

    // -- state --------------------------------------------------------------

    /// Back to the spinner, for a retry.
    pub fn show_looking_up(&self) {
        let imp = self.imp();
        imp.stack
            .get()
            .expect("built")
            .set_visible_child_name("looking-up");
        imp.download.get().expect("built").set_sensitive(false);
        imp.retry.get().expect("built").set_visible(false);
    }

    /// Show what the link turned out to be.
    pub fn show_media(&self, info: Info) {
        let imp = self.imp();
        match &info {
            Info::Single(media) => self.fill_single(media),
            Info::Collection(playlist) => self.fill_collection(playlist),
        }
        imp.info.replace(Some(info));
        self.refresh_format_rows();
        self.refresh_destination();

        imp.stack
            .get()
            .expect("built")
            .set_visible_child_name("ready");
        imp.download.get().expect("built").set_sensitive(true);
        imp.retry.get().expect("built").set_visible(false);
        // The safe, expected action gets the focus so Enter does the obvious
        // thing.
        imp.download.get().expect("built").grab_focus();
    }

    /// Show why the link could not be read.
    pub fn show_failure(&self, failure: &Failure) {
        let imp = self.imp();
        if let Some(page) = imp.failure_page.get() {
            page.set_title(failure.title());
            let mut description = failure.guidance().to_string();
            if let Some(detail) = failure.detail() {
                description.push_str("\n\n");
                description.push_str(detail);
            }
            page.set_description(Some(&description));
        }
        imp.stack
            .get()
            .expect("built")
            .set_visible_child_name("failure");
        imp.download.get().expect("built").set_sensitive(false);
        imp.retry
            .get()
            .expect("built")
            .set_visible(failure.is_retryable());
    }

    /// A poster for the preview.
    pub fn set_poster(&self, texture: Option<&gtk::gdk::Texture>) {
        let (Some(picture), Some(placeholder)) =
            (self.imp().poster.get(), self.imp().poster_placeholder.get())
        else {
            return;
        };
        picture.set_paintable(texture.map(|t| t.upcast_ref::<gtk::gdk::Paintable>()));
        picture.set_visible(texture.is_some());
        placeholder.set_visible(texture.is_none());
    }

    fn fill_single(&self, media: &Media) {
        let imp = self.imp();
        imp.heading.get().expect("built").set_label(&media.title);

        let mut parts: Vec<String> = Vec::new();
        if let Some(uploader) = &media.uploader {
            parts.push(uploader.clone());
        }
        if media.is_live {
            // A livestream has no end, so nothing here can say how long it will
            // take or how large it will be.
            parts.push("Live — will record until stopped".to_string());
        }
        imp.subheading
            .get()
            .expect("built")
            .set_label(&parts.join(" · "));

        let duration = imp.duration.get().expect("built");
        match media.duration.filter(|_| !media.is_live) {
            Some(seconds) => {
                duration.set_label(&media::format_duration(seconds));
                duration.set_visible(true);
            }
            None => duration.set_visible(false),
        }

        // Only meaningful for one video, so the row and the extra combo entry
        // both belong to this branch.
        self.set_exact_formats(media);
        imp.items_group.get().expect("built").set_visible(false);
        imp.transcribe.get().expect("built").set_visible(true);
    }

    fn fill_collection(&self, playlist: &Playlist) {
        let imp = self.imp();
        imp.heading.get().expect("built").set_label(&playlist.title);

        let mut parts = vec![format!("{} items", playlist.entries.len())];
        if let Some(uploader) = &playlist.uploader {
            parts.insert(0, uploader.clone());
        }
        imp.subheading
            .get()
            .expect("built")
            .set_label(&parts.join(" · "));
        imp.duration.get().expect("built").set_visible(false);

        let list = imp.items_list.get().expect("built");
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }

        let mut checks = Vec::new();
        for entry in &playlist.entries {
            let check = gtk::CheckButton::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&entry.title))
                .subtitle(
                    entry
                        .duration
                        .map(media::format_duration)
                        .unwrap_or_default(),
                )
                .activatable_widget(&check)
                .build();
            row.add_prefix(&check);
            list.append(&row);
            checks.push((entry.index, check));
        }
        imp.checks.replace(checks);
        imp.items_group.get().expect("built").set_visible(true);

        // Transcribing forty items is an afternoon of CPU nobody asked for by
        // flipping one switch, so the switch is not offered for a collection.
        let transcribe = imp.transcribe.get().expect("built");
        transcribe.set_active(false);
        transcribe.set_visible(false);

        // A format id from item one means nothing for item two.
        self.clear_exact_formats();
    }

    fn set_exact_formats(&self, media: &Media) {
        let imp = self.imp();
        let labels: Vec<String> = media.formats.iter().map(|f| f.label()).collect();
        let exact = imp.exact_format.get().expect("built");

        if labels.is_empty() {
            self.clear_exact_formats();
            return;
        }
        exact.set_model(Some(&string_list(
            labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));

        let quality = imp.quality.get().expect("built");
        let mut entries: Vec<&str> = Quality::ALL.iter().map(|q| q.label()).collect();
        entries.push(CHOOSE_A_FORMAT);
        let selected = quality.selected();
        quality.set_model(Some(&string_list(entries)));
        quality.set_selected(selected.min(Quality::ALL.len() as u32 - 1));
    }

    fn clear_exact_formats(&self) {
        let imp = self.imp();
        imp.exact_format.get().expect("built").set_visible(false);
        let quality = imp.quality.get().expect("built");
        let selected = quality.selected();
        quality.set_model(Some(&string_list(
            Quality::ALL.iter().map(|q| q.label()).collect::<Vec<_>>(),
        )));
        quality.set_selected(selected.min(Quality::ALL.len() as u32 - 1));
    }

    fn apply_defaults(&self) {
        let imp = self.imp();
        let settings = imp.settings.borrow().clone();

        imp.audio_only
            .get()
            .expect("built")
            .set_active(settings.audio_only);
        let quality_index = Quality::ALL
            .iter()
            .position(|q| *q == settings.quality)
            .unwrap_or(0) as u32;
        imp.quality
            .get()
            .expect("built")
            .set_selected(quality_index);
        let audio_index = AudioFormat::ALL
            .iter()
            .position(|f| *f == settings.audio_format)
            .unwrap_or(0) as u32;
        imp.audio_format
            .get()
            .expect("built")
            .set_selected(audio_index);

        let transcribe = imp.transcribe.get().expect("built");
        let available = imp.whisper_available.get();
        // Present but insensitive, with the reason in the tooltip. Hiding it
        // would mean the switch appears and disappears between machines, which
        // is harder to learn than one that greys out.
        transcribe.set_sensitive(available);
        transcribe.set_active(available && settings.transcribe_by_default);
        transcribe.set_tooltip_text(
            (!available).then_some(
                "Transcripts need whisper.cpp. Magpie looks for whisper-cli on your PATH.",
            ),
        );

        self.refresh_format_rows();
        self.refresh_destination();
    }

    fn refresh_format_rows(&self) {
        let imp = self.imp();
        let audio_only = imp.audio_only.get().expect("built").is_active();
        let quality = imp.quality.get().expect("built");
        let choosing_exact = !audio_only && self.quality_is_choose_a_format();

        quality.set_visible(!audio_only);
        imp.audio_format
            .get()
            .expect("built")
            .set_visible(audio_only);
        imp.exact_format
            .get()
            .expect("built")
            .set_visible(choosing_exact);
    }

    fn quality_is_choose_a_format(&self) -> bool {
        let quality = self.imp().quality.get().expect("built");
        quality.selected() as usize >= Quality::ALL.len()
    }

    fn refresh_destination(&self) {
        let imp = self.imp();
        let destination = imp.destination.borrow().clone();
        let folder = imp.folder.get().expect("built");

        let mut shown = destination.display().to_string();
        // Everyone's paths start with the same seven characters; the home tilde
        // is what a file manager shows too.
        if let Some(home) = glib::home_dir().to_str() {
            if let Some(rest) = shown.strip_prefix(home) {
                shown = format!("~{rest}");
            }
        }
        if let Some(collection) = self.collection_folder() {
            shown = format!("{shown}/{collection}");
        }
        folder.set_subtitle(&shown);
    }

    /// The subfolder a collection will be downloaded into.
    fn collection_folder(&self) -> Option<String> {
        match self.imp().info.borrow().as_ref() {
            Some(Info::Collection(playlist)) => Some(request::folder_name(&playlist.title)),
            _ => None,
        }
    }

    fn set_all_items(&self, active: bool) {
        for (_, check) in self.imp().checks.borrow().iter() {
            check.set_active(active);
        }
    }

    fn choose_folder(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose Download Folder")
            .modal(true)
            .initial_folder(&gtk::gio::File::for_path(
                self.imp().destination.borrow().as_path(),
            ))
            .build();

        let parent = self.root().and_downcast::<gtk::Window>();
        dialog.select_folder(
            parent.as_ref(),
            gtk::gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = add)]
                self,
                move |result| {
                    // A cancelled file dialog is an answer, not an error.
                    if let Some(path) = result.ok().and_then(|file| file.path()) {
                        add.imp().destination.replace(path);
                        add.refresh_destination();
                    }
                }
            ),
        );
    }

    // -- outcome ------------------------------------------------------------

    fn confirm(&self) {
        let Some(choice) = self.build_choice() else {
            return;
        };
        self.imp().choice.replace(Some(choice));
        self.emit_by_name::<()>("confirmed", &[]);
        self.close();
    }

    fn build_choice(&self) -> Option<Choice> {
        let imp = self.imp();
        let info = imp.info.borrow();
        let info = info.as_ref()?;

        let selection = self.build_selection(info);
        let collection = match info {
            Info::Collection(playlist) => {
                let checks = imp.checks.borrow();
                let items: Vec<usize> = checks
                    .iter()
                    .filter(|(_, check)| check.is_active())
                    .map(|(index, _)| *index)
                    .collect();
                if items.is_empty() {
                    // Nothing ticked is not a request to download everything.
                    return None;
                }
                Some(Collection {
                    folder: request::folder_name(&playlist.title),
                    // Every item ticked is expressed as no filter at all, which
                    // is one fewer argument and lets yt-dlp pick up items added
                    // to the playlist since the dialog opened.
                    items: if items.len() == checks.len() {
                        Vec::new()
                    } else {
                        items
                    },
                })
            }
            Info::Single(_) => None,
        };

        let transcribe = {
            let row = imp.transcribe.get().expect("built");
            (row.is_sensitive() && row.is_active() && collection.is_none())
                .then(|| imp.settings.borrow().transcript.clone())
        };

        let (title, thumbnail) = match info {
            Info::Single(media) => (media.title.clone(), media.thumbnail.clone()),
            Info::Collection(playlist) => (playlist.title.clone(), None),
        };

        Some(Choice {
            url: imp.url.borrow().clone(),
            title,
            thumbnail,
            destination: imp.destination.borrow().clone(),
            selection,
            collection,
            transcribe,
        })
    }

    fn build_selection(&self, info: &Info) -> Selection {
        let imp = self.imp();
        if imp.audio_only.get().expect("built").is_active() {
            let index = imp.audio_format.get().expect("built").selected() as usize;
            return Selection::Audio(AudioFormat::ALL.get(index).copied().unwrap_or_default());
        }

        if self.quality_is_choose_a_format() {
            if let Info::Single(media) = info {
                let index = imp.exact_format.get().expect("built").selected() as usize;
                if let Some(format) = media.formats.get(index) {
                    return Selection::Exact(format.id.clone());
                }
            }
        }

        let index = imp.quality.get().expect("built").selected() as usize;
        Selection::Video(Quality::ALL.get(index).copied().unwrap_or_default())
    }
}

fn string_list<'a, I: IntoIterator<Item = &'a str>>(items: I) -> gtk::StringList {
    let list = gtk::StringList::new(&[]);
    for item in items {
        list.append(item);
    }
    list
}
