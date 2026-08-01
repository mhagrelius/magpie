//! Reading `yt-dlp --dump-single-json`.
//!
//! Every field here is treated as optional, including the ones yt-dlp always
//! sends. That is not defensiveness for its own sake: `--dump-json` is a dump of
//! an internal dictionary whose keys change between releases, across
//! extractors, and between a video and a livestream. A `#[derive(Deserialize)]`
//! struct over it is a struct that stops working the week a field is renamed,
//! and it would fail the whole parse over a field the Add dialog only uses for
//! a subtitle.

use serde_json::Value;

/// One downloadable stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Format {
    /// yt-dlp's `format_id`, passed back verbatim when the user picks this one.
    pub id: String,
    pub ext: String,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Bytes, exact where the server said so and estimated otherwise.
    pub filesize: Option<u64>,
    /// Total bitrate in kbps.
    pub bitrate: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
}

impl Format {
    /// The line shown in the format list.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if self.has_video {
            parts.push(match self.height {
                Some(height) => format!("{height}p"),
                None => "Video".to_string(),
            });
            if let Some(fps) = self.fps.filter(|f| *f >= 50.0) {
                // 60 fps is worth calling out; 30 is the assumption.
                parts.push(format!("{} fps", fps.round()));
            }
        } else if self.has_audio {
            parts.push("Audio".to_string());
            if let Some(bitrate) = self.bitrate {
                parts.push(format!("{} kbps", bitrate.round()));
            }
        }

        parts.push(self.ext.to_ascii_uppercase());

        if let Some(size) = self.filesize {
            parts.push(super::progress::format_bytes(size));
        }
        // A video-only stream will be merged with an audio one, and saying so
        // is the difference between the list looking broken and looking honest.
        if self.has_video && !self.has_audio {
            parts.push("video only, audio added".to_string());
        }

        parts.join(" · ")
    }
}

/// One video.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Media {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    /// Seconds. Absent for a livestream, and for sites that do not say.
    pub duration: Option<u64>,
    pub thumbnail: Option<String>,
    pub is_live: bool,
    pub formats: Vec<Format>,
    pub url: String,
}

/// One item of a playlist, as `--flat-playlist` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// One-based, and the number `--playlist-items` expects.
    pub index: usize,
    pub title: String,
    pub duration: Option<u64>,
    pub url: String,
}

/// A playlist, channel, or anything else yt-dlp calls a playlist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Playlist {
    pub title: String,
    pub uploader: Option<String>,
    pub entries: Vec<Entry>,
    pub url: String,
}

/// What the link turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Info {
    Single(Media),
    Collection(Playlist),
}

/// Nothing usable came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// stdout was not JSON. Usually because yt-dlp wrote an error instead.
    NotJson,
    /// It was JSON, but nothing that names a video.
    NoMedia,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NotJson => write!(f, "yt-dlp did not return video information"),
            ParseError::NoMedia => write!(f, "the link does not point at a video"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse one `--dump-single-json` document.
pub fn parse(stdout: &str) -> Result<Info, ParseError> {
    // yt-dlp may print a warning line before the JSON, so start at the first
    // brace rather than assuming the whole of stdout is the document.
    let start = stdout.find('{').ok_or(ParseError::NotJson)?;
    let root: Value = serde_json::from_str(&stdout[start..]).map_err(|_| ParseError::NotJson)?;

    let is_collection = root.get("_type").and_then(Value::as_str) == Some("playlist")
        || root.get("entries").is_some_and(Value::is_array);

    if is_collection {
        Ok(Info::Collection(playlist(&root)?))
    } else {
        Ok(Info::Single(media(&root)?))
    }
}

fn media(root: &Value) -> Result<Media, ParseError> {
    let title = string(root, "title")
        .or_else(|| string(root, "fulltitle"))
        .ok_or(ParseError::NoMedia)?;

    let mut formats: Vec<Format> = root
        .get("formats")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(format).collect())
        .unwrap_or_default();
    sort_formats(&mut formats);

    Ok(Media {
        id: string(root, "id").unwrap_or_default(),
        title,
        // Sites disagree about which of these three they populate.
        uploader: string(root, "uploader")
            .or_else(|| string(root, "channel"))
            .or_else(|| string(root, "uploader_id")),
        duration: number(root, "duration").map(|d| d as u64),
        thumbnail: string(root, "thumbnail").or_else(|| best_thumbnail(root)),
        // `is_live` is absent on most sites and `null` on some of the rest.
        is_live: root
            .get("is_live")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        formats,
        url: string(root, "webpage_url")
            .or_else(|| string(root, "original_url"))
            .unwrap_or_default(),
    })
}

