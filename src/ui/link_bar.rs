//! The entry at the top of the window.
//!
//! Always visible, because pasting a link is the whole interaction and a
//! downloader that makes you open a dialog to reach the entry has put a door in
//! front of its front door.
//!
//! Emits `link-submitted` with a URL. It does not fetch, validate against a
//! network, or start anything.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;
use std::cell::OnceCell;
use std::sync::OnceLock;

use crate::model::url;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LinkBar {
        pub entry: OnceCell<gtk::Entry>,
        pub add: OnceCell<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LinkBar {
        const NAME: &'static str = "MagpieLinkBar";
        type Type = super::LinkBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // A bin layout so the single child fills the widget; the row inside
            // does the arranging.
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for LinkBar {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn dispose(&self) {
            // A plain GtkWidget subclass does not unparent its children for us,
            // and a child left parented at dispose is a GTK criticism on the
            // way out.
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("link-submitted")
                    .param_types([str::static_type()])
                    .build()]
            })
        }
    }

    impl WidgetImpl for LinkBar {}
}

glib::wrapper! {
    pub struct LinkBar(ObjectSubclass<imp::LinkBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LinkBar {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkBar {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn build(&self) {
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .placeholder_text("Paste a video or playlist link")
            .input_purpose(gtk::InputPurpose::Url)
            .activates_default(false)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Video or playlist link")]);

        let paste = gtk::Button::builder()
            .icon_name("edit-paste-symbolic")
            .tooltip_text("Paste from Clipboard")
            .valign(gtk::Align::Center)
            .build();
        paste.add_css_class("flat");

        let add = gtk::Button::builder()
            .label("Add")
            .tooltip_text("Add Download")
            .valign(gtk::Align::Center)
            // Insensitive until there is something plausible in the entry, so
            // the button never promises to act on prose.
            .sensitive(false)
            .build();
        add.add_css_class("suggested-action");

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        row.append(&entry);
        row.append(&paste);
        row.append(&add);
        row.add_css_class("link-bar");
        row.set_parent(self);

        entry.connect_changed(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |entry| bar.refresh(&entry.text())
        ));

        entry.connect_activate(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |_| bar.submit()
        ));

        add.connect_clicked(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |_| bar.submit()
        ));

        paste.connect_clicked(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            move |button| bar.paste_from(&button.clipboard())
        ));

        let _ = self.imp().entry.set(entry);
        let _ = self.imp().add.set(add);
    }

    fn entry(&self) -> &gtk::Entry {
        self.imp().entry.get().expect("built in constructed")
    }

    fn refresh(&self, text: &str) {
        if let Some(add) = self.imp().add.get() {
            add.set_sensitive(url::parse(text).is_some());
        }
    }

    fn submit(&self) {
        let text = self.entry().text();
        let Some(link) = url::parse(&text) else {
            return;
        };
        // Cleared here rather than by the window: the next paste should land in
        // an empty entry whatever the dialog goes on to do, including being
        // cancelled.
        self.entry().set_text("");
        self.emit_by_name::<()>("link-submitted", &[&link.url]);
    }

    /// Read the clipboard and, if it holds a link, submit it in one step.
    ///
    /// Pasting a link into a downloader has exactly one meaning. Making the user
    /// then press Add is a click that asks a question with one answer.
    fn paste_from(&self, clipboard: &gtk::gdk::Clipboard) {
        clipboard.read_text_async(
            gtk::gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = bar)]
                self,
                move |result| {
                    let Ok(Some(text)) = result else { return };
                    match url::parse(&text) {
                        Some(link) => bar.emit_by_name::<()>("link-submitted", &[&link.url]),
                        // Not a link: put it in the entry anyway so the user can
                        // see what they actually copied and fix it.
                        None => {
                            bar.entry().set_text(&text);
                            bar.entry().grab_focus();
                        }
                    }
                }
            ),
        );
    }

    /// Put the cursor in the entry, for the New Download shortcut.
    pub fn focus_entry(&self) {
        self.entry().grab_focus();
    }

    /// Show a link the user has not submitted yet, for a retry that failed to
    /// parse or a link handed in from outside.
    pub fn set_text(&self, text: &str) {
        self.entry().set_text(text);
        self.entry().set_position(-1);
    }

    /// Compact form for a narrow window: the Add button loses its label and
    /// keeps its tooltip.
    pub fn set_compact(&self, compact: bool) {
        if let Some(add) = self.imp().add.get() {
            if compact {
                add.set_label("");
                add.set_icon_name("list-add-symbolic");
            } else {
                add.set_icon_name("");
                add.set_label("Add");
            }
        }
    }
}
