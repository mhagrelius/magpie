//! The widget tree, built and driven with no network and no window on screen.
//!
//! One `#[test]`, containing a list of cases. GTK is thread-affine and
//! `--test-threads=1` only serialises tests — it does not make them share a
//! thread — so a second `#[test]` touching a widget is a second thread calling
//! into GTK, which is undefined rather than slow.
//!
//! Windows are constructed and never presented. Dialogs are presented with no
//! parent, which `AdwDialog` allows and which is necessary: until it is
//! presented, a dialog's child is not parented into its internal tree, so there
//! is nothing to look at. Each case closes what it opened.
//!
//! Visibility is asserted with `get_visible` rather than `is_visible`, because
//! the latter is false for everything inside a window that was never mapped.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;

use magpie::model::failure::Failure;
use magpie::model::job::{Job, Progress, State, TranscriptState};
use magpie::model::media::{Entry, Format, Info, Media, Playlist};
use magpie::model::progress::Snapshot;
use magpie::model::quality::Quality;
use magpie::model::settings::Settings;
use magpie::model::tools::{Found, Freshness, Installers};
use magpie::ui::{AddDialog, MagpieWindow, Preferences, ToolReport};

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    ("a fresh window shows the empty state", empty_window),
    ("a queue fills the list", populated_window),
    ("the list is ordered by what is happening", ordering),
    ("a banner appears and goes", banner),
    ("the link bar refuses prose", link_bar),
    ("the add dialog waits, then fills in", add_dialog_states),
    (
        "a playlist gets a picker and no transcript switch",
        add_dialog_playlist,
    ),
    ("a failed link states a cause", add_dialog_failure),
    ("preferences open on every page", preferences_pages),
    (
        "a missing tool is explained, not hidden",
        preferences_missing_tools,
    ),
    ("the shortcuts dialog lists the accelerators", shortcuts),
];

#[test]
fn widgets() {
    adw::init().expect("GTK and libadwaita initialise");
    // A test that waits on an animation is a test that fails on a slow machine.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }

    let mut failures = Vec::<String>::new();
    for (name, case) in CASES {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(case));
        if let Err(panic) = result {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panicked".to_string());
            failures.push(format!("{name}: {message}"));
        }
        settle();
    }
    assert!(failures.is_empty(), "\n  {}", failures.join("\n  "));
}

// -- cases ------------------------------------------------------------------

fn empty_window() {
    let window = window();
    window.set_jobs(&[], &|_| None);
    assert_eq!(
        visible_stack_child(&window),
        Some("empty".to_string()),
        "nothing queued shows the status page, not an empty list"
    );
    window.destroy();
}

fn populated_window() {
    let window = window();
    let jobs = vec![
        job(1, "Downloading", State::Running),
        job(2, "Waiting", State::Waiting),
        job(3, "Failed", State::Failed(Failure::Unavailable)),
    ];
    let progress = downloading();
    window.set_jobs(&jobs, &|id| (id == 1).then(|| progress.clone()));

    assert_eq!(visible_stack_child(&window), Some("list".to_string()));
    assert_eq!(rows(&window).len(), 3);

    // Rebinding with the same ids reuses the rows rather than rebuilding them,
    // which is what keeps focus and scroll position across a progress tick.
    let before: Vec<_> = rows(&window);
    window.set_jobs(&jobs, &|id| (id == 1).then(|| progress.clone()));
    let after: Vec<_> = rows(&window);
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(a == b, "rows were rebuilt instead of rebound");
    }

    // A removed job takes its row with it.
    window.set_jobs(&jobs[..1], &|_| None);
    assert_eq!(rows(&window).len(), 1);
    window.destroy();
}