fn playlist(root: &Value) -> Result<Playlist, ParseError> {
    let entries: Vec<Entry> = root
        .get("entries")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                // A playlist can contain an entry that is `null`: a video
                // deleted since the playlist was made. Numbering has to skip it
                // rather than renumber, because `--playlist-items` counts the
                // gap too.
                .enumerate()
                .filter_map(|(offset, item)| entry(item, offset + 1))
                .collect()
        })
        .unwrap_or_default();

    if entries.is_empty() {
        return Err(ParseError::NoMedia);
    }

    Ok(Playlist {
        title: string(root, "title")
            .or_else(|| string(root, "playlist_title"))
            .unwrap_or_else(|| "Playlist".to_string()),
        uploader: string(root, "uploader")
            .or_else(|| string(root, "channel"))
            .or_else(|| string(root, "uploader_id")),
        entries,
        url: string(root, "webpage_url")
            .or_else(|| string(root, "original_url"))
            .unwrap_or_default(),
    })
}

fn entry(item: &Value, index: usize) -> Option<Entry> {
    if !item.is_object() {
        return None;
    }
    let id = string(item, "id");
    let url = string(item, "url")
        .or_else(|| string(item, "webpage_url"))
        // `--flat-playlist` gives a bare id for some extractors.
        .or_else(|| {
            id.as_ref()
                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        })?;

    Some(Entry {
        index,
        title: string(item, "title").unwrap_or_else(|| format!("Item {index}")),
        duration: number(item, "duration").map(|d| d as u64),
        url,
    })
}

fn format(value: &Value) -> Option<Format> {
    let id = string(value, "format_id")?;
    let vcodec = string(value, "vcodec").unwrap_or_else(|| "none".into());
    let acodec = string(value, "acodec").unwrap_or_else(|| "none".into());
    let has_video = vcodec != "none";
    let has_audio = acodec != "none";

    // A storyboard has neither, and appears in `formats` regardless.
    if !has_video && !has_audio {
        return None;
    }

    Some(Format {
        id,
        ext: string(value, "ext").unwrap_or_else(|| "?".into()),
        height: number(value, "height").map(|h| h as u32).filter(|h| *h > 0),
        fps: number(value, "fps"),
        filesize: number(value, "filesize")
            .or_else(|| number(value, "filesize_approx"))
            .map(|s| s as u64),
        bitrate: number(value, "abr").or_else(|| number(value, "tbr")),
        has_video,
        has_audio,
    })
}

/// Video first, tallest first; audio after, loudest first.
fn sort_formats(formats: &mut [Format]) {
    formats.sort_by(|a, b| {
        b.has_video
            .cmp(&a.has_video)
            .then(b.height.unwrap_or(0).cmp(&a.height.unwrap_or(0)))
            .then(
                b.bitrate
                    .unwrap_or(0.0)
                    .partial_cmp(&a.bitrate.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.id.cmp(&b.id))
    });
}

/// The largest thumbnail in the `thumbnails` array, when there is no top-level
/// `thumbnail` key.
fn best_thumbnail(root: &Value) -> Option<String> {
    let list = root.get("thumbnails")?.as_array()?;
    list.iter()
        .filter(|t| t.get("url").is_some())
        .max_by_key(|t| number(t, "width").unwrap_or(0.0) as u64)
        .and_then(|t| string(t, "url"))
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "NA")
        .map(str::to_string)
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64().filter(|n| n.is_finite())
}

