//! The interface an assistant transcribes a video through.
//!
//! `magpie agent transcribe <url>` downloads a video's audio, runs whisper over
//! it, and prints where the words went. It is the same pipeline the window
//! runs — the same job, the same queue, the same record in `library.json` —
//! reached from a command line instead of from a dialog. That is the whole
//! design: a second implementation of transcription would be a second set of
//! bugs, and a transcript made from the command line would not appear in the
//! window that made the last one.
//!
//! **It waits.** A transcript takes minutes, and the honest way to say so is to
//! take minutes and then answer. The alternative — hand back a job id and let
//! the caller poll — sounds cheaper and is not: it would leave the caller
//! guessing when to look, and in a process the user cannot see there is nothing
//! to keep the work alive after the answer. Progress goes to stderr as it
//! happens; stdout carries one JSON object and nothing else.
//!
//! **It refuses before it starts, not after.** A playlist link, a missing
//! whisper, a directory that is not there: each is answered in the first second
//! rather than ten minutes into a download. Every one of those refusals is
//! decided in this module, which is why the sentences a caller will actually
//! see are checkable with no display, no network and a machine with nothing
//! installed.
//!
//! **It never claims more than happened.** A download that worked and a
//! transcript that did not is a failure carrying the path of the audio it left
//! behind, not a success with a missing field.
//!
//! Nothing here spawns anything. It asks the filesystem two things — whether a
//! directory exists, and how big a file is — because both are part of the
//! answer rather than part of the doing, and the clock one, when a job it built
//! records the moment it was added.

pub mod command;
pub mod help;
pub mod view;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::job::{Job, State, TranscriptState};
use crate::model::quality::AudioFormat;
use crate::model::request::Selection;
use crate::model::tools::{Installers, Tool};
use crate::model::transcript::Wish;
use crate::model::url;

pub use command::{parse, Ask, Command};
pub use view::{JobView, Ready, Response};

/// The kind of a failure, as a stable string a caller can branch on.
///
/// Separate from the message because the two have different jobs: the kind is
/// for code deciding what to do next, the message is for the model deciding
/// what to say. Neither reads well doing the other's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    UnknownVerb,
    MissingArgument,
    UnknownField,
    BadValue,
    /// Not something yt-dlp could be given.
    BadUrl,
    NotFound,
    /// The reference fits more than one download. `candidates` says which.
    Ambiguous,
    /// A program Magpie would have to run is not installed.
    ToolMissing,
    /// yt-dlp could not fetch the video. The message is the cause, in the words
    /// the window's own error dialog uses.
    DownloadFailed,
    /// The audio arrived and the words did not.
    TranscriptFailed,
    /// The job went away while it was being waited on — cancelled from the
    /// window, or removed.
    Cancelled,
    /// Understood, and not allowed.
    Refused,
}

/// One of the things a reference could have meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub id: u64,
    pub name: String,
    /// Where it got to, so two downloads of the same talk can be told apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Why a command did not run, or did not finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentError {
    #[serde(rename = "error")]
    pub kind: ErrorKind,
    /// A whole sentence. This is the part a model reads, so it says what was
    /// wrong rather than naming the rule that was broken.
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// What to do about it, when there is a specific answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl AgentError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            candidates: Vec::new(),
            hint: None,
        }
    }

    pub fn hinted(kind: ErrorKind, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            hint: Some(hint.into()),
            ..Self::new(kind, message)
        }
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AgentError {}

/// What this machine can do, as the surface needs to know it.
///
/// A handful of answers rather than `ui::ToolReport`, so that every refusal —
/// and every sentence explaining one — is decided here and testable on a
/// machine with nothing installed, which is the state worth being sure about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facilities {
    pub ytdlp: bool,
    pub ffmpeg: bool,
    pub whisper: bool,
    pub diarizer: bool,
    /// What the user has to install things with, so the advice can name one
    /// they already have.
    pub installers: Installers,
}

impl Facilities {
    fn has(&self, tool: Tool) -> bool {
        match tool {
            Tool::YtDlp => self.ytdlp,
            Tool::Ffmpeg | Tool::Ffprobe => self.ffmpeg,
            Tool::Whisper => self.whisper,
            Tool::Diarizer => self.diarizer,
            // Not needed for a transcript, and never a reason to refuse one.
            Tool::JsRuntime => true,
        }
    }

