//! The model half, end to end, over recorded yt-dlp output.
//!
//! No display, no network, no subprocess. Each test walks a whole path a user
//! could take — paste, choose, run, fail, restart — through the real functions
//! the application calls, and checks the thing that would actually be wrong if
//! it broke. The unit tests beside each file cover the pieces; these cover the
//! joins between them, which is where the old application's bugs lived.

use std::path::PathBuf;

use magpie::model::failure::{self, Failure};
use magpie::model::job::{Job, Progress, State, TranscriptState};
use magpie::model::library::Library;
use magpie::model::media::{self, Info};
use magpie::model::progress::{parse_line, Event, LineBuffer};
use magpie::model::quality::{AudioFormat, Quality};
use magpie::model::queue::Queue;
use magpie::model::request::{self, Collection, Cookies, Selection};
use magpie::model::settings::Settings;
use magpie::model::store;
use magpie::model::transcript;
use magpie::model::url;

/// A `--dump-json` payload shaped like the real thing, cut down to the fields
/// the application reads plus a few it does not, so that ignoring the extras is
/// part of what is tested.
const VIDEO_JSON: &str = r#"{
  "id": "dQw4w9WgXcQ",
  "title": "Blackbird singing in the dead of night",
  "uploader": "The Ornithology Channel",
  "channel": "The Ornithology Channel",
  "duration": 272,
  "view_count": 1402331,
  "upload_date": "20250714",
  "thumbnail": "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg",
  "description": "A blackbird, singing.",
  "webpage_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  "extractor": "youtube",
  "_some_field_yt_dlp_may_rename": true,
  "formats": [
    {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none"},
    {"format_id": "18", "ext": "mp4", "vcodec": "avc1.42001E", "acodec": "mp4a.40.2",
     "height": 360, "fps": 30, "filesize": 12000000, "tbr": 560},
    {"format_id": "137", "ext": "mp4", "vcodec": "avc1.640028", "acodec": "none",
     "height": 1080, "fps": 30, "filesize": 248000000, "tbr": 4200},
    {"format_id": "271", "ext": "webm", "vcodec": "vp09.00.50.08", "acodec": "none",
     "height": 1440, "fps": 30, "filesize_approx": 620000000},
    {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus",
     "abr": 130.2, "filesize": 4400000}
  ]
}"#;

const PLAYLIST_JSON: &str = r#"{
  "_type": "playlist",
  "id": "PLabc",
  "title": "Bach: the complete cantatas",
  "uploader": "Netherlands Bach Society",
  "playlist_count": 4,
  "webpage_url": "https://www.youtube.com/playlist?list=PLabc",
  "entries": [
    {"id": "aaaaaaaaaaa", "title": "BWV 4", "duration": 1200},
    {"id": "bbbbbbbbbbb", "title": "BWV 8", "duration": 1337},
    null,
    {"id": "ddddddddddd", "title": "BWV 21", "duration": 1611}
  ]
}"#;

fn settings() -> Settings {
    Settings::default()
}

fn destination() -> PathBuf {
    PathBuf::from("/home/matty/Downloads")
}

