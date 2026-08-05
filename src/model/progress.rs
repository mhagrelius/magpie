//! Turning yt-dlp's output back into numbers.
//!
//! Two problems the old application had, both here. It split each read on `\n`
//! and dropped the remainder, so a progress line that straddled two reads was
//! silently mis-parsed; and it asked yt-dlp for `_str` fields like `1.23MiB`
//! and parsed them back into integers with a regex. The template asks for raw
//! numbers instead, and [`LineBuffer`] keeps the remainder.

use std::collections::VecDeque;

use super::request::SENTINEL;

/// Splits a byte stream into lines across read boundaries.
///
/// Splits on `\r` as well as `\n`: yt-dlp uses a carriage return to redraw
/// post-processor status in place, and a `\r`-only chunk would otherwise sit in
/// the buffer until the process exited.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: String,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk and take whatever complete lines it completed.
    ///
    /// Invalid UTF-8 is replaced rather than rejected — a mangled character in
    /// a video title is not a reason to lose the progress line it sits on.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.push_str(&String::from_utf8_lossy(chunk));

        let mut lines = Vec::new();
        while let Some(index) = self.pending.find(['\n', '\r']) {
            let line: String = self.pending.drain(..index).collect();
            self.pending.drain(..1);
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    /// Whatever is left when the stream ends, if it is not empty.
    pub fn flush(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        (!rest.trim().is_empty()).then_some(rest)
    }
}

/// One reading from the download.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// yt-dlp's own word: `downloading`, `finished` or `error`.
    pub status: String,
    pub downloaded_bytes: u64,
    /// The real total when the server sent one, otherwise yt-dlp's estimate.
    pub total_bytes: Option<u64>,
    pub bytes_per_second: Option<f64>,
    pub seconds_remaining: Option<u64>,
    /// How far into the queue this is, and how long the queue is: `8 of 107`.
    ///
    /// From `playlist_autonumber`, which counts what is being downloaded, rather
    /// than `playlist_index`, which counts the playlist. Picking four items out
    /// of forty makes those two disagree, and it is the first that answers "how
    /// much of this is left".
    pub item: Option<(usize, usize)>,
    /// Which item of the playlist this is — the number `--playlist-items` uses
    /// and the number yt-dlp puts at the front of the filename. What identifies
    /// the item, where [`Snapshot::item`] measures the progress through them.
    pub playlist_index: Option<usize>,
}

impl Snapshot {
    /// Completion in the range 0.0 to 1.0, or `None` when the total is unknown
    /// and a progress bar would be lying.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total_bytes?;
        if total == 0 {
            return None;
        }
        Some((self.downloaded_bytes as f64 / total as f64).clamp(0.0, 1.0))
    }
}

/// What a line of yt-dlp output turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Progress(Snapshot),
    /// A post-processing step: merging streams, extracting audio. These have no
    /// progress of their own, only a name and a start and a finish.
    Postprocessing {
        status: String,
        processor: String,
    },
    /// Anything else yt-dlp said. Kept for the error report; never parsed for
    /// meaning beyond [`super::failure`].
    Chatter(String),
}

/// Classify one line of yt-dlp's stdout.
pub fn parse_line(line: &str) -> Event {
    let Some(rest) = line.strip_prefix(SENTINEL) else {
        return Event::Chatter(line.trim_end().to_string());
    };
    let fields: Vec<&str> = rest.trim_start_matches('\t').split('\t').collect();

    match fields.first().copied() {
        Some("download") => {
            let playlist_index = number(&fields, 7).map(|n| n as usize);
            // Older Magpie templates had no autonumber field. The playlist index
            // is the same number whenever nothing was filtered out, so it is the
            // right thing to fall back to.
            let position = number(&fields, 9).map(|n| n as usize).or(playlist_index);
            let count = number(&fields, 8).map(|n| n as usize);

            Event::Progress(Snapshot {
                status: field(&fields, 1).unwrap_or("downloading").to_string(),
                downloaded_bytes: number(&fields, 2).unwrap_or(0.0) as u64,
                // `total_bytes` is absent for a chunked response; the estimate is
                // what yt-dlp has instead, and a bar drawn from an estimate beats
                // no bar at all.
                total_bytes: number(&fields, 3)
                    .or_else(|| number(&fields, 4))
                    .map(|n| n as u64),
                bytes_per_second: number(&fields, 5),
                seconds_remaining: number(&fields, 6).map(|n| n as u64),
                item: match (position, count) {
                    (Some(position), Some(count)) if count > 1 => Some((position, count)),
                    _ => None,
                },
                playlist_index,
            })
        }
        Some("postprocess") => Event::Postprocessing {
            status: field(&fields, 1).unwrap_or("started").to_string(),
            processor: field(&fields, 2).unwrap_or("").to_string(),
        },
        _ => Event::Chatter(line.trim_end().to_string()),
    }
}

