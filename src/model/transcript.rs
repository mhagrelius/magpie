//! Transcribing a finished download with whisper.cpp.
//!
//! Same shape as the download half: this file decides what to run and how to
//! read the output, and `ui/` runs it. Whisper is not bundled — see DESIGN.md
//! on why a self-updating executable outside the package manager is not
//! something to ship — so everything here assumes `whisper-cli` was installed
//! by the user and may not be there at all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A whisper.cpp model, by size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Model {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
}

impl Model {
    pub const ALL: [Model; 4] = [Model::Tiny, Model::Base, Model::Small, Model::Medium];

    pub fn name(self) -> &'static str {
        match self {
            Model::Tiny => "tiny",
            Model::Base => "base",
            Model::Small => "small",
            Model::Medium => "medium",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Model::Tiny => "Tiny",
            Model::Base => "Base",
            Model::Small => "Small",
            Model::Medium => "Medium",
        }
    }

    /// The trade-off, in the words someone choosing would want.
    pub fn description(self) -> &'static str {
        match self {
            Model::Tiny => "75 MB · fastest, roughest",
            Model::Base => "142 MB · quick, usable for clear speech",
            Model::Small => "466 MB · a good balance",
            Model::Medium => "1.5 GB · slowest, most accurate",
        }
    }

    /// Approximate download size, for the confirmation before it starts.
    pub fn bytes(self) -> u64 {
        match self {
            Model::Tiny => 77_700_000,
            Model::Base => 148_000_000,
            Model::Small => 488_000_000,
            Model::Medium => 1_530_000_000,
        }
    }

    pub fn file_name(self) -> String {
        format!("ggml-{}.bin", self.name())
    }

    pub fn path_in(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.file_name())
    }

    /// Where the weights come from. Hugging Face is whisper.cpp's own
    /// distribution point for these files.
    ///
    /// Note the org: the **source** moved to `ggml-org/whisper.cpp` on GitHub, but
    /// the **models** are still under `ggerganov/whisper.cpp` on Hugging Face, and
    /// there is no `ggml-org` mirror there — it 404s. Verified 2026-08-01. Do not
    /// "tidy" these to match the GitHub org; every download would break.
    pub fn download_url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name()
        )
    }
}

/// What the transcript file should look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// Plain text, no timings.
    #[default]
    Text,
    /// SubRip, the subtitle format everything reads.
    Srt,
    /// WebVTT, for the web and for GNOME Videos.
    Vtt,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Text, Format::Srt, Format::Vtt];

    pub fn label(self) -> &'static str {
        match self {
            Format::Text => "Plain text",
            Format::Srt => "Subtitles (SRT)",
            Format::Vtt => "Subtitles (WebVTT)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Srt => "srt",
            Format::Vtt => "vtt",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Format::Text => "--output-txt",
            Format::Srt => "--output-srt",
            Format::Vtt => "--output-vtt",
        }
    }
}

/// What the user asked for when they turned the Transcribe switch on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wish {
    pub model: Model,
    pub format: Format,
    /// An ISO language code, or `None` to let whisper detect it.
    pub language: Option<String>,
}

/// Containers whisper.cpp reads directly. Everything else goes through ffmpeg.
///
/// Notably absent: `m4a`, `opus` and `webm`, which is what an audio-only
/// download usually produces, and every video container. So the conversion path
/// is the common one, not the exception.
const NATIVE: [&str; 4] = ["wav", "mp3", "flac", "ogg"];

/// Whether this file has to be converted before whisper will read it.
pub fn needs_conversion(media: &Path) -> bool {
    let extension = media
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    !NATIVE.contains(&extension.as_str())
}

/// Where the 16 kHz mono copy goes.
///
/// The old application wrote it next to the user's file, as a sibling `.wav`
/// that appeared in their Videos folder and was deleted again a few minutes
/// later. A scratch file belongs in the cache directory.
pub fn conversion_path(cache_dir: &Path, job_id: u64) -> PathBuf {
    cache_dir.join(format!("transcribe-{job_id}.wav"))
}

/// ffmpeg arguments that produce what whisper.cpp wants: 16 kHz, mono, PCM.
pub fn conversion_argv(media: &Path, wav: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        media.display().to_string(),
        // Whisper resamples internally anyway; doing it here means the model
        // never sees a rate it has to guess about.
        "-ar".into(),
        "16000".into(),
        "-ac".into(),
        "1".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        wav.display().to_string(),
    ]
}

/// The transcript file `whisper-cli` will write for this media file.
pub fn output_path(media: &Path, format: Format) -> PathBuf {
    media.with_extension(format.extension())
}

/// `whisper-cli` arguments.
///
/// `-of` takes the output path *without* an extension; whisper appends the one
/// matching the format flag. Passing a path that already ends in `.txt`
/// produces `name.txt.txt`.
pub fn argv(model_path: &Path, audio: &Path, output_stem: &Path, wish: &Wish) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        model_path.display().to_string(),
        "-f".to_string(),
        audio.display().to_string(),
        "--print-progress".to_string(),
        // Suppresses whisper's system-info and timing banners. It does *not* stop
        // it printing each transcribed segment to stdout — nothing does — so those
        // lines still arrive and are discarded by `parse_progress` returning
        // `None` for them. The file is the product; stdout is noise either way.
        "--no-prints".to_string(),
        wish.format.flag().to_string(),
        "-of".to_string(),
        output_stem.display().to_string(),
    ];

    if let Some(language) = &wish.language {
        args.push("-l".to_string());
        args.push(language.clone());
    }
    args
}

