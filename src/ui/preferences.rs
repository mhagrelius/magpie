//! Preferences: three pages, an `AdwPreferencesDialog`.
//!
//! The dialog owns a copy of the settings and emits `changed` when a control
//! moves; the application reads the copy back and persists it. Nothing here
//! writes a file, and nothing here reaches into the queue.
//!
//! The Tools page is the unusual one. Most applications have no business showing
//! the user which binaries they found — but Magpie's most common failure is a
//! yt-dlp too old for the site it is pointed at, and that failure is invisible
//! from the error message alone. A page that says which yt-dlp is in use, how old
//! it is, and how to get a newer one turns a mystery into a chore.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;

use crate::model::progress::format_bytes;
use crate::model::quality::{AudioFormat, Quality};
use crate::model::queue::MAX_PARALLELISM;
use crate::model::settings::{Settings, BROWSERS};
use crate::model::tools::Tool;
use crate::model::transcript::{self, Model};

use super::toolbox::{self, Report};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Preferences {
        pub settings: RefCell<Settings>,
        pub report: RefCell<Report>,
        pub models_dir: RefCell<PathBuf>,
        pub download_fallback: RefCell<PathBuf>,
        /// Set while the dialog is filling controls from the settings, so that
        /// the `notify` handlers do not write the value they were just given
        /// back over the top and emit a change for every row on open.
        pub loading: Cell<bool>,
        pub model_download: RefCell<Option<toolbox::ModelDownload>>,

        pub folder_row: OnceCell<adw::ActionRow>,
        pub quality_row: OnceCell<adw::ComboRow>,
        pub audio_row: OnceCell<adw::ComboRow>,
        pub audio_only_row: OnceCell<adw::SwitchRow>,
        pub confirm_row: OnceCell<adw::SwitchRow>,
        pub parallel_row: OnceCell<adw::SpinRow>,
        pub cookies_row: OnceCell<adw::ComboRow>,
        pub rate_row: OnceCell<adw::EntryRow>,

        pub model_row: OnceCell<adw::ComboRow>,
        pub model_status: OnceCell<adw::ActionRow>,
        pub model_button: OnceCell<gtk::Button>,
        pub model_progress: OnceCell<gtk::ProgressBar>,
        pub transcript_format_row: OnceCell<adw::ComboRow>,
        pub language_row: OnceCell<adw::ComboRow>,
        pub transcribe_default_row: OnceCell<adw::SwitchRow>,

        /// Widgets are kept beside their row rather than found by walking the
        /// row's children, which breaks the moment a suffix is added.
        pub tool_rows: RefCell<Vec<super::ToolRow>>,
        /// Tools with an installer running, so a second press cannot start it
        /// twice while the first is still going.
        pub installing: RefCell<Vec<Tool>>,
        pub whisper_banner: OnceCell<adw::Banner>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Preferences {
        const NAME: &'static str = "MagpiePreferences";
        type Type = super::Preferences;
        type ParentType = adw::PreferencesDialog;
    }

    impl ObjectImpl for Preferences {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("changed").build(),
                    Signal::builder("tools-rescan-requested").build(),
                ]
            })
        }
    }

    impl WidgetImpl for Preferences {}
    impl AdwDialogImpl for Preferences {}
    impl PreferencesDialogImpl for Preferences {}
}

/// One row of the Tools page and everything that changes on it.
pub struct ToolRow {
    tool: Tool,
    row: adw::ActionRow,
    icon: gtk::Image,
    copy: gtk::Button,
    install: gtk::Button,
    spinner: adw::Spinner,
}