fn ordering() {
    // Running first, then waiting, then finished — the row someone is watching
    // is the reason the window is open, and it must not sink under the history.
    let window = window();
    let jobs = vec![
        job(1, "Old and done", State::Done),
        job(2, "Newer and done", State::Done),
        job(3, "Waiting", State::Waiting),
        job(4, "Running", State::Running),
    ];
    window.set_jobs(&jobs, &|_| None);

    let ids = row_ids(&window);
    assert_eq!(
        ids,
        vec![4, 3, 2, 1],
        "running, waiting, then finished newest first"
    );
    window.destroy();
}

fn banner() {
    let window = window();
    let banner = find::<adw::Banner>(window.upcast_ref()).expect("a banner");
    assert!(
        !banner.is_revealed(),
        "hidden until there is something to say"
    );

    window.set_banner(Some(("yt-dlp is not installed.", "Show Me How")));
    assert!(banner.is_revealed());
    assert_eq!(banner.button_label().as_deref(), Some("Show Me How"));

    window.set_banner(None);
    assert!(!banner.is_revealed());
    window.destroy();
}

fn link_bar() {
    let window = window();
    let entry = find::<gtk::Entry>(window.upcast_ref()).expect("the link entry");
    let add = find_button_labelled(window.upcast_ref(), "Add").expect("the Add button");

    assert!(!add.is_sensitive(), "nothing typed yet");

    entry.set_text("how do I download a video");
    assert!(!add.is_sensitive(), "prose is not a link");

    entry.set_text("youtube.com/watch?v=dQw4w9WgXcQ");
    assert!(
        add.is_sensitive(),
        "a scheme-less address bar paste is a link"
    );

    // Submitting hands over the normalised URL and clears the box, so the next
    // paste lands in an empty entry whatever happens next.
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    window.connect_closure(
        "link-submitted",
        false,
        glib::closure_local!(
            #[strong]
            seen,
            move |_: MagpieWindow, url: &str| {
                *seen.borrow_mut() = Some(url.to_string());
            }
        ),
    );
    add.emit_clicked();
    assert_eq!(
        seen.borrow().as_deref(),
        Some("https://youtube.com/watch?v=dQw4w9WgXcQ")
    );
    assert_eq!(entry.text().as_str(), "");
    window.destroy();
}

fn add_dialog_states() {
    let dialog = AddDialog::new(
        "https://youtu.be/dQw4w9WgXcQ",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        true,
    );
    dialog.present(gtk::Widget::NONE);
    settle();

    let stack = find_stack_containing(dialog.upcast_ref(), "looking-up").expect("the state stack");
    let download = find_button_labelled(dialog.upcast_ref(), "Download").expect("Download");

    assert_eq!(stack.visible_child_name().as_deref(), Some("looking-up"));
    assert!(
        !download.is_sensitive(),
        "nothing to download until the metadata lands"
    );

    dialog.show_media(Info::Single(video()));
    assert_eq!(stack.visible_child_name().as_deref(), Some("ready"));
    assert!(download.is_sensitive());

    // Audio only swaps which format row is on show, rather than adding a third
    // control that contradicts the other two.
    let switches = find_all::<adw::SwitchRow>(dialog.upcast_ref());
    let audio_only = switches
        .iter()
        .find(|row| row.title() == "Audio only")
        .expect("the Audio only switch");
    let combos = find_all::<adw::ComboRow>(dialog.upcast_ref());
    let quality = combos
        .iter()
        .find(|row| row.title() == "Quality")
        .expect("the Quality row");

    assert!(quality.get_visible());
    audio_only.set_active(true);
    assert!(
        !quality.get_visible(),
        "quality is meaningless for audio only"
    );
    audio_only.set_active(false);
    assert!(quality.get_visible());

    // The escape hatch: the last entry of the quality list reveals the raw
    // format picker, which only exists because the metadata arrived.
    let exact = combos
        .iter()
        .find(|row| row.subtitle().as_deref() == Some("Passed to yt-dlp exactly as listed"))
        .expect("the specific-format row");
    assert!(!exact.get_visible());
    quality.set_selected(Quality::ALL.len() as u32);
    assert!(exact.get_visible());

    dialog.close();
}

