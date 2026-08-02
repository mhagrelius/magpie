//! What comes back.
//!
//! These are not the stored records. A [`Job`] carries a selection, a
//! collection, a queue position and a state machine, none of which mean
//! anything to a caller that asked for a transcript — and it does *not* carry
//! the two things such a caller wants most, which are where the words ended up
//! and how big they are.
//!
//! So a view answers the question instead of describing the record: one sentence
//! of status in the same words the window shows, the file paths as absolute
//! strings, and the sizes read off disk so a caller knows what it is about to
//! read. Ids are still here, because an id is how the next call names this job
//! without ambiguity.
//!
//! Everything empty is left out rather than serialised as `null`. A response
//! that goes into a context window should spend its tokens on what is there.

use std::path::Path;

use serde::Serialize;

use super::help::Verb;
use crate::model::job::{Job, State, TranscriptState};
use crate::model::transcript::{Format, Model};

/// A file Magpie wrote.
#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub path: String,
    /// Absent when the file is no longer where Magpie left it, which is worth
    /// distinguishing from a file that is there and empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl FileView {
    pub fn of(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
            bytes: std::fs::metadata(path).ok().map(|meta| meta.len()),
        }
    }
}

/// Where the words have got to.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptView {
    /// `waiting`, `preparing`, `transcribing`, `identifying`, `ready` or
    /// `failed`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub format: Format,
    pub model: Model,
    /// The language asked for, or absent when whisper detected it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// What the speaker pass found: "2 speakers · Alice, Speaker 2". Absent
    /// when nobody asked who was speaking, or when the words are not written
    /// yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speakers: Option<String>,
    /// Why there is no transcript, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl TranscriptView {
    /// The transcript half of a job, or `None` if none was ever asked for.
    pub fn of(job: &Job) -> Option<Self> {
        let wish = job.transcribe.as_ref()?;
        let file = job.transcript_path().map(|path| FileView::of(path));

        Some(Self {
            state: transcript_state_name(&job.transcript_state),
            path: file.as_ref().map(|file| file.path.clone()),
            bytes: file.as_ref().and_then(|file| file.bytes),
            format: wish.format,
            model: wish.model,
            language: wish.language.clone(),
            speakers: job.speakers.clone(),
            reason: match &job.transcript_state {
                TranscriptState::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
        })
    }
}

/// One download, as a caller that cannot see the window needs it.
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: u64,
    pub title: String,
    pub url: String,
    /// `waiting`, `running`, `paused`, `done` or `failed`.
    pub state: &'static str,
    /// The same sentence the row shows — "Saved to Downloads · 2 speakers ·
    /// Alice, Speaker 2". One line that can be relayed to a person as it
    /// stands.
    pub status: String,
    pub added: String,
    /// The media file, once yt-dlp has reported where it put it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<FileView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<TranscriptView>,
    /// Why the download failed, in the words the dialog would use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

impl JobView {
    pub fn of(job: &Job) -> Self {
        Self {
            id: job.id,
            title: job.title.clone(),
            url: job.url.clone(),
            state: state_name(&job.state),
            status: job.status_line(None),
            added: job.added.to_rfc3339(),
            media: job.single_output().map(|path| FileView::of(path)),
            transcript: TranscriptView::of(job),
            failure: match &job.state {
                State::Failed(failure) => Some(failure.title().to_string()),
                _ => None,
            },
        }
    }
}

/// One program Magpie runs, and whether it is here.
#[derive(Debug, Clone, Serialize)]
pub struct ToolView {
    pub name: &'static str,
    pub purpose: &'static str,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Only yt-dlp has an age worth reporting: its version is a release date,
    /// and being months behind is the single most common cause of a download
    /// failing for reasons the error blames on something else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_days: Option<i64>,
    #[serde(skip_serializing_if = "is_false")]
    pub stale: bool,
    /// What to run to get it, when it is missing. A command for the user to
    /// run, not one to run for them: several of them need a password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

/// A model file, and whether it has been fetched.
#[derive(Debug, Clone, Serialize)]
pub struct ModelView {
    pub name: String,
    pub on_disk: bool,
    /// What it weighs: the file on disk if there is one, otherwise what the
    /// download will cost.
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
}

/// What can actually be done on this machine right now.
#[derive(Debug, Clone, Serialize)]
pub struct Ready {
    pub transcribe: bool,
    pub speakers: bool,
    /// A sentence per thing standing in the way. Empty when both are true.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

/// Everything a verb can answer with.
///
/// Internally tagged, so every response says which verb produced it. A caller
/// reading back a transcript of several calls can tell them apart without
/// tracking what it asked for.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Response {
    /// Plain text, printed as-is rather than as JSON.
    Help {
        #[serde(skip)]
        text: String,
    },
    Describe {
        verbs: &'static [Verb],
    },
    Tools {
        tools: Vec<ToolView>,
        /// The whisper models, and which of them have been downloaded.
        speech_models: Vec<ModelView>,
        /// The two speaker models, reported as the single thing they are: one
        /// without the other is useless.
        speaker_models: ModelView,
        ready: Ready,
    },
    /// A transcript that exists, because this verb waits for it.
    Transcribed {
        job: JobView,
    },
    List {
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// How many came back.
        count: usize,
        /// How many matched. Differs from `count` only when the limit cut the
        /// list short, which is what `truncated` announces.
        matched: usize,
        #[serde(skip_serializing_if = "is_false")]
        truncated: bool,
        jobs: Vec<JobView>,
    },
    Show {
        job: JobView,
    },
}

fn state_name(state: &State) -> &'static str {
    match state {
        State::Waiting => "waiting",
        State::Running => "running",
        State::Paused => "paused",
        State::Done => "done",
        State::Failed(_) => "failed",
    }
}

/// The transcript states, named for what is happening rather than for the
/// variant. `converting` is an ffmpeg detail; `preparing` is what it is for.
fn transcript_state_name(state: &TranscriptState) -> &'static str {
    match state {
        TranscriptState::None => "none",
        TranscriptState::Waiting => "waiting",
        TranscriptState::Converting => "preparing",
        TranscriptState::Running => "transcribing",
        TranscriptState::Identifying => "identifying",
        TranscriptState::Done(_) => "ready",
        TranscriptState::Failed(_) => "failed",
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}
