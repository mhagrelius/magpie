//! The window: a header bar, a banner, the link bar, and the list.
//!
//! There is no sidebar and no navigation, because there is one thing to look at.
//! What the window does have is three states of the same list — nothing yet,
//! something happening, and yt-dlp missing — and switching between them is most
//! of the code here.
//!
//! The window holds no queue. It is given jobs to display and emits what the
//! user asked for; [`super::MagpieApplication`] owns the state.

use std::cell::{OnceCell, RefCell};
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::glib::subclass::Signal;

use crate::model::job::{Job, Progress, State};

use super::job_row::JobRow;
use super::link_bar::LinkBar;

/// Rows rendered at once.
///
/// The library keeps a couple of thousand finished downloads; a `GtkListBox` with
/// two thousand rows of image, labels and buttons is a visibly slow window. The
/// list shows the queue and the recent past, and Clear Finished is how the rest
/// goes. A `GtkListView` with a factory would lift the cap, and is the change to
/// make if this list ever becomes something people browse rather than watch.
const VISIBLE_ROWS: usize = 100;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MagpieWindow {
        pub link_bar: OnceCell<LinkBar>,
        pub banner: OnceCell<adw::Banner>,
        pub toasts: OnceCell<adw::ToastOverlay>,
        pub stack: OnceCell<gtk::Stack>,
        pub list: OnceCell<gtk::ListBox>,
        pub subtitle: OnceCell<adw::WindowTitle>,
        pub rows: RefCell<Vec<JobRow>>,
        pub banner_action: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MagpieWindow {
        const NAME: &'static str = "MagpieWindow";
        type Type = super::MagpieWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for MagpieWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("link-submitted")
                        .param_types([str::static_type()])
                        .build(),
                    // A job id and what to do with it. One signal rather than
                    // eight keeps the application's wiring to a single match.
                    Signal::builder("job-action")
                        .param_types([u64::static_type(), str::static_type()])
                        .build(),
                    Signal::builder("banner-activated").build(),
                ]
            })
        }
    }

    impl WidgetImpl for MagpieWindow {}
    impl WindowImpl for MagpieWindow {}
    impl ApplicationWindowImpl for MagpieWindow {}
    impl AdwApplicationWindowImpl for MagpieWindow {}
}