fn add_dialog_playlist() {
    let dialog = AddDialog::new(
        "https://www.youtube.com/playlist?list=PLabc",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        true,
    );
    dialog.present(gtk::Widget::NONE);
    dialog.show_media(Info::Collection(playlist()));
    settle();

    let checks = find_all::<gtk::CheckButton>(dialog.upcast_ref());
    assert_eq!(checks.len(), 3, "one per item");
    assert!(checks.iter().all(gtk::CheckButton::is_active), "all ticked");

    let none = find_button_labelled(dialog.upcast_ref(), "Select None").expect("Select None");
    none.emit_clicked();
    assert!(checks.iter().all(|check| !check.is_active()));

    // Transcribing forty items is an afternoon of CPU nobody asked for by
    // flipping one switch, so the switch is not offered here at all.
    let transcribe = find_all::<adw::SwitchRow>(dialog.upcast_ref())
        .into_iter()
        .find(|row| row.title() == "Transcribe")
        .expect("the row still exists");
    assert!(!transcribe.get_visible());
    assert!(!transcribe.is_active());

    dialog.close();
}

fn add_dialog_failure() {
    let dialog = AddDialog::new(
        "https://youtu.be/private",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        false,
    );
    dialog.present(gtk::Widget::NONE);
    dialog.show_failure(&Failure::SignInRequired);
    settle();

    let stack = find_stack_containing(dialog.upcast_ref(), "failure").expect("the stack");
    assert_eq!(stack.visible_child_name().as_deref(), Some("failure"));

    let pages = find_all::<adw::StatusPage>(dialog.upcast_ref());
    let shown = pages
        .iter()
        .find(|page| page.title() == Failure::SignInRequired.title())
        .expect("the failure is named, not the word Error");
    assert!(
        shown
            .description()
            .is_some_and(|text| text.contains("cookies")),
        "and the remedy is on screen"
    );

    // Retry is withheld where trying again cannot work.
    let retry = find_button_labelled(dialog.upcast_ref(), "Try Again").expect("the button exists");
    assert!(!retry.get_visible());

    dialog.show_failure(&Failure::Network);
    assert!(
        retry.get_visible(),
        "a connection problem is worth retrying"
    );

    // The Transcribe switch is insensitive rather than absent, with the reason
    // in a tooltip — a control that comes and goes between machines is harder to
    // learn than one that greys out.
    let transcribe = find_all::<adw::SwitchRow>(dialog.upcast_ref())
        .into_iter()
        .find(|row| row.title() == "Transcribe")
        .expect("the Transcribe row");
    assert!(!transcribe.is_sensitive());
    assert!(transcribe
        .tooltip_text()
        .is_some_and(|text| text.contains("whisper")));

    dialog.close();
}

fn preferences_pages() {
    let preferences = preferences();
    preferences.present(gtk::Widget::NONE);
    settle();
    for page in ["general", "transcripts", "tools"] {
        preferences.set_visible_page_name(page);
        assert_eq!(
            preferences.visible_page_name().as_deref(),
            Some(page),
            "the banner needs to be able to jump to a page by name"
        );
    }
    preferences.close();
}