glib::wrapper! {
    pub struct Preferences(ObjectSubclass<imp::Preferences>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Preferences {
    pub fn new(
        settings: &Settings,
        report: &Report,
        models_dir: PathBuf,
        download_fallback: PathBuf,
    ) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.settings.replace(settings.clone());
        imp.report.replace(report.clone());
        imp.models_dir.replace(models_dir);
        imp.download_fallback.replace(download_fallback);
        dialog.load();
        dialog
    }

    /// The settings as the dialog currently has them.
    pub fn settings(&self) -> Settings {
        self.imp().settings.borrow().clone()
    }

    /// Replace the tool report after a rescan.
    pub fn set_report(&self, report: &Report) {
        self.imp().report.replace(report.clone());
        self.refresh_tools();
    }

    fn build(&self) {
        self.set_title("Preferences");
        self.add(&self.build_general());
        self.add(&self.build_transcripts());
        self.add(&self.build_tools());
    }

    fn changed(&self) {
        if self.imp().loading.get() {
            return;
        }
        self.emit_by_name::<()>("changed", &[]);
    }

    // -- General ------------------------------------------------------------

    fn build_general(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("General")
            .name("general")
            .icon_name("preferences-system-symbolic")
            .build();

        // Downloads
        let choose = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Choose Folder")
            .valign(gtk::Align::Center)
            .build();
        choose.add_css_class("flat");
        choose.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.choose_folder()
        ));

        let folder_row = adw::ActionRow::builder().title("Save downloads to").build();
        folder_row.add_suffix(&choose);
        folder_row.set_activatable_widget(Some(&choose));

        let quality_row = adw::ComboRow::builder()
            .title("Quality")
            .subtitle("The default for new downloads")
            .model(&string_list(Quality::ALL.iter().map(|q| q.label())))
            .build();

        let audio_only_row = adw::SwitchRow::builder()
            .title("Audio only by default")
            .subtitle("Start each download with the video skipped")
            .build();

        let audio_row = adw::ComboRow::builder()
            .title("Audio format")
            .model(&string_list(AudioFormat::ALL.iter().map(|f| f.label())))
            .build();

        let downloads = adw::PreferencesGroup::builder().title("Downloads").build();
        downloads.add(&folder_row);
        downloads.add(&quality_row);
        downloads.add(&audio_only_row);
        downloads.add(&audio_row);
        page.add(&downloads);

        // Behaviour
        let confirm_row = adw::SwitchRow::builder()
            .title("Ask before each download")
            .subtitle("Turn off to start immediately with these settings")
            .build();

        let parallel_row = adw::SpinRow::builder()
            .title("Downloads at once")
            .subtitle("More at once is rarely faster and more often rate limited")
            .adjustment(&gtk::Adjustment::new(
                1.0,
                1.0,
                MAX_PARALLELISM as f64,
                1.0,
                1.0,
                0.0,
            ))
            .build();

        let behaviour = adw::PreferencesGroup::builder().title("Behaviour").build();
        behaviour.add(&confirm_row);
        behaviour.add(&parallel_row);
        page.add(&behaviour);

        // Network
        let mut browsers: Vec<String> = vec!["Do not use cookies".to_string()];
        browsers.extend(BROWSERS.iter().map(|name| {
            let mut chars = name.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }));

        let cookies_row = adw::ComboRow::builder()
            .title("Use cookies from a browser")
            .subtitle("Needed for anything that asks you to sign in or confirm your age")
            .model(&string_list(browsers.iter().map(String::as_str)))
            .build();

        let rate_row = adw::EntryRow::builder()
            .title("Speed limit")
            .text("")
            .build();
        // The format is not guessable, so an example is part of the label rather
        // than something to discover by being rejected.
        rate_row.set_tooltip_text(Some(
            "A size per second, such as 2M or 500K. Leave empty for no limit.",
        ));

        let network = adw::PreferencesGroup::builder()
            .title("Network")
            .description(
                "Cookies are read from the browser's own profile. \
                 Magpie never stores them.",
            )
            .build();
        network.add(&cookies_row);
        network.add(&rate_row);
        page.add(&network);

        // Wiring, after every row exists.
        quality_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().quality =
                    Quality::ALL[(row.selected() as usize).min(Quality::ALL.len() - 1)];
                dialog.changed();
            }
        ));
        audio_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().audio_format =
                    AudioFormat::ALL[(row.selected() as usize).min(AudioFormat::ALL.len() - 1)];
                dialog.changed();
                dialog.refresh_ffmpeg_warning();
            }
        ));
        audio_only_row.connect_active_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().audio_only = row.is_active();
                dialog.changed();
            }
        ));
        confirm_row.connect_active_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().confirm_each_download = row.is_active();
                dialog.changed();
            }
        ));
        parallel_row.connect_value_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().simultaneous_downloads = row.value() as usize;
                dialog.changed();
            }
        ));
        cookies_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                let index = row.selected() as usize;
                dialog.imp().settings.borrow_mut().cookies_from_browser = index
                    .checked_sub(1)
                    .and_then(|index| BROWSERS.get(index))
                    .map(|name| name.to_string());
                dialog.changed();
            }
        ));
        rate_row.connect_changed(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                let text = row.text().trim().to_string();
                dialog.imp().settings.borrow_mut().rate_limit = (!text.is_empty()).then_some(text);
                // Sanitising here rather than on save means a value yt-dlp would
                // reject is dropped before it can fail a download, and the row
                // says so.
                let kept = dialog
                    .imp()
                    .settings
                    .borrow()
                    .clone()
                    .sanitised()
                    .rate_limit;
                let entered = dialog.imp().settings.borrow().rate_limit.clone();
                if entered.is_some() && kept.is_none() {
                    row.add_css_class("error");
                } else {
                    row.remove_css_class("error");
                }
                dialog.changed();
            }
        ));

        let imp = self.imp();
        let _ = imp.folder_row.set(folder_row);
        let _ = imp.quality_row.set(quality_row);
        let _ = imp.audio_row.set(audio_row);
        let _ = imp.audio_only_row.set(audio_only_row);
        let _ = imp.confirm_row.set(confirm_row);
        let _ = imp.parallel_row.set(parallel_row);
        let _ = imp.cookies_row.set(cookies_row);
        let _ = imp.rate_row.set(rate_row);
        page
    }

    // -- Transcripts --------------------------------------------------------

    fn build_transcripts(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Transcripts")
            .name("transcripts")
            .icon_name("text-x-generic-symbolic")
            .build();

        let banner = adw::Banner::builder()
            .title("Transcripts need whisper.cpp, which is not installed")
            .revealed(false)
            .build();

        let model_row = adw::ComboRow::builder()
            .title("Model")
            .model(&string_list(Model::ALL.iter().map(|m| m.label())))
            .build();

        let model_button = gtk::Button::builder().valign(gtk::Align::Center).build();
        let model_progress = gtk::ProgressBar::builder()
            .visible(false)
            .valign(gtk::Align::Center)
            .width_request(120)
            .build();

        let model_status = adw::ActionRow::builder().title("Model file").build();
        model_status.add_suffix(&model_progress);
        model_status.add_suffix(&model_button);

        let models = adw::PreferencesGroup::builder()
            .title("Speech model")
            .description("Models are downloaded from Hugging Face and stored in your data folder.")
            .build();
        models.add(&model_row);
        models.add(&model_status);

        let transcript_format_row = adw::ComboRow::builder()
            .title("Format")
            .model(&string_list(
                transcript::Format::ALL.iter().map(|f| f.label()),
            ))
            .build();

        let mut languages = vec!["Detect automatically".to_string()];
        languages.extend(
            transcript::LANGUAGES
                .iter()
                .map(|(_, name)| name.to_string()),
        );
        let language_row = adw::ComboRow::builder()
            .title("Language")
            .model(&string_list(languages.iter().map(String::as_str)))
            .build();
        // Sixteen entries plus automatic is past the point where scrolling beats
        // typing.
        language_row.set_enable_search(true);

        let transcribe_default_row = adw::SwitchRow::builder()
            .title("Transcribe by default")
            .subtitle("Turn the switch on for each new download")
            .build();

        let output = adw::PreferencesGroup::builder().title("Output").build();
        output.add(&transcript_format_row);
        output.add(&language_row);
        output.add(&transcribe_default_row);

        page.add(&{
            // A banner belongs at the top of the content it qualifies, and an
            // AdwPreferencesPage takes groups, so it travels inside one.
            let group = adw::PreferencesGroup::new();
            group.add(&banner);
            group
        });
        page.add(&models);
        page.add(&output);

        model_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().transcript.model =
                    Model::ALL[(row.selected() as usize).min(Model::ALL.len() - 1)];
                dialog.changed();
                dialog.refresh_model_status();
            }
        ));
        transcript_format_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().transcript.format = transcript::Format::ALL
                    [(row.selected() as usize).min(transcript::Format::ALL.len() - 1)];
                dialog.changed();
            }
        ));
        language_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                let index = row.selected() as usize;
                dialog.imp().settings.borrow_mut().transcript.language = index
                    .checked_sub(1)
                    .and_then(|index| transcript::LANGUAGES.get(index))
                    .map(|(code, _)| code.to_string());
                dialog.changed();
            }
        ));
        transcribe_default_row.connect_active_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                dialog.imp().settings.borrow_mut().transcribe_by_default = row.is_active();
                dialog.changed();
            }
        ));
        model_button.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.model_button_pressed()
        ));

        let imp = self.imp();
        let _ = imp.model_row.set(model_row);
        let _ = imp.model_status.set(model_status);
        let _ = imp.model_button.set(model_button);
        let _ = imp.model_progress.set(model_progress);
        let _ = imp.transcript_format_row.set(transcript_format_row);
        let _ = imp.language_row.set(language_row);
        let _ = imp.transcribe_default_row.set(transcribe_default_row);
        let _ = imp.whisper_banner.set(banner);
        page
    }

    // -- Tools --------------------------------------------------------------

    fn build_tools(&self) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Tools")
            .name("tools")
            .icon_name("utilities-terminal-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Installed programs")
            // Short, because the Check Again button shares this line. And no
            // longer "rather than bundling them": whisper.cpp is bundled, so a
            // blanket claim would be a lie the Flatpak tells every user.
            .description("What Magpie runs, and where it found each one.")
            .build();

        let rescan = gtk::Button::with_label("Check Again");
        rescan.add_css_class("flat");
        rescan.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.emit_by_name::<()>("tools-rescan-requested", &[])
        ));
        group.set_header_suffix(Some(&rescan));

        let mut rows = Vec::new();
        for tool in [
            Tool::YtDlp,
            Tool::JsRuntime,
            Tool::Ffmpeg,
            Tool::Ffprobe,
            Tool::Whisper,
        ] {
            let row = adw::ActionRow::builder().title(tool.label()).build();

            // Copy is always available, because the command is useful whether or
            // not Magpie can run it — `sudo apt install ffmpeg` needs a terminal,
            // and retyping it from a subtitle is a chore with a typo in it.
            let copy = gtk::Button::builder()
                .icon_name("edit-copy-symbolic")
                .tooltip_text("Copy Command")
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            copy.add_css_class("flat");
            copy.connect_clicked(glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                move |button| dialog.copy_command(tool, button)
            ));

            // Only shown when there is a command Magpie can run without a
            // password. See `Installers::can_run_unprivileged`.
            let install = gtk::Button::builder()
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            install.add_css_class("suggested-action");
            install.connect_clicked(glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.run_install(tool)
            ));

            let spinner = adw::Spinner::builder()
                .valign(gtk::Align::Center)
                .visible(false)
                .build();

            let icon = gtk::Image::builder()
                .icon_name("emblem-ok-symbolic")
                .valign(gtk::Align::Center)
                .build();

            row.add_suffix(&spinner);
            row.add_suffix(&install);
            row.add_suffix(&copy);
            row.add_suffix(&icon);
            group.add(&row);
            rows.push(ToolRow {
                tool,
                row,
                icon,
                copy,
                install,
                spinner,
            });
        }
        self.imp().tool_rows.replace(rows);
        page.add(&group);
        page
    }

    // -- loading and refreshing --------------------------------------------

    fn load(&self) {
        let imp = self.imp();
        imp.loading.set(true);
        let settings = imp.settings.borrow().clone();

        select(
            imp.quality_row.get(),
            index_of(&Quality::ALL, settings.quality),
        );
        select(
            imp.audio_row.get(),
            index_of(&AudioFormat::ALL, settings.audio_format),
        );
        select(
            imp.model_row.get(),
            index_of(&Model::ALL, settings.transcript.model),
        );
        select(
            imp.transcript_format_row.get(),
            index_of(&transcript::Format::ALL, settings.transcript.format),
        );
        select(
            imp.language_row.get(),
            settings
                .transcript
                .language
                .as_deref()
                .and_then(|code| {
                    transcript::LANGUAGES
                        .iter()
                        .position(|(candidate, _)| *candidate == code)
                })
                .map(|index| index as u32 + 1)
                .unwrap_or(0),
        );
        select(
            imp.cookies_row.get(),
            settings
                .cookies_from_browser
                .as_deref()
                .and_then(|name| BROWSERS.iter().position(|candidate| *candidate == name))
                .map(|index| index as u32 + 1)
                .unwrap_or(0),
        );

        if let Some(row) = imp.audio_only_row.get() {
            row.set_active(settings.audio_only);
        }
        if let Some(row) = imp.confirm_row.get() {
            row.set_active(settings.confirm_each_download);
        }
        if let Some(row) = imp.transcribe_default_row.get() {
            row.set_active(settings.transcribe_by_default);
        }
        if let Some(row) = imp.parallel_row.get() {
            row.set_value(settings.simultaneous_downloads as f64);
        }
        if let Some(row) = imp.rate_row.get() {
            row.set_text(settings.rate_limit.as_deref().unwrap_or(""));
        }

        self.refresh_folder();
        self.refresh_model_status();
        self.refresh_tools();
        self.refresh_ffmpeg_warning();
        imp.loading.set(false);
    }

    fn refresh_folder(&self) {
        let imp = self.imp();
        let Some(row) = imp.folder_row.get() else {
            return;
        };
        let directory = imp
            .settings
            .borrow()
            .resolved_download_directory(&imp.download_fallback.borrow());

        let mut shown = directory.display().to_string();
        if let Some(home) = glib::home_dir().to_str() {
            if let Some(rest) = shown.strip_prefix(home) {
                shown = format!("~{rest}");
            }
        }
        row.set_subtitle(&shown);
    }

    fn refresh_ffmpeg_warning(&self) {
        let imp = self.imp();
        let Some(row) = imp.audio_row.get() else {
            return;
        };
        let needs = imp.settings.borrow().audio_format.needs_ffmpeg();
        let missing = !imp.report.borrow().has_ffmpeg();

        // The choice stays selectable — it will work the moment ffmpeg is
        // installed, and disabling it would leave no way to say why.
        if needs && missing {
            row.set_subtitle("Converting needs FFmpeg, which is not installed");
            row.add_css_class("warning");
        } else {
            row.set_subtitle(imp.settings.borrow().audio_format.description());
            row.remove_css_class("warning");
        }
    }

    fn refresh_tools(&self) {
        let imp = self.imp();
        let report = imp.report.borrow().clone();
        let installers = report.installers;
        let busy = imp.installing.borrow().clone();

        for entry in imp.tool_rows.borrow().iter() {
            let tool = entry.tool;
            let running = busy.contains(&tool);
            let stale = tool == Tool::YtDlp && report.freshness.is_stale();

            // While a command is running the row says so and offers nothing to
            // press, because pressing it again would start a second one.
            entry.spinner.set_visible(running);
            if running {
                entry.row.set_subtitle("Working…");
                entry.copy.set_visible(false);
                entry.install.set_visible(false);
                entry.icon.set_visible(false);
                continue;
            }
            entry.icon.set_visible(true);

            // The command this row would run, and the word for the button.
            let (command, verb) = match (report.found(tool), stale) {
                (None, _) => (Some(tool.install_command(installers)), "Install"),
                (Some(_), true) => (tool.upgrade_command(installers), "Update"),
                (Some(_), false) => (None, ""),
            };

            match report.found(tool) {
                Some(found) => {
                    let mut subtitle = found.path.display().to_string();
                    if let Some(version) = &found.version {
                        subtitle = format!("{version} · {subtitle}");
                    }
                    if let Some(advice) = report.freshness.advice().filter(|_| tool == Tool::YtDlp)
                    {
                        subtitle = format!("{subtitle}\n{advice}");
                    }
                    entry.row.set_subtitle(&subtitle);
                    Self::set_status_icon(
                        &entry.icon,
                        if stale {
                            "dialog-warning-symbolic"
                        } else {
                            "emblem-ok-symbolic"
                        },
                        if stale { "warning" } else { "success" },
                    );
                }
                None => {
                    entry.row.set_subtitle(&format!(
                        "Not installed. {}\n{}",
                        tool.purpose(),
                        tool.install_hint(installers)
                    ));
                    Self::set_status_icon(
                        &entry.icon,
                        if tool.is_required() {
                            "dialog-error-symbolic"
                        } else {
                            "dialog-information-symbolic"
                        },
                        if tool.is_required() {
                            "error"
                        } else {
                            "dimmed"
                        },
                    );
                }
            }

            // Copy whenever there is a command; run it only when it names an
            // installer Magpie found and is allowed to run. `can_run` is the same
            // rule the runner enforces, so the button cannot promise what the
            // runner would refuse.
            entry.copy.set_visible(command.is_some());
            match command.filter(|command| installers.can_run(command)) {
                Some(command) => {
                    entry.install.set_label(verb);
                    // The command in the tooltip, because a button that changes
                    // the user's environment should say exactly what it will do.
                    entry.install.set_tooltip_text(Some(&command));
                    entry.install.set_visible(true);
                }
                None => entry.install.set_visible(false),
            }
        }

        if let Some(banner) = imp.whisper_banner.get() {
            banner.set_revealed(!report.has_whisper());
        }
        self.refresh_ffmpeg_warning();
    }

    fn set_status_icon(icon: &gtk::Image, name: &str, class: &str) {
        icon.set_icon_name(Some(name));
        for existing in ["error", "warning", "success", "dimmed"] {
            icon.remove_css_class(existing);
        }
        icon.add_css_class(class);
    }

    /// The command for this tool, on the clipboard.
    fn copy_command(&self, tool: Tool, button: &gtk::Button) {
        let imp = self.imp();
        let report = imp.report.borrow();
        let installers = report.installers;
        let stale = tool == Tool::YtDlp && report.freshness.is_stale();

        let command = match (report.found(tool), stale) {
            (Some(_), true) => tool.upgrade_command(installers),
            (None, _) => Some(tool.install_command(installers)),
            _ => None,
        };
        drop(report);

        if let Some(command) = command {
            button.clipboard().set_text(&command);
            self.add_toast(adw::Toast::new(&format!("Copied “{command}”")));
        }
    }

    /// Run the command this row is offering.
    ///
    /// The user has seen it — it is the row's subtitle and the button's tooltip —
    /// and has pressed a button labelled Install or Update, so this does not ask
    /// again. What it does do is show the output on failure, because "it did not
    /// work" with no reason is worse than the terminal they were avoiding.
    fn run_install(&self, tool: Tool) {
        let imp = self.imp();
        if imp.installing.borrow().contains(&tool) {
            return;
        }

        let report = imp.report.borrow().clone();
        let stale = tool == Tool::YtDlp && report.freshness.is_stale();
        let command = match (report.found(tool), stale) {
            (Some(_), true) => tool.upgrade_command(report.installers),
            (None, _) => Some(tool.install_command(report.installers)),
            _ => None,
        };
        let Some(command) = command else { return };

        imp.installing.borrow_mut().push(tool);
        self.refresh_tools();

        let reported = command.clone();
        let started = toolbox::run_installer(
            &report,
            &command,
            |_| {},
            glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                move |result| {
                    dialog.imp().installing.borrow_mut().retain(|t| *t != tool);
                    match result {
                        Ok(()) => {
                            dialog
                                .add_toast(adw::Toast::new(&format!("{} is ready", tool.label())));
                            // Re-survey rather than assume: the point is to show
                            // the path and version of what just landed.
                            dialog.emit_by_name::<()>("tools-rescan-requested", &[]);
                        }
                        Err(reason) => {
                            dialog.refresh_tools();
                            dialog.show_install_failure(tool, &reported, &reason);
                        }
                    }
                }
            ),
        );

        if let Err(reason) = started {
            imp.installing.borrow_mut().retain(|t| *t != tool);
            self.refresh_tools();
            self.show_install_failure(tool, &command, &reason);
        }
    }

    fn show_install_failure(&self, tool: Tool, command: &str, reason: &str) {
        if reason == "cancelled" {
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Could not install {}", tool.label()))
            .body(format!(
                "Magpie ran “{command}” and it did not succeed. \
                 Running it in a terminal will show the whole story."
            ))
            .build();

        let text = gtk::Label::builder()
            .label(reason.trim())
            .wrap(true)
            .selectable(true)
            .xalign(0.0)
            .build();
        text.add_css_class("monospace");
        text.add_css_class("caption");
        let expander = adw::ExpanderRow::builder()
            .title("What it reported")
            .build();
        let row = adw::ActionRow::new();
        row.set_child(Some(&text));
        expander.add_row(&row);
        let group = adw::PreferencesGroup::new();
        group.add(&expander);
        dialog.set_extra_child(Some(&group));

        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.present(Some(self));
    }

    fn refresh_model_status(&self) {
        let imp = self.imp();
        let (Some(row), Some(button), Some(progress)) = (
            imp.model_status.get(),
            imp.model_button.get(),
            imp.model_progress.get(),
        ) else {
            return;
        };
        if imp.model_download.borrow().is_some() {
            return;
        }

        let model = imp.settings.borrow().transcript.model;
        let models_dir = imp.models_dir.borrow().clone();
        progress.set_visible(false);

        match toolbox::model_on_disk(&models_dir, model) {
            Some(size) => {
                row.set_subtitle(&format!("{} · {}", model.label(), format_bytes(size)));
                button.set_label("Remove");
                button.remove_css_class("suggested-action");
                button.add_css_class("destructive-action");
            }
            None => {
                row.set_subtitle(&format!("Not downloaded · {}", model.description()));
                button.set_label("Download");
                button.remove_css_class("destructive-action");
                button.add_css_class("suggested-action");
            }
        }
    }

    fn choose_folder(&self) {
        let imp = self.imp();
        let current = imp
            .settings
            .borrow()
            .resolved_download_directory(&imp.download_fallback.borrow());

        let dialog = gtk::FileDialog::builder()
            .title("Choose Download Folder")
            .modal(true)
            .initial_folder(&gtk::gio::File::for_path(&current))
            .build();

        dialog.select_folder(
            self.root().and_downcast::<gtk::Window>().as_ref(),
            gtk::gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = preferences)]
                self,
                move |result| {
                    if let Some(path) = result.ok().and_then(|file| file.path()) {
                        preferences.imp().settings.borrow_mut().download_directory = Some(path);
                        preferences.refresh_folder();
                        preferences.changed();
                    }
                }
            ),
        );
    }

    fn model_button_pressed(&self) {
        let imp = self.imp();
        let model = imp.settings.borrow().transcript.model;
        let models_dir = imp.models_dir.borrow().clone();

        // Already downloading: this press is a cancel.
        if let Some(download) = imp.model_download.borrow_mut().take() {
            download.cancel();
            self.refresh_model_status();
            return;
        }

        if toolbox::model_on_disk(&models_dir, model).is_some() {
            self.confirm_model_removal(model, models_dir);
            return;
        }

        let Some(progress) = imp.model_progress.get() else {
            return;
        };
        let Some(button) = imp.model_button.get() else {
            return;
        };
        progress.set_fraction(0.0);
        progress.set_visible(true);
        button.set_label("Cancel");
        button.remove_css_class("suggested-action");
        button.remove_css_class("destructive-action");
        if let Some(row) = imp.model_status.get() {
            row.set_subtitle(&format!(
                "Downloading · about {}",
                format_bytes(model.bytes())
            ));
        }

        let dialog = self.downgrade();
        let download = toolbox::download_model(
            &models_dir,
            model,
            {
                let dialog = dialog.clone();
                move |fraction| {
                    if let Some(dialog) = dialog.upgrade() {
                        if let Some(progress) = dialog.imp().model_progress.get() {
                            progress.set_fraction(fraction);
                        }
                    }
                }
            },
            move |result| {
                let Some(dialog) = dialog.upgrade() else {
                    return;
                };
                dialog.imp().model_download.replace(None);
                if let Err(error) = &result {
                    if error != "cancelled" {
                        if let Some(row) = dialog.imp().model_status.get() {
                            row.set_subtitle(&format!("Download failed · {error}"));
                        }
                    }
                }
                dialog.refresh_model_status();
                dialog.changed();
            },
        );
        imp.model_download.replace(Some(download));
    }

    fn confirm_model_removal(&self, model: Model, models_dir: PathBuf) {
        let alert = adw::AlertDialog::builder()
            .heading(format!("Remove the {} model?", model.label()))
            .body(format!(
                "The file will be deleted. Downloading it again takes about {}.",
                format_bytes(model.bytes())
            ))
            .build();
        alert.add_response("cancel", "Cancel");
        alert.add_response("remove", "Remove");
        alert.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");

        let removed = Rc::new(Cell::new(false));
        alert.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                #[strong]
                removed,
                move |_, response| {
                    if response != "remove" || removed.replace(true) {
                        return;
                    }
                    let _ = std::fs::remove_file(model.path_in(&models_dir));
                    dialog.refresh_model_status();
                    dialog.changed();
                }
            ),
        );
        alert.present(Some(self));
    }
}

fn index_of<T: PartialEq>(all: &[T], value: T) -> u32 {
    all.iter()
        .position(|candidate| *candidate == value)
        .unwrap_or(0) as u32
}

fn select(row: Option<&adw::ComboRow>, index: u32) {
    if let Some(row) = row {
        row.set_selected(index);
    }
}

fn string_list<'a, I: IntoIterator<Item = &'a str>>(items: I) -> gtk::StringList {
    let list = gtk::StringList::new(&[]);
    for item in items {
        list.append(item);
    }
    list
}
