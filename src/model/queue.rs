//! What runs next.
//!
//! The whole file exists to get one thing right that the old application got
//! wrong: it advanced the queue from the *success* handler, so a single private
//! video left the thirty-nine behind it Waiting forever. [`Queue::ready`] is
//! recomputed from the current state of every job, so any terminal outcome —
//! done, failed, cancelled, removed — lets the next one start, and there is no
//! handler that can be forgotten.

use super::job::{Job, State, TranscriptState};

/// Downloads that may run at once.
///
/// One by default. Two connections to the same site are not twice as fast; they
/// are twice as likely to be rate limited, and on a domestic connection the
/// bottleneck is the line rather than the request.
pub const DEFAULT_PARALLELISM: usize = 1;
pub const MAX_PARALLELISM: usize = 4;

#[derive(Debug, Default)]
pub struct Queue {
    jobs: Vec<Job>,
    next_id: u64,
    parallelism: usize,
}

impl Queue {
    pub fn new(parallelism: usize) -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
            parallelism: parallelism.clamp(1, MAX_PARALLELISM),
        }
    }

    /// Rebuild from a persisted list.
    ///
    /// Anything that was running when the process ended comes back as Waiting:
    /// the subprocess is gone, so its 47% is a lie, and yt-dlp will resume from
    /// the `.part` file when the job starts again.
    pub fn restore(jobs: Vec<Job>, parallelism: usize) -> Self {
        let next_id = jobs.iter().map(|job| job.id).max().unwrap_or(0) + 1;
        let jobs = jobs
            .into_iter()
            .map(|mut job| {
                if job.state.is_active() {
                    job.state = State::Waiting;
                }
                // A transcript made before Magpie kept a list of them lives only
                // in the state. Without this the row offers to transcribe a file
                // that already has words beside it.
                if let TranscriptState::Done(path) = &job.transcript_state {
                    if job.transcripts.is_empty() {
                        job.transcripts.push(path.clone());
                    }
                }
                job
            })
            .collect();
        Self {
            jobs,
            next_id,
            parallelism: parallelism.clamp(1, MAX_PARALLELISM),
        }
    }

    pub fn set_parallelism(&mut self, parallelism: usize) {
        self.parallelism = parallelism.clamp(1, MAX_PARALLELISM);
    }

    /// Take the next id without adding anything, for a job under construction.
    pub fn reserve_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add(&mut self, job: Job) {
        self.next_id = self.next_id.max(job.id + 1);
        self.jobs.push(job);
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    pub fn remove(&mut self, id: u64) -> Option<Job> {
        let index = self.jobs.iter().position(|job| job.id == id)?;
        Some(self.jobs.remove(index))
    }

    /// Drop everything that has finished, successfully or not.
    pub fn clear_finished(&mut self) -> Vec<Job> {
        let (finished, rest) = std::mem::take(&mut self.jobs)
            .into_iter()
            .partition(|job| job.state.is_terminal());
        self.jobs = rest;
        finished
    }

    pub fn has_finished(&self) -> bool {
        self.jobs.iter().any(|job| job.state.is_terminal())
    }

    /// Jobs currently holding a subprocess, paused ones included.
    ///
    /// A paused job still owns its process and its socket, so it counts against
    /// the limit. Letting a pause start another download would mean pausing
    /// three jobs to get three running.
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|job| job.state.is_active()).count()
    }

    /// The ids that should be started now, oldest first.
    ///
    /// Computed from state rather than pushed by whoever finished, which is the
    /// fix for the stall.
    pub fn ready(&self) -> Vec<u64> {
        let free = self.parallelism.saturating_sub(self.active_count());
        self.jobs
            .iter()
            .filter(|job| job.state == State::Waiting)
            .take(free)
            .map(|job| job.id)
            .collect()
    }

    /// A one-line summary for the window subtitle, or `None` when nothing is
    /// happening and the title should stand alone.
    pub fn summary(&self) -> Option<String> {
        let active = self.active_count();
        let waiting = self
            .jobs
            .iter()
            .filter(|job| job.state == State::Waiting)
            .count();

        match (active, waiting) {
            (0, 0) => None,
            (active, 0) => Some(plural(active, "download", "downloads")),
            (0, waiting) => Some(format!(
                "{} waiting",
                plural(waiting, "download", "downloads")
            )),
            (active, waiting) => Some(format!("{active} downloading · {waiting} waiting")),
        }
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::failure::Failure;
    use std::path::PathBuf;

    fn queue(count: usize, parallelism: usize) -> Queue {
        let mut queue = Queue::new(parallelism);
        for index in 0..count {
            let id = queue.reserve_id();
            queue.add(Job::new(
                id,
                format!("https://example.com/{index}"),
                format!("Item {index}"),
                PathBuf::from("/videos"),
            ));
        }
        queue
    }

    #[test]
    fn a_failed_job_does_not_strand_the_rest_of_the_playlist() {
        // The bug this file exists for. The old queue advanced from the success
        // handler only, so one private video in a forty-item playlist left the
        // remaining thirty-nine Pending until the app was restarted.
        let mut queue = queue(3, 1);

        let first = queue.ready()[0];
        queue.get_mut(first).unwrap().state = State::Running;
        assert!(queue.ready().is_empty(), "the limit is one");

        queue.get_mut(first).unwrap().state = State::Failed(Failure::Unavailable);
        assert_eq!(queue.ready().len(), 1, "the next one starts anyway");
    }

    #[test]
    fn cancelling_by_removal_also_lets_the_next_one_start() {
        let mut queue = queue(2, 1);
        let first = queue.ready()[0];
        queue.get_mut(first).unwrap().state = State::Running;
        queue.remove(first);
        assert_eq!(queue.ready().len(), 1);
    }

    #[test]
    fn a_paused_job_keeps_its_slot() {
        // It still owns a subprocess and a socket. Handing its slot to another
        // download would mean pausing three to get three running.
        let mut queue = queue(2, 1);
        let first = queue.ready()[0];
        queue.get_mut(first).unwrap().state = State::Paused;
        assert!(queue.ready().is_empty());
    }

    #[test]
    fn the_limit_is_respected_and_capped() {
        let mut queue = queue(10, 3);
        assert_eq!(queue.ready().len(), 3);

        queue.set_parallelism(99);
        assert_eq!(queue.ready().len(), MAX_PARALLELISM);
        queue.set_parallelism(0);
        assert_eq!(queue.ready().len(), 1, "zero would be a stalled queue");
    }

    #[test]
    fn jobs_start_in_the_order_they_were_added() {
        let queue = queue(3, 2);
        assert_eq!(queue.ready(), vec![1, 2]);
    }

    #[test]
    fn a_restored_running_job_comes_back_as_waiting() {
        // Its subprocess died with the last session; reporting 47% would be a
        // number about nothing. yt-dlp resumes from the .part file.
        let mut queue = queue(2, 1);
        queue.get_mut(1).unwrap().state = State::Running;
        queue.get_mut(2).unwrap().state = State::Done;
        let jobs = queue.jobs().to_vec();

        let restored = Queue::restore(jobs, 1);
        assert_eq!(restored.get(1).unwrap().state, State::Waiting);
        assert_eq!(
            restored.get(2).unwrap().state,
            State::Done,
            "done stays done"
        );
    }

    #[test]
    fn a_transcript_made_before_the_list_existed_is_not_offered_again() {
        // The path used to live only in the state. A restored job whose words
        // are already written must not show a Transcribe button.
        let mut queue = queue(1, 1);
        let job = queue.get_mut(1).unwrap();
        job.state = State::Done;
        job.outputs = vec![PathBuf::from("/videos/a.mkv")];
        job.transcript_state = TranscriptState::Done(PathBuf::from("/videos/a.txt"));

        let restored = Queue::restore(queue.jobs().to_vec(), 1);
        let job = restored.get(1).unwrap();
        assert_eq!(job.transcripts, vec![PathBuf::from("/videos/a.txt")]);
        assert!(!job.can_transcribe());
    }

    #[test]
    fn a_restored_queue_does_not_reissue_an_existing_id() {
        let jobs = queue(3, 1).jobs().to_vec();
        let mut restored = Queue::restore(jobs, 1);
        assert_eq!(restored.reserve_id(), 4);
    }

    #[test]
    fn clearing_finished_keeps_what_is_still_going() {
        let mut queue = queue(3, 1);
        queue.get_mut(1).unwrap().state = State::Done;
        queue.get_mut(2).unwrap().state = State::Failed(Failure::Network);
        queue.get_mut(3).unwrap().state = State::Running;

        let cleared = queue.clear_finished();
        assert_eq!(cleared.len(), 2);
        assert_eq!(queue.jobs().len(), 1);
        assert!(!queue.has_finished());
    }

    #[test]
    fn the_summary_counts_what_is_happening() {
        let mut queue = queue(3, 1);
        assert_eq!(queue.summary().as_deref(), Some("3 downloads waiting"));

        queue.get_mut(1).unwrap().state = State::Running;
        assert_eq!(
            queue.summary().as_deref(),
            Some("1 downloading · 2 waiting")
        );

        queue.get_mut(2).unwrap().state = State::Done;
        queue.get_mut(3).unwrap().state = State::Done;
        assert_eq!(queue.summary().as_deref(), Some("1 download"));

        queue.get_mut(1).unwrap().state = State::Done;
        assert_eq!(
            queue.summary(),
            None,
            "nothing to say when nothing is going"
        );
    }
}
