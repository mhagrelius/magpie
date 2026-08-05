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

use super::collection::{self, Item};
use super::failure::Failure;
use super::progress::{self, Meter, Pace, Snapshot};
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
    /// The words exist; working out who said them.
    ///
    /// A state of its own rather than more `Running`, because it is a second
    /// program with its own progress and its own way of failing, and because
    /// "transcribing 100%" sitting still for a minute is the bug this avoids.
    Identifying,
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
    /// How long the current stage has taken to get as far as it has. Fed by the
    /// application, which is the half that owns a clock.
    pub pace: Pace,
    /// When the current stage began, so elapsed time can be measured against it.
    /// Cleared and re-stamped as the job moves from download to transcript.
    pub started: Option<std::time::Instant>,
}

impl Progress {
    pub fn observe(&mut self, snapshot: Snapshot) {
        self.meter.observe(&snapshot);
        self.snapshot = snapshot;
        self.postprocessing = None;
    }

    /// Start timing a new stage, discarding the last one's readings.
    ///
    /// A download and the transcript that follows it move at unrelated speeds,
    /// so carrying the samples across would predict the second from the first.
    pub fn begin_stage(&mut self, now: std::time::Instant) {
        self.started = Some(now);
        self.pace = Pace::default();
    }

    /// How long the current stage has been going.
    pub fn elapsed(&self, now: std::time::Instant) -> std::time::Duration {
        match self.started {
            Some(started) => now.saturating_duration_since(started),
            None => std::time::Duration::ZERO,
        }
    }

    /// Note the time passing without a new reading, so a stage that reports
    /// nothing for minutes still shows the minutes.
    pub fn tick(&mut self, now: std::time::Instant) {
        let elapsed = self.elapsed(now);
        self.pace.tick(elapsed);
    }

