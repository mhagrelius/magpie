//! The whole surface, checked without a display, a network or a tool.
//!
//! These are the sentences an assistant will actually see. A refusal that
//! arrives ten minutes into a download is the failure this file exists to
//! prevent, so most of what is asserted is *when* something is refused and what
//! the refusal says to do next.

use super::*;
use crate::model::diarize;
use crate::model::failure::Failure;
use crate::model::transcript::{Format, Model};

const URL: &str = "https://youtu.be/dQw4w9WgXcQ";

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn ask(line: &str) -> Ask {
    match parse(&args(line)).expect("a command") {
        Command::Transcribe(ask) => ask,
        other => panic!("expected a transcribe, got {other:?}"),
    }
}

fn everything() -> Facilities {
    Facilities {
        ytdlp: true,
        ffmpeg: true,
        whisper: true,
        diarizer: true,
        installers: Installers {
            uv: true,
            ..Installers::default()
        },
    }
}

fn downloads() -> PathBuf {
    PathBuf::from("/home/matty/Downloads")
}

fn planned(line: &str) -> Plan {
    plan(
        &ask(line),
        &Wish::default(),
        &downloads(),
        Path::new("/tmp"),
    )
    .expect("a plan the machine could carry out")
}

fn job_with(state: State, transcript: TranscriptState) -> Job {
    let mut job = planned(&format!("transcribe {URL}")).job(7);
    job.title = "A talk about bees".into();
    job.state = state;
    job.transcript_state = transcript;
    job.outputs = vec![PathBuf::from("/home/matty/Downloads/A talk about bees.m4a")];
    job
}

// --- planning ---------------------------------------------------------------

#[test]
fn a_transcript_downloads_audio_and_asks_for_words() {
    let job = planned(&format!("transcribe {URL}")).job(1);
    assert_eq!(
        job.selection,
        Selection::Audio(AudioFormat::Best),
        "a transcript needs no pictures, and Best is a copy rather than a transcode"
    );
    assert!(job.transcribe.is_some());
    assert_eq!(job.transcript_state, TranscriptState::Waiting);
    assert_eq!(job.destination, downloads());
}

#[test]
fn nothing_said_means_what_preferences_say() {
    // Not a second set of defaults. A user who set SRT and the medium model in
    // the window gets them here too, or the two ways of using Magpie are two
    // products.
    let preference = Wish {
        format: Format::Srt,
        model: Model::Medium,
        language: Some("de".into()),
        diarize: Some(diarize::Wish::default()),
    };
    let plan = plan(
        &ask(&format!("transcribe {URL}")),
        &preference,
        &downloads(),
        Path::new("/tmp"),
    )
    .expect("a plan");
    assert_eq!(plan.wish, preference);
}

#[test]
fn what_is_said_wins_over_what_preferences_say() {
    let preference = Wish {
        format: Format::Vtt,
        diarize: Some(diarize::Wish::default()),
        ..Wish::default()
    };
    let plan = plan(
        &ask(&format!("transcribe {URL} format=text speakers=no")),
        &preference,
        &downloads(),
        Path::new("/tmp"),
    )
    .expect("a plan");

    assert_eq!(plan.wish.format, Format::Text);
    assert!(
        !plan.wish.identifies_speakers(),
        "an explicit speakers=no has to beat a preference that says yes"
    );
}

#[test]
fn a_playlist_is_refused_before_anything_is_downloaded() {
    // Forty videos through whisper is an afternoon of CPU started by one
    // argument, which is why the window does not offer the switch for a
    // playlist either.
    let error = plan(
        &ask("transcribe https://www.youtube.com/playlist?list=PL1"),
        &Wish::default(),
        &downloads(),
        Path::new("/tmp"),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Refused);
    assert!(error.hint.unwrap_or_default().contains("single video"));
}

#[test]
fn a_video_from_inside_a_playlist_is_the_video_that_was_clicked() {
    let plan = planned("transcribe https://www.youtube.com/watch?v=abc&list=PL1");
    assert!(plan.url.contains("v=abc"));
}

#[test]
fn prose_is_not_a_link() {
    let error = plan(
        &ask("transcribe the-bee-talk"),
        &Wish::default(),
        &downloads(),
        Path::new("/tmp"),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::BadUrl);
}