fn field<'a>(fields: &[&'a str], index: usize) -> Option<&'a str> {
    // yt-dlp renders a missing template field as the literal "NA".
    match fields.get(index).copied() {
        Some("NA") | Some("") | Some("None") | None => None,
        Some(value) => Some(value),
    }
}

fn number(fields: &[&str], index: usize) -> Option<f64> {
    field(fields, index)?
        .parse::<f64>()
        .ok()
        .filter(|n| *n >= 0.0)
}

/// A rolling average of download speed, and the time remaining computed from it.
///
/// yt-dlp's own `eta` is derived from the instantaneous rate and swings by a
/// factor of three between updates. A number that jumps between "4 minutes" and
/// "40 seconds" twice a second is worse than no number, so the displayed figure
/// comes from the last ten samples instead.
#[derive(Debug, Default, Clone)]
pub struct Meter {
    samples: VecDeque<f64>,
}

const SAMPLE_COUNT: usize = 10;

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, snapshot: &Snapshot) {
        if let Some(speed) = snapshot.bytes_per_second.filter(|s| *s > 0.0) {
            if self.samples.len() == SAMPLE_COUNT {
                self.samples.pop_front();
            }
            self.samples.push_back(speed);
        }
    }

    pub fn bytes_per_second(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        Some(self.samples.iter().sum::<f64>() / self.samples.len() as f64)
    }

    /// Seconds left, from the smoothed rate. Falls back to yt-dlp's estimate
    /// while there are no samples yet.
    pub fn seconds_remaining(&self, snapshot: &Snapshot) -> Option<u64> {
        let (Some(total), Some(rate)) = (snapshot.total_bytes, self.bytes_per_second()) else {
            return snapshot.seconds_remaining;
        };
        if rate <= 0.0 {
            return snapshot.seconds_remaining;
        }
        let remaining = total.saturating_sub(snapshot.downloaded_bytes);
        Some((remaining as f64 / rate).ceil() as u64)
    }

    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

/// How long a stage has been going and how long is left, for work that reports a
/// fraction rather than bytes.
///
/// [`Meter`] answers the same question from a byte rate, which is the better
/// instrument when there are bytes. A playlist has none — its unit is the item,
/// and the per-item byte count says nothing about the ninety-nine after it — and
/// whisper has none either. Both do report how far along they are, and the time
/// that took.
///
/// The rate is the average across the whole stage rather than a recent window,
/// which is the opposite of what `Meter` does and for the opposite reason: a
/// download's throughput moves with the line, where an item-by-item average is
/// only meaningful over many items. The clock is passed in, so nothing here
/// reads it.
#[derive(Debug, Clone, Default)]
pub struct Pace {
    /// How long the stage has been going, whether or not it has moved.
    elapsed: f64,
    first: Option<Reading>,
    latest: Option<Reading>,
}

#[derive(Debug, Clone, Copy)]
struct Reading {
    elapsed: f64,
    fraction: f64,
}

/// Ground covered before an estimate is worth showing. Below this, one slow
/// first item predicts hours that never happen.
const PACE_MIN_FRACTION: f64 = 0.03;

/// And time spent, for the same reason from the other direction.
const PACE_MIN_SECONDS: f64 = 15.0;

impl Pace {
    /// Move the clock without a new reading, so a stage that says nothing for
    /// minutes still shows the minutes passing.
    pub fn tick(&mut self, elapsed: std::time::Duration) {
        self.elapsed = self.elapsed.max(elapsed.as_secs_f64());
    }

    /// Record how far along the stage is, and when.
    pub fn observe(&mut self, elapsed: std::time::Duration, fraction: f64) {
        self.tick(elapsed);
        let reading = Reading {
            elapsed: self.elapsed,
            fraction: fraction.clamp(0.0, 1.0),
        };
        self.first.get_or_insert(reading);
        self.latest = Some(reading);
    }

    pub fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.elapsed)
    }

    /// Seconds until the stage is finished, once there is enough to say so.
    pub fn seconds_remaining(&self) -> Option<u64> {
        let (first, latest) = (self.first?, self.latest?);
        let covered = latest.fraction - first.fraction;
        let took = latest.elapsed - first.elapsed;

        if covered < PACE_MIN_FRACTION || took < PACE_MIN_SECONDS || latest.fraction >= 1.0 {
            return None;
        }
        Some(((1.0 - latest.fraction) * took / covered).ceil() as u64)
    }
}

