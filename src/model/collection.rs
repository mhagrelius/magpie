//! What a playlist job is made of, and where each of its items has got to.
//!
//! A collection is one job in the queue and one subprocess, but a hundred and
//! seven separate things to the person who queued it. The row that says
//! `Downloading · 8 of 107 · 100% · Almost done` is telling the truth about the
//! eighth file and a lie about the afternoon, because the percentage and the
//! time left belong to the item rather than to the playlist.
//!
//! Everything here works out the playlist-shaped answer instead, from what the
//! job already knows: which items were asked for, which files have landed, and
//! which item yt-dlp is on now. Nothing is fetched and no state is kept — the
//! finished files *are* the record of what is done, which is what makes this
//! survive a restart without a second thing to keep in step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::job::{Job, Progress};
use super::media::Entry;

/// One item of a collection, as the queue remembers it.
///
/// Only what a list needs to read. The URL is deliberately absent: a job
/// downloads the collection, not the items, and keeping a hundred URLs that
/// nothing invokes would be a hundred more chances for `library.json` to
/// disagree with itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Item {
    /// One-based position in the playlist: the number `--playlist-items` uses
    /// and the number yt-dlp puts at the front of the filename.
    pub index: usize,
    pub title: String,
    /// Seconds, when the site said.
    #[serde(default)]
    pub duration: Option<u64>,
}

/// Where one item has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Waiting,
    Running,
    /// The file that landed.
    Done(PathBuf),
}

/// Where one item's transcript has got to, when one was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Words {
    /// Nobody asked, or there is nothing to transcribe yet.
    None,
    /// Asked for, waiting its turn behind the items before it.
    Waiting,
    /// whisper is on this one now.
    Running,
    /// The transcript that was written.
    Done(PathBuf),
    /// whisper could not do this one, and the pass moved on.
    Failed,
}

/// One line of the expanded view.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub index: usize,
    pub title: String,
    pub duration: Option<u64>,
    pub stage: Stage,
    pub words: Words,
}

impl Line {
    pub fn path(&self) -> Option<&Path> {
        match &self.stage {
            Stage::Done(path) => Some(path),
            _ => None,
        }
    }

    pub fn transcript(&self) -> Option<&Path> {
        match &self.words {
            Words::Done(path) => Some(path),
            _ => None,
        }
    }
}

/// What a job should remember of a playlist it has just been told about.
///
/// `wanted` is the job's own item filter, empty meaning everything — the same
/// convention `--playlist-items` is given.
pub fn items(entries: &[Entry], wanted: &[usize]) -> Vec<Item> {
    entries
        .iter()
        .filter(|entry| wanted.is_empty() || wanted.contains(&entry.index))
        .map(|entry| Item {
            index: entry.index,
            title: entry.title.clone(),
            duration: entry.duration,
        })
        .collect()
}

/// One line per item this job is downloading, in playlist order.
///
/// Empty for anything that is not a collection.
pub fn lines(job: &Job, progress: Option<&Progress>) -> Vec<Line> {
    if job.collection.is_none() {
        return Vec::new();
    }

    let indices = indices(job, progress);
    let finished = finished(job, &indices);
    let running = running(job, progress).filter(|index| !finished.contains_key(index));
    let titles: HashMap<usize, &Item> = job.items.iter().map(|item| (item.index, item)).collect();

    // The item whisper is on: the first without a transcript, and only while a
    // pass is actually running.
    let transcribing = job
        .transcript_is_running()
        .then(|| job.next_untranscribed())
        .flatten();

    indices
        .into_iter()
        .map(|index| {
            let stage = match (finished.get(&index), running == Some(index)) {
                (Some(path), _) => Stage::Done(path.to_path_buf()),
                (None, true) => Stage::Running,
                (None, false) => Stage::Waiting,
            };
            Line {
                index,
                title: title(titles.get(&index).copied(), &stage, index),
                duration: titles.get(&index).and_then(|item| item.duration),
                words: words(job, &stage, transcribing),
                stage,
            }
        })
        .collect()
}

/// Where one item's transcript stands.
fn words(job: &Job, stage: &Stage, transcribing: Option<&PathBuf>) -> Words {
    let Stage::Done(media) = stage else {
        // Nothing to transcribe until the file exists, however keen the wish.
        return match job.transcribe {
            Some(_) => Words::Waiting,
            None => Words::None,
        };
    };

    if let Some(transcript) = job.transcript_for(media) {
        return Words::Done(transcript.clone());
    }
    if job.transcript_failures.contains(media) {
        return Words::Failed;
    }
    if transcribing == Some(media) {
        return Words::Running;
    }
    match job.transcribe {
        Some(_) => Words::Waiting,
        None => Words::None,
    }
}