fn preferences_missing_tools() {
    let preferences = preferences();
    preferences.present(gtk::Widget::NONE);
    settle();
    let rows = find_all::<adw::ActionRow>(preferences.upcast_ref());

    let whisper = rows
        .iter()
        .find(|row| row.title() == "whisper.cpp")
        .expect("a row for whisper");
    let subtitle = whisper.subtitle().unwrap_or_default();
    assert!(subtitle.contains("Not installed"), "{subtitle}");
    // Ubuntu has no package, so the remedy is Magpie's own build script. The
    // row has to name the command, not just the problem.
    assert!(
        subtitle.contains("--with-whisper"),
        "and says the command that gets it: {subtitle}"
    );

    // A stale yt-dlp is the most common cause of a failed download, so its age
    // is stated rather than left for the user to work out.
    let ytdlp = rows
        .iter()
        .find(|row| row.title() == "yt-dlp")
        .expect("a row for yt-dlp");
    let subtitle = ytdlp.subtitle().unwrap_or_default();
    assert!(subtitle.contains("2024.11.18"), "{subtitle}");
    assert!(subtitle.contains("updating yt-dlp"), "{subtitle}");

    // The fixture has uv, so the stale yt-dlp gets a button that runs the
    // upgrade rather than a command to retype.
    let update = find_button_labelled(preferences.upcast_ref(), "Update")
        .expect("an Update button for the stale yt-dlp");
    assert!(update.get_visible());
    assert_eq!(
        update.tooltip_text().as_deref(),
        // A forced reinstall with the `default` group, not a plain upgrade: a
        // yt-dlp first installed without it would otherwise never gain the EJS
        // solver scripts YouTube extraction needs.
        Some("uv tool install --force \"yt-dlp[default]\""),
        "a button that changes the environment says exactly what it will run"
    );

    // Nothing to press for whisper: building C++ from a GUI button is a failure
    // nobody could read.
    let buttons = find_all::<gtk::Button>(whisper.upcast_ref());
    assert!(
        buttons
            .iter()
            .all(|b| b.label().as_deref() != Some("Install")),
        "whisper offers Copy, not Install"
    );

    preferences.close();
}

fn shortcuts() {
    // Reached through the window action, which is also how the menu reaches it.
    let window = window();
    gtk::prelude::ActionGroupExt::activate_action(&window, "shortcuts", None);
    settle();
    window.destroy();
}

// -- fixtures ---------------------------------------------------------------

fn window() -> MagpieWindow {
    // No application: a window is constructible without one, and a test that
    // needed an AdwApplication would be a test of GApplication's lifecycle.
    glib::Object::new()
}

fn preferences() -> Preferences {
    Preferences::new(
        &Settings::default(),
        &report(),
        PathBuf::from("/tmp/magpie-widget-tests/models"),
        PathBuf::from("/home/matty/Downloads"),
    )
}

/// yt-dlp present but old, ffmpeg present, whisper absent — every branch of the
/// Tools page at once.
fn report() -> ToolReport {
    ToolReport {
        ytdlp: Some(Found {
            path: PathBuf::from("/usr/bin/yt-dlp"),
            version: Some("2024.11.18".into()),
        }),
        ffmpeg: Some(Found {
            path: PathBuf::from("/usr/bin/ffmpeg"),
            version: Some("6.1.1".into()),
        }),
        ffprobe: None,
        whisper: None,
        freshness: Freshness::Stale { days: 620 },
        // uv present, so the Tools page offers to run the upgrade itself rather
        // than printing a command to retype.
        installers: Installers {
            uv: true,
            ..Installers::default()
        },
        uv_path: Some(PathBuf::from("/home/matty/.local/bin/uv")),
        pipx_path: None,
        // Absent, which is the state worth looking at: it is the one degradation
        // that costs formats without saying so.
        js_runtime: None,
    }
}

fn job(id: u64, title: &str, state: State) -> Job {
    let mut job = Job::new(
        id,
        format!("https://youtu.be/{id}"),
        title.to_string(),
        PathBuf::from("/home/matty/Downloads"),
    );
    job.state = state;
    if job.state == State::Done {
        job.outputs = vec![PathBuf::from("/home/matty/Downloads").join(format!("{title}.mkv"))];
        job.transcript_state = TranscriptState::None;
    }
    job
}