/// How long something has been going: `4 minutes so far`.
///
/// The counterpart to [`format_remaining`], for a stage that cannot yet say how
/// much is left. A number that is climbing is still evidence of life, which is
/// the whole job of this line.
pub fn format_elapsed(seconds: u64) -> String {
    match seconds {
        0..=44 => "just started".to_string(),
        s if s < 90 => "1 minute so far".to_string(),
        s if s < 3600 => format!("{} minutes so far", (s + 30) / 60),
        s if s < 5400 => "1 hour so far".to_string(),
        s => format!("{} hours so far", (s + 1800) / 3600),
    }
}

/// `1.4 GB`, in the units a file manager would use.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// `3.2 MB/s`.
pub fn format_speed(bytes_per_second: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_second.max(0.0) as u64))
}

/// How long is left, in words rather than a clock reading.
///
/// `1:05:00` makes the reader do arithmetic to find out whether they have time
/// for a coffee. Rounded words do not.
pub fn format_remaining(seconds: u64) -> String {
    match seconds {
        0..=5 => "Almost done".to_string(),
        s if s < 60 => format!("{s} seconds left"),
        s if s < 120 => "1 minute left".to_string(),
        s if s < 3600 => format!("{} minutes left", (s + 30) / 60),
        s if s < 7200 => "1 hour left".to_string(),
        s => format!("{} hours left", (s + 1800) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str =
        "\u{1f}magpie\tdownload\tdownloading\t5242880\t10485760\tNA\t1048576.0\t5\tNA\tNA";

    #[test]
    fn a_line_split_across_two_reads_is_still_one_line() {
        // The bug this file exists for: the old parser split each chunk on
        // newlines and threw the remainder away, so any progress line unlucky
        // enough to straddle a read boundary vanished.
        let mut buffer = LineBuffer::new();
        let (head, tail) = LINE.as_bytes().split_at(20);

        assert!(buffer.push(head).is_empty());
        let lines = buffer.push(&[tail, b"\n"].concat());
        assert_eq!(lines, vec![LINE.to_string()]);
    }

    #[test]
    fn a_carriage_return_ends_a_line_too() {
        // yt-dlp redraws post-processor status in place with `\r`. Waiting for
        // a `\n` would hold those lines until the process exited.
        let mut buffer = LineBuffer::new();
        assert_eq!(buffer.push(b"first\rsecond\n"), vec!["first", "second"]);
    }

    #[test]
    fn a_multibyte_character_split_across_reads_does_not_lose_the_line() {
        let mut buffer = LineBuffer::new();
        let text = "café\n".as_bytes();
        let (head, tail) = text.split_at(4); // mid-way through the é
        buffer.push(head);
        let lines = buffer.push(tail);
        assert_eq!(lines.len(), 1, "the line survived, however the é fared");
    }

    #[test]
    fn a_progress_line_becomes_numbers() {
        let Event::Progress(snapshot) = parse_line(LINE) else {
            panic!("expected progress");
        };
        assert_eq!(snapshot.downloaded_bytes, 5_242_880);
        assert_eq!(snapshot.total_bytes, Some(10_485_760));
        assert_eq!(snapshot.bytes_per_second, Some(1_048_576.0));
        assert_eq!(snapshot.seconds_remaining, Some(5));
        assert_eq!(snapshot.fraction(), Some(0.5));
    }

    #[test]
    fn an_estimated_total_is_used_when_the_server_sent_no_length() {
        // Common on a chunked response. Without the fallback the row shows a
        // pulsing indeterminate bar for the whole download.
        let line = "\u{1f}magpie\tdownload\tdownloading\t100\tNA\t400\tNA\tNA\tNA\tNA";
        let Event::Progress(snapshot) = parse_line(line) else {
            panic!("expected progress");
        };
        assert_eq!(snapshot.total_bytes, Some(400));
        assert_eq!(snapshot.fraction(), Some(0.25));
    }

    #[test]
    fn an_unknown_total_yields_no_fraction_rather_than_a_wrong_one() {
        let line = "\u{1f}magpie\tdownload\tdownloading\t100\tNA\tNA\tNA\tNA\tNA\tNA";
        let Event::Progress(snapshot) = parse_line(line) else {
            panic!("expected progress");
        };
        assert_eq!(snapshot.total_bytes, None);
        assert_eq!(snapshot.fraction(), None);
    }

    #[test]
    fn a_video_title_containing_the_separator_is_not_mistaken_for_progress() {
        // The old test was "does the line contain a pipe character", which
        // matched a great many titles.
        let title = "[download] Destination: Rock | Paper | Scissors\ttab\t.mkv";
        assert!(matches!(parse_line(title), Event::Chatter(_)));
    }

    #[test]
    fn position_in_a_playlist_is_only_reported_when_there_is_a_playlist() {
        let single = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t1\t1\t1";
        let Event::Progress(snapshot) = parse_line(single) else {
            panic!()
        };
        assert_eq!(snapshot.item, None, "a playlist of one is not a playlist");

        let many = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t3\t40\t3";
        let Event::Progress(snapshot) = parse_line(many) else {
            panic!()
        };
        assert_eq!(snapshot.item, Some((3, 40)));
        assert_eq!(snapshot.playlist_index, Some(3));
    }

    #[test]
    fn a_filtered_playlist_counts_the_download_queue_not_the_playlist() {
        // `--playlist-items 20,30,40` downloads three videos. yt-dlp reports the
        // second of them as playlist_index 30 of 3 entries, which would read as
        // "30 of 3"; playlist_autonumber is the 2 that the row wants. The index
        // is still what names the file, so both are kept.
        let line = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t30\t3\t2";
        let Event::Progress(snapshot) = parse_line(line) else {
            panic!()
        };
        assert_eq!(snapshot.item, Some((2, 3)));
        assert_eq!(snapshot.playlist_index, Some(30));
    }

    #[test]
    fn a_line_from_a_template_without_the_autonumber_still_places_the_item() {
        // yt-dlp is asked for both, but a job started by an older Magpie is
        // still running against the old template when this one restarts.
        let line = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t8\t107";
        let Event::Progress(snapshot) = parse_line(line) else {
            panic!()
        };
        assert_eq!(snapshot.item, Some((8, 107)));
    }

    #[test]
    fn a_pace_says_nothing_until_it_has_seen_enough_to_be_worth_saying() {
        let mut pace = Pace::default();
        pace.observe(std::time::Duration::from_secs(2), 0.0);
        assert_eq!(pace.seconds_remaining(), None, "no ground covered yet");

        pace.observe(std::time::Duration::from_secs(5), 0.01);
        assert_eq!(
            pace.seconds_remaining(),
            None,
            "one percent in three seconds predicts nothing"
        );
    }

    #[test]
    fn a_pace_turns_ground_covered_into_time_left() {
        // A quarter of a hundred-item playlist in ten minutes: thirty to go.
        let mut pace = Pace::default();
        pace.observe(std::time::Duration::from_secs(0), 0.0);
        pace.observe(std::time::Duration::from_secs(600), 0.25);
        assert_eq!(pace.seconds_remaining(), Some(1800));

        // And it keeps counting the clock between readings, so a stage that goes
        // quiet still has something true to show.
        pace.tick(std::time::Duration::from_secs(900));
        assert_eq!(pace.elapsed().as_secs(), 900);
    }

    #[test]
    fn a_finished_pace_has_nothing_left_to_predict() {
        let mut pace = Pace::default();
        pace.observe(std::time::Duration::from_secs(0), 0.0);
        pace.observe(std::time::Duration::from_secs(600), 1.0);
        assert_eq!(pace.seconds_remaining(), None);
    }

    #[test]
    fn elapsed_time_reads_as_words_too() {
        assert_eq!(format_elapsed(3), "just started");
        assert_eq!(format_elapsed(120), "2 minutes so far");
        assert_eq!(format_elapsed(3600), "1 hour so far");
        assert_eq!(format_elapsed(9000), "3 hours so far");
    }

    #[test]
    fn a_postprocessing_line_is_recognised() {
        let line = "\u{1f}magpie\tpostprocess\tstarted\tMerger";
        assert_eq!(
            parse_line(line),
            Event::Postprocessing {
                status: "started".into(),
                processor: "Merger".into()
            }
        );
    }

    #[test]
    fn smoothed_speed_ignores_the_spike_that_makes_the_eta_jump() {
        let mut meter = Meter::new();
        for speed in [1e6, 1e6, 1e6, 1e6, 9e6] {
            meter.observe(&Snapshot {
                bytes_per_second: Some(speed),
                ..Default::default()
            });
        }
        // The instantaneous 9 MB/s would have said "1 second left".
        let average = meter.bytes_per_second().expect("a rate");
        assert!((2.5e6..3.0e6).contains(&average), "{average}");
    }

    #[test]
    fn the_remaining_time_falls_back_to_yt_dlps_estimate_before_any_samples() {
        let meter = Meter::new();
        let snapshot = Snapshot {
            seconds_remaining: Some(42),
            ..Default::default()
        };
        assert_eq!(meter.seconds_remaining(&snapshot), Some(42));
    }

    #[test]
    fn sizes_and_times_read_as_words() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(5_242_880), "5.2 MB");
        assert_eq!(format_bytes(1_400_000_000), "1.4 GB");
        assert_eq!(format_speed(3_200_000.0), "3.2 MB/s");

        assert_eq!(format_remaining(2), "Almost done");
        assert_eq!(format_remaining(45), "45 seconds left");
        assert_eq!(format_remaining(90), "1 minute left");
        assert_eq!(format_remaining(600), "10 minutes left");
        assert_eq!(format_remaining(3900), "1 hour left");
    }
}