/// How far through transcribing a collection this job is.
///
/// Counted in items, plus how far into the one in hand — the same shape as the
/// download's own figure, and for the same reason: the pass is the thing being
/// waited on, not the file.
pub fn transcript_fraction(job: &Job, progress: Option<&Progress>) -> Option<f64> {
    let total = job.outputs.len();
    if total == 0 {
        return None;
    }
    let settled = (job.transcribed_count() + job.transcript_failures.len()) as f64;
    let current = progress
        .and_then(|progress| progress.transcript_fraction)
        .unwrap_or(0.0);
    Some(((settled + current) / total as f64).clamp(0.0, 1.0))
}

/// How many items this job is downloading, when that is known.
pub fn total(job: &Job, progress: Option<&Progress>) -> Option<usize> {
    let collection = job.collection.as_ref()?;
    if !collection.items.is_empty() {
        return Some(collection.items.len());
    }
    if !job.items.is_empty() {
        return Some(job.items.len());
    }
    // Nothing was ever fetched for this job — added without the dialog, or
    // queued by a Magpie that did not keep the entries. yt-dlp counts them as it
    // goes.
    progress.and_then(|progress| progress.snapshot.item.map(|(_, count)| count))
}

/// How many have finished. The files that landed are the record.
pub fn done(job: &Job, progress: Option<&Progress>) -> usize {
    match total(job, progress) {
        Some(total) => job.outputs.len().min(total),
        None => job.outputs.len(),
    }
}

/// How far through the collection this job is, counting items rather than bytes.
///
/// The current item's own progress counts for its share of one item, which is
/// what stops the bar sitting still for the twenty minutes a large item takes.
pub fn fraction(job: &Job, progress: Option<&Progress>) -> Option<f64> {
    let total = total(job, progress)?;
    if total == 0 {
        return None;
    }
    let done = done(job, progress) as f64;
    let current = progress
        .filter(|progress| progress.postprocessing.is_none())
        .and_then(|progress| progress.snapshot.fraction())
        .unwrap_or(0.0);
    Some(((done + current) / total as f64).clamp(0.0, 1.0))
}

/// `8 of 107`, as the row says it.
pub fn position(job: &Job, progress: Option<&Progress>) -> Option<(usize, usize)> {
    let (position, counted) = progress?.snapshot.item?;
    // Our own count when we have one: yt-dlp counts what it extracted, which for
    // a playlist with a deleted video in it can differ from what was asked for.
    Some((position, total(job, progress).unwrap_or(counted)))
}

/// The indices this job is downloading, in order.
fn indices(job: &Job, progress: Option<&Progress>) -> Vec<usize> {
    let Some(collection) = job.collection.as_ref() else {
        return Vec::new();
    };
    if !collection.items.is_empty() {
        let mut items = collection.items.clone();
        items.sort_unstable();
        return items;
    }
    if !job.items.is_empty() {
        let mut items: Vec<usize> = job.items.iter().map(|item| item.index).collect();
        items.sort_unstable();
        return items;
    }
    // Nothing known but the count, so the items are numbered rather than named.
    // Better than an empty list: the eighth of a hundred and seven is still a
    // useful thing to be able to see.
    match total(job, progress) {
        Some(total) => (1..=total).collect(),
        None => Vec::new(),
    }
}

/// Which file belongs to which item.
///
/// yt-dlp writes a collection's files as `008 - Title.ext`, so the file says
/// which item it is. When it does not — a template from an older Magpie, or a
/// title that begins with digits of its own — the fallback is position: the
/// files arrive in the order the items were asked for.
fn finished<'a>(job: &'a Job, indices: &[usize]) -> HashMap<usize, &'a PathBuf> {
    let mut finished: HashMap<usize, &PathBuf> = HashMap::new();
    let mut unplaced: Vec<&PathBuf> = Vec::new();

    for output in &job.outputs {
        match index_of(output).filter(|index| indices.contains(index)) {
            Some(index) => {
                finished.insert(index, output);
            }
            None => unplaced.push(output),
        }
    }

    for output in unplaced {
        let Some(index) = indices.iter().find(|index| !finished.contains_key(index)) else {
            break;
        };
        finished.insert(*index, output);
    }
    finished
}

/// The item yt-dlp is on now, if this job is actually running.
fn running(job: &Job, progress: Option<&Progress>) -> Option<usize> {
    if !job.state.is_active() {
        return None;
    }
    progress?.snapshot.playlist_index
}