fn downloading() -> Progress {
    let mut progress = Progress::default();
    for _ in 0..10 {
        progress.observe(Snapshot {
            status: "downloading".into(),
            downloaded_bytes: 47_000_000,
            total_bytes: Some(100_000_000),
            bytes_per_second: Some(3_200_000.0),
            seconds_remaining: Some(17),
            item: None,
        });
    }
    progress
}

fn video() -> Media {
    Media {
        id: "dQw4w9WgXcQ".into(),
        title: "Blackbird singing in the dead of night".into(),
        uploader: Some("The Ornithology Channel".into()),
        duration: Some(272),
        thumbnail: None,
        is_live: false,
        formats: vec![Format {
            id: "137".into(),
            ext: "mp4".into(),
            height: Some(1080),
            fps: Some(30.0),
            filesize: Some(248_000_000),
            bitrate: None,
            has_video: true,
            has_audio: false,
        }],
        url: "https://youtu.be/dQw4w9WgXcQ".into(),
    }
}

fn playlist() -> Playlist {
    Playlist {
        title: "Bach: cantatas".into(),
        uploader: Some("Netherlands Bach Society".into()),
        entries: (1..=3)
            .map(|index| Entry {
                index,
                title: format!("BWV {index}"),
                duration: Some(1200),
                url: format!("https://youtu.be/item{index}"),
            })
            .collect(),
        url: "https://www.youtube.com/playlist?list=PLabc".into(),
    }
}

// -- helpers ----------------------------------------------------------------

fn settle() {
    let context = glib::MainContext::default();
    for _ in 0..50 {
        let mut worked = false;
        while context.iteration(false) {
            worked = true;
        }
        if !worked {
            break;
        }
    }
}

fn visible_stack_child(window: &MagpieWindow) -> Option<String> {
    // The outermost stack is the empty/list one; the link bar has none.
    find::<gtk::Stack>(window.upcast_ref())?
        .visible_child_name()
        .map(|name| name.to_string())
}

fn rows(window: &MagpieWindow) -> Vec<gtk::ListBoxRow> {
    find_all::<gtk::ListBoxRow>(window.upcast_ref())
}

fn row_ids(window: &MagpieWindow) -> Vec<u64> {
    // The row's own id is not public, so the order is read from the titles,
    // which the fixtures make unambiguous.
    rows(window)
        .iter()
        .filter_map(|row| {
            let label = find::<gtk::Label>(row.upcast_ref())?;
            match label.label().as_str() {
                "Running" => Some(4),
                "Waiting" => Some(3),
                "Newer and done" => Some(2),
                "Old and done" => Some(1),
                _ => None,
            }
        })
        .collect()
}

/// The stack that has a page called `name`.
///
/// Not simply the first `GtkStack` found: libadwaita's own containers use stacks
/// internally, and which one comes first in the tree is not this test's business.
fn find_stack_containing(root: &gtk::Widget, name: &str) -> Option<gtk::Stack> {
    find_all::<gtk::Stack>(root)
        .into_iter()
        .find(|stack| stack.child_by_name(name).is_some())
}

/// The first descendant of type `T`, breadth first.
fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Option<T> {
    find_all::<T>(root).into_iter().next()
}

/// Every descendant of type `T`, in tree order.
fn find_all<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Vec<T> {
    let mut found = Vec::new();
    let mut queue = vec![root.clone()];
    while let Some(widget) = queue.pop() {
        if let Ok(matched) = widget.clone().downcast::<T>() {
            found.push(matched);
        }
        let mut children = Vec::new();
        let mut child = widget.first_child();
        while let Some(current) = child {
            children.push(current.clone());
            child = current.next_sibling();
        }
        // Reversed, so popping walks the children left to right.
        children.reverse();
        queue.extend(children);
    }
    found
}

fn find_button_labelled(root: &gtk::Widget, label: &str) -> Option<gtk::Button> {
    find_all::<gtk::Button>(root).into_iter().find(|button| {
        button.label().as_deref() == Some(label) || button.tooltip_text().as_deref() == Some(label)
    })
}