    /// Record how far along the current stage is.
    pub fn advance(&mut self, now: std::time::Instant, fraction: f64) {
        if self.started.is_none() {
            self.started = Some(now);
        }
        let elapsed = self.elapsed(now);
        self.pace.observe(elapsed, fraction);
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
    /// What is in the collection, for the expanded row: the items that were
    /// asked for, in playlist order.
    ///
    /// Remembered rather than re-fetched, because a hundred-item playlist takes
    /// a second to list and the row would otherwise ask for it again on every
    /// redraw. `default` so a `library.json` written before this existed loads;
    /// a job that has none still expands, from the files that have landed. See
    /// [`super::collection`].
    #[serde(default)]
    pub items: Vec<Item>,
    pub transcribe: Option<transcript::Wish>,
    #[serde(default)]
    pub state: State,
    #[serde(default)]
    pub transcript_state: TranscriptState,
    /// The file yt-dlp produced. Several, for a collection.
    #[serde(default)]
    pub outputs: Vec<PathBuf>,
    /// The transcripts written so far, one per output that has one.
    ///
    /// A list rather than a single path because a playlist is transcribed item
    /// by item, and the pass has to survive being stopped, quit and resumed the
    /// next day. Matched back to their media by filename, which is safe because
    /// a transcript is written as the media file with a different extension.
    #[serde(default)]
    pub transcripts: Vec<PathBuf>,
    /// Media files whisper could not do anything with.
    ///
    /// Recorded so the pass moves on. Without it, one item of silence or of
    /// audio the model refuses is retried for ever and the other hundred and
    /// six never start — the same shape of bug as the queue's own
    /// advance-on-any-outcome rule exists to prevent.
    #[serde(default)]
    pub transcript_failures: Vec<PathBuf>,
    /// What the speaker pass found, once it has run — "3 speakers · Alice,
    /// Speaker 2, Speaker 3".
    ///
    /// Kept on the job rather than recomputed, because the answer lives in a file
    /// Magpie has already finished writing and re-reading it to redraw a row
    /// would be reading a transcript off disk sixty times a second.
    #[serde(default)]
    pub speakers: Option<String>,
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
            items: Vec::new(),
            transcribe: None,
            state: State::Waiting,
            transcript_state: TranscriptState::None,
            outputs: Vec::new(),
            transcripts: Vec::new(),
            transcript_failures: Vec::new(),
            speakers: None,
            added: Utc::now(),
        }
    }

    /// The yt-dlp invocation for this job, given the settings that are not the
    /// job's own business.
    pub fn request(
        &self,
        cookies: Cookies,
        rate_limit: Option<String>,
        js_runtime: Option<PathBuf>,
        cache_dir: &std::path::Path,
    ) -> Request {
        Request {
            url: self.url.clone(),
            destination: self.destination.clone(),
            selection: self.selection.clone(),
            collection: self.collection.clone(),
            cookies,
            rate_limit,
            js_runtime,
            filepath_sink: super::request::sink_path(cache_dir, self.id),
        }
    }

    /// The single media file, when there is exactly one.
    pub fn single_output(&self) -> Option<&PathBuf> {
        match self.outputs.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The one transcript a single download produced.
    ///
    /// A collection has as many transcripts as items, which is why nothing here
    /// answers for one: its row offers them item by item instead.
    pub fn transcript_path(&self) -> Option<&PathBuf> {
        match (&self.transcript_state, &self.collection) {
            (TranscriptState::Done(path), None) => Some(path),
            _ => None,
        }
    }

    /// The transcript beside this media file, if one has been written.
    ///
    /// Matched by name rather than kept in a map: whisper writes the transcript
    /// as the media file with a different extension, so the two names differ in
    /// nothing else, and a map would be a second thing to keep in step.
    pub fn transcript_for(&self, media: &std::path::Path) -> Option<&PathBuf> {
        self.transcripts
            .iter()
            .find(|path| path.parent() == media.parent() && path.file_stem() == media.file_stem())
    }

    /// Downloaded files with no transcript beside them and no failed attempt.
    pub fn untranscribed(&self) -> impl Iterator<Item = &PathBuf> {
        self.outputs.iter().filter(|media| {
            self.transcript_for(media).is_none() && !self.transcript_failures.contains(media)
        })
    }

    /// The next file to hand to whisper.
    pub fn next_untranscribed(&self) -> Option<&PathBuf> {
        self.transcribe.as_ref()?;
        self.untranscribed().next()
    }

    /// Whether a transcript pass is under way right now.
    pub fn transcript_is_running(&self) -> bool {
        matches!(
            self.transcript_state,
            TranscriptState::Converting | TranscriptState::Running | TranscriptState::Identifying
        )
    }

    /// Whether asking for a transcript would do anything.
    ///
    /// The answer to "I downloaded this last week and now I want the words" —
    /// and, for a playlist, to "I have the hundred and seven files and none of
    /// the transcripts". Nothing about it depends on what was asked for at the
    /// time.
    pub fn can_transcribe(&self) -> bool {
        self.state == State::Done
            && !self.transcript_is_running()
            && self.untranscribed().next().is_some()
    }

    /// Whether a transcript should start now that the download is done.
    pub fn wants_transcript_now(&self) -> bool {
        self.state == State::Done
            && matches!(
                self.transcript_state,
                TranscriptState::None | TranscriptState::Waiting
            )
            && self.next_untranscribed().is_some()
    }

    /// How many of this job's files have transcripts.
    pub fn transcribed_count(&self) -> usize {
        self.outputs
            .iter()
            .filter(|media| self.transcript_for(media).is_some())
            .count()
    }

    /// Whether this job's transcript was assumed rather than asked for.
    ///
    /// "Transcribe by default" is applied when a link is pasted. A link that
    /// looks like a playlist is exempted there, but some do not look like one
    /// until yt-dlp says so — and a collection *can* be transcribed, so this is
    /// not about what is possible. It is about consent: a hundred and seven
    /// whisper runs is a decision, and a switch meant for single videos did not
    /// make it. The Add dialog and the row's Transcribe button are where it is
    /// made instead.
    pub fn transcript_was_presumed(&self) -> bool {
        self.transcribe.is_some()
            && self.collection.is_some()
            && self.transcripts.is_empty()
            && matches!(
                self.transcript_state,
                TranscriptState::None | TranscriptState::Waiting
            )
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

        if self.collection.is_some() || progress.snapshot.item.is_some() {
            return self.collection_line(progress);
        }

        let mut parts = vec!["Downloading".to_string()];

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

    /// A collection's line, which is about the collection.
    ///
    /// The per-item figures are the wrong scale here. `8 of 107 · 100% · Almost
    /// done` was true of the eighth file and quite false about the afternoon,
    /// and it is the afternoon the reader is asking about. So the time left is
    /// measured from how long the items so far have taken, and the percentage —
    /// which would only repeat `8 of 107` in another notation — is left to the
    /// bar. What the current file is doing has its own line, in the expanded
    /// view.
    fn collection_line(&self, progress: &Progress) -> String {
        let mut parts = match collection::position(self, Some(progress)) {
            Some((position, count)) => vec![format!("Downloading {position} of {count}")],
            None => vec!["Downloading".to_string()],
        };

        if let Some(rate) = progress.meter.bytes_per_second() {
            parts.push(progress::format_speed(rate));
        }
        // Only once enough items have gone by to mean anything; before that the
        // count is the honest answer on its own.
        if let Some(seconds) = progress.pace.seconds_remaining() {
            parts.push(progress::format_remaining(seconds));
        }
        parts.join(" · ")
    }

    fn done_line(&self, progress: Option<&Progress>) -> String {
        let saved = format!("Saved to {}", self.destination_label());
        // A collection's folder is named after the playlist, so "Saved to Bach —
        // the complete cantatas" repeats the title directly above it and pushes
        // the part that is actually moving off the end of the line. While a pass
        // is running the stage leads instead; where the files went has not
        // changed and the folder button still opens it.
        let stage = |verb: &str, fraction: Option<f64>| match self.collection {
            Some(_) => capitalise(&stage_line(&self.transcript_verb(verb), fraction, progress)),
            None => format!(
                "{saved} · {}",
                stage_line(&self.transcript_verb(verb), fraction, progress)
            ),
        };

        match &self.transcript_state {
            TranscriptState::None => saved,
            TranscriptState::Waiting => format!("{saved} · transcript queued"),
            TranscriptState::Converting => stage("preparing audio", None),
            TranscriptState::Running => stage("transcribing", self.stage_fraction(progress)),
            TranscriptState::Identifying => {
                stage("identifying speakers", self.stage_fraction(progress))
            }
            // The count is the answer the user asked for, so it belongs on the
            // row rather than in a toast they may have missed.
            TranscriptState::Done(_) => match (&self.collection, &self.speakers) {
                (Some(_), _) => format!("{saved} · {}", self.transcript_tally()),
                (None, Some(speakers)) => format!("{saved} · {speakers}"),
                (None, None) => format!("{saved} · transcript ready"),
            },
            TranscriptState::Failed(_) => format!("{saved} · transcript failed"),
        }
    }

    /// `transcribing` for one video; `transcribing 4 of 107` for a playlist,
    /// where which item it is on is the thing worth knowing.
    fn transcript_verb(&self, verb: &str) -> String {
        if self.collection.is_none() {
            return verb.to_string();
        }
        let position = (self.transcribed_count() + self.transcript_failures.len() + 1)
            .min(self.outputs.len().max(1));
        format!("{verb} {position} of {}", self.outputs.len())
    }

    /// What a finished pass over a collection came to.
    pub fn transcript_tally(&self) -> String {
        let (done, total) = (self.transcribed_count(), self.outputs.len());
        if done == total {
            return match total {
                1 => "transcript ready".to_string(),
                total => format!("{total} transcripts"),
            };
        }
        // Some item whisper could not read. Saying so is the difference between
        // a number that looks wrong and a number that is explained.
        format!("{done} of {total} transcribed")
    }

    /// The percentage on the row while a stage runs.
    ///
    /// Only for a single video. A playlist's line already says which item it is
    /// on, and how far into that one item it is answers nothing about the pass —
    /// the bar and the time left are measured across the whole of it instead.
    fn stage_fraction(&self, progress: Option<&Progress>) -> Option<f64> {
        match self.collection {
            Some(_) => None,
            None => self.transcript_fraction(progress),
        }
    }

    fn transcript_fraction(&self, progress: Option<&Progress>) -> Option<f64> {
        progress.and_then(|progress| progress.transcript_fraction)
    }

    /// The fraction for the row's progress bar, or `None` for an indeterminate
    /// one.
    pub fn fraction(&self, progress: Option<&Progress>) -> Option<f64> {
        match self.state {
            State::Done => match self.transcript_state {
                // A pass over a playlist measures the pass: item four of a
                // hundred and seven reaching 100% is not the bar filling.
                TranscriptState::Converting
                | TranscriptState::Running
                | TranscriptState::Identifying
                    if self.collection.is_some() =>
                {
                    collection::transcript_fraction(self, progress)
                }
                TranscriptState::Running | TranscriptState::Identifying => {
                    progress.and_then(|p| p.transcript_fraction)
                }
                TranscriptState::Converting => None,
                _ => Some(1.0),
            },
            State::Running | State::Paused => {
                let progress = progress?;
                if progress.postprocessing.is_some() {
                    return None;
                }
                // A collection's bar measures the collection. One file reaching
                // 100% is not a hundred and seven of them reaching it.
                if self.collection.is_some() {
                    return collection::fraction(self, Some(progress));
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
                TranscriptState::Converting
                    | TranscriptState::Running
                    | TranscriptState::Identifying
            )
    }
}

/// For a line that leads with the stage rather than following "Saved to X".
fn capitalise(line: &str) -> String {
    let mut characters = line.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// `transcribing 15% · 40 minutes left`, or as much of it as is known yet.
///
/// A stage with no percentage still has a clock. Whisper says nothing at all
/// until its first progress line, and a row reading `transcribing` with no
/// figure beside it for four minutes is indistinguishable from a row that has
/// stopped.
fn stage_line(verb: &str, fraction: Option<f64>, progress: Option<&Progress>) -> String {
    let mut line = match fraction {
        Some(fraction) => format!("{verb} {}%", (fraction * 100.0).round()),
        None => verb.to_string(),
    };

    let timing = progress.and_then(|progress| match progress.pace.seconds_remaining() {
        Some(seconds) => Some(progress::format_remaining(seconds)),
        None => match progress.pace.elapsed().as_secs() {
            0 => None,
            seconds => Some(progress::format_elapsed(seconds)),
        },
    });
    if let Some(timing) = timing {
        line.push_str(" · ");
        line.push_str(&timing);
    }
    line
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
            ..Default::default()
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
    fn a_collection_reports_the_collection_rather_than_the_file_in_hand() {
        // The row this replaces read `Downloading · 8 of 107 · 100% · Almost
        // done`, where the percentage and the time left were the eighth file's.
        // On a hundred-and-seven-item playlist that is an hour of "almost done".
        let mut job = job();
        job.state = State::Running;
        job.collection = Some(Collection {
            folder: "Audiobooks".into(),
            items: Vec::new(),
        });
        job.items = (1..=107)
            .map(|index| Item {
                index,
                title: format!("Chapter {index}"),
                duration: None,
            })
            .collect();
        job.outputs = (1..=7)
            .map(|index| PathBuf::from(format!("/downloads/{index:03} - Chapter {index}.m4a")))
            .collect();

        let progress = running(Snapshot {
            status: "downloading".into(),
            downloaded_bytes: 1000,
            total_bytes: Some(1000),
            bytes_per_second: Some(7_900_000.0),
            seconds_remaining: Some(1),
            item: Some((8, 107)),
            playlist_index: Some(8),
        });

        let line = job.status_line(Some(&progress));
        assert!(line.starts_with("Downloading 8 of 107"), "{line}");
        assert!(!line.contains("Almost done"), "{line}");

        let fraction = job.fraction(Some(&progress)).expect("a fraction");
        assert!((fraction - 8.0 / 107.0).abs() < 0.001, "{fraction}");
    }

    #[test]
    fn a_collection_says_how_long_the_whole_thing_has_left() {
        let mut job = job();
        job.state = State::Running;
        job.collection = Some(Collection {
            folder: "Audiobooks".into(),
            items: Vec::new(),
        });

        let mut progress = running(Snapshot {
            item: Some((3, 12)),
            ..Default::default()
        });
        assert!(
            !job.status_line(Some(&progress)).contains("left"),
            "nothing to go on yet"
        );

        // A quarter of the way in after five minutes: a quarter of an hour left.
        progress
            .pace
            .observe(std::time::Duration::from_secs(0), 0.0);
        progress
            .pace
            .observe(std::time::Duration::from_secs(300), 0.25);
        assert!(
            job.status_line(Some(&progress)).contains("15 minutes left"),
            "{}",
            job.status_line(Some(&progress))
        );
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
    fn a_collection_is_transcribed_one_item_at_a_time() {
        let mut job = job();
        job.state = State::Done;
        job.transcribe = Some(transcript::Wish::default());
        job.outputs = vec![
            "/videos/001 - a.mkv".into(),
            "/videos/002 - b.mkv".into(),
            "/videos/003 - c.mkv".into(),
        ];

        assert_eq!(
            job.next_untranscribed().map(PathBuf::as_path),
            Some(std::path::Path::new("/videos/001 - a.mkv"))
        );

        // The transcript is matched to its media by name, so recording one moves
        // the pass on by exactly one item.
        job.transcripts.push("/videos/001 - a.txt".into());
        assert_eq!(
            job.next_untranscribed().map(PathBuf::as_path),
            Some(std::path::Path::new("/videos/002 - b.mkv"))
        );
        assert_eq!(job.transcribed_count(), 1);

        // One item whisper cannot read must not cost the rest. It is recorded
        // and the pass steps over it.
        job.transcript_failures.push("/videos/002 - b.mkv".into());
        assert_eq!(
            job.next_untranscribed().map(PathBuf::as_path),
            Some(std::path::Path::new("/videos/003 - c.mkv"))
        );

        job.transcripts.push("/videos/003 - c.txt".into());
        assert_eq!(job.next_untranscribed(), None, "nothing left to do");
        assert_eq!(job.transcript_tally(), "2 of 3 transcribed");
    }

    #[test]
    fn a_finished_download_can_be_transcribed_after_the_fact() {
        // The catch-up: files on disk, no words beside them, and nobody asked
        // for a transcript at the time.
        let mut job = job();
        job.state = State::Done;
        job.outputs = vec!["/videos/a.mkv".into()];
        assert!(job.transcribe.is_none());
        assert!(job.can_transcribe());

        job.transcripts.push("/videos/a.txt".into());
        assert!(!job.can_transcribe(), "it already has words");

        // And not while one is being made.
        job.transcripts.clear();
        job.transcript_state = TranscriptState::Running;
        assert!(!job.can_transcribe());
    }

    #[test]
    fn a_transcript_asked_for_before_the_link_turned_out_to_be_a_playlist_is_let_go_of() {
        // "Transcribe everything" is applied when the link is pasted, and the
        // link is only known to be a playlist a second later. The row would
        // otherwise say "transcript queued" for a transcript nothing will ever
        // start.
        let mut job = job();
        job.transcribe = Some(transcript::Wish::default());
        job.transcript_state = TranscriptState::Waiting;
        assert!(!job.transcript_was_presumed(), "one video is fine");

        job.collection = Some(Collection {
            folder: "Audiobooks".into(),
            items: vec![],
        });
        assert!(job.transcript_was_presumed());

        job.transcript_state = TranscriptState::Done("/videos/a.txt".into());
        assert!(
            !job.transcript_was_presumed(),
            "a transcript that exists is not impossible"
        );
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
    fn a_transcript_with_no_percentage_yet_still_shows_it_is_alive() {
        // whisper says nothing until its first progress line, which on a long
        // recording is minutes in. `transcribing` on its own for four minutes
        // reads exactly like `transcribing` on a job that has died.
        let mut job = job();
        job.state = State::Done;
        job.transcript_state = TranscriptState::Running;

        let mut progress = Progress::default();
        progress.pace.tick(std::time::Duration::from_secs(240));
        assert_eq!(
            job.status_line(Some(&progress)),
            "Saved to Videos · transcribing · 4 minutes so far"
        );

        progress.transcript_fraction = Some(0.15);
        progress
            .pace
            .observe(std::time::Duration::from_secs(240), 0.0);
        progress
            .pace
            .observe(std::time::Duration::from_secs(600), 0.15);
        assert_eq!(
            job.status_line(Some(&progress)),
            "Saved to Videos · transcribing 15% · 34 minutes left"
        );
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