    fn absent(&self, tool: Tool) -> AgentError {
        AgentError::hinted(
            ErrorKind::ToolMissing,
            format!("{} is not installed, and {}", tool.label(), why(tool)),
            tool.install_hint(self.installers),
        )
    }
}

/// What each tool is for, in a sentence that completes "X is not installed,
/// and …".
fn why(tool: Tool) -> &'static str {
    match tool {
        Tool::YtDlp => "every download goes through it.",
        Tool::Ffmpeg | Tool::Ffprobe => {
            "the audio has to be converted to 16 kHz mono before whisper will read it."
        }
        Tool::Whisper => "it is what turns the audio into words.",
        Tool::Diarizer => "it is what works out who is speaking.",
        Tool::JsRuntime => "YouTube needs one to reveal every format.",
    }
}

/// The tools a transcript needs, in the order they would be used.
///
/// FFmpeg is on the list unconditionally, unlike in the window: an agent
/// transcript downloads audio in whatever format the site serves, which is
/// almost never one whisper reads, and asking for speakers forces the
/// conversion even when it is.
const REQUIRED: [Tool; 3] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Whisper];

/// What can be done here right now, for the `tools` verb.
pub fn readiness(facilities: &Facilities) -> Ready {
    let missing: Vec<String> = REQUIRED
        .into_iter()
        .chain(std::iter::once(Tool::Diarizer))
        .filter(|tool| !facilities.has(*tool))
        .map(|tool| {
            let error = facilities.absent(tool);
            match error.hint {
                Some(hint) => format!("{} {hint}", error.message),
                None => error.message,
            }
        })
        .collect();

    Ready {
        transcribe: REQUIRED.into_iter().all(|tool| facilities.has(tool)),
        speakers: REQUIRED
            .into_iter()
            .chain(std::iter::once(Tool::Diarizer))
            .all(|tool| facilities.has(tool)),
        missing,
    }
}

/// A `transcribe` that has been understood.
///
/// The link is one yt-dlp could take, the directory exists, and the wish is the
/// user's preferences with the caller's overrides applied. Everything left is
/// running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub url: String,
    pub destination: PathBuf,
    pub wish: Wish,
}

impl Plan {
    /// The job that will carry this out.
    ///
    /// Audio only, always. A transcript needs no pictures, and fetching the
    /// video would multiply the download for nothing — `transcribe` is not a
    /// way to download a video, and says so. `AudioFormat::Best` is a copy of
    /// whatever the site serves rather than a transcode, so the only conversion
    /// is the one whisper needs.
    pub fn job(&self, id: u64) -> Job {
        let mut job = Job::new(
            id,
            self.url.clone(),
            // Until `--dump-json` answers. A row that says nothing is worse
            // than a row that says the link.
            self.url.clone(),
            self.destination.clone(),
        );
        job.selection = Selection::Audio(AudioFormat::Best);
        job.transcribe = Some(self.wish.clone());
        job.transcript_state = TranscriptState::Waiting;
        job
    }
}

/// Turn a request into a plan, or say why it cannot be one.
///
/// `defaults` is what Preferences → Transcripts says, so an unmentioned option
/// means the same here as it does in the window. `cwd` is the directory the
/// command was run in, which is not this process's when the command was handed
/// to a running Magpie.
pub fn plan(
    ask: &Ask,
    defaults: &Wish,
    download_directory: &Path,
    cwd: &Path,
) -> Result<Plan, AgentError> {
    let link = url::parse(&ask.url).ok_or_else(|| {
        AgentError::hinted(
            ErrorKind::BadUrl,
            format!("`{}` is not a link.", ask.url),
            "Give the whole address, starting with https://.",
        )
    })?;

    if link.kind == url::Kind::Collection {
        return Err(AgentError::hinted(
            ErrorKind::Refused,
            "That link is a playlist or a channel, and this transcribes one video.",
            "Pass the link to a single video. Transcribing a collection is hours of CPU, \
             which is not something to start by accident.",
        ));
    }

    let destination = match &ask.directory {
        None => download_directory.to_path_buf(),
        Some(directory) => resolve_directory(directory, cwd)?,
    };

    let mut wish = defaults.clone();
    if let Some(format) = ask.format {
        wish.format = format;
    }
    if let Some(model) = ask.model {
        wish.model = model;
    }
    if let Some(language) = &ask.language {
        wish.language = language.clone();
    }
    if let Some(speakers) = &ask.speakers {
        wish.diarize = *speakers;
    }

    Ok(Plan {
        url: link.url,
        destination,
        wish,
    })
}

