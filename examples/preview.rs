//! Render the real widget tree to a PNG.
//!
//! Screenshotting a live GNOME Wayland session needs interactive consent, which
//! makes "does this look right?" hard to answer while iterating. This builds the
//! actual widgets against made-up jobs and paints them offscreen instead, so a
//! design change can be looked at in one command.
//!
//! The states worth a picture are the ones that are hard to reach on purpose: a
//! failed download, a paused one, a machine with no yt-dlp, a playlist of forty.
//!
//! ```sh
//! cargo run --example preview -- /tmp/preview
//! cargo run --example preview -- /tmp/preview dark
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;

use magpie::model::failure::Failure;
use magpie::model::job::{Job, Progress, State, TranscriptState};
use magpie::model::media::{Entry, Format, Info, Media, Playlist};
use magpie::model::progress::Snapshot;
use magpie::model::quality::Quality;
use magpie::model::request::{Collection, Selection};
use magpie::model::settings::Settings;
use magpie::model::tools::{Found, Freshness, Installers};
use magpie::model::transcript;
use magpie::ui::{AddDialog, MagpieWindow, Preferences, ToolReport};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/preview".to_string());
    let dark = args.next().is_some_and(|scheme| scheme == "dark");

    gtk::init().expect("a display — run under xvfb-run if there is none");
    adw::init().expect("libadwaita");

    // An animating widget is a widget that is not finished being laid out.
    // Turning animations off makes a snapshot deterministic rather than a race
    // against a transition.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }

    adw::StyleManager::default().set_color_scheme(if dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
    if let Some(display) = gtk::gdk::Display::default() {
        magpie::ui::load_stylesheet(&display);
    }

    fs::create_dir_all(&out).expect("output directory");
    let suffix = if dark { "dark" } else { "light" };

    // The window with a queue in every interesting state at once.
    let window: MagpieWindow = glib::Object::new();
    let (jobs, progress) = busy_queue();
    window.set_jobs(&jobs, &|id| progress.get(&id).cloned());
    window.set_summary(Some("1 downloading · 2 waiting"));
    render_window(&window, 720, 640, &format!("{out}/window-{suffix}.png"));

    // Nothing yet: the state a first run opens in.
    let empty: MagpieWindow = glib::Object::new();
    empty.set_jobs(&[], &|_| None);
    render_window(&empty, 720, 560, &format!("{out}/empty-{suffix}.png"));

    // yt-dlp missing, which is the whole application unavailable — and still not
    // a blank screen or a modal gate.
    let blocked: MagpieWindow = glib::Object::new();
    blocked.set_jobs(&[], &|_| None);
    blocked.set_banner(Some((
        "yt-dlp is not installed. Magpie needs it to download anything.",
        "Show Me How",
    )));
    render_window(&blocked, 720, 560, &format!("{out}/no-ytdlp-{suffix}.png"));

    // Narrow, where the Add button loses its label. The breakpoint drives this
    // in the real window; here it is set directly, since an offscreen render has
    // no window to measure.
    let narrow: MagpieWindow = glib::Object::new();
    narrow.set_jobs(&jobs, &|id| progress.get(&id).cloned());
    render_window(&narrow, 380, 700, &format!("{out}/narrow-{suffix}.png"));

    // The Add dialog, with a video's metadata in it.
    let dialog = AddDialog::new(
        "https://youtu.be/dQw4w9WgXcQ",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        true,
    );
    dialog.show_media(Info::Single(single_video()));
    render_dialog(&dialog, 360, 200, &format!("{out}/add-video-{suffix}.png"));

    // The same dialog for a playlist: the item picker, and no transcript switch.
    let playlist_dialog = AddDialog::new(
        "https://www.youtube.com/playlist?list=PL1",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        true,
    );
    playlist_dialog.show_media(Info::Collection(playlist()));
    render_dialog(
        &playlist_dialog,
        360,
        200,
        &format!("{out}/add-playlist-{suffix}.png"),
    );

    // A link that could not be read, showing the cause and the remedy rather
    // than yt-dlp's stderr.
    let failed_dialog = AddDialog::new(
        "https://youtu.be/private",
        &Settings::default(),
        PathBuf::from("/home/matty/Downloads"),
        false,
    );
    failed_dialog.show_failure(&Failure::SignInRequired);
    render_dialog(
        &failed_dialog,
        360,
        200,
        &format!("{out}/add-failed-{suffix}.png"),
    );

    // Preferences, against a machine that has ffmpeg and nothing else — the
    // state where the Tools page has something to say.
    let preferences = Preferences::new(
        &Settings::default(),
        &partial_toolbox(),
        PathBuf::from("/tmp/magpie-preview/models"),
        PathBuf::from("/home/matty/Downloads"),
    );
    preferences.set_visible_page_name("tools");
    render_dialog(&preferences, 640, 560, &format!("{out}/tools-{suffix}.png"));

    let general = Preferences::new(
        &Settings::default(),
        &partial_toolbox(),
        PathBuf::from("/tmp/magpie-preview/models"),
        PathBuf::from("/home/matty/Downloads"),
    );
    render_dialog(
        &general,
        640,
        700,
        &format!("{out}/preferences-{suffix}.png"),
    );

    println!("wrote {out}/*-{suffix}.png");
}

