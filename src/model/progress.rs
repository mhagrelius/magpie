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
    /// Position within a collection, when there is one.
    pub item: Option<(usize, usize)>,
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
        Some("download") => Event::Progress(Snapshot {
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
            item: match (number(&fields, 7), number(&fields, 8)) {
                (Some(index), Some(count)) if count > 1.0 => Some((index as usize, count as usize)),
                _ => None,
            },
        }),
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
        let single = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t1\t1";
        let Event::Progress(snapshot) = parse_line(single) else {
            panic!()
        };
        assert_eq!(snapshot.item, None, "a playlist of one is not a playlist");

        let many = "\u{1f}magpie\tdownload\tdownloading\t1\t2\tNA\tNA\tNA\t3\t40";
        let Event::Progress(snapshot) = parse_line(many) else {
            panic!()
        };
        assert_eq!(snapshot.item, Some((3, 40)));
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