fn resolve_directory(directory: &str, cwd: &Path) -> Result<PathBuf, AgentError> {
    // There is no shell between the caller and here — the arguments went to
    // `exec` as they were written — so a `~` is a directory called `~`.
    if directory.starts_with('~') {
        return Err(AgentError::hinted(
            ErrorKind::BadValue,
            format!("`{directory}` was not expanded, because nothing here is a shell."),
            "Give the whole path, or a path relative to where you ran this.",
        ));
    }

    let path = Path::new(directory);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    if !resolved.is_dir() {
        return Err(AgentError::hinted(
            ErrorKind::BadValue,
            format!("There is no directory at {}.", resolved.display()),
            "Magpie writes into a folder that exists; it does not create one.",
        ));
    }
    Ok(resolved)
}

/// Whether this machine can carry out this plan at all.
///
/// Run before anything is queued, so a missing tool costs a second rather than
/// the length of a download.
pub fn check(plan: &Plan, facilities: &Facilities) -> Result<(), AgentError> {
    for tool in REQUIRED {
        if !facilities.has(tool) {
            return Err(facilities.absent(tool));
        }
    }

    if plan.wish.identifies_speakers() && !facilities.has(Tool::Diarizer) {
        let mut error = facilities.absent(Tool::Diarizer);
        error.hint = Some(format!(
            "{} Or pass speakers=no and take the transcript without names.",
            error.hint.unwrap_or_default()
        ));
        return Err(error);
    }

    Ok(())
}

/// The answer for a job that is being waited on, or `None` while it is still
/// going.
///
/// The whole of "did this work" lives here, in one pure function, because it is
/// the judgement a caller acts on and the one place an over-claim could hide. A
/// download that succeeded without producing words is a failure that names the
/// audio it left behind — not a success with an absent field, which is how a
/// caller ends up telling someone their transcript is ready when it is not.
pub fn outcome(job: &Job) -> Option<Result<Response, AgentError>> {
    match &job.state {
        State::Waiting | State::Running | State::Paused => None,

        State::Failed(failure) => Some(Err(AgentError::hinted(
            ErrorKind::DownloadFailed,
            format!("{}.", failure.title()),
            failure.guidance(),
        ))),

        State::Done => match &job.transcript_state {
            // yt-dlp exited happily without reporting a file, which is the one
            // way a finished download can leave nothing to transcribe.
            TranscriptState::Waiting if job.single_output().is_none() => {
                Some(Err(AgentError::new(
                    ErrorKind::TranscriptFailed,
                    "yt-dlp finished without producing a single audio file, so there was \
                     nothing to transcribe.",
                )))
            }
            TranscriptState::Waiting
            | TranscriptState::Converting
            | TranscriptState::Running
            | TranscriptState::Identifying => None,

            TranscriptState::Done(_) => Some(Ok(Response::Transcribed {
                job: JobView::of(job),
            })),

            TranscriptState::Failed(reason) => Some(Err(kept_the_audio(
                job,
                format!("The audio downloaded, but there is no transcript: {reason}."),
            ))),
            // Only reachable if the transcript was cancelled from the window
            // while this was waiting on it.
            TranscriptState::None => Some(Err(kept_the_audio(
                job,
                "The audio downloaded and the transcript was stopped before it finished."
                    .to_string(),
            ))),
        },
    }
}

/// A transcript failure that says where the audio is.
///
/// The file is on the user's disk either way, and a caller that does not know
/// that either leaves it there forever or tells the user nothing happened.
fn kept_the_audio(job: &Job, message: String) -> AgentError {
    match job.single_output() {
        Some(path) => AgentError::hinted(
            ErrorKind::TranscriptFailed,
            message,
            format!("The audio is at {}.", path.display()),
        ),
        None => AgentError::new(ErrorKind::TranscriptFailed, message),
    }
}