/// A queue holding one of everything: downloading, paused, waiting, transcribing,
/// finished, and failed for a reason that offers no retry.
fn busy_queue() -> (Vec<Job>, HashMap<u64, Progress>) {
    let downloads = PathBuf::from("/home/matty/Downloads");
    let mut jobs = Vec::new();
    let mut progress = HashMap::new();

    let mut running = Job::new(
        1,
        "https://youtu.be/aaaaaaaaaaa".into(),
        "Blackbird singing in the dead of night".into(),
        downloads.clone(),
    );
    running.state = State::Running;
    running.selection = Selection::Video(Quality::UpTo1080);
    progress.insert(1, meter_at(47_000_000, 100_000_000, 3_200_000.0, None));
    jobs.push(running);

    let mut playlist_item = Job::new(
        2,
        "https://www.youtube.com/playlist?list=PL1".into(),
        "Bach — the complete cantatas".into(),
        downloads.clone(),
    );
    playlist_item.state = State::Running;
    playlist_item.collection = Some(Collection {
        folder: "Bach — the complete cantatas".into(),
        items: Vec::new(),
    });
    progress.insert(
        2,
        meter_at(12_000_000, 240_000_000, 1_100_000.0, Some((3, 40))),
    );
    jobs.push(playlist_item);

    let mut paused = Job::new(
        3,
        "https://youtu.be/bbbbbbbbbbb".into(),
        "How to solder without crying".into(),
        downloads.clone(),
    );
    paused.state = State::Paused;
    progress.insert(3, meter_at(4_000_000, 60_000_000, 900_000.0, None));
    jobs.push(paused);

    let mut waiting = Job::new(
        4,
        "https://youtu.be/ccccccccccc".into(),
        "Wind ensemble rehearsal, take 4".into(),
        downloads.clone(),
    );
    waiting.transcribe = Some(transcript::Wish::default());
    waiting.transcript_state = TranscriptState::Waiting;
    jobs.push(waiting);

    let mut transcribing = Job::new(
        5,
        "https://youtu.be/ddddddddddd".into(),
        "A very long lecture about the history of the paperclip and why it matters".into(),
        downloads.clone(),
    );
    transcribing.state = State::Done;
    transcribing.outputs = vec![downloads.join("A very long lecture.mkv")];
    transcribing.transcribe = Some(transcript::Wish::default());
    transcribing.transcript_state = TranscriptState::Running;
    progress.insert(
        5,
        Progress {
            transcript_fraction: Some(0.62),
            ..Progress::default()
        },
    );
    jobs.push(transcribing);

    let mut done = Job::new(
        6,
        "https://youtu.be/eeeeeeeeeee".into(),
        "Field recording — dawn chorus".into(),
        downloads.clone(),
    );
    done.state = State::Done;
    done.outputs = vec![downloads.join("Field recording — dawn chorus.opus")];
    done.transcript_state = TranscriptState::Done(downloads.join("Field recording.txt"));
    jobs.push(done);

    // Failed with a cause that no amount of retrying will change, so the row
    // shows no Try Again button.
    let mut failed = Job::new(
        7,
        "https://youtu.be/fffffffffff".into(),
        "Something that has been taken down".into(),
        downloads,
    );
    failed.state = State::Failed(Failure::Unavailable);
    jobs.push(failed);

    (jobs, progress)
}

/// Ten identical samples, so the smoothed rate equals the given one and the
/// picture is the same every run.
fn meter_at(downloaded: u64, total: u64, speed: f64, item: Option<(usize, usize)>) -> Progress {
    let mut progress = Progress::default();
    for _ in 0..10 {
        progress.observe(Snapshot {
            status: "downloading".into(),
            downloaded_bytes: downloaded,
            total_bytes: Some(total),
            bytes_per_second: Some(speed),
            seconds_remaining: Some(((total - downloaded) as f64 / speed) as u64),
            item,
        });
    }
    progress
}

fn single_video() -> Media {
    Media {
        id: "dQw4w9WgXcQ".into(),
        title: "Blackbird singing in the dead of night".into(),
        uploader: Some("The Ornithology Channel".into()),
        duration: Some(272),
        thumbnail: None,
        is_live: false,
        formats: vec![
            Format {
                id: "137".into(),
                ext: "mp4".into(),
                height: Some(1080),
                fps: Some(60.0),
                filesize: Some(248_000_000),
                bitrate: None,
                has_video: true,
                has_audio: false,
            },
            Format {
                id: "251".into(),
                ext: "webm".into(),
                height: None,
                fps: None,
                filesize: Some(4_100_000),
                bitrate: Some(130.0),
                has_video: false,
                has_audio: true,
            },
        ],
        url: "https://youtu.be/dQw4w9WgXcQ".into(),
    }
}

