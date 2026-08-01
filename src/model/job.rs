//! One download, from added to finished, and the words that describe it.
//!
//! A job is the persisted thing: it holds what the user asked for, not what the
//! machine is currently doing about it. Live figures — bytes so far, current
//! speed — belong to the process that is running and die with it, which is why
//! [`Progress`] is not serialised. That split is what lets the queue survive a
//! restart without pretending a dead subprocess is still at 47%.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::failure::Failure;
use super::progress::{self, Meter, Snapshot};
use super::request::{Collection, Cookies, Request, Selection};
use super::transcript;

/// Where a job has got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// In the queue, not started.
    #[default]
    Waiting,
    /// A subprocess is running.
    Running,
    /// Running, but stopped with `SIGSTOP`.
    Paused,
    Done,
    Failed(Failure),
}

impl State {
    pub fn is_terminal(&self) -> bool {
        matches!(self, State::Done | State::Failed(_))
    }

    pub fn is_active(&self) -> bool {
        matches!(self, State::Running | State::Paused)
    }
}

/// Where the transcript has got to, if one was asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptState {
    /// Not asked for.
    #[default]
    None,
    /// Asked for, waiting on the download.
    Waiting,
    /// Converting to 16 kHz mono, before whisper sees it.
    Converting,
    Running,
    Done(PathBuf),
    Failed(String),
}

/// Live figures for a running job. Rebuilt from scratch each time it starts.
///
/// `Clone` because the window is handed a snapshot of it to render rather than a
/// borrow it would have to hold while rebuilding rows.
#[derive(Debug, Default, Clone)]
pub struct Progress {
    pub snapshot: Snapshot,
    pub meter: Meter,
    /// Set while yt-dlp is merging streams or converting audio, when there is
    /// no byte count to report and the bar has to go indeterminate.
    pub postprocessing: Option<String>,
    /// 0.0 to 1.0 while whisper is running.
    pub transcript_fraction: Option<f64>,
}

impl Progress {
    pub fn observe(&mut self, snapshot: Snapshot) {
        self.meter.observe(&snapshot);
        self.snapshot = snapshot;
        self.postprocessing = None;
    }
}

/// A queued download.
///
/// Kebab-case on disk, like every other key in the two JSON files. `library.json`
/// is a plain text file in the user's home directory and someone will eventually
/// edit it; a format that spells two of its keys one way and the rest another is
/// an unkindness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Job {
    pub id: u64,
    pub url: String,
    /// What to call it in the list. From `--dump-json`, or the URL if the
    /// metadata fetch failed and the user added it anyway.
    pub title: String,
    pub thumbnail: Option<String>,
    pub destination: PathBuf,
    pub selection: Selection,
    pub collection: Option<Collection>,
    pub transcribe: Option<transcript::Wish>,
    #[serde(default)]
    pub state: State,
    #[serde(default)]
    pub transcript_state: TranscriptState,
    /// The file yt-dlp produced. Several, for a collection.
    #[serde(default)]
    pub outputs: Vec<PathBuf>,
    pub added: DateTime<Utc>,
}

impl Job {
    pub fn new(id: u64, url: String, title: String, destination: PathBuf) -> Self {
        Self {
            id,
            url,
            title,
            thumbnail: None,
            destination,
            selection: Selection::default(),
            collection: None,
            transcribe: None,
            state: State::Waiting,
            transcript_state: TranscriptState::None,
            outputs: Vec::new(),
            added: Utc::now(),
        }
    }

    /// The yt-dlp invocation for this job, given the settings that are not the
    /// job's own business.
    pub fn request(
        &self,
        cookies: Cookies,
        rate_limit: Option<String>,
        cache_dir: &std::path::Path,
    ) -> Request {
        Request {
            url: self.url.clone(),
            destination: self.destination.clone(),
            selection: self.selection.clone(),
            collection: self.collection.clone(),
            cookies,
            rate_limit,
            filepath_sink: super::request::sink_path(cache_dir, self.id),
        }
    }

