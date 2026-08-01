//! Working out how many people are talking, and when each of them talks.
//!
//! The same shape as the transcript half: this file decides what to run and how
//! to read the output, and `ui/` runs it. The engine is sherpa-onnx's
//! `sherpa-onnx-offline-speaker-diarization`, which is a plain C++ binary with no
//! Python behind it — the alternatives in this space (pyannote.audio, WhisperX,
//! NeMo) are all PyTorch, and a video downloader that pulls in a machine learning
//! runtime because someone ticked a switch is not a trade worth making.
//!
//! What it actually does, in the order it happens: a pyannote segmentation model
//! finds the stretches of speech, a speaker embedding model turns each stretch
//! into a vector, and those vectors are clustered. The number of clusters *is*
//! the number of speakers, which is why this can answer "how many" without being
//! told. whisper.cpp's own `--tinydiarize` cannot: it marks the points where the
//! speaker changes and never says whether the voice coming back is one already
//! heard. `--diarize` is not diarization at all — it compares the loudness of the
//! left and right channels, so on the mono file a download produces it reports
//! nothing.
//!
//! Nothing here spawns anything, so every flag combination and every shape of
//! output is checkable with no display, no models and no audio.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many voices to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Count {
    /// Let the clustering decide. The honest default: the person who just
    /// downloaded an hour of conference audio does not know the answer either.
    #[default]
    Detect,
    /// The user knows, and saying so is strictly better than guessing — a fixed
    /// count turns an unbounded clustering problem into a bounded one, and it is
    /// the difference between an interview coming out as two speakers and coming
    /// out as five because one of them coughed.
    Fixed(u8),
}

impl Count {
    /// The most a person is likely to name, and past which "detect" is the
    /// better answer anyway.
    pub const MAX: u8 = 10;

    pub fn label(self) -> String {
        match self {
            Count::Detect => "Detect automatically".to_string(),
            Count::Fixed(1) => "1 speaker".to_string(),
            Count::Fixed(n) => format!("{n} speakers"),
        }
    }
}

/// What the user asked for when they turned the Identify speakers switch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Wish {
    pub count: Count,
}

/// One of the two ONNX models diarization needs.
///
/// Two, not one, because the job is two jobs: finding speech and telling voices
/// apart are different models trained on different things. Both are needed
/// before anything can run, which is why the preferences page treats them as a
/// single download rather than making the user reason about the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    /// pyannote segmentation 3.0: where the speech is, and where it overlaps.
    Segmentation,
    /// WeSpeaker CAM++, trained on VoxCeleb: a voice as a vector.
    Embedding,
}

impl Asset {
    pub const ALL: [Asset; 2] = [Asset::Segmentation, Asset::Embedding];

    pub fn file_name(self) -> &'static str {
        match self {
            // Named for what they are rather than keeping the upstream names.
            // Both are called `model.onnx` at one end or the other, and two files
            // called `model.onnx` in one directory is not a directory.
            Asset::Segmentation => "diarize-segmentation.onnx",
            Asset::Embedding => "diarize-embedding.onnx",
        }
    }

    pub fn path_in(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.file_name())
    }

    /// The exact size on the server, checked against the response so a truncated
    /// file reads as absent rather than as a model ONNX Runtime will fail to
    /// load halfway through a job.
    pub fn bytes(self) -> u64 {
        match self {
            Asset::Segmentation => 5_992_913,
            Asset::Embedding => 29_292_684,
        }
    }

    /// Where the weights come from.
    ///
    /// Two different hosts, which looks careless and is not. The embedding model
    /// is published only as a release asset on GitHub, under a tag whose name —
    /// `speaker-recongition-models` — is misspelled upstream. Do not "fix" that
    /// spelling: the tag is the address, and the corrected form 404s.
    ///
    /// The segmentation model is published on GitHub only inside a `.tar.bz2`.
    /// Hugging Face carries the same file unpacked, which is the difference
    /// between reusing the downloader that already exists and teaching Magpie to
    /// extract archives. Both verified 2026-08-01.
    pub fn download_url(self) -> &'static str {
        match self {
            Asset::Segmentation => {
                "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/\
                 resolve/main/model.onnx"
            }
            Asset::Embedding => {
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/\
                 speaker-recongition-models/wespeaker_en_voxceleb_CAM%2B%2B.onnx"
            }
        }
    }
}

/// What both models together weigh, for the confirmation before the download.
pub fn total_bytes() -> u64 {
    Asset::ALL.iter().map(|asset| asset.bytes()).sum()
}