/// Downloads matching some text, newest first.
pub fn list(jobs: &[Job], query: Option<&str>, limit: usize) -> Response {
    let needle = query.unwrap_or_default().trim().to_lowercase();
    let mut matched: Vec<&Job> = jobs
        .iter()
        .filter(|job| {
            needle.is_empty()
                || job.title.to_lowercase().contains(&needle)
                || job.url.to_lowercase().contains(&needle)
        })
        .collect();
    // Newest first, because the thing just made is the thing being asked
    // about. Ties broken by id so two calls can be compared.
    matched.sort_by_key(|job| (std::cmp::Reverse(job.added), std::cmp::Reverse(job.id)));

    let total = matched.len();
    let shown: Vec<JobView> = matched
        .iter()
        .take(limit)
        .map(|job| JobView::of(job))
        .collect();

    Response::List {
        query: query.map(str::to_string),
        count: shown.len(),
        matched: total,
        truncated: total > shown.len(),
        jobs: shown,
    }
}

/// One download in full.
pub fn show(jobs: &[Job], reference: &str) -> Result<Response, AgentError> {
    let id = resolve(jobs, reference)?;
    let job = jobs.iter().find(|job| job.id == id).expect("just resolved");
    Ok(Response::Show {
        job: JobView::of(job),
    })
}

/// Find the download someone meant.
///
/// A number is an id and nothing else — a download called "2024 review" should
/// not be found by asking for `2024` when there is a job numbered 2024 — and
/// anything else is matched against the title first, then the link.
pub fn resolve(jobs: &[Job], reference: &str) -> Result<u64, AgentError> {
    let wanted = reference.trim();

    if let Ok(id) = wanted.parse::<u64>() {
        return jobs
            .iter()
            .find(|job| job.id == id)
            .map(|job| job.id)
            .ok_or_else(|| {
                AgentError::hinted(
                    ErrorKind::NotFound,
                    format!("There is no download numbered {id}."),
                    "`magpie agent list` shows what there is.",
                )
            });
    }

    let exact: Vec<&Job> = jobs
        .iter()
        .filter(|job| job.title.trim().eq_ignore_ascii_case(wanted))
        .collect();
    if !exact.is_empty() {
        return pick(wanted, &exact);
    }

    let lowered = wanted.to_lowercase();
    let partial: Vec<&Job> = jobs
        .iter()
        .filter(|job| {
            job.title.to_lowercase().contains(&lowered) || job.url.to_lowercase().contains(&lowered)
        })
        .collect();
    if !partial.is_empty() {
        return pick(wanted, &partial);
    }

    Err(AgentError::hinted(
        ErrorKind::NotFound,
        format!("No download matches `{wanted}`."),
        "`magpie agent list` shows what there is, with ids.",
    ))
}

fn pick(wanted: &str, matches: &[&Job]) -> Result<u64, AgentError> {
    if let [only] = matches {
        return Ok(only.id);
    }
    Err(AgentError {
        kind: ErrorKind::Ambiguous,
        message: format!(
            "`{wanted}` matches {} downloads. Name one by its id.",
            matches.len()
        ),
        candidates: matches
            .iter()
            .map(|job| Candidate {
                id: job.id,
                name: job.title.clone(),
                context: Some(job.status_line(None)),
            })
            .collect(),
        hint: None,
    })
}

/// Render a result the way the command line prints it.
///
/// Help is text; everything else is one JSON object carrying `ok`, so a caller
/// that only reads stdout can tell success from failure without the exit status
/// — and one that only reads the exit status does not have to parse anything.
pub fn render(result: &Result<Response, AgentError>) -> String {
    #[derive(Serialize)]
    struct Envelope<'a, T: Serialize> {
        ok: bool,
        #[serde(flatten)]
        body: &'a T,
    }

    let rendered = match result {
        Ok(Response::Help { text }) => return text.clone(),
        Ok(response) => serde_json::to_string_pretty(&Envelope {
            ok: true,
            body: response,
        }),
        Err(error) => serde_json::to_string_pretty(&Envelope {
            ok: false,
            body: error,
        }),
    };

    // Serialising these cannot fail — they are plain data with no maps keyed by
    // anything but strings — but a panic in a command that has already
    // downloaded something would be the worst possible way to say so.
    rendered.unwrap_or_else(|error| {
        format!(r#"{{"ok": false, "error": "internal", "message": "{error}"}}"#)
    })
}

#[cfg(test)]
mod tests;