    /// The single media file, when there is exactly one. A transcript is only
    /// offered for a single download; transcribing forty playlist items is an
    /// afternoon of CPU nobody asked for by ticking one switch.
    pub fn single_output(&self) -> Option<&PathBuf> {
        match self.outputs.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    pub fn transcript_path(&self) -> Option<&PathBuf> {
        match &self.transcript_state {
            TranscriptState::Done(path) => Some(path),
            _ => None,
        }
    }

    /// Whether a transcript should start now that the download is done.
    pub fn wants_transcript_now(&self) -> bool {
        self.state == State::Done
            && self.transcribe.is_some()
            && matches!(
                self.transcript_state,
                TranscriptState::None | TranscriptState::Waiting
            )
            && self.single_output().is_some()
    }

    /// What the folder is called, for "Saved to Videos".
    pub fn destination_label(&self) -> String {
        let directory = match &self.collection {
            Some(collection) => self.destination.join(&collection.folder),
            None => self.destination.clone(),
        };
        directory
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| directory.display().to_string())
    }

    /// The row's subtitle: one line saying where this job has got to.
    ///
    /// Reads as a sentence rather than a readout. `47% · 3.2MiB/s · ETA 01:12`
    /// is four facts and no meaning; "Downloading · 47% · 1 minute left" is the
    /// two that answer "should I wait?".
    pub fn status_line(&self, progress: Option<&Progress>) -> String {
        match &self.state {
            State::Waiting => match self.transcribe {
                Some(_) => "Waiting · will be transcribed".to_string(),
                None => "Waiting".to_string(),
            },
            State::Paused => "Paused".to_string(),
            State::Failed(failure) => failure.title().to_string(),
            State::Running => self.running_line(progress),
            State::Done => self.done_line(progress),
        }
    }

    fn running_line(&self, progress: Option<&Progress>) -> String {
        let Some(progress) = progress else {
            return "Starting".to_string();
        };

        if let Some(processor) = &progress.postprocessing {
            return match processor.as_str() {
                "Merger" => "Combining video and audio".to_string(),
                "ExtractAudio" => "Converting audio".to_string(),
                _ => "Finishing up".to_string(),
            };
        }

        let mut parts = vec!["Downloading".to_string()];

        if let Some((index, count)) = progress.snapshot.item {
            parts.push(format!("{index} of {count}"));
        }
        if let Some(fraction) = progress.snapshot.fraction() {
            parts.push(format!("{}%", (fraction * 100.0).round()));
        } else if progress.snapshot.downloaded_bytes > 0 {
            // No total, so a percentage would be invented. The bytes so far are
            // at least true.
            parts.push(progress::format_bytes(progress.snapshot.downloaded_bytes));
        }
        if let Some(rate) = progress.meter.bytes_per_second() {
            parts.push(progress::format_speed(rate));
        }
        if let Some(seconds) = progress.meter.seconds_remaining(&progress.snapshot) {
            parts.push(progress::format_remaining(seconds));
        }

        parts.join(" · ")
    }

    fn done_line(&self, progress: Option<&Progress>) -> String {
        let saved = format!("Saved to {}", self.destination_label());
        match &self.transcript_state {
            TranscriptState::None => saved,
            TranscriptState::Waiting => format!("{saved} · transcript queued"),
            TranscriptState::Converting => format!("{saved} · preparing audio"),
            TranscriptState::Running => {
                let percent = progress
                    .and_then(|p| p.transcript_fraction)
                    .map(|f| format!(" {}%", (f * 100.0).round()))
                    .unwrap_or_default();
                format!("{saved} · transcribing{percent}")
            }
            TranscriptState::Done(_) => format!("{saved} · transcript ready"),
            TranscriptState::Failed(_) => format!("{saved} · transcript failed"),
        }
    }

    /// The fraction for the row's progress bar, or `None` for an indeterminate
    /// one.
    pub fn fraction(&self, progress: Option<&Progress>) -> Option<f64> {
        match self.state {
            State::Done => match self.transcript_state {
                TranscriptState::Running => progress.and_then(|p| p.transcript_fraction),
                TranscriptState::Converting => None,
                _ => Some(1.0),
            },
            State::Running | State::Paused => {
                let progress = progress?;
                if progress.postprocessing.is_some() {
                    return None;
                }
                progress.snapshot.fraction()
            }
            State::Waiting | State::Failed(_) => Some(0.0),
        }
    }