/// `sherpa-onnx-offline-speaker-diarization` arguments.
///
/// Kaldi-style `--key=value`, and the audio is positional. It reads 16 kHz WAV
/// and nothing else, which costs nothing here because the transcript path has
/// already made one — see `transcript::conversion_argv`. The one thing that must
/// not happen is that scratch file being deleted when whisper finishes, because
/// this is the second reader of it.
pub fn argv(segmentation: &Path, embedding: &Path, audio: &Path, wish: &Wish) -> Vec<String> {
    let mut args = vec![
        format!("--segmentation.pyannote-model={}", segmentation.display()),
        format!("--embedding.model={}", embedding.display()),
        // Defaults to true and echoes the whole command line to stderr, where it
        // would be the first thing in a failure report and explain nothing.
        "--print-args=false".to_string(),
    ];

    match wish.count {
        // Left to the tool's own default threshold rather than a number invented
        // here. `num-clusters` defaults to -1, which *is* the detect mode, so the
        // absence of the flag is the request.
        Count::Detect => {}
        Count::Fixed(n) => args.push(format!("--clustering.num-clusters={}", n.max(1))),
    }

    args.push(audio.display().to_string());
    args
}

/// A stretch of one voice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    /// Which cluster, counted from zero. Meaningless on its own — it is a label,
    /// not an identity — but consistent across the whole file, which is the
    /// entire point of clustering rather than marking turn boundaries.
    pub speaker: usize,
}

impl Turn {
    /// How much of this turn and that span are the same seconds.
    pub fn overlap(&self, start: f64, end: f64) -> f64 {
        (self.end.min(end) - self.start.max(start)).max(0.0)
    }
}

/// One `1.583 -- 3.406 speaker_00` line, if this is one.
///
/// Everything before them is a dump of the resolved configuration and the word
/// `Started`, and nothing after. Rather than track that position, this matches
/// the shape of the line: two numbers, the separator, and `speaker_` followed by
/// digits. The configuration dump contains `num_clusters=-1` and similar, and
/// none of it can be mistaken for this.
pub fn parse_turn(line: &str) -> Option<Turn> {
    let (start, rest) = line.trim().split_once(" -- ")?;
    let (end, speaker) = rest.split_once(' ')?;

    let start: f64 = start.trim().parse().ok()?;
    let end: f64 = end.trim().parse().ok()?;
    let speaker: usize = speaker.trim().strip_prefix("speaker_")?.parse().ok()?;

    // A zero-length or backwards turn would survive the parse and then divide by
    // itself somewhere downstream.
    (end > start).then_some(Turn {
        start,
        end,
        speaker,
    })
}

/// Percentage out of one of the progress lines, if this is one.
///
/// Written to stderr as `progress 7.14%`, unconditionally rather than only for a
/// terminal, which is what makes a real progress bar possible for a step that is
/// otherwise a silent minute.
pub fn parse_progress(line: &str) -> Option<f64> {
    let rest = line.rsplit_once("progress").map(|(_, rest)| rest)?;
    let digits: String = rest
        .trim_start_matches([' ', '=', ':'])
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let percent: f64 = digits.parse().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent / 100.0)
}