#[test]
fn a_relative_directory_is_relative_to_where_the_command_was_run() {
    // Not to this process's working directory, which for a command handed to a
    // running Magpie is wherever that window was launched from.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("notes")).unwrap();

    let plan = plan(
        &ask(&format!("transcribe {URL} dir=notes")),
        &Wish::default(),
        &downloads(),
        dir.path(),
    )
    .expect("a plan");
    assert_eq!(plan.destination, dir.path().join("notes"));
}

#[test]
fn a_directory_that_is_not_there_is_said_so_rather_than_created() {
    let error = plan(
        &ask(&format!("transcribe {URL} dir=/nowhere/at/all")),
        &Wish::default(),
        &downloads(),
        Path::new("/tmp"),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::BadValue);
    assert!(
        error.message.contains("/nowhere/at/all"),
        "{}",
        error.message
    );
}

#[test]
fn a_tilde_says_that_nothing_here_is_a_shell() {
    // The arguments went to `exec` as written, so `~/Videos` is a directory
    // called `~`. Failing with "there is no directory at ./~/Videos" would send
    // the caller looking for the wrong thing.
    let error = plan(
        &ask(&format!("transcribe {URL} dir=~/Videos")),
        &Wish::default(),
        &downloads(),
        Path::new("/tmp"),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::BadValue);
    assert!(error.message.contains("shell"), "{}", error.message);
}

// --- what the machine has ---------------------------------------------------

#[test]
fn a_missing_tool_is_named_with_the_command_that_would_fix_it() {
    let plan = planned(&format!("transcribe {URL}"));
    let bare = Facilities::default();

    let error = check(&plan, &bare).unwrap_err();
    assert_eq!(error.kind, ErrorKind::ToolMissing);
    assert!(error.message.starts_with("yt-dlp"), "{}", error.message);
    // The advice names something the user can actually run. With no uv and no
    // pipx that is apt, and it says why that copy may not be enough.
    assert!(error
        .hint
        .unwrap_or_default()
        .contains("apt install yt-dlp"));

    let with_uv = Facilities {
        ytdlp: true,
        installers: Installers {
            uv: true,
            ..Installers::default()
        },
        ..Facilities::default()
    };
    let error = check(&plan, &with_uv).unwrap_err();
    assert!(error.message.starts_with("FFmpeg"), "{}", error.message);
}

#[test]
fn whisper_is_required_and_the_diarizer_only_when_speakers_were_asked_for() {
    let plain = planned(&format!("transcribe {URL}"));
    let speakers = planned(&format!("transcribe {URL} speakers=2"));

    let no_diarizer = Facilities {
        diarizer: false,
        ..everything()
    };
    assert!(check(&plain, &no_diarizer).is_ok());

    let error = check(&speakers, &no_diarizer).unwrap_err();
    assert_eq!(error.kind, ErrorKind::ToolMissing);
    assert!(
        error.message.starts_with("sherpa-onnx"),
        "{}",
        error.message
    );
    // A way forward that does not involve installing anything, because there
    // is one and the transcript is what was actually wanted.
    assert!(error.hint.unwrap_or_default().contains("speakers=no"));

    let no_whisper = Facilities {
        whisper: false,
        ..everything()
    };
    assert!(check(&plain, &no_whisper).is_err());
    assert!(check(&plain, &everything()).is_ok());
}

#[test]
fn readiness_says_what_is_in_the_way_in_sentences() {
    let ready = readiness(&everything());
    assert!(ready.transcribe && ready.speakers);
    assert!(ready.missing.is_empty());

    let ready = readiness(&Facilities::default());
    assert!(!ready.transcribe && !ready.speakers);
    assert_eq!(ready.missing.len(), 4);
    assert!(ready.missing[0].contains("yt-dlp"), "{:?}", ready.missing);

    // Everything but the diarizer: a transcript can be made, names cannot.
    let ready = readiness(&Facilities {
        diarizer: false,
        ..everything()
    });
    assert!(ready.transcribe);
    assert!(!ready.speakers);
    assert_eq!(ready.missing.len(), 1);
}

// --- how it went ------------------------------------------------------------

#[test]
fn a_job_that_is_still_going_has_no_answer_yet() {
    for state in [State::Waiting, State::Running, State::Paused] {
        assert!(outcome(&job_with(state, TranscriptState::Waiting)).is_none());
    }
    for stage in [
        TranscriptState::Waiting,
        TranscriptState::Converting,
        TranscriptState::Running,
        TranscriptState::Identifying,
    ] {
        assert!(
            outcome(&job_with(State::Done, stage.clone())).is_none(),
            "{stage:?} is a stage, not an answer"
        );
    }
}