    /// Whether a progress bar belongs on this row at all.
    pub fn shows_progress(&self) -> bool {
        self.state.is_active()
            || matches!(
                self.transcript_state,
                TranscriptState::Converting | TranscriptState::Running
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job::new(
            1,
            "https://youtu.be/abc".into(),
            "A talk".into(),
            PathBuf::from("/home/matty/Videos"),
        )
    }

    fn running(snapshot: Snapshot) -> Progress {
        let mut progress = Progress::default();
        // Ten identical samples so the smoothed rate equals the instantaneous
        // one and the assertions can be exact.
        for _ in 0..10 {
            progress.observe(snapshot.clone());
        }
        progress
    }

    #[test]
    fn a_waiting_job_says_whether_a_transcript_is_coming() {
        let mut job = job();
        assert_eq!(job.status_line(None), "Waiting");
        job.transcribe = Some(transcript::Wish::default());
        assert_eq!(job.status_line(None), "Waiting · will be transcribed");
    }

    #[test]
    fn a_running_job_answers_should_i_wait() {
        let mut job = job();
        job.state = State::Running;
        let progress = running(Snapshot {
            status: "downloading".into(),
            downloaded_bytes: 47_000_000,
            total_bytes: Some(100_000_000),
            bytes_per_second: Some(3_200_000.0),
            seconds_remaining: Some(17),
            item: None,
        });
        assert_eq!(
            job.status_line(Some(&progress)),
            "Downloading · 47% · 3.2 MB/s · 17 seconds left"
        );
    }

    #[test]
    fn an_unknown_total_shows_the_bytes_so_far_rather_than_a_made_up_percentage() {
        let mut job = job();
        job.state = State::Running;
        let progress = running(Snapshot {
            downloaded_bytes: 5_000_000,
            total_bytes: None,
            bytes_per_second: Some(1_000_000.0),
            ..Default::default()
        });
        let line = job.status_line(Some(&progress));
        assert!(line.contains("5.0 MB"), "{line}");
        assert!(!line.contains('%'), "{line}");
        assert_eq!(job.fraction(Some(&progress)), None, "an honest bar pulses");
    }

    #[test]
    fn post_processing_is_named_rather_than_left_at_100_percent() {
        // Merging a 4K stream takes a minute with no bytes to report. The old
        // application sat at 100% for that minute and looked stuck.
        let mut job = job();
        job.state = State::Running;
        let mut progress = running(Snapshot {
            downloaded_bytes: 100,
            total_bytes: Some(100),
            ..Default::default()
        });
        progress.postprocessing = Some("Merger".into());

        assert_eq!(
            job.status_line(Some(&progress)),
            "Combining video and audio"
        );
        assert_eq!(job.fraction(Some(&progress)), None);
    }

    #[test]
    fn a_position_in_a_playlist_is_part_of_the_status() {
        let mut job = job();
        job.state = State::Running;
        let progress = running(Snapshot {
            downloaded_bytes: 1,
            total_bytes: Some(2),
            bytes_per_second: Some(1_000.0),
            item: Some((3, 40)),
            ..Default::default()
        });
        assert!(job.status_line(Some(&progress)).contains("3 of 40"));
    }

    #[test]
    fn a_failed_job_shows_the_cause_not_the_word_error() {
        let mut job = job();
        job.state = State::Failed(Failure::SignInRequired);
        assert_eq!(
            job.status_line(None),
            "The site asked for a signed-in account"
        );
    }

    #[test]
    fn a_finished_job_says_where_the_file_went() {
        let mut job = job();
        job.state = State::Done;
        assert_eq!(job.status_line(None), "Saved to Videos");

        job.collection = Some(Collection {
            folder: "Bach cantatas".into(),
            items: vec![],
        });
        assert_eq!(job.status_line(None), "Saved to Bach cantatas");
    }

    #[test]
    fn a_transcript_is_only_offered_for_a_single_file() {
        // Ticking one switch should not start forty whisper runs.
        let mut job = job();
        job.state = State::Done;
        job.transcribe = Some(transcript::Wish::default());

        job.outputs = vec!["/videos/a.mkv".into()];
        assert!(job.wants_transcript_now());

        job.outputs = vec!["/videos/a.mkv".into(), "/videos/b.mkv".into()];
        assert!(!job.wants_transcript_now());
    }

    #[test]
    fn a_transcript_is_not_started_twice() {
        let mut job = job();
        job.state = State::Done;
        job.transcribe = Some(transcript::Wish::default());
        job.outputs = vec!["/videos/a.mkv".into()];

        job.transcript_state = TranscriptState::Running;
        assert!(!job.wants_transcript_now());
        job.transcript_state = TranscriptState::Done("/videos/a.txt".into());
        assert!(!job.wants_transcript_now());
    }

    #[test]
    fn a_finished_row_keeps_a_progress_bar_only_while_a_transcript_runs() {
        let mut job = job();
        job.state = State::Done;
        assert!(!job.shows_progress());

        job.transcript_state = TranscriptState::Running;
        assert!(job.shows_progress());
    }
}