glib::wrapper! {
    pub struct MagpieWindow(ObjectSubclass<imp::MagpieWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl MagpieWindow {
    pub fn new(application: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder()
            .property("application", application)
            .build()
    }

    fn build(&self) {
        self.set_title(Some("Magpie"));
        self.set_default_size(720, 640);
        // Narrow enough for a phone-width window and short enough for the
        // link bar plus one row, which is the smallest the window is still
        // useful at.
        self.set_size_request(360, 294);

        let title = adw::WindowTitle::new("Magpie", "");

        let menu = gio::Menu::new();
        let list_section = gio::Menu::new();
        list_section.append(Some("Clear Finished"), Some("win.clear-finished"));
        list_section.append(Some("Open Download Folder"), Some("win.open-folder"));
        menu.append_section(None, &list_section);

        let app_section = gio::Menu::new();
        app_section.append(Some("Preferences"), Some("app.preferences"));
        app_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        app_section.append(Some("About Magpie"), Some("app.about"));
        menu.append_section(None, &app_section);

        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .primary(true)
            .build();

        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_end(&menu_button);

        let banner = adw::Banner::builder().revealed(false).build();
        banner.connect_button_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.emit_by_name::<()>("banner-activated", &[])
        ));

        let link_bar = LinkBar::new();
        link_bar.connect_closure(
            "link-submitted",
            false,
            glib::closure_local!(
                #[watch(rename_to = window)]
                self,
                move |_: LinkBar, url: &str| {
                    window.emit_by_name::<()>("link-submitted", &[&url]);
                }
            ),
        );

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        list.add_css_class("boxed-list");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(700)
                    .tightening_threshold(600)
                    .child(&list)
                    .build(),
            )
            .build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();
        stack.add_named(&Self::build_empty(), Some("empty"));
        stack.add_named(&scroller, Some("list"));
        stack.set_visible_child_name("empty");

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        content.append(
            &adw::Clamp::builder()
                .maximum_size(700)
                .tightening_threshold(600)
                .margin_top(12)
                .margin_start(12)
                .margin_end(12)
                .child(&link_bar)
                .build(),
        );
        content.append(&stack);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&content));

        let view = adw::ToolbarView::builder().content(&toasts).build();
        view.add_top_bar(&header);
        view.add_top_bar(&banner);
        self.set_content(Some(&view));

        // Below this the Add button keeps its tooltip and loses its label, which
        // is the only thing in the window that does not simply narrow.
        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            420.0,
            adw::LengthUnit::Sp,
        ));
        breakpoint.connect_apply(glib::clone!(
            #[weak]
            link_bar,
            move |_| link_bar.set_compact(true)
        ));
        breakpoint.connect_unapply(glib::clone!(
            #[weak]
            link_bar,
            move |_| link_bar.set_compact(false)
        ));
        self.add_breakpoint(breakpoint);

        self.install_actions();

        let imp = self.imp();
        let _ = imp.link_bar.set(link_bar);
        let _ = imp.banner.set(banner);
        let _ = imp.toasts.set(toasts);
        let _ = imp.stack.set(stack);
        let _ = imp.list.set(list);
        let _ = imp.subtitle.set(title);
    }

    fn build_empty() -> gtk::Widget {
        adw::StatusPage::builder()
            .icon_name("folder-download-symbolic")
            .title("Nothing downloading")
            .description("Paste a link above to bring a video home.")
            .build()
            .upcast()
    }

    fn install_actions(&self) {
        let shortcuts = gio::ActionEntry::builder("shortcuts")
            .activate(|window: &Self, _, _| window.show_shortcuts())
            .build();
        let new_download = gio::ActionEntry::builder("new-download")
            .activate(|window: &Self, _, _| {
                if let Some(bar) = window.imp().link_bar.get() {
                    bar.focus_entry();
                }
            })
            .build();
        // These two are the window's to name and the application's to carry out:
        // it owns the queue and the settings.
        let clear = gio::ActionEntry::builder("clear-finished")
            .activate(|window: &Self, _, _| {
                window.emit_by_name::<()>("job-action", &[&0u64, &"clear-finished"]);
            })
            .build();
        let open_folder = gio::ActionEntry::builder("open-folder")
            .activate(|window: &Self, _, _| {
                window.emit_by_name::<()>("job-action", &[&0u64, &"open-folder"]);
            })
            .build();
        self.add_action_entries([shortcuts, new_download, clear, open_folder]);
    }

    fn show_shortcuts(&self) {
        let section = adw::ShortcutsSection::new(Some("General"));
        for (accelerator, title) in [
            ("<Control>n", "Focus the link box"),
            ("<Control>v", "Paste and add a link"),
            ("<Control><Shift>o", "Open the download folder"),
            ("<Control>comma", "Preferences"),
            ("<Control>question", "Keyboard shortcuts"),
            ("<Control>w", "Close the window"),
            ("<Control>q", "Quit"),
        ] {
            section.add(adw::ShortcutsItem::new(title, accelerator));
        }

        let dialog = adw::ShortcutsDialog::new();
        dialog.add(section);
        dialog.present(Some(self));
    }

    /// Show `jobs`, reusing the rows that are already right.
    ///
    /// Rows are matched by job id rather than rebuilt: replacing the whole list
    /// on every progress tick would lose the keyboard focus, drop the scroll
    /// position, and re-request every thumbnail four times a second.
    pub fn set_jobs(&self, jobs: &[Job], progress: &dyn Fn(u64) -> Option<Progress>) {
        let imp = self.imp();
        let list = imp.list.get().expect("built");

        // What is happening now, then what is about to, then what already did.
        //
        // Not simply newest-first: that buries a running download under every
        // finished one, and the row someone is watching is the reason the window
        // is open. Within a group, the queue's own order for anything unfinished
        // and newest-first for the past.
        let mut ordered: Vec<&Job> = jobs.iter().collect();
        ordered.sort_by_key(|job| {
            let group = match job.state {
                State::Running | State::Paused => 0,
                State::Waiting => 1,
                State::Done | State::Failed(_) => 2,
            };
            let within = if group == 2 {
                // Reversed, so the most recently finished is at the top of its
                // group rather than the bottom.
                u64::MAX - job.id
            } else {
                job.id
            };
            (group, within)
        });
        ordered.truncate(VISIBLE_ROWS);

        let mut rows = imp.rows.borrow_mut();

        // Drop rows for jobs that are gone.
        rows.retain(|row| {
            let kept = ordered.iter().any(|job| job.id == row.id());
            if !kept {
                list.remove(row);
            }
            kept
        });

        for (position, job) in ordered.iter().enumerate() {
            let row = match rows.iter().find(|row| row.id() == job.id) {
                Some(row) => row.clone(),
                None => {
                    let row = JobRow::new(job.id);
                    self.connect_row(&row);
                    rows.push(row.clone());
                    row
                }
            };
            row.bind(job, progress(job.id).as_ref());
            if row.parent().is_none() {
                list.append(&row);
            }
            // `set_index` is not a thing on GtkListBoxRow, so a moved row is
            // re-inserted at its place.
            if list.row_at_index(position as i32).as_ref() != Some(row.upcast_ref()) {
                list.remove(&row);
                list.insert(&row, position as i32);
            }
        }

        drop(rows);
        imp.stack
            .get()
            .expect("built")
            .set_visible_child_name(if ordered.is_empty() { "empty" } else { "list" });
    }

    fn connect_row(&self, row: &JobRow) {
        for (signal, action) in [
            ("pause-requested", "pause"),
            ("resume-requested", "resume"),
            ("cancel-requested", "cancel"),
            ("retry-requested", "retry"),
            ("remove-requested", "remove"),
            ("open-requested", "open"),
            ("transcript-requested", "transcript"),
            ("details-requested", "details"),
        ] {
            row.connect_closure(
                signal,
                false,
                glib::closure_local!(
                    #[watch(rename_to = window)]
                    self,
                    move |row: JobRow| {
                        window.emit_by_name::<()>("job-action", &[&row.id(), &action]);
                    }
                ),
            );
        }
    }

    /// A poster for one row, if that row is on screen.
    pub fn set_poster(&self, id: u64, texture: &gtk::gdk::Texture) {
        if let Some(row) = self.imp().rows.borrow().iter().find(|row| row.id() == id) {
            row.set_poster(Some(texture));
        }
    }

    /// The line under the window title.
    pub fn set_summary(&self, summary: Option<&str>) {
        if let Some(title) = self.imp().subtitle.get() {
            title.set_subtitle(summary.unwrap_or(""));
        }
    }

    /// A condition that persists until something changes: a missing tool, a
    /// yt-dlp too old to work. Never used for events — those are toasts.
    pub fn set_banner(&self, message: Option<(&str, &str)>) {
        let Some(banner) = self.imp().banner.get() else {
            return;
        };
        match message {
            Some((text, button)) => {
                banner.set_title(text);
                banner.set_button_label(Some(button));
                banner.set_revealed(true);
            }
            None => banner.set_revealed(false),
        }
    }

    pub fn toast(&self, message: &str) {
        if let Some(toasts) = self.imp().toasts.get() {
            toasts.add_toast(adw::Toast::new(message));
        }
    }

    /// A toast with a button, for anything undoable.
    pub fn toast_with_action(&self, message: &str, label: &str, action: &str) {
        if let Some(toasts) = self.imp().toasts.get() {
            let toast = adw::Toast::new(message);
            toast.set_button_label(Some(label));
            toast.set_action_name(Some(action));
            toasts.add_toast(toast);
        }
    }

    pub fn set_link_text(&self, text: &str) {
        if let Some(bar) = self.imp().link_bar.get() {
            bar.set_text(text);
        }
    }
}