#[test]
fn a_finished_transcript_comes_back_with_the_file_it_wrote() {
    let mut job = job_with(
        State::Done,
        TranscriptState::Done(PathBuf::from("/home/matty/Downloads/A talk about bees.txt")),
    );
    job.speakers = Some("2 speakers · Alice, Speaker 2".into());

    let response = outcome(&job).expect("an answer").expect("a good one");
    let rendered = render(&Ok(response));

    assert!(rendered.contains("\"ok\": true"), "{rendered}");
    assert!(
        rendered.contains("\"action\": \"transcribed\""),
        "{rendered}"
    );
    assert!(rendered.contains("A talk about bees.txt"), "{rendered}");
    assert!(rendered.contains("\"state\": \"ready\""), "{rendered}");
    assert!(rendered.contains("Alice"), "{rendered}");
    // The audio is named too: it is on the user's disk, and a caller that does
    // not know that leaves it there forever.
    assert!(rendered.contains("A talk about bees.m4a"), "{rendered}");
}

#[test]
fn a_failed_download_reports_the_cause_and_the_remedy() {
    let job = job_with(
        State::Failed(Failure::SignInRequired),
        TranscriptState::None,
    );
    let error = outcome(&job).expect("an answer").unwrap_err();

    assert_eq!(error.kind, ErrorKind::DownloadFailed);
    assert!(
        error.message.contains("signed-in account"),
        "{}",
        error.message
    );
    // The remedy is a setting, and the guidance is the window's own words for
    // it rather than a second explanation that could disagree.
    assert!(error.hint.unwrap_or_default().contains("cookies"));
}

#[test]
fn audio_without_words_is_a_failure_that_says_where_the_audio_is() {
    // The one outcome most likely to be reported as a success by accident: the
    // download worked, so a caller looking at the download would say it was
    // fine. What was asked for was a transcript.
    let job = job_with(
        State::Done,
        TranscriptState::Failed("whisper wrote no transcript".into()),
    );
    let error = outcome(&job).expect("an answer").unwrap_err();

    assert_eq!(error.kind, ErrorKind::TranscriptFailed);
    assert!(error.message.contains("whisper wrote no transcript"));
    assert!(error
        .hint
        .unwrap_or_default()
        .contains("A talk about bees.m4a"));

    let rendered = render(&Err(error_of(&job)));
    assert!(rendered.contains("\"ok\": false"), "{rendered}");
    assert!(
        rendered.contains("\"error\": \"transcript-failed\""),
        "{rendered}"
    );
}

fn error_of(job: &Job) -> AgentError {
    outcome(job).expect("an answer").unwrap_err()
}

#[test]
fn a_download_that_produced_nothing_to_transcribe_does_not_wait_forever() {
    // yt-dlp exited zero without reporting a file. The transcript never starts,
    // so without this the command would sit on a job that will never move.
    let mut job = job_with(State::Done, TranscriptState::Waiting);
    job.outputs.clear();
    let error = outcome(&job).expect("an answer").unwrap_err();
    assert_eq!(error.kind, ErrorKind::TranscriptFailed);
}

// --- finding one again ------------------------------------------------------

fn history() -> Vec<Job> {
    let mut first = job_with(
        State::Done,
        TranscriptState::Done(PathBuf::from("/videos/bees.txt")),
    );
    first.id = 1;
    first.title = "A talk about bees".into();
    first.url = "https://youtu.be/bees".into();

    let mut second = job_with(State::Done, TranscriptState::None);
    second.id = 2;
    second.title = "A talk about wasps".into();
    second.url = "https://youtu.be/wasps".into();
    second.added = first.added + chrono::Duration::seconds(60);

    vec![first, second]
}

#[test]
fn a_download_is_found_by_its_id_or_by_what_it_is_called() {
    let jobs = history();
    assert_eq!(resolve(&jobs, "2").unwrap(), 2);
    assert_eq!(resolve(&jobs, "wasps").unwrap(), 2);
    assert_eq!(resolve(&jobs, "A talk about bees").unwrap(), 1);
    // By its link, which is what a caller has when the user pasted one.
    assert_eq!(resolve(&jobs, "youtu.be/bees").unwrap(), 1);
}

