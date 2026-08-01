//! The list on disk.
//!
//! One file holds both the queue and the history, because a finished download
//! *is* the history entry — same job, later state. The old application had a
//! SQLite `history` table it created at startup and never once read or wrote,
//! and a queue that lived in renderer memory, so closing the window lost
//! everything including the record of what had already been downloaded.
//!
//! There is a cap, because this file is read on every start and a list nobody
//! ever clears would grow until that took a noticeable moment.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::job::Job;
use super::store::{self, Outcome};

/// Finished jobs kept before the oldest are dropped.
///
/// Two thousand rows of this shape is a few hundred kilobytes, which is nothing
/// to read at startup, and it is more history than anyone scrolls.
const HISTORY_LIMIT: usize = 2000;

/// The file's contents.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Library {
    /// A version marker, so a future format change can migrate rather than
    /// recover.
    pub version: u32,
    pub jobs: Vec<Job>,
}

impl Library {
    pub fn path_in(data_dir: &Path) -> PathBuf {
        data_dir.join("library.json")
    }

    pub fn load(path: &Path) -> Result<(Self, Outcome), store::Error> {
        store::load(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), store::Error> {
        store::save(path, self)
    }

    /// Replace the list, dropping the oldest finished jobs past the cap.
    ///
    /// Only finished ones are dropped: a queue of three thousand waiting
    /// downloads is unusual but it is not the application's to discard.
    pub fn replace(&mut self, jobs: &[Job]) {
        self.version = 1;
        let finished = jobs.iter().filter(|job| job.state.is_terminal()).count();
        let excess = finished.saturating_sub(HISTORY_LIMIT);

        let mut dropped = 0;
        self.jobs = jobs
            .iter()
            .filter(|job| {
                if job.state.is_terminal() && dropped < excess {
                    dropped += 1;
                    return false;
                }
                true
            })
            .cloned()
            .collect();
    }

    /// Finished jobs whose file matches a query, newest first.
    ///
    /// Present because it is the one thing an MCP tool would want that the
    /// window does not currently show — see DESIGN.md on the deferred
    /// integration. Kept here rather than invented later so the shape of the
    /// data does not have to change to accommodate it.
    pub fn search(&self, query: &str) -> Vec<&Job> {
        let needle = query.trim().to_lowercase();
        let mut found: Vec<&Job> = self
            .jobs
            .iter()
            .filter(|job| job.state.is_terminal())
            .filter(|job| {
                needle.is_empty()
                    || job.title.to_lowercase().contains(&needle)
                    || job.url.to_lowercase().contains(&needle)
            })
            .collect();
        found.sort_by_key(|job| std::cmp::Reverse(job.added));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::job::State;
    use chrono::{Duration, Utc};

    fn job(id: u64, title: &str, state: State) -> Job {
        let mut job = Job::new(
            id,
            format!("https://example.com/{id}"),
            title.to_string(),
            PathBuf::from("/videos"),
        );
        job.state = state;
        job.added = Utc::now() - Duration::seconds(id as i64);
        job
    }

    #[test]
    fn the_queue_survives_a_restart() {
        // The thing the old application could not do. A download still waiting
        // when the window closed simply ceased to exist.
        let dir = tempfile::tempdir().unwrap();
        let path = Library::path_in(dir.path());

        let jobs = vec![
            job(1, "Finished", State::Done),
            job(2, "Still waiting", State::Waiting),
        ];
        let mut library = Library::default();
        library.replace(&jobs);
        library.save(&path).expect("saves");

        let (reopened, outcome) = Library::load(&path).expect("loads");
        assert_eq!(outcome, Outcome::Loaded);
        assert_eq!(reopened.jobs.len(), 2);
        assert_eq!(reopened.jobs[1].title, "Still waiting");
        assert_eq!(reopened.jobs[1].state, State::Waiting);
    }

    #[test]
    fn a_first_run_has_an_empty_library_and_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = Library::path_in(dir.path());
        let (library, outcome) = Library::load(&path).expect("defaults");
        assert_eq!(outcome, Outcome::Fresh);
        assert!(library.jobs.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn the_history_is_capped_by_dropping_the_oldest_finished_first() {
        let mut jobs: Vec<Job> = (0..HISTORY_LIMIT as u64 + 10)
            .map(|id| job(id + 1, "old", State::Done))
            .collect();
        jobs.push(job(99_999, "waiting", State::Waiting));

        let mut library = Library::default();
        library.replace(&jobs);

        assert_eq!(library.jobs.len(), HISTORY_LIMIT + 1);
        assert_eq!(
            library.jobs.last().unwrap().title,
            "waiting",
            "an unfinished job is never dropped to make room"
        );
    }

    #[test]
    fn a_queue_longer_than_the_cap_is_not_trimmed() {
        // The cap is about history, not about the user's intentions.
        let jobs: Vec<Job> = (0..HISTORY_LIMIT as u64 + 50)
            .map(|id| job(id + 1, "waiting", State::Waiting))
            .collect();
        let mut library = Library::default();
        library.replace(&jobs);
        assert_eq!(library.jobs.len(), jobs.len());
    }

    #[test]
    fn searching_finds_finished_downloads_newest_first() {
        let mut library = Library::default();
        library.replace(&[
            job(1, "Bach cantata BWV 4", State::Done),
            job(2, "Bach cantata BWV 8", State::Done),
            job(3, "Bach cantata BWV 12", State::Waiting),
        ]);

        let found = library.search("bwv");
        assert_eq!(found.len(), 2, "a queued job is not history yet");
        assert_eq!(found[0].id, 1, "id 1 was added most recently");
        assert!(library.search("mahler").is_empty());
        assert_eq!(
            library.search("  ").len(),
            2,
            "an empty query is everything"
        );
    }
}