/// How many distinct voices the turns describe.
///
/// Counted rather than taken as the highest index plus one: the clusters that
/// survive are not guaranteed to be numbered without gaps.
pub fn speaker_count(turns: &[Turn]) -> usize {
    let mut seen: Vec<usize> = Vec::new();
    for turn in turns {
        if !seen.contains(&turn.speaker) {
            seen.push(turn.speaker);
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_line_is_read_and_the_banner_around_it_is_not() {
        // Verified against sherpa-onnx v1.13.4: three decimal places either side,
        // a space-hyphen-hyphen-space separator, and the cluster zero-padded.
        assert_eq!(
            parse_turn("1.583 -- 3.406 speaker_00"),
            Some(Turn {
                start: 1.583,
                end: 3.406,
                speaker: 0
            })
        );
        assert_eq!(
            parse_turn("9.346 -- 11.472 speaker_02").map(|t| t.speaker),
            Some(2)
        );

        // The configuration dump the tool prints before it starts. It contains
        // numbers, an `=`, and the word `clusters`, so a looser parser reads it
        // as a turn at some improbable timestamp.
        for line in [
            "OfflineSpeakerDiarizationConfig(segmentation=..., clustering=FastClusteringConfig(num_clusters=-1, threshold=0.5))",
            "Started",
            "",
            "Duration : 16.000 s",
            "Real time factor (RTF): 0.367 / 16.000 = 0.023",
        ] {
            assert_eq!(parse_turn(line), None, "{line}");
        }
    }

    #[test]
    fn a_turn_that_ends_before_it_starts_is_not_a_turn() {
        assert_eq!(parse_turn("3.000 -- 3.000 speaker_00"), None);
        assert_eq!(parse_turn("4.000 -- 1.000 speaker_00"), None);
    }

    #[test]
    fn a_speaker_index_of_ten_or_more_still_reads() {
        // The format zero-pads to two digits but does not truncate to two.
        assert_eq!(
            parse_turn("1.000 -- 2.000 speaker_11").map(|t| t.speaker),
            Some(11)
        );
    }

    #[test]
    fn detecting_the_count_means_asking_for_no_particular_count() {
        // `num-clusters` defaults to -1 and -1 is the detect mode, so passing a
        // number here would silently turn the automatic answer into a fixed one.
        let args = argv(
            Path::new("/m/seg.onnx"),
            Path::new("/m/emb.onnx"),
            Path::new("/cache/a.wav"),
            &Wish::default(),
        );
        assert!(!args.iter().any(|a| a.contains("num-clusters")), "{args:?}");

        let wish = Wish {
            count: Count::Fixed(3),
        };
        let args = argv(
            Path::new("/m/seg.onnx"),
            Path::new("/m/emb.onnx"),
            Path::new("/cache/a.wav"),
            &wish,
        );
        assert!(
            args.contains(&"--clustering.num-clusters=3".to_string()),
            "{args:?}"
        );
    }

    #[test]
    fn a_fixed_count_of_zero_would_be_rejected_by_the_tool() {
        // Validation upstream fails when num_clusters < 1 *and* the threshold is
        // negative, so a zero here is an argument error rather than a fallback.
        let wish = Wish {
            count: Count::Fixed(0),
        };
        let args = argv(Path::new("/s"), Path::new("/e"), Path::new("/a.wav"), &wish);
        assert!(args.contains(&"--clustering.num-clusters=1".to_string()));
    }

    #[test]
    fn the_audio_is_the_last_argument_and_carries_no_flag() {
        let args = argv(
            Path::new("/m/seg.onnx"),
            Path::new("/m/emb.onnx"),
            Path::new("/cache/transcribe-7.wav"),
            &Wish::default(),
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("/cache/transcribe-7.wav")
        );
    }

    #[test]
    fn the_command_line_is_not_echoed_into_the_failure_report() {
        let args = argv(
            Path::new("/s"),
            Path::new("/e"),
            Path::new("/a.wav"),
            &Wish::default(),
        );
        assert!(args.contains(&"--print-args=false".to_string()));
    }

    #[test]
    fn progress_lines_are_read_and_turn_lines_are_not() {
        // Verified against sherpa-onnx v1.13.4, which writes these to stderr.
        let seven = parse_progress("progress 7.14%").expect("a progress line");
        assert!((seven - 0.0714).abs() < 1e-9, "{seven}");
        assert_eq!(parse_progress("progress 100.00%"), Some(1.0));
        assert_eq!(parse_progress("1.583 -- 3.406 speaker_00"), None);
        assert_eq!(parse_progress("Started"), None);
    }

    #[test]
    fn the_speaker_count_is_the_distinct_clusters_not_the_highest_index() {
        let turns = vec![
            Turn {
                start: 0.0,
                end: 1.0,
                speaker: 0,
            },
            Turn {
                start: 1.0,
                end: 2.0,
                speaker: 2,
            },
            Turn {
                start: 2.0,
                end: 3.0,
                speaker: 0,
            },
        ];
        assert_eq!(speaker_count(&turns), 2);
        assert_eq!(speaker_count(&[]), 0);
    }

    #[test]
    fn overlap_is_zero_rather_than_negative_when_two_spans_do_not_touch() {
        let turn = Turn {
            start: 10.0,
            end: 12.0,
            speaker: 0,
        };
        assert_eq!(turn.overlap(0.0, 5.0), 0.0);
        assert_eq!(turn.overlap(11.0, 20.0), 1.0);
        assert_eq!(turn.overlap(10.5, 11.5), 1.0);
    }

    #[test]
    fn the_two_models_do_not_collide_in_the_models_directory() {
        // Both are called `model.onnx` upstream.
        let dir = Path::new("/home/matty/.local/share/magpie/models");
        let paths: Vec<PathBuf> = Asset::ALL.iter().map(|a| a.path_in(dir)).collect();
        assert_ne!(paths[0], paths[1]);
    }

    #[test]
    fn the_misspelled_upstream_release_tag_is_preserved() {
        // `speaker-recongition-models` is upstream's typo and upstream's address.
        // Correcting it here would 404 every download.
        assert!(Asset::Embedding
            .download_url()
            .contains("speaker-recongition-models"));
    }
}