#[test]
fn a_number_is_an_id_and_not_a_search() {
    // Otherwise `show 2` on a machine with a download called "2024 review"
    // would open something nobody named.
    let mut jobs = history();
    jobs[0].title = "2 ways to keep bees".into();
    assert_eq!(resolve(&jobs, "2").unwrap(), 2);

    let error = resolve(&jobs, "99").unwrap_err();
    assert_eq!(error.kind, ErrorKind::NotFound);
    assert!(error.message.contains("99"));
}

#[test]
fn text_matching_two_downloads_asks_rather_than_guesses() {
    let jobs = history();
    let error = resolve(&jobs, "a talk about").unwrap_err();

    assert_eq!(error.kind, ErrorKind::Ambiguous);
    assert_eq!(error.candidates.len(), 2);
    assert!(error.candidates.iter().any(|c| c.id == 1));
    // Enough context to tell them apart without a second call.
    assert!(error.candidates[0].context.is_some());
}

#[test]
fn an_exact_title_beats_a_partial_match_on_the_same_words() {
    let mut jobs = history();
    jobs[1].title = "A talk about bees, part two".into();
    assert_eq!(
        resolve(&jobs, "A talk about bees").unwrap(),
        1,
        "the title that matches exactly is the one that was meant"
    );
}

#[test]
fn a_list_is_newest_first_and_says_when_it_was_cut_short() {
    let jobs = history();

    let Response::List {
        count,
        matched,
        truncated,
        jobs: shown,
        ..
    } = list(&jobs, None, 20)
    else {
        panic!("a list");
    };
    assert_eq!((count, matched, truncated), (2, 2, false));
    assert_eq!(shown[0].id, 2, "the most recent download comes first");

    let Response::List {
        count,
        matched,
        truncated,
        ..
    } = list(&jobs, None, 1)
    else {
        panic!("a list");
    };
    assert_eq!(
        (count, matched, truncated),
        (1, 2, true),
        "nothing is dropped silently"
    );

    let Response::List { count, .. } = list(&jobs, Some("bees"), 20) else {
        panic!("a list");
    };
    assert_eq!(count, 1);
}

#[test]
fn a_listed_download_carries_where_its_transcript_went() {
    let jobs = history();
    let Response::Show { job } = show(&jobs, "1").expect("a job") else {
        panic!("a show");
    };
    let transcript = job.transcript.expect("one was asked for");
    assert_eq!(transcript.state, "ready");
    assert_eq!(transcript.path.as_deref(), Some("/videos/bees.txt"));
}

// --- how it is printed ------------------------------------------------------

#[test]
fn help_is_text_and_everything_else_is_one_json_object() {
    let help = render(&Ok(Response::Help {
        text: "the whole surface".into(),
    }));
    assert_eq!(
        help, "the whole surface",
        "help is for reading, not parsing"
    );

    let listed = render(&Ok(list(&history(), None, 20)));
    let value: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON");
    assert_eq!(value["ok"], serde_json::json!(true));
    assert_eq!(value["action"], serde_json::json!("list"));
}

#[test]
fn an_error_is_a_kind_to_branch_on_and_a_sentence_to_relay() {
    let error = parse(&args("frobnicate")).unwrap_err();
    let value: serde_json::Value = serde_json::from_str(&render(&Err(error))).expect("valid JSON");

    assert_eq!(value["ok"], serde_json::json!(false));
    assert_eq!(value["error"], serde_json::json!("unknown-verb"));
    assert!(value["message"].as_str().unwrap().contains("transcribe"));
    assert!(value["hint"]
        .as_str()
        .unwrap()
        .contains("magpie agent help"));
}

#[test]
fn describe_carries_every_verb_a_caller_could_generate_a_tool_from() {
    let rendered = render(&Ok(Response::Describe { verbs: help::VERBS }));
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    let verbs = value["verbs"].as_array().expect("an array");

    assert_eq!(verbs.len(), help::VERBS.len());
    for verb in verbs {
        assert!(verb["name"].is_string());
        assert!(verb["usage"].is_string());
        assert!(verb["returns"].is_string());
        assert!(verb["mutates"].is_boolean());
        assert!(
            verb["slow"].is_boolean(),
            "a caller sets its timeout from this"
        );
    }
}
