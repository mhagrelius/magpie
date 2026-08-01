//! What the user picked, turned into a yt-dlp format selector.
//!
//! This file exists because the old application had a quality preference that
//! did nothing. `1080p` was stored, displayed, round-tripped through settings,
//! and never once became a `-f` argument — its only effect was to decide
//! whether the format picker appeared. The mapping below is the missing half.

use serde::{Deserialize, Serialize};

/// A video quality the Add dialog offers.
/// The spellings are given explicitly: `rename_all = "kebab-case"` renders
/// `UpTo1080` as `up-to1080`, which is not a name anyone would write by hand
/// into a config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Quality {
    #[default]
    #[serde(rename = "best")]
    Best,
    #[serde(rename = "up-to-1080")]
    UpTo1080,
    #[serde(rename = "up-to-720")]
    UpTo720,
    #[serde(rename = "up-to-480")]
    UpTo480,
}

/// A container for an audio-only download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AudioFormat {
    /// Whatever the site serves, kept as-is. No transcode, so no ffmpeg and no
    /// generation loss.
    #[default]
    Best,
    Mp3,
    M4a,
}

impl Quality {
    pub const ALL: [Quality; 4] = [
        Quality::Best,
        Quality::UpTo1080,
        Quality::UpTo720,
        Quality::UpTo480,
    ];

    /// The label shown in the combo row. Sentence case, as the HIG asks.
    pub fn label(self) -> &'static str {
        match self {
            Quality::Best => "Best available",
            Quality::UpTo1080 => "Up to 1080p",
            Quality::UpTo720 => "Up to 720p",
            Quality::UpTo480 => "Up to 480p",
        }
    }

    /// The `-f` selector.
    ///
    /// `bestvideo*` rather than `bestvideo` so a stream that happens to carry
    /// audio still qualifies, and `height<=?` rather than `height<=` so the
    /// ceiling is a preference rather than a requirement — a video that only
    /// exists at 1440p downloads at 1440p instead of failing the selector
    /// outright, which is what `<=` would do.
    pub fn selector(self) -> &'static str {
        match self {
            Quality::Best => "bestvideo*+bestaudio/best",
            Quality::UpTo1080 => "bestvideo*[height<=?1080]+bestaudio/best[height<=?1080]",
            Quality::UpTo720 => "bestvideo*[height<=?720]+bestaudio/best[height<=?720]",
            Quality::UpTo480 => "bestvideo*[height<=?480]+bestaudio/best[height<=?480]",
        }
    }
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 3] = [AudioFormat::Best, AudioFormat::Mp3, AudioFormat::M4a];

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Best => "Best available",
            AudioFormat::Mp3 => "MP3",
            AudioFormat::M4a => "M4A",
        }
    }

    /// The one-line explanation under the label, since the trade-off here is
    /// not obvious from three format names.
    pub fn description(self) -> &'static str {
        match self {
            AudioFormat::Best => "Original quality, no conversion",
            AudioFormat::Mp3 => "Plays anywhere, needs FFmpeg",
            AudioFormat::M4a => "Better quality than MP3 at the same size",
        }
    }

    /// Whether choosing this format means ffmpeg has to exist.
    pub fn needs_ffmpeg(self) -> bool {
        !matches!(self, AudioFormat::Best)
    }

    /// The arguments that select and, where asked, convert the audio.
    ///
    /// `--audio-quality 0` is the fix for a setting the old application
    /// declared, stored and never passed: without it yt-dlp's default is VBR
    /// quality 5, around 128 kbps, which is not what "MP3" implies when the
    /// source was better than that.
    pub fn args(self) -> Vec<String> {
        let strings = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect();
        match self {
            AudioFormat::Best => strings(&["-f", "bestaudio/best"]),
            AudioFormat::Mp3 => strings(&[
                "-f",
                "bestaudio/best",
                "-x",
                "--audio-format",
                "mp3",
                "--audio-quality",
                "0",
            ]),
            AudioFormat::M4a => strings(&[
                "-f",
                "bestaudio[ext=m4a]/bestaudio/best",
                "-x",
                "--audio-format",
                "m4a",
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quality_ceiling_is_a_preference_not_a_requirement() {
        // `height<=1080` fails outright on a video published only at 1440p.
        // `height<=?1080` prefers 1080 and takes what exists. This is the
        // difference between "smaller file" and "no file".
        for quality in Quality::ALL {
            let selector = quality.selector();
            assert!(
                !selector.contains("<=1") && !selector.contains("<=4") && !selector.contains("<=7"),
                "{selector} uses a hard ceiling"
            );
        }
    }

    #[test]
    fn every_video_preset_can_merge_a_separate_audio_stream() {
        // The old picker only offered formats that carried their own audio,
        // which on YouTube tops out at 360p. Anything above that is DASH: video
        // and audio arrive separately and ffmpeg muxes them.
        for quality in Quality::ALL {
            assert!(
                quality.selector().contains("+bestaudio"),
                "{} cannot reach past 360p",
                quality.label()
            );
        }
    }

    #[test]
    fn every_video_preset_falls_back_to_something() {
        for quality in Quality::ALL {
            assert!(
                quality.selector().contains('/'),
                "{quality:?} has no fallback"
            );
        }
    }

    #[test]
    fn mp3_asks_for_the_best_bitrate_rather_than_the_default() {
        let args = AudioFormat::Mp3.args();
        let quality = args.iter().position(|a| a == "--audio-quality");
        assert_eq!(quality.map(|i| args[i + 1].as_str()), Some("0"));
    }

    #[test]
    fn keeping_the_original_audio_needs_no_ffmpeg() {
        // The point of "Best available" is that it is a copy, not a transcode:
        // it works on a machine with no ffmpeg, which is what makes it the
        // right default.
        assert!(!AudioFormat::Best.needs_ffmpeg());
        assert!(!AudioFormat::Best.args().contains(&"-x".to_string()));
        assert!(AudioFormat::Mp3.needs_ffmpeg());
        assert!(AudioFormat::M4a.needs_ffmpeg());
    }
}