fn cache() -> PathBuf {
    PathBuf::from("/home/matty/.cache/magpie")
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

#[test]
fn a_pasted_link_becomes_a_download_that_can_reach_1080p() {
    // The whole happy path for one video, and the specific thing the old
    // application could not do: 1080p needs two streams merged, and its picker
    // only ever offered formats that carried their own audio.
    let link = url::parse("  youtube.com/watch?v=dQw4w9WgXcQ  ").expect("a link");
    let Info::Single(video) = media::parse(VIDEO_JSON).expect("parses") else {
        panic!("expected one video");
    };

    let mut job = Job::new(1, link.url, video.title.clone(), destination());
    job.selection = Selection::Video(Quality::UpTo1080);
    job.thumbnail = video.thumbnail.clone();

    let request = job.request(Cookies::None, None, None, &cache());
    let args = request.argv();

    let selector = value_after(&args, "-f").expect("a format selector");
    assert!(selector.contains("bestvideo*"), "{selector}");
    assert!(selector.contains("+bestaudio"), "{selector}");
    assert!(selector.contains("height<=?1080"), "{selector}");
    assert_eq!(value_after(&args, "--merge-output-format"), Some("mkv/mp4"));
    assert_eq!(args.last().map(String::as_str), Some(job.url.as_str()));
}

#[test]
fn the_format_list_offers_the_quality_the_old_picker_hid() {
    // 1080p and 1440p are video-only on YouTube. The old picker filtered them
    // out, so its list topped out at the 360p muxed format.
    let Info::Single(video) = media::parse(VIDEO_JSON).unwrap() else {
        panic!()
    };
    let heights: Vec<u32> = video.formats.iter().filter_map(|f| f.height).collect();
    assert_eq!(heights, vec![1440, 1080, 360], "tallest first");
    assert!(
        video.formats.iter().any(|f| f.has_video && !f.has_audio),
        "video-only formats are listed, with a note that audio is added"
    );
}

#[test]
fn a_download_reports_progress_then_a_finished_file() {
    // A recorded stream, fed in chunks that split lines the way a pipe does.
    let mut job = Job::new(
        1,
        "https://youtu.be/x".into(),
        "A video".into(),
        destination(),
    );
    job.state = State::Running;
    let mut progress = Progress::default();
    let mut buffer = LineBuffer::new();

    let stream = concat!(
        "[youtube] Extracting URL: https://youtu.be/x\n",
        "[info] x: Downloading 1 format(s): 137+251\n",
        "\u{1f}magpie\tdownload\tdownloading\t0\t248000000\tNA\tNA\tNA\tNA\tNA\n",
        "\u{1f}magpie\tdownload\tdownloading\t62000000\t248000000\tNA\t3200000.0\t58\tNA\tNA\n",
        "\u{1f}magpie\tdownload\tdownloading\t124000000\t248000000\tNA\t3200000.0\t39\tNA\tNA\n",
        "\u{1f}magpie\tdownload\tfinished\t248000000\t248000000\tNA\t3200000.0\t0\tNA\tNA\n",
        "\u{1f}magpie\tpostprocess\tstarted\tMerger\n",
        "[Merger] Merging formats into \"A video.mkv\"\n",
    );

    // Deliberately awkward chunk sizes, so lines land across boundaries.
    let bytes = stream.as_bytes();
    for chunk in bytes.chunks(37) {
        for line in buffer.push(chunk) {
            match parse_line(&line) {
                Event::Progress(snapshot) => progress.observe(snapshot),
                Event::Postprocessing { status, processor } => {
                    progress.postprocessing = (status != "finished").then_some(processor);
                }
                Event::Chatter(_) => {}
            }
        }
    }

    // Mid-download the row answered "should I wait?"; now it is merging, which
    // has no byte count and so no honest percentage.
    assert_eq!(
        job.status_line(Some(&progress)),
        "Combining video and audio"
    );
    assert_eq!(job.fraction(Some(&progress)), None);
    assert_eq!(progress.snapshot.downloaded_bytes, 248_000_000);
}

#[test]
fn the_speed_shown_is_smoothed_rather_than_the_last_reading() {
    let mut job = Job::new(
        1,
        "https://youtu.be/x".into(),
        "A video".into(),
        destination(),
    );
    job.state = State::Running;
    let mut progress = Progress::default();

    // Nine steady samples then one spike, which is what a fragment boundary
    // looks like. yt-dlp's own eta would jump to a second.
    for (downloaded, speed) in [
        (10, 1e6),
        (20, 1e6),
        (30, 1e6),
        (40, 1e6),
        (50, 1e6),
        (60, 1e6),
        (70, 1e6),
        (80, 1e6),
        (90, 1e6),
        (100, 40e6),
    ] {
        progress.observe(magpie::model::progress::Snapshot {
            status: "downloading".into(),
            downloaded_bytes: downloaded * 1_000_000,
            total_bytes: Some(200_000_000),
            bytes_per_second: Some(speed),
            seconds_remaining: Some(2),
            ..Default::default()
        });
    }

    let line = job.status_line(Some(&progress));
    assert!(line.contains("50%"), "{line}");
    assert!(
        !line.contains("Almost done") && !line.contains("2 seconds"),
        "the spike must not be believed: {line}"
    );
}

#[test]
fn a_playlist_downloads_the_ticked_items_into_a_folder_of_its_own() {
    let link = url::parse("https://www.youtube.com/playlist?list=PLabc").expect("a link");
    assert_eq!(link.kind, url::Kind::Collection);

    let Info::Collection(playlist) = media::parse(PLAYLIST_JSON).expect("parses") else {
        panic!("expected a playlist");
    };
    // The deleted third entry is skipped without renumbering the fourth, which
    // is what `--playlist-items` counts.
    assert_eq!(
        playlist.entries.iter().map(|e| e.index).collect::<Vec<_>>(),
        vec![1, 2, 4]
    );

    // The user unticks the first.
    let chosen: Vec<usize> = playlist.entries.iter().skip(1).map(|e| e.index).collect();
    let mut job = Job::new(1, link.url, playlist.title.clone(), destination());
    job.collection = Some(Collection {
        folder: request::folder_name(&playlist.title),
        items: chosen,
    });

    let args = job.request(Cookies::None, None, None, &cache()).argv();
    assert_eq!(value_after(&args, "--playlist-items"), Some("2,4"));
    assert_eq!(
        value_after(&args, "-P"),
        Some("/home/matty/Downloads/Bach- the complete cantatas"),
        "the colon is not a legal filename character everywhere it might be copied"
    );
    assert!(value_after(&args, "-o").unwrap().contains("playlist_index"));
    assert_eq!(job.destination_label(), "Bach- the complete cantatas");
}

#[test]
fn a_failed_item_lets_the_rest_of_the_playlist_run() {
    // The old application advanced its queue from the success handler alone, so
    // one private video left every item behind it Pending until a restart.
    let mut queue = Queue::new(1);
    for index in 1..=4u64 {
        queue.add(Job::new(
            index,
            format!("https://youtu.be/{index}"),
            format!("Item {index}"),
            destination(),
        ));
    }

    let mut completed = Vec::new();
    let mut guard = 0;
    while let Some(&id) = queue.ready().first() {
        guard += 1;
        assert!(guard < 20, "the queue stopped advancing");

        queue.get_mut(id).unwrap().state = State::Running;
        // Every second one fails, in a way that is not retryable.
        let outcome = if id % 2 == 0 {
            State::Failed(Failure::Unavailable)
        } else {
            State::Done
        };
        queue.get_mut(id).unwrap().state = outcome;
        completed.push(id);
    }

    assert_eq!(completed, vec![1, 2, 3, 4], "every item was attempted");
    assert!(queue.ready().is_empty());
    assert_eq!(queue.summary(), None, "nothing is left going");
}

#[test]
fn a_bot_wall_names_the_setting_that_fixes_it_and_offers_no_pointless_retry() {
    let stderr = concat!(
        "WARNING: [youtube] abc: Falling back to generic n function search\n",
        "WARNING: unable to download webpage: HTTP Error 403: Forbidden\n",
        "ERROR: [youtube] abc: Sign in to confirm you're not a bot. ",
        "Use --cookies-from-browser or --cookies for the authentication.\n",
    );
    let cause = failure::classify(stderr);

    assert_eq!(cause, Failure::SignInRequired);
    assert!(cause.guidance().contains("cookies"));
    assert!(!cause.is_retryable(), "the wall will still be there");

    // And the setting the guidance points at produces the flag.
    let mut settings = settings();
    settings.cookies_from_browser = Some("firefox".into());
    let job = Job::new(1, "https://youtu.be/abc".into(), "x".into(), destination());
    let args = job.request(settings.cookies(), None, None, &cache()).argv();
    assert_eq!(
        value_after(&args, "--cookies-from-browser"),
        Some("firefox")
    );
}

#[test]
fn an_audio_only_download_needs_no_ffmpeg_unless_it_was_asked_to_convert() {
    let mut job = Job::new(1, "https://youtu.be/x".into(), "x".into(), destination());

    job.selection = Selection::Audio(AudioFormat::Best);
    let args = job.request(Cookies::None, None, None, &cache()).argv();
    assert!(!args.contains(&"-x".to_string()), "a copy, not a transcode");

    job.selection = Selection::Audio(AudioFormat::Mp3);
    let args = job.request(Cookies::None, None, None, &cache()).argv();
    assert!(args.contains(&"-x".to_string()));
    assert_eq!(value_after(&args, "--audio-format"), Some("mp3"));
    // The setting the old application declared, stored, and never passed.
    assert_eq!(value_after(&args, "--audio-quality"), Some("0"));
}

#[test]
fn a_transcript_follows_the_download_through_conversion() {
    let mut job = Job::new(
        1,
        "https://youtu.be/x".into(),
        "A talk".into(),
        destination(),
    );
    job.transcribe = Some(transcript::Wish {
        format: transcript::Format::Srt,
        ..transcript::Wish::default()
    });
    job.transcript_state = TranscriptState::Waiting;

    // Not yet: the download has not finished.
    assert!(!job.wants_transcript_now());

    job.state = State::Done;
    job.outputs = vec![destination().join("A talk.mkv")];
    assert!(job.wants_transcript_now());

    let media_path = job.single_output().unwrap().clone();
    assert!(
        transcript::needs_conversion(&media_path),
        "mkv is the common case, not the exception"
    );

    let wav = transcript::conversion_path(&cache(), job.id);
    assert!(
        wav.starts_with(cache()),
        "the scratch file does not land in the user's folder"
    );
    let ffmpeg = transcript::conversion_argv(&media_path, &wav);
    assert!(ffmpeg.contains(&"16000".to_string()));
    assert!(ffmpeg.contains(&"-nostdin".to_string()));

    let wish = job.transcribe.clone().unwrap();
    let stem = media_path.with_extension("");
    let args = transcript::argv(&PathBuf::from("/models/ggml-small.bin"), &wav, &stem, &wish);
    assert!(args.contains(&"--output-srt".to_string()));
    assert_eq!(
        value_after(&args, "-of"),
        Some("/home/matty/Downloads/A talk"),
        "no extension, or whisper writes A talk.srt.srt"
    );
    assert_eq!(
        transcript::output_path(&media_path, wish.format),
        destination().join("A talk.srt")
    );

    // And whisper's own progress is readable.
    assert_eq!(
        transcript::parse_progress("whisper_print_progress_callback: progress =  62%"),
        Some(0.62)
    );
}

#[test]
fn the_queue_and_the_history_come_back_after_a_restart() {
    // The old application kept its queue in renderer memory and had a SQLite
    // history table it never wrote to, so closing the window lost both.
    let dir = tempfile::tempdir().expect("a temp dir");
    let data = dir.path();

    let mut queue = Queue::new(1);
    let finished = {
        let mut job = Job::new(1, "https://youtu.be/a".into(), "Done".into(), destination());
        job.state = State::Done;
        job.outputs = vec![destination().join("Done.mkv")];
        job
    };
    let running = {
        let mut job = Job::new(
            2,
            "https://youtu.be/b".into(),
            "Halfway".into(),
            destination(),
        );
        job.state = State::Running;
        job.selection = Selection::Audio(AudioFormat::M4a);
        job
    };
    let waiting = Job::new(
        3,
        "https://youtu.be/c".into(),
        "Queued".into(),
        destination(),
    );
    queue.add(finished);
    queue.add(running);
    queue.add(waiting);

    let mut library = Library::default();
    library.replace(queue.jobs());
    library.save(&Library::path_in(data)).expect("saves");

    // Reopen.
    let (reopened, outcome) = Library::load(&Library::path_in(data)).expect("loads");
    assert_eq!(outcome, store::Outcome::Loaded);
    let restored = Queue::restore(reopened.jobs.clone(), 1);

    assert_eq!(restored.get(1).unwrap().state, State::Done);
    // A subprocess that no longer exists is not still at 47%; it goes back in
    // the queue and yt-dlp resumes from the .part file.
    assert_eq!(restored.get(2).unwrap().state, State::Waiting);
    assert_eq!(
        restored.get(2).unwrap().selection,
        Selection::Audio(AudioFormat::M4a),
        "the format the user chose survived too"
    );
    assert_eq!(restored.ready(), vec![2], "the interrupted one goes first");

    assert_eq!(reopened.search("done").len(), 1);
    assert!(
        reopened.search("queued").is_empty(),
        "a job that never ran is not history"
    );
}

#[test]
fn a_library_truncated_by_a_power_cut_does_not_stop_the_app_starting() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = Library::path_in(dir.path());
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(&path, r#"{"version": 1, "jobs": [{"id": 1, "url": "htt"#).unwrap();

    let (library, outcome) = Library::load(&path).expect("recovers rather than failing");
    assert!(library.jobs.is_empty());
    let store::Outcome::Recovered { backup } = outcome else {
        panic!("expected recovery, got {outcome:?}");
    };
    assert!(backup.exists(), "the damaged file is kept, not binned");
}

#[test]
fn a_config_file_from_a_later_version_keeps_the_settings_it_recognises() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = Settings::path_in(dir.path());
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(
        &path,
        r#"{
          "quality": "up-to-720",
          "audio-format": "m4a",
          "simultaneous-downloads": 99,
          "rate-limit": "not a rate",
          "cookies-from-browser": "netscape",
          "a-setting-from-the-future": {"nested": true}
        }"#,
    )
    .unwrap();

    let (settings, outcome) = store::load::<Settings>(&path).expect("loads");
    assert_eq!(outcome, store::Outcome::Loaded);
    let settings = settings.sanitised();

    assert_eq!(settings.quality, Quality::UpTo720);
    assert_eq!(settings.audio_format, AudioFormat::M4a);
    assert_eq!(settings.simultaneous_downloads, 4, "clamped, not honoured");
    assert_eq!(settings.rate_limit, None, "yt-dlp would reject it");
    assert_eq!(settings.cookies_from_browser, None, "not a browser");
    assert_eq!(
        settings.window_width,
        Settings::default().window_width,
        "an unknown key did not lose the rest of the file"
    );

    // Loading never writes, so the user's hand-edited file is still theirs.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("a-setting-from-the-future"));
}

#[test]
fn asking_about_a_link_never_fetches_more_than_it_has_to() {
    // A 200-video playlist resolved item by item takes minutes and reads as a
    // hang, which is what `--flat-playlist` prevents.
    let playlist = request::info_argv("https://youtube.com/playlist?list=PL1", true);
    assert!(playlist.contains(&"--flat-playlist".to_string()));
    assert!(playlist.contains(&"--yes-playlist".to_string()));

    let single = request::info_argv("https://youtu.be/x", false);
    assert!(single.contains(&"--no-playlist".to_string()));
    assert!(!single.contains(&"--flat-playlist".to_string()));

    // And the metadata call ignores a user's yt-dlp config, so a `--quiet` in it
    // cannot swallow the JSON this parses.
    for args in [playlist, single] {
        assert!(args.contains(&"--ignore-config".to_string()));
    }
}
