//! `MagpieApplication`: the only object that owns state or runs anything.
//!
//! Every widget in `ui/` emits intent and waits. This file is where intent
//! becomes a subprocess, a state change, and a line in `library.json`. Having one
//! such place is what keeps the rest of the tree free of `RefCell`s pointing at
//! each other, and it is what makes the queue's behaviour a property of one file
//! rather than of the order in which handlers happen to fire.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::load_stylesheet;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;

use crate::model::diarize;
use crate::model::failure::{self, Failure};
use crate::model::job::{Job, Progress, State, TranscriptState};
use crate::model::library::Library;
use crate::model::media::{self, Info};
use crate::model::progress::{parse_line, Event};
use crate::model::queue::Queue;
use crate::model::request;
use crate::model::settings::Settings;
use crate::model::speakers;
use crate::model::store::Outcome;
use crate::model::transcript;
use crate::APP_ID;

use super::add_dialog::{AddDialog, Choice};
use super::preferences::Preferences;
use super::process::{self, Handle, Stream};
use super::thumbnail;
use super::toolbox::{self, Report};
use super::window::MagpieWindow;

/// How often the list is redrawn while something is downloading.
///
/// yt-dlp emits a progress line several times a second per fragment. Rebuilding
/// rows that often is wasted work and makes the speed figure flicker; a quarter
/// of a second is faster than anyone reads and slow enough to be free.
const REDRAW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MagpieApplication {
        pub settings: RefCell<Settings>,
        pub queue: RefCell<Queue>,
        pub library: RefCell<Library>,
        pub report: RefCell<Report>,
        /// Subprocesses, by job id. A job with no handle is not running,
        /// whatever its state says.
        pub handles: RefCell<HashMap<u64, Handle>>,
        pub progress: RefCell<HashMap<u64, Progress>>,
        pub thumbnails: OnceCell<thumbnail::Cache>,
        pub window: RefCell<Option<MagpieWindow>>,
        pub preferences: RefCell<Option<Preferences>>,
        /// Set when something changed and the list should be redrawn on the next
        /// tick.
        pub dirty: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MagpieApplication {
        const NAME: &'static str = "MagpieApplication";
        type Type = super::MagpieApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MagpieApplication {}

    impl ApplicationImpl for MagpieApplication {
        fn startup(&self) {
            // Chain up first: the toolkit initialises in the parent handler, and
            // anything touching GTK before it is undefined.
            self.parent_startup();
            let app = self.obj();

            if let Some(display) = gtk::gdk::Display::default() {
                load_stylesheet(&display);
            }
            app.install_actions();
            app.load_state();
            app.start_redraw_tick();
            app.survey_tools();
        }

        fn activate(&self) {
            let app = self.obj();
            let window = app.window();
            app.refresh();
            window.present();
        }

        fn shutdown(&self) {
            let app = self.obj();
            // A clean exit stops the children here. An unclean one — SIGTERM on
            // logout, or a crash — never reaches this handler, which is why
            // `process::spawn` also asks the kernel to signal them when this
            // process dies.
            for handle in app.imp().handles.borrow().values() {
                handle.cancel();
            }
            app.remember_geometry();
            app.persist();
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for MagpieApplication {}
    impl AdwApplicationImpl for MagpieApplication {}
}

glib::wrapper! {
    pub struct MagpieApplication(ObjectSubclass<imp::MagpieApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for MagpieApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl MagpieApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::default())
            .build()
    }

    // -- directories --------------------------------------------------------

    fn config_dir(&self) -> PathBuf {
        glib::user_config_dir().join("magpie")
    }

    fn data_dir(&self) -> PathBuf {
        glib::user_data_dir().join("magpie")
    }

    fn cache_dir(&self) -> PathBuf {
        glib::user_cache_dir().join("magpie")
    }

    /// The XDG download directory, or the home directory if the user has no
    /// such folder. Never a directory Magpie invents.
    fn download_fallback(&self) -> PathBuf {
        glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir)
    }

    fn destination(&self) -> PathBuf {
        self.imp()
            .settings
            .borrow()
            .resolved_download_directory(&self.download_fallback())
    }

    // -- state --------------------------------------------------------------

    fn load_state(&self) {
        let imp = self.imp();
        let _ = std::fs::create_dir_all(self.cache_dir());

        let (settings, settings_outcome) =
            crate::model::store::load::<Settings>(&Settings::path_in(&self.config_dir()))
                .unwrap_or_else(|_| (Settings::default(), Outcome::Fresh));
        let settings = settings.sanitised();

        let (library, library_outcome) = Library::load(&Library::path_in(&self.data_dir()))
            .unwrap_or_else(|_| (Library::default(), Outcome::Fresh));

        imp.queue.replace(Queue::restore(
            library.jobs.clone(),
            settings.simultaneous_downloads,
        ));
        imp.settings.replace(settings);
        imp.library.replace(library);
        let _ = imp.thumbnails.set(thumbnail::Cache::new(&self.cache_dir()));

        // A recovered file is worth saying out loud once, because the user's
        // queue or preferences just came back empty and they deserve to know it
        // was not their imagination.
        for (what, outcome) in [
            ("preferences", settings_outcome),
            ("download list", library_outcome),
        ] {
            if let Outcome::Recovered { backup } = outcome {
                let message = format!(
                    "Your {what} file could not be read and was moved to {}",
                    backup.display()
                );
                glib::idle_add_local_once(glib::clone!(
                    #[weak(rename_to = app)]
                    self,
                    move || app.window().toast(&message)
                ));
            }
        }
    }

    fn persist(&self) {
        let imp = self.imp();
        let jobs = imp.queue.borrow().jobs().to_vec();
        {
            let mut library = imp.library.borrow_mut();
            library.replace(&jobs);
        }
        let library = imp.library.borrow().clone();
        if let Err(error) = library.save(&Library::path_in(&self.data_dir())) {
            eprintln!("magpie: {error}");
        }

        let settings = imp.settings.borrow().clone();
        if let Err(error) =
            crate::model::store::save(&Settings::path_in(&self.config_dir()), &settings)
        {
            eprintln!("magpie: {error}");
        }
    }

    /// Record the window's size, if it still has one to report.
    ///
    /// `width()` and `height()` return 0 once the window is unmapped, and
    /// `shutdown` runs late enough that they often do. Writing those zeroes over a
    /// good stored size is how the window came back at its minimum size on the
    /// next launch, so a non-positive reading is discarded rather than saved.
    fn remember_geometry(&self) {
        let Some(window) = self.imp().window.borrow().clone() else {
            return;
        };
        let (width, height) = (window.width(), window.height());
        let mut settings = self.imp().settings.borrow_mut();
        settings.window_maximized = window.is_maximized();
        if !window.is_maximized() && width > 0 && height > 0 {
            settings.window_width = width;
            settings.window_height = height;
        }
    }

    // -- window -------------------------------------------------------------

    fn window(&self) -> MagpieWindow {
        if let Some(window) = self.imp().window.borrow().clone() {
            return window;
        }

        let window = MagpieWindow::new(self);
        let settings = self.imp().settings.borrow().clone();
        // Size restored before `present`, so the window never appears at the
        // default size and then jumps.
        window.set_default_size(settings.window_width, settings.window_height);
        if settings.window_maximized {
            window.maximize();
        }

        window.connect_closure(
            "link-submitted",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |_: MagpieWindow, url: &str| app.link_submitted(url)
            ),
        );
        window.connect_closure(
            "job-action",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |_: MagpieWindow, id: u64, action: &str| app.job_action(id, action)
            ),
        );
        window.connect_closure(
            "banner-activated",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |_: MagpieWindow| app.show_preferences(Some("tools"))
            ),
        );
        window.connect_close_request(glib::clone!(
            #[weak(rename_to = app)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_| {
                app.remember_geometry();
                app.persist();
                glib::Propagation::Proceed
            }
        ));

        self.imp().window.replace(Some(window.clone()));
        window
    }

    fn install_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| {
                // While the window still exists. Ctrl+Q does not fire
                // `close-request`, so without this the only reading came from
                // `shutdown`, by which time the window is unmapped and reports 0.
                app.remember_geometry();
                app.quit();
            })
            .build();
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| app.show_preferences(None))
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        // Reachable from a toast button, which needs an action rather than a
        // callback.
        let undo_remove = gio::ActionEntry::builder("undo-remove")
            .activate(|app: &Self, _, _| app.undo_remove())
            .build();
        self.add_action_entries([quit, preferences, about, undo_remove]);

        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("app.preferences", &["<Control>comma"]);
        self.set_accels_for_action("win.new-download", &["<Control>n"]);
        self.set_accels_for_action("win.shortcuts", &["<Control>question"]);
        self.set_accels_for_action("win.open-folder", &["<Control><Shift>o"]);
    }

    /// Redraw the list on a timer rather than on every progress line.
    fn start_redraw_tick(&self) {
        glib::timeout_add_local(
            REDRAW_INTERVAL,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    if app.imp().dirty.replace(false) {
                        app.refresh();
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    fn touch(&self) {
        self.imp().dirty.set(true);
    }

    /// Redraw everything the window shows.
    fn refresh(&self) {
        let imp = self.imp();
        let Some(window) = imp.window.borrow().clone() else {
            return;
        };

        // Copied out so no borrow is live while the window rebuilds rows and
        // fires signals back into this object.
        let jobs = imp.queue.borrow().jobs().to_vec();
        let summary = imp.queue.borrow().summary();
        let progress = imp.progress.borrow().clone();
        let banner = imp.report.borrow().banner();

        window.set_jobs(&jobs, &|id| progress.get(&id).cloned());
        window.set_summary(summary.as_deref());
        window.set_banner(
            banner
                .as_ref()
                .map(|(message, button)| (message.as_str(), *button)),
        );

        for job in &jobs {
            self.load_poster(job);
        }
    }

    fn load_poster(&self, job: &Job) {
        let Some(url) = job.thumbnail.clone().or_else(|| {
            // A guess from the URL, so a row has a picture before `--dump-json`
            // has been anywhere near it.
            crate::model::url::guessed_thumbnail(&job.url)
        }) else {
            return;
        };
        let Some(cache) = self.imp().thumbnails.get() else {
            return;
        };
        let id = job.id;
        cache.load(
            &url,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |texture| {
                    if let Some(window) = app.imp().window.borrow().clone() {
                        window.set_poster(id, &texture);
                    }
                }
            ),
        );
    }

    // -- tools --------------------------------------------------------------

    fn survey_tools(&self) {
        let override_path = self.imp().settings.borrow().ytdlp_path.clone();
        toolbox::survey(
            override_path,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |report| {
                    app.imp().report.replace(report.clone());
                    if let Some(preferences) = app.imp().preferences.borrow().clone() {
                        preferences.set_report(&report);
                    }
                    app.refresh();
                    // A tool may have appeared while a job sat waiting for it.
                    app.pump();
                }
            ),
        );
    }

    fn ytdlp(&self) -> Option<PathBuf> {
        self.imp().report.borrow().ytdlp_path().cloned()
    }

    // -- adding -------------------------------------------------------------

    fn link_submitted(&self, url: &str) {
        let Some(ytdlp) = self.ytdlp() else {
            self.window()
                .toast("yt-dlp is not installed. See Preferences for how to install it.");
            self.window().set_link_text(url);
            return;
        };

        if !self.imp().settings.borrow().confirm_each_download {
            // The fast path: no dialog, defaults straight from preferences. The
            // title comes back from `--dump-json` and updates the row when it
            // arrives.
            self.enqueue_directly(url);
            return;
        }

        let dialog = AddDialog::new(
            url,
            &self.imp().settings.borrow(),
            self.destination(),
            self.imp().report.borrow().has_whisper(),
            self.imp().report.borrow().has_diarizer(),
        );
        dialog.connect_closure(
            "confirmed",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |dialog: AddDialog| {
                    if let Some(choice) = dialog.choice() {
                        app.enqueue(choice);
                    }
                }
            ),
        );
        dialog.connect_closure(
            "retry-requested",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |dialog: AddDialog| app.fetch_info(&dialog)
            ),
        );

        dialog.present(Some(&self.window()));
        self.fetch_info(&dialog);
        let _ = ytdlp;
    }

    fn fetch_info(&self, dialog: &AddDialog) {
        let Some(ytdlp) = self.ytdlp() else {
            dialog.show_failure(&Failure::ToolMissing);
            return;
        };
        let url = dialog.url();
        let collection = crate::model::url::is_collection(&url);
        let args = request::info_argv(&url, collection);

        let cache = self.imp().thumbnails.get().cloned();

        glib::spawn_future_local(glib::clone!(
            #[weak]
            dialog,
            async move {
                let capture = match process::capture(&ytdlp, &args).await {
                    Ok(capture) => capture,
                    Err(error) => {
                        dialog.show_failure(&Failure::Unknown(error.to_string()));
                        return;
                    }
                };

                match media::parse(&capture.stdout) {
                    Ok(info) => {
                        if let (Some(cache), Some(url)) = (cache, poster_url(&info)) {
                            cache.load(
                                &url,
                                glib::clone!(
                                    #[weak]
                                    dialog,
                                    move |texture| dialog.set_poster(Some(&texture))
                                ),
                            );
                        }
                        dialog.show_media(info);
                    }
                    // Not JSON almost always means yt-dlp refused; its stderr
                    // says why, and `classify` turns that into something
                    // actionable.
                    Err(_) => dialog.show_failure(&failure::classify(&capture.stderr)),
                }
            }
        ));
    }

    fn enqueue(&self, choice: Choice) {
        let imp = self.imp();
        let id = imp.queue.borrow_mut().reserve_id();

        let mut job = Job::new(id, choice.url, choice.title, choice.destination);
        job.thumbnail = choice.thumbnail;
        job.selection = choice.selection;
        job.collection = choice.collection;
        job.transcribe = choice.transcribe;
        if job.transcribe.is_some() {
            job.transcript_state = TranscriptState::Waiting;
        }

        imp.queue.borrow_mut().add(job);
        self.refresh();
        self.persist();
        self.pump();
    }

    /// Add without asking, for when "Ask before each download" is off.
    ///
    /// The title is the URL until `--dump-json` answers, because a row that says
    /// nothing is worse than a row that says the link.
    fn enqueue_directly(&self, url: &str) {
        let imp = self.imp();
        let settings = imp.settings.borrow().clone();
        let id = imp.queue.borrow_mut().reserve_id();

        let mut job = Job::new(id, url.to_string(), url.to_string(), self.destination());
        job.selection = if settings.audio_only {
            request::Selection::Audio(settings.audio_format)
        } else {
            request::Selection::Video(settings.quality)
        };
        if settings.transcribe_by_default && imp.report.borrow().has_whisper() {
            job.transcribe = Some(settings.transcript.clone());
            job.transcript_state = TranscriptState::Waiting;
        }

        imp.queue.borrow_mut().add(job);
        self.refresh();
        self.pump();
        self.name_job_later(id, url.to_string());
    }

    /// Fill in a job's real title and playlist folder once yt-dlp answers.
    fn name_job_later(&self, id: u64, url: String) {
        let Some(ytdlp) = self.ytdlp() else { return };
        let collection = crate::model::url::is_collection(&url);
        let args = request::info_argv(&url, collection);

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = app)]
            self,
            async move {
                let Ok(capture) = process::capture(&ytdlp, &args).await else {
                    return;
                };
                let Ok(info) = media::parse(&capture.stdout) else {
                    return;
                };

                let mut queue = app.imp().queue.borrow_mut();
                let Some(job) = queue.get_mut(id) else { return };
                match &info {
                    Info::Single(item) => {
                        job.title = item.title.clone();
                        job.thumbnail = item.thumbnail.clone();
                    }
                    Info::Collection(playlist) => {
                        job.title = playlist.title.clone();
                        job.collection = Some(request::Collection {
                            folder: request::folder_name(&playlist.title),
                            items: Vec::new(),
                        });
                    }
                }
                drop(queue);
                app.refresh();
            }
        ));
    }

    // -- running ------------------------------------------------------------

    /// Start whatever the queue says should be running.
    ///
    /// Called after every state change. Because `Queue::ready` is computed from
    /// the current state rather than pushed by whoever finished, there is no
    /// outcome that can forget to advance the queue.
    fn pump(&self) {
        let Some(ytdlp) = self.ytdlp() else { return };
        let ready = self.imp().queue.borrow().ready();
        for id in ready {
            self.start(id, &ytdlp);
        }
    }

    fn start(&self, id: u64, ytdlp: &Path) {
        let imp = self.imp();
        let settings = imp.settings.borrow().clone();
        let cache_dir = self.cache_dir();

        let js_runtime = imp
            .report
            .borrow()
            .js_runtime
            .as_ref()
            .map(|found| found.path.clone());

        let Some(request) = imp.queue.borrow().get(id).map(|job| {
            job.request(
                settings.cookies(),
                settings.rate_limit.clone(),
                js_runtime,
                &cache_dir,
            )
        }) else {
            return;
        };

        // A stale sink from a previous attempt would be read as this attempt's
        // output.
        let sink = request.filepath_sink.clone();
        let _ = std::fs::remove_file(&sink);

        let handle = process::run(
            ytdlp,
            &request.argv(),
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |stream, line| {
                    if stream == Stream::Stdout {
                        app.download_line(id, line);
                    }
                }
            ),
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |outcome| app.download_finished(id, outcome, &sink)
            ),
        );

        match handle {
            Ok(handle) => {
                imp.handles.borrow_mut().insert(id, handle);
                imp.progress.borrow_mut().insert(id, Progress::default());
                if let Some(job) = imp.queue.borrow_mut().get_mut(id) {
                    job.state = State::Running;
                }
                self.refresh();
            }
            // Launching failed, which on this path means the binary vanished
            // between the survey and now.
            Err(_) => self.finish(id, State::Failed(Failure::ToolMissing)),
        }
    }

    fn download_line(&self, id: u64, line: &str) {
        let mut progress = self.imp().progress.borrow_mut();
        let Some(entry) = progress.get_mut(&id) else {
            return;
        };
        match parse_line(line) {
            Event::Progress(snapshot) => entry.observe(snapshot),
            Event::Postprocessing { status, processor } => {
                entry.postprocessing = (status != "finished").then_some(processor);
            }
            Event::Chatter(_) => return,
        }
        drop(progress);
        self.touch();
    }

    fn download_finished(&self, id: u64, outcome: process::Outcome, sink: &Path) {
        self.imp().handles.borrow_mut().remove(&id);

        let state = match outcome {
            process::Outcome::Success => {
                let outputs = read_sink(sink);
                if let Some(job) = self.imp().queue.borrow_mut().get_mut(id) {
                    job.outputs = outputs;
                }
                State::Done
            }
            process::Outcome::Failed { stderr } => State::Failed(failure::classify(&stderr)),
            // A cancelled job leaves the list entirely: it was cancelled, so
            // there is nothing to report and nothing to retry.
            process::Outcome::Cancelled => {
                self.imp().queue.borrow_mut().remove(id);
                self.imp().progress.borrow_mut().remove(&id);
                let _ = std::fs::remove_file(sink);
                self.after_change();
                return;
            }
        };
        let _ = std::fs::remove_file(sink);
        self.finish(id, state);
    }

    fn finish(&self, id: u64, state: State) {
        {
            let mut queue = self.imp().queue.borrow_mut();
            if let Some(job) = queue.get_mut(id) {
                job.state = state;
            }
        }
        self.imp().progress.borrow_mut().remove(&id);

        let job = self.imp().queue.borrow().get(id).cloned();
        if let Some(job) = job {
            if job.state == State::Done {
                self.window().toast(&format!("Saved {}", job.title));
            }
            if job.wants_transcript_now() {
                self.start_transcript(id);
            }
        }
        self.after_change();
    }

    /// Redraw, save, and start anything that can now start.
    fn after_change(&self) {
        self.refresh();
        self.persist();
        self.pump();
    }

    // -- transcripts --------------------------------------------------------

    fn start_transcript(&self, id: u64) {
        let imp = self.imp();
        let Some(job) = imp.queue.borrow().get(id).cloned() else {
            return;
        };
        let Some(media_path) = job.single_output().cloned() else {
            return;
        };
        let Some(wish) = job.transcribe.clone() else {
            return;
        };

        let Some(whisper) = imp.report.borrow().whisper.clone().map(|found| found.path) else {
            self.fail_transcript(id, "whisper.cpp is not installed");
            return;
        };
        let models_dir = toolbox::models_directory(&self.data_dir());
        if toolbox::model_on_disk(&models_dir, wish.model).is_none() {
            self.fail_transcript(
                id,
                "the speech model has not been downloaded — see Preferences",
            );
            return;
        }
        let model_path = wish.model.path_in(&models_dir);

        if transcript::needs_conversion_for(&media_path, &wish) {
            let Some(ffmpeg) = imp.report.borrow().ffmpeg.clone().map(|found| found.path) else {
                self.fail_transcript(id, "FFmpeg is needed to prepare the audio");
                return;
            };
            let wav = transcript::conversion_path(&self.cache_dir(), id);
            self.set_transcript_state(id, TranscriptState::Converting);

            let args = transcript::conversion_argv(&media_path, &wav);
            let handle = process::run(
                &ffmpeg,
                &args,
                |_, _| {},
                glib::clone!(
                    #[weak(rename_to = app)]
                    self,
                    move |outcome| match outcome {
                        process::Outcome::Success => {
                            app.run_whisper(
                                id,
                                &whisper,
                                &model_path,
                                &wav,
                                &media_path,
                                &wish,
                                true,
                            )
                        }
                        process::Outcome::Cancelled => {}
                        process::Outcome::Failed { .. } => {
                            app.fail_transcript(id, "the audio could not be prepared")
                        }
                    }
                ),
            );
            if let Ok(handle) = handle {
                imp.handles.borrow_mut().insert(id, handle);
            } else {
                self.fail_transcript(id, "FFmpeg could not be started");
            }
        } else {
            self.run_whisper(
                id,
                &whisper,
                &model_path,
                &media_path.clone(),
                &media_path,
                &wish,
                false,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_whisper(
        &self,
        id: u64,
        whisper: &Path,
        model: &Path,
        audio: &Path,
        media_path: &Path,
        wish: &transcript::Wish,
        scratch: bool,
    ) {
        let stem = media_path.with_extension("");
        let output = transcript::output_path(media_path, wish.format);
        let args = transcript::argv(model, audio, &stem, wish);
        let scratch_path = scratch.then(|| audio.to_path_buf());
        // Both needed in the completion handler, which outlives this call.
        let wish_after = wish.clone();
        let media_after = media_path.to_path_buf();
        let audio_after = audio.to_path_buf();

        self.set_transcript_state(id, TranscriptState::Running);

        let handle = process::run(
            whisper,
            &args,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |_, line| {
                    // whisper writes its progress to stderr, so both pipes are
                    // fed through the same parser rather than guessing which.
                    if let Some(fraction) = transcript::parse_progress(line) {
                        let mut progress = app.imp().progress.borrow_mut();
                        progress.entry(id).or_default().transcript_fraction = Some(fraction);
                        drop(progress);
                        app.touch();
                    }
                }
            ),
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |outcome| {
                    app.imp().handles.borrow_mut().remove(&id);

                    // The words exist and someone asked who said them. The
                    // scratch wav is the diarizer's input too, so it is left in
                    // place and that stage disposes of it.
                    if matches!(outcome, process::Outcome::Success)
                        && output.exists()
                        && wish_after.identifies_speakers()
                    {
                        app.identify_speakers(
                            id,
                            &media_after,
                            &audio_after,
                            &output,
                            &wish_after,
                            scratch_path.clone(),
                        );
                        return;
                    }

                    if let Some(scratch) = &scratch_path {
                        let _ = std::fs::remove_file(scratch);
                    }
                    app.imp().progress.borrow_mut().remove(&id);

                    match outcome {
                        process::Outcome::Success if output.exists() => {
                            app.set_transcript_state(id, TranscriptState::Done(output.clone()));
                            app.persist();
                        }
                        // Exited zero without writing the file: whisper does
                        // this when the audio is silent or the model failed to
                        // load, and reporting success would leave a row claiming
                        // a transcript that is not there.
                        process::Outcome::Success => {
                            app.fail_transcript(id, "whisper wrote no transcript")
                        }
                        process::Outcome::Cancelled => {
                            app.set_transcript_state(id, TranscriptState::None)
                        }
                        process::Outcome::Failed { .. } => {
                            app.fail_transcript(id, "whisper could not transcribe this audio")
                        }
                    }
                }
            ),
        );

        match handle {
            Ok(handle) => {
                self.imp().handles.borrow_mut().insert(id, handle);
                self.imp().progress.borrow_mut().entry(id).or_default();
            }
            Err(_) => self.fail_transcript(id, "whisper.cpp could not be started"),
        }
    }

    /// Work out who is speaking, now that there is a transcript to mark up.
    ///
    /// Every failure here lands on [`Self::finish_without_speakers`] rather than
    /// on `fail_transcript`. The transcript is already written and already good;
    /// losing it because the *second* tool could not load a model would be
    /// throwing away the thing that took ten minutes to make over the thing that
    /// takes ten seconds.
    fn identify_speakers(
        &self,
        id: u64,
        media_path: &Path,
        audio: &Path,
        output: &Path,
        wish: &transcript::Wish,
        scratch: Option<PathBuf>,
    ) {
        let imp = self.imp();
        let Some(diarize_wish) = wish.diarize else {
            self.finish_without_speakers(id, output, scratch, None);
            return;
        };
        let Some(diarizer) = imp.report.borrow().diarizer.clone().map(|found| found.path) else {
            self.finish_without_speakers(id, output, scratch, Some("sherpa-onnx is not installed"));
            return;
        };

        let models_dir = toolbox::models_directory(&self.data_dir());
        if !toolbox::diarize_models_on_disk(&models_dir) {
            self.finish_without_speakers(
                id,
                output,
                scratch,
                Some("the speaker models have not been downloaded — see Preferences"),
            );
            return;
        }

        // The file whisper was asked for the timings in. For a subtitle
        // transcript that is the user's own output, which is then rewritten in
        // place with the names on it.
        let timing_path = transcript::output_path(media_path, wish.timing_format());
        let timing_scratch = wish.timing_file_is_scratch().then(|| timing_path.clone());

        let args = diarize::argv(
            &diarize::Asset::Segmentation.path_in(&models_dir),
            &diarize::Asset::Embedding.path_in(&models_dir),
            audio,
            &diarize_wish,
        );

        self.set_transcript_state(id, TranscriptState::Identifying);
        imp.progress
            .borrow_mut()
            .entry(id)
            .or_default()
            .transcript_fraction = Some(0.0);

        // Turns arrive one line at a time on stdout and are collected here
        // rather than re-read from anywhere: this is the only copy.
        let turns: Rc<RefCell<Vec<diarize::Turn>>> = Rc::new(RefCell::new(Vec::new()));
        let collected = turns.clone();

        let format = wish.format;
        // One copy for the completion handler and one for the failure path here,
        // which runs only when the process never started.
        let failed_to_start = output.to_path_buf();
        let output = output.to_path_buf();

        let handle = process::run(
            &diarizer,
            &args,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |_, line| {
                    if let Some(turn) = diarize::parse_turn(line) {
                        collected.borrow_mut().push(turn);
                        return;
                    }
                    if let Some(fraction) = diarize::parse_progress(line) {
                        let mut progress = app.imp().progress.borrow_mut();
                        progress.entry(id).or_default().transcript_fraction = Some(fraction);
                        drop(progress);
                        app.touch();
                    }
                }
            ),
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |outcome| {
                    if let Some(scratch) = &scratch {
                        let _ = std::fs::remove_file(scratch);
                    }
                    app.imp().handles.borrow_mut().remove(&id);
                    app.imp().progress.borrow_mut().remove(&id);

                    let turns = turns.borrow();
                    match outcome {
                        process::Outcome::Success if !turns.is_empty() => {
                            let summary = app.write_speakers(&timing_path, &output, &turns, format);
                            // The timing file only gets deleted once the labelled
                            // output is safely written, and never when it *is*
                            // the output.
                            if let (Some(scratch), true) = (&timing_scratch, summary.is_some()) {
                                if scratch != &output {
                                    let _ = std::fs::remove_file(scratch);
                                }
                            }
                            match summary {
                                Some(summary) => {
                                    if let Some(job) = app.imp().queue.borrow_mut().get_mut(id) {
                                        job.speakers = Some(summary);
                                    }
                                    app.set_transcript_state(
                                        id,
                                        TranscriptState::Done(output.clone()),
                                    );
                                    app.persist();
                                }
                                None => app.finish_without_speakers(
                                    id,
                                    &output,
                                    None,
                                    Some("the transcript could not be marked up"),
                                ),
                            }
                        }
                        // Exited zero having found nothing. Silence, or audio the
                        // segmentation model heard no speech in.
                        process::Outcome::Success => app.finish_without_speakers(
                            id,
                            &output,
                            None,
                            Some("no speech was found to attribute"),
                        ),
                        process::Outcome::Cancelled => {
                            app.set_transcript_state(id, TranscriptState::None)
                        }
                        process::Outcome::Failed { .. } => app.finish_without_speakers(
                            id,
                            &output,
                            None,
                            Some("the speakers could not be identified"),
                        ),
                    }
                }
            ),
        );

        match handle {
            Ok(handle) => {
                self.imp().handles.borrow_mut().insert(id, handle);
            }
            Err(_) => self.finish_without_speakers(
                id,
                &failed_to_start,
                None,
                Some("sherpa-onnx could not be started"),
            ),
        }
    }

    /// Read the timings, attach the voices, and write the transcript back out.
    ///
    /// Returns the sentence describing what was found, or `None` if there was
    /// nothing to work with — in which case the plain transcript stays as it is.
    fn write_speakers(
        &self,
        timing_path: &Path,
        output: &Path,
        turns: &[diarize::Turn],
        format: transcript::Format,
    ) -> Option<String> {
        let source = std::fs::read_to_string(timing_path).ok()?;
        let cues = speakers::parse_cues(&source);
        if cues.is_empty() {
            return None;
        }

        let lines = speakers::align(cues, turns);
        let cast = speakers::cast(&lines);
        if cast.is_empty() {
            return None;
        }

        let rendered = speakers::render(&lines, &cast, format);
        std::fs::write(output, rendered).ok()?;
        Some(speakers::summary(&cast))
    }

    /// Keep the transcript, say why it has no names on it.
    fn finish_without_speakers(
        &self,
        id: u64,
        output: &Path,
        scratch: Option<PathBuf>,
        reason: Option<&str>,
    ) {
        if let Some(scratch) = &scratch {
            let _ = std::fs::remove_file(scratch);
        }
        self.imp().progress.borrow_mut().remove(&id);
        self.set_transcript_state(id, TranscriptState::Done(output.to_path_buf()));
        if let Some(reason) = reason {
            self.window()
                .toast(&format!("Transcript ready, but no speakers — {reason}"));
        }
        self.persist();
    }

    fn set_transcript_state(&self, id: u64, state: TranscriptState) {
        if let Some(job) = self.imp().queue.borrow_mut().get_mut(id) {
            job.transcript_state = state;
        }
        self.refresh();
    }

    fn fail_transcript(&self, id: u64, reason: &str) {
        self.set_transcript_state(id, TranscriptState::Failed(reason.to_string()));
        self.window().toast(&format!("No transcript — {reason}"));
        self.persist();
    }

    // -- row actions --------------------------------------------------------

    fn job_action(&self, id: u64, action: &str) {
        match action {
            "clear-finished" => self.clear_finished(),
            "open-folder" => self.open_path(&self.destination(), false),
            "pause" => self.set_paused(id, true),
            "resume" => self.set_paused(id, false),
            "cancel" => self.cancel(id),
            "retry" => self.retry(id),
            "remove" => self.remove(id),
            "open" => self.open_output(id),
            "transcript" => self.open_transcript(id),
            "details" => self.show_details(id),
            _ => {}
        }
    }

    fn set_paused(&self, id: u64, paused: bool) {
        let handle = self.imp().handles.borrow().get(&id).cloned();
        let Some(handle) = handle else { return };
        if paused {
            handle.pause();
        } else {
            handle.resume();
        }
        if let Some(job) = self.imp().queue.borrow_mut().get_mut(id) {
            job.state = if paused {
                State::Paused
            } else {
                State::Running
            };
        }
        if !paused {
            // The rate before the pause tells you nothing about the rate after
            // it, and a stale average would report minutes remaining that are
            // not.
            if let Some(progress) = self.imp().progress.borrow_mut().get_mut(&id) {
                progress.meter.reset();
            }
        }
        self.refresh();
    }

    fn cancel(&self, id: u64) {
        let handle = self.imp().handles.borrow().get(&id).cloned();
        match handle {
            // The process's own exit handler does the removal, so there is one
            // path rather than two that have to agree.
            Some(handle) => handle.cancel(),
            None => self.remove(id),
        }
    }

    fn retry(&self, id: u64) {
        if let Some(job) = self.imp().queue.borrow_mut().get_mut(id) {
            job.state = State::Waiting;
            job.outputs.clear();
        }
        self.after_change();
    }

    fn remove(&self, id: u64) {
        if let Some(handle) = self.imp().handles.borrow().get(&id) {
            handle.cancel();
            return;
        }
        let removed = self.imp().queue.borrow_mut().remove(id);
        self.imp().progress.borrow_mut().remove(&id);
        self.after_change();

        if let Some(job) = removed {
            // The list is the only record, so removing a row is destructive and
            // gets an undo rather than a confirmation.
            self.imp().library.borrow_mut().jobs.push(job);
            self.window()
                .toast_with_action("Removed from the list", "Undo", "app.undo-remove");
        }
    }

    fn undo_remove(&self) {
        let job = self.imp().library.borrow_mut().jobs.pop();
        if let Some(job) = job {
            self.imp().queue.borrow_mut().add(job);
            self.after_change();
        }
    }

    fn clear_finished(&self) {
        let cleared = self.imp().queue.borrow_mut().clear_finished();
        if cleared.is_empty() {
            self.window().toast("Nothing finished to clear");
            return;
        }
        let count = cleared.len();
        self.after_change();
        self.window().toast(&format!(
            "Cleared {count} finished download{}",
            if count == 1 { "" } else { "s" }
        ));
    }

    fn open_output(&self, id: u64) {
        let Some(job) = self.imp().queue.borrow().get(id).cloned() else {
            return;
        };
        match job.outputs.first() {
            // The containing folder rather than the file: a video player opening
            // over the window is rarely what "show me" means, and the folder is
            // where the transcript is too.
            Some(path) => self.open_path(path, true),
            None => self.open_path(&job.destination, false),
        }
    }

    fn open_transcript(&self, id: u64) {
        let path = self
            .imp()
            .queue
            .borrow()
            .get(id)
            .and_then(|job| job.transcript_path().cloned());
        if let Some(path) = path {
            self.open_path(&path, false);
        }
    }

    /// Hand a path to the desktop. `reveal` opens the containing folder with the
    /// file selected instead of opening the file itself.
    fn open_path(&self, path: &Path, reveal: bool) {
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        let window = self.imp().window.borrow().clone();
        let parent = window
            .as_ref()
            .map(|window| window.upcast_ref::<gtk::Window>());

        let report = glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |result: Result<(), glib::Error>| {
                if result.is_err() {
                    app.window().toast("Could not open that in Files");
                }
            }
        );

        if reveal {
            launcher.open_containing_folder(parent, gio::Cancellable::NONE, report);
        } else {
            launcher.launch(parent, gio::Cancellable::NONE, report);
        }
    }

    fn show_details(&self, id: u64) {
        let Some(job) = self.imp().queue.borrow().get(id).cloned() else {
            return;
        };
        let State::Failed(failure) = &job.state else {
            return;
        };

        let dialog = adw::AlertDialog::builder()
            .heading(failure.title())
            .body(failure.guidance())
            .build();

        if let Some(detail) = failure.detail() {
            // yt-dlp's own words, behind an expander: useful when reporting a
            // bug, noise the rest of the time.
            let text = gtk::Label::builder()
                .label(detail)
                .wrap(true)
                .selectable(true)
                .xalign(0.0)
                .build();
            text.add_css_class("monospace");
            text.add_css_class("caption");

            let expander = adw::ExpanderRow::builder()
                .title("What yt-dlp reported")
                .build();
            let row = adw::ActionRow::new();
            row.set_child(Some(&text));
            expander.add_row(&row);

            let group = adw::PreferencesGroup::new();
            group.add(&expander);
            dialog.set_extra_child(Some(&group));
        }

        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.present(Some(&self.window()));
    }

    // -- dialogs ------------------------------------------------------------

    fn show_preferences(&self, page: Option<&str>) {
        let dialog = Preferences::new(
            &self.imp().settings.borrow(),
            &self.imp().report.borrow(),
            toolbox::models_directory(&self.data_dir()),
            self.download_fallback(),
        );
        dialog.connect_closure(
            "changed",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |dialog: Preferences| app.settings_changed(dialog.settings())
            ),
        );
        dialog.connect_closure(
            "tools-rescan-requested",
            false,
            glib::closure_local!(
                #[watch(rename_to = app)]
                self,
                move |_: Preferences| app.survey_tools()
            ),
        );
        if let Some(page) = page {
            dialog.set_visible_page_name(page);
        }
        self.imp().preferences.replace(Some(dialog.clone()));
        dialog.present(Some(&self.window()));
    }

    fn settings_changed(&self, settings: Settings) {
        let settings = settings.sanitised();
        let ytdlp_changed = self.imp().settings.borrow().ytdlp_path != settings.ytdlp_path;
        self.imp()
            .queue
            .borrow_mut()
            .set_parallelism(settings.simultaneous_downloads);
        self.imp().settings.replace(settings);
        self.persist();
        if ytdlp_changed {
            self.survey_tools();
        }
        // A raised limit may mean something can start now.
        self.pump();
        self.refresh();
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Magpie")
            .application_icon(APP_ID)
            .developer_name("Matthew Hagrelius")
            .version(env!("CARGO_PKG_VERSION"))
            .license_type(gtk::License::Gpl30)
            .website("https://github.com/mhagrelius/magpie")
            .issue_url("https://github.com/mhagrelius/magpie/issues")
            .comments(
                "Bring videos home from the web.\n\n\
                 Magpie downloads through yt-dlp and transcribes with whisper.cpp. \
                 It bundles neither, so both stay current with the rest of your system.",
            )
            .build();
        about.add_acknowledgement_section(
            Some("Standing on"),
            &[
                "yt-dlp https://github.com/yt-dlp/yt-dlp",
                "whisper.cpp https://github.com/ggml-org/whisper.cpp",
            ],
        );
        about.present(Some(&self.window()));
    }
}

/// The paths `--print-to-file after_move:filepath` collected.
fn read_sink(sink: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(sink)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn poster_url(info: &Info) -> Option<String> {
    match info {
        Info::Single(media) => media.thumbnail.clone(),
        Info::Collection(_) => None,
    }
}