/// `4:32`, or `1:02:11` when it runs past an hour.
pub fn format_duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_with_nothing_but_a_title_still_parses() {
        // Every other field is optional in practice, and losing the whole
        // parse over a missing `uploader` would mean the Add dialog cannot open
        // for a site that does not report one.
        let Info::Single(media) = parse(r#"{"title": "Just a title"}"#).expect("parses") else {
            panic!("expected a single video");
        };
        assert_eq!(media.title, "Just a title");
        assert_eq!(media.uploader, None);
        assert_eq!(media.duration, None);
        assert!(media.formats.is_empty());
    }

    #[test]
    fn a_warning_printed_before_the_json_does_not_break_the_parse() {
        // yt-dlp writes some warnings to stdout.
        let stdout = "WARNING: Falling back on generic information extractor\n\
                      {\"title\": \"A talk\"}";
        assert!(parse(stdout).is_ok());
    }

    #[test]
    fn an_error_instead_of_json_is_reported_as_such() {
        assert_eq!(parse("ERROR: Unsupported URL"), Err(ParseError::NotJson));
        assert_eq!(parse(""), Err(ParseError::NotJson));
        assert_eq!(parse(r#"{"id": "abc"}"#), Err(ParseError::NoMedia));
    }

    #[test]
    fn a_storyboard_is_not_offered_as_a_format() {
        // `formats` contains image storyboards with no codecs at all. Listing
        // them as a download choice is how you get a folder of JPEGs.
        let json = r#"{"title": "T", "formats": [
            {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none"},
            {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "abr": 128}
        ]}"#;
        let Info::Single(media) = parse(json).unwrap() else {
            panic!()
        };
        assert_eq!(media.formats.len(), 1);
        assert_eq!(media.formats[0].id, "251");
    }

    #[test]
    fn video_formats_are_listed_before_audio_tallest_first() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "a", "ext": "m4a", "vcodec": "none", "acodec": "aac", "abr": 128},
            {"format_id": "720", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 720},
            {"format_id": "1080", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 1080}
        ]}"#;
        let Info::Single(media) = parse(json).unwrap() else {
            panic!()
        };
        let ids: Vec<&str> = media.formats.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["1080", "720", "a"]);
    }

    #[test]
    fn a_video_only_format_says_that_audio_will_be_added() {
        // Otherwise a 1080p entry looks like it produces a silent file, which
        // is what the old application's users concluded and why its picker
        // hid them.
        let format = Format {
            id: "137".into(),
            ext: "mp4".into(),
            height: Some(1080),
            fps: Some(60.0),
            filesize: Some(250_000_000),
            bitrate: None,
            has_video: true,
            has_audio: false,
        };
        let label = format.label();
        assert!(label.contains("1080p"), "{label}");
        assert!(label.contains("60 fps"), "{label}");
        assert!(label.contains("audio added"), "{label}");
    }

    #[test]
    fn a_deleted_playlist_entry_does_not_renumber_the_ones_after_it() {
        // `--playlist-items` counts the gap, so entry three must stay entry
        // three or the wrong videos get downloaded.
        let json = r#"{"_type": "playlist", "title": "Mixtape", "entries": [
            {"id": "aaaaaaaaaaa", "title": "One"},
            null,
            {"id": "ccccccccccc", "title": "Three"}
        ]}"#;
        let Info::Collection(playlist) = parse(json).unwrap() else {
            panic!()
        };
        assert_eq!(playlist.entries.len(), 2);
        assert_eq!(playlist.entries[1].index, 3);
        assert_eq!(playlist.entries[1].title, "Three");
    }

    #[test]
    fn a_playlist_is_recognised_from_its_entries_alone() {
        // Not every extractor sets `_type`.
        let json = r#"{"title": "Series", "entries": [{"id": "x", "url": "https://e/1"}]}"#;
        assert!(matches!(parse(json), Ok(Info::Collection(_))));
    }

    #[test]
    fn the_largest_thumbnail_is_used_when_there_is_no_single_one() {
        let json = r#"{"title": "T", "thumbnails": [
            {"url": "small.jpg", "width": 120},
            {"url": "large.jpg", "width": 1280}
        ]}"#;
        let Info::Single(media) = parse(json).unwrap() else {
            panic!()
        };
        assert_eq!(media.thumbnail.as_deref(), Some("large.jpg"));
    }

    #[test]
    fn a_livestream_is_flagged_rather_than_given_a_duration_of_zero() {
        let json = r#"{"title": "Live now", "is_live": true, "duration": null}"#;
        let Info::Single(media) = parse(json).unwrap() else {
            panic!()
        };
        assert!(media.is_live);
        assert_eq!(media.duration, None);
    }

    #[test]
    fn durations_read_as_clock_times() {
        assert_eq!(format_duration(272), "4:32");
        assert_eq!(format_duration(3731), "1:02:11");
        assert_eq!(format_duration(0), "0:00");
    }
}