/// The leading `008` of a filename yt-dlp wrote for a collection.
fn index_of(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    let end = name.find(|c: char| !c.is_ascii_digit())?;
    // The separator is part of the template, so requiring it keeps a video
    // called "2001 A Space Odyssey" from claiming item 2001.
    if end == 0 || !name[end..].starts_with(" - ") {
        return None;
    }
    name[..end].parse().ok()
}

fn title(item: Option<&Item>, stage: &Stage, index: usize) -> String {
    if let Some(item) = item {
        return item.title.clone();
    }
    // The file that landed is named after the video, so a job that never had
    // its entries fetched still reads properly once an item is finished.
    if let Stage::Done(path) = stage {
        if let Some(title) = title_of(path) {
            return title;
        }
    }
    format!("Item {index}")
}

/// A video's title, read back off the file yt-dlp named after it.
fn title_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let title = match stem.find(" - ") {
        Some(offset) if stem[..offset].chars().all(|c| c.is_ascii_digit()) => &stem[offset + 3..],
        _ => stem,
    };
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::job::State;
    use crate::model::progress::Snapshot;
    use crate::model::request::Collection;

    fn job() -> Job {
        let mut job = Job::new(
            1,
            "https://youtube.com/playlist?list=PL1".into(),
            "World of Warcraft (Audiobooks)".into(),
            PathBuf::from("/home/matty/Downloads"),
        );
        job.state = State::Running;
        job.collection = Some(Collection {
            folder: "World of Warcraft (Audiobooks)".into(),
            items: Vec::new(),
        });
        job.items = (1..=4)
            .map(|index| Item {
                index,
                title: format!("Chapter {index}"),
                duration: Some(600),
            })
            .collect();
        job
    }

    fn at(position: usize, index: usize, count: usize, fraction: f64) -> Progress {
        let mut progress = Progress::default();
        progress.observe(Snapshot {
            status: "downloading".into(),
            downloaded_bytes: (fraction * 1000.0) as u64,
            total_bytes: Some(1000),
            item: Some((position, count)),
            playlist_index: Some(index),
            ..Default::default()
        });
        progress
    }

    #[test]
    fn a_single_video_has_no_items_to_expand() {
        let mut job = job();
        job.collection = None;
        assert!(lines(&job, None).is_empty());
    }

    #[test]
    fn each_item_says_whether_it_is_done_running_or_still_to_come() {
        let mut job = job();
        job.outputs = vec![
            PathBuf::from("/downloads/001 - Chapter 1.m4a"),
            PathBuf::from("/downloads/002 - Chapter 2.m4a"),
        ];
        let progress = at(3, 3, 4, 0.5);

        let lines = lines(&job, Some(&progress));
        let stages: Vec<&Stage> = lines.iter().map(|line| &line.stage).collect();
        assert_eq!(
            stages,
            vec![
                &Stage::Done(PathBuf::from("/downloads/001 - Chapter 1.m4a")),
                &Stage::Done(PathBuf::from("/downloads/002 - Chapter 2.m4a")),
                &Stage::Running,
                &Stage::Waiting,
            ]
        );
        assert_eq!(lines[0].title, "Chapter 1");
    }

    #[test]
    fn a_playlist_nobody_fetched_the_entries_for_is_still_worth_expanding() {
        // The case that matters for a queue written before the entries were
        // kept: the count comes off the progress line, the finished items are
        // named by their files, and the rest are numbered.
        let mut job = job();
        job.items.clear();
        job.outputs = vec![PathBuf::from(
            "/downloads/001 - The Lore You Never Knew.m4a",
        )];
        let progress = at(2, 2, 3, 0.25);

        let lines = lines(&job, Some(&progress));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].title, "The Lore You Never Knew");
        assert_eq!(lines[1].title, "Item 2");
        assert_eq!(lines[1].stage, Stage::Running);
    }

    #[test]
    fn picking_four_items_out_of_forty_lists_those_four() {
        let mut job = job();
        job.items.clear();
        job.collection = Some(Collection {
            folder: "Some of it".into(),
            items: vec![30, 10, 20],
        });
        let indices: Vec<usize> = lines(&job, None).iter().map(|line| line.index).collect();
        assert_eq!(
            indices,
            vec![10, 20, 30],
            "in playlist order, not click order"
        );
    }

    #[test]
    fn a_file_is_matched_to_its_item_by_the_number_in_its_name() {
        // The download order is not the answer: a private video in the middle is
        // skipped, and every file after it would be attributed to the wrong item.
        let mut job = job();
        job.outputs = vec![
            PathBuf::from("/downloads/001 - Chapter 1.m4a"),
            PathBuf::from("/downloads/003 - Chapter 3.m4a"),
        ];
        let lines = lines(&job, None);
        assert_eq!(lines[1].stage, Stage::Waiting, "item two was skipped");
        assert!(matches!(lines[2].stage, Stage::Done(_)));
    }

    #[test]
    fn a_title_that_begins_with_digits_is_not_read_as_an_index() {
        assert_eq!(index_of(Path::new("/d/2001 A Space Odyssey.mkv")), None);
        assert_eq!(index_of(Path::new("/d/012 - Anything.mkv")), Some(12));
        assert_eq!(
            title_of(Path::new("/d/2001 A Space Odyssey.mkv")).as_deref(),
            Some("2001 A Space Odyssey")
        );
    }

    #[test]
    fn a_file_that_cannot_be_placed_falls_back_to_the_order_it_arrived_in() {
        let mut job = job();
        job.outputs = vec![PathBuf::from("/downloads/Chapter One.m4a")];
        assert!(matches!(lines(&job, None)[0].stage, Stage::Done(_)));
    }

    #[test]
    fn progress_across_a_collection_counts_items_not_bytes() {
        // The bug on the row: one item at 100% is not a playlist that is almost
        // finished.
        let mut job = job();
        job.outputs = vec![PathBuf::from("/downloads/001 - Chapter 1.m4a")];
        let progress = at(2, 2, 4, 1.0);

        assert_eq!(fraction(&job, Some(&progress)), Some(0.5));
        assert_eq!(done(&job, Some(&progress)), 1);
        assert_eq!(position(&job, Some(&progress)), Some((2, 4)));
    }

    #[test]
    fn the_count_comes_from_what_was_asked_for_rather_than_what_yt_dlp_extracted() {
        // A playlist with a deleted video in it reports fewer entries than the
        // dialog listed; the number the user chose is the honest denominator.
        let mut job = job();
        let progress = at(2, 2, 3, 0.0);
        assert_eq!(position(&job, Some(&progress)), Some((2, 4)));

        job.items.clear();
        job.collection.as_mut().unwrap().items = vec![1, 2, 3];
        assert_eq!(position(&job, Some(&progress)), Some((2, 3)));
    }

    #[test]
    fn only_the_items_that_were_ticked_are_remembered() {
        let entries: Vec<Entry> = (1..=4)
            .map(|index| Entry {
                index,
                title: format!("Chapter {index}"),
                duration: Some(60),
                url: format!("https://e/{index}"),
            })
            .collect();

        assert_eq!(items(&entries, &[]).len(), 4, "empty means everything");
        let some = items(&entries, &[2, 4]);
        assert_eq!(
            some.iter().map(|item| item.index).collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn each_item_carries_its_own_transcript() {
        let mut job = job();
        job.state = State::Done;
        job.transcribe = Some(Default::default());
        job.outputs = (1..=4)
            .map(|index| PathBuf::from(format!("/downloads/{index:03} - Chapter {index}.m4a")))
            .collect();
        job.transcripts = vec![PathBuf::from("/downloads/001 - Chapter 1.txt")];
        job.transcript_failures = vec![PathBuf::from("/downloads/002 - Chapter 2.m4a")];
        job.transcript_state = crate::model::job::TranscriptState::Running;

        let lines = lines(&job, None);
        assert_eq!(
            lines[0].words,
            Words::Done(PathBuf::from("/downloads/001 - Chapter 1.txt"))
        );
        assert_eq!(
            lines[1].words,
            Words::Failed,
            "whisper could not do that one"
        );
        assert_eq!(lines[2].words, Words::Running, "the pass is on this one");
        assert_eq!(lines[3].words, Words::Waiting);

        // A quarter of the way through the pass: two settled of four.
        assert_eq!(transcript_fraction(&job, None), Some(0.5));
    }

    #[test]
    fn items_have_no_transcript_state_when_none_was_asked_for() {
        let mut job = job();
        job.state = State::Done;
        job.outputs = vec![PathBuf::from("/downloads/001 - Chapter 1.m4a")];
        assert_eq!(lines(&job, None)[0].words, Words::None);
    }

    #[test]
    fn nothing_is_running_once_the_job_has_stopped() {
        let mut job = job();
        job.state = State::Done;
        job.outputs = vec![PathBuf::from("/downloads/001 - Chapter 1.m4a")];
        let progress = at(2, 2, 4, 0.5);

        let lines = lines(&job, Some(&progress));
        assert_eq!(lines[1].stage, Stage::Waiting, "not still downloading");
    }
}