/// Percentage out of one of whisper's progress lines, if this is one.
///
/// whisper.cpp writes `whisper_print_progress_callback: progress =  35%` to
/// stderr, and does so whether or not it is a terminal.
pub fn parse_progress(line: &str) -> Option<f64> {
    // The *last* occurrence: the function name in whisper's own prefix is
    // `whisper_print_progress_callback`, so splitting at the first "progress"
    // lands in the middle of the prefix rather than at the number.
    let rest = line.rsplit_once("progress").map(|(_, rest)| rest)?;
    let digits: String = rest
        .trim_start_matches([' ', '=', ':'])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let percent: f64 = digits.parse().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent / 100.0)
}

/// The language choices offered, beyond automatic detection.
///
/// Not whisper's full ninety-nine: a list that long in a combo row is a list
/// nobody scrolls. These are the ones with the most speakers plus the ones a
/// European desktop is likely to need, and the escape hatch is that automatic
/// detection is the default and works.
pub const LANGUAGES: [(&str, &str); 16] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("zh", "Chinese"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
    ("pt", "Portuguese"),
    ("fr", "French"),
    ("de", "German"),
    ("ru", "Russian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("sv", "Swedish"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_download_formats_all_need_converting() {
        // An audio-only download is usually opus in a webm container, and a
        // video download is mkv. Whisper reads neither, so treating conversion
        // as the exceptional path would be backwards.
        for name in ["clip.webm", "clip.m4a", "clip.opus", "clip.mkv", "clip.mp4"] {
            assert!(needs_conversion(Path::new(name)), "{name}");
        }
        for name in ["clip.wav", "clip.mp3", "clip.FLAC", "clip.ogg"] {
            assert!(!needs_conversion(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_file_with_no_extension_is_converted_rather_than_assumed() {
        assert!(needs_conversion(Path::new("clip")));
    }

    #[test]
    fn the_scratch_wav_does_not_land_in_the_users_folder() {
        // The old application wrote a sibling `.wav` next to the download, so a
        // half-gigabyte file appeared in Videos and vanished again.
        let path = conversion_path(Path::new("/home/matty/.cache/magpie"), 7);
        assert!(path.starts_with("/home/matty/.cache/magpie"));
    }

    #[test]
    fn the_output_stem_carries_no_extension() {
        // `-of out.txt` with `--output-txt` produces `out.txt.txt`.
        let media = Path::new("/videos/A talk.mkv");
        let stem = media.with_extension("");
        let args = argv(
            Path::new("/models/ggml-small.bin"),
            Path::new("/cache/x.wav"),
            &stem,
            &Wish::default(),
        );
        let of = args.iter().position(|a| a == "-of").map(|i| &args[i + 1]);
        assert_eq!(of.map(String::as_str), Some("/videos/A talk"));
        assert_eq!(
            output_path(media, Format::Text),
            PathBuf::from("/videos/A talk.txt")
        );
    }

    #[test]
    fn automatic_detection_passes_no_language_flag() {
        let args = argv(
            Path::new("/m.bin"),
            Path::new("/a.wav"),
            Path::new("/out"),
            &Wish::default(),
        );
        assert!(!args.contains(&"-l".to_string()));

        let wish = Wish {
            language: Some("es".into()),
            ..Wish::default()
        };
        let args = argv(
            Path::new("/m.bin"),
            Path::new("/a.wav"),
            Path::new("/out"),
            &wish,
        );
        assert_eq!(
            args.iter()
                .position(|a| a == "-l")
                .map(|i| args[i + 1].as_str()),
            Some("es")
        );
    }

    #[test]
    fn each_format_asks_for_exactly_one_output_kind() {
        for format in Format::ALL {
            let wish = Wish {
                format,
                ..Wish::default()
            };
            let args = argv(
                Path::new("/m.bin"),
                Path::new("/a.wav"),
                Path::new("/out"),
                &wish,
            );
            let flags = args.iter().filter(|a| a.starts_with("--output-")).count();
            assert_eq!(flags, 1, "{format:?}");
        }
    }

    #[test]
    fn whispers_progress_lines_are_read_and_other_lines_are_not() {
        assert_eq!(
            parse_progress("whisper_print_progress_callback: progress =  35%"),
            Some(0.35)
        );
        assert_eq!(parse_progress("progress = 100%"), Some(1.0));
        assert_eq!(
            parse_progress("whisper_init_from_file_with_params_no_state"),
            None
        );
        // A timestamped transcript line is not a progress line.
        assert_eq!(
            parse_progress("[00:00:04.000 --> 00:00:08.000]  Hello"),
            None
        );
    }

    #[test]
    fn a_transcribed_segment_on_stdout_is_not_mistaken_for_progress() {
        // `--no-prints` silences whisper's banners but not the segments, so these
        // lines do arrive. Verified against whisper.cpp v1.9.1.
        for line in [
            "[00:00:00.000 --> 00:00:10.500]   And so, my fellow Americans, ask not",
            "whisper_full_with_state: auto-detected language: en (p = 0.98)",
        ] {
            assert_eq!(parse_progress(line), None, "{line}");
        }
    }

    #[test]
    fn the_conversion_never_waits_on_a_prompt() {
        // Without `-nostdin`, ffmpeg asked to overwrite a file blocks forever on
        // a stdin nobody is attached to, and the job looks like a hang.
        let args = conversion_argv(Path::new("/a.mkv"), Path::new("/b.wav"));
        assert!(args.contains(&"-nostdin".to_string()));
        assert!(args.contains(&"-y".to_string()));
    }
}