fn playlist() -> Playlist {
    let titles = [
        "BWV 4 — Christ lag in Todesbanden",
        "BWV 8 — Liebster Gott, wenn werd ich sterben",
        "BWV 12 — Weinen, Klagen, Sorgen, Zagen",
        "BWV 21 — Ich hatte viel Bekümmernis",
        "BWV 29 — Wir danken dir, Gott",
        "BWV 34 — O ewiges Feuer, o Ursprung der Liebe",
    ];
    Playlist {
        title: "Bach — the complete cantatas".into(),
        uploader: Some("Netherlands Bach Society".into()),
        entries: titles
            .iter()
            .enumerate()
            .map(|(offset, title)| Entry {
                index: offset + 1,
                title: (*title).to_string(),
                duration: Some(1200 + offset as u64 * 137),
                url: format!("https://youtu.be/item{offset}"),
            })
            .collect(),
        url: "https://www.youtube.com/playlist?list=PL1".into(),
    }
}

/// ffmpeg present, yt-dlp present but old, whisper absent. Every branch of the
/// Tools page is then on screen at once.
fn partial_toolbox() -> ToolReport {
    ToolReport {
        ytdlp: Some(Found {
            path: PathBuf::from("/usr/bin/yt-dlp"),
            version: Some("2024.11.18".into()),
        }),
        ffmpeg: Some(Found {
            path: PathBuf::from("/usr/bin/ffmpeg"),
            version: Some("6.1.1".into()),
        }),
        ffprobe: Some(Found {
            path: PathBuf::from("/usr/bin/ffprobe"),
            version: Some("6.1.1".into()),
        }),
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

/// A window's content, because a bare X server has no window manager to map a
/// window with.
fn render_window(window: &MagpieWindow, width: i32, height: i32, path: &str) {
    if let Some(content) = window.content() {
        window.set_content(gtk::Widget::NONE);
        render(&content, width, height, path);
    }
}

/// A dialog's child, for the same reason.
///
/// The size is *measured*, not passed in. An earlier version took width and
/// height arguments, which meant every picture showed whatever the caller
/// guessed rather than what the dialog would actually do — so a dialog that
/// opened too short for its content looked fine here. `width` and `height` are
/// now floors only.
fn render_dialog(dialog: &impl IsA<adw::Dialog>, width: i32, height: i32, path: &str) {
    let dialog = dialog.as_ref();
    if let Some(child) = dialog.child() {
        dialog.set_child(gtk::Widget::NONE);

        let (_, natural_width, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
        let natural_width = natural_width.max(width);
        let (_, natural_height, _, _) = child.measure(gtk::Orientation::Vertical, natural_width);

        render(&child, natural_width, natural_height.max(height), path);
    }
}

fn render(widget: &impl IsA<gtk::Widget>, width: i32, height: i32, path: &str) {
    for factor in [1, 2, 3, 4] {
        if try_render(widget, width, height * factor, path) {
            return;
        }
    }
    eprintln!("{path}: nothing was drawn, even with room to spare");
}

fn try_render(widget: &impl IsA<gtk::Widget>, width: i32, height: i32, path: &str) -> bool {
    let window = gtk::Window::builder()
        .default_width(width)
        .default_height(height)
        .child(widget)
        .build();
    // No titlebar: these are pictures of a widget, and a window decoration
    // around one reads as a mistake.
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.present();

    settle();
    let drawn = snapshot(
        &window,
        window.width().max(width),
        window.height().max(height),
        path,
    );

    // Take the widget back before the window goes, so a caller can render the
    // same one twice.
    window.set_child(gtk::Widget::NONE);
    window.destroy();
    drawn
}

/// Run the main loop until there is nothing left to lay out.
///
/// One drain is not enough: presenting a widget schedules work that schedules
/// more, so this pumps until it stops finding any, with a bound so a
/// misbehaving widget cannot hang the run.
fn settle() {
    let context = glib::MainContext::default();
    for _ in 0..100 {
        let mut worked = false;
        while context.iteration(false) {
            worked = true;
        }
        if !worked {
            break;
        }
    }
}

/// Paint a realised window into a PNG. Reports whether anything was drawn.
fn snapshot(window: &impl IsA<gtk::Widget>, width: i32, height: i32, path: &str) -> bool {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);

    let Some(node) = snapshot.to_node() else {
        return false;
    };
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer
        .realize(gtk::gdk::Surface::NONE)
        .expect("a renderer");
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).expect("write the png");
    renderer.unrealize();
    true
}
