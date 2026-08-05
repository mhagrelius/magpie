//! A download request, and the yt-dlp argument vector it becomes.
//!
//! This is the seam between the two halves: `model/` decides what to run, `ui/`
//! runs it. Nothing here spawns a process, so every flag combination the
//! application can produce is checkable in a unit test.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::quality::{AudioFormat, Quality};

/// The prefix on every line yt-dlp emits from Magpie's progress template.
///
/// yt-dlp interleaves its own chatter with template output on stdout, and the
/// old application's "does the line contain a pipe character" test matched
/// video titles. A sentinel that no title would contain settles it.
pub const SENTINEL: &str = "\u{1f}magpie";

/// What to fetch.
///
/// Serialisable because it is part of a queued job, and the queue outlives the
/// window: a download still waiting when the app quits comes back with the
/// format the user chose, not a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Selection {
    /// A quality preset, merging separate video and audio streams as needed.
    Video(Quality),
    /// Audio only, in the given container.
    Audio(AudioFormat),
    /// A format id straight out of `--dump-json`, for the escape hatch in the
    /// Add dialog. Passed through untouched.
    Exact(String),
}

impl Default for Selection {
    fn default() -> Self {
        Selection::Video(Quality::default())
    }
}

/// Which items of a collection to take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Folder created under the destination, already sanitised.
    pub folder: String,
    /// One-based indices, in yt-dlp's `--playlist-items` sense. Empty means
    /// every item.
    pub items: Vec<usize>,
}

/// Where cookies come from, when a site wants to know who is asking.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Cookies {
    #[default]
    None,
    /// A browser name yt-dlp understands: `firefox`, `chrome`, `chromium`,
    /// `brave`, `edge`, `opera`, `vivaldi`, `safari`, `whale`.
    FromBrowser(String),
}

/// Everything one yt-dlp invocation needs to know.
#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    pub destination: PathBuf,
    pub selection: Selection,
    pub collection: Option<Collection>,
    pub cookies: Cookies,
    /// A yt-dlp rate string such as `2M`, or none for unlimited.
    pub rate_limit: Option<String>,
    /// A JavaScript engine for YouTube's signature challenges, if one was found.
    /// See `model::tools::Tool::JsRuntime`.
    pub js_runtime: Option<PathBuf>,
    /// File that `--print-to-file after_move:filepath` writes the finished
    /// paths into, one per line.
    pub filepath_sink: PathBuf,
}

impl Request {
    /// The directory the files will actually land in.
    pub fn output_directory(&self) -> PathBuf {
        match &self.collection {
            Some(collection) => self.destination.join(&collection.folder),
            None => self.destination.clone(),
        }
    }

    /// The full argument vector, less the program name.
    pub fn argv(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // Colour codes in stderr would end up quoted verbatim in an error
        // dialog, and progress is only emitted for a terminal unless asked for.
        flags(&mut args, &["--no-color", "--newline", "--progress"]);
        flags(&mut args, &["--progress-template", DOWNLOAD_TEMPLATE]);
        flags(&mut args, &["--progress-template", POSTPROCESS_TEMPLATE]);

        // The finished path, straight from yt-dlp after any conversion and
        // move. Scraping `[download] Destination:` names the file *before*
        // post-processing, so every MP3 the old application produced reported a
        // path that no longer existed.
        flags(&mut args, &["--print-to-file", "after_move:filepath"]);
        args.push(self.filepath_sink.display().to_string());

        args.push("-P".into());
        args.push(self.output_directory().display().to_string());
        flags(&mut args, &["-o", self.output_template()]);

        match &self.collection {
            Some(collection) => {
                args.push("--yes-playlist".into());
                if !collection.items.is_empty() {
                    args.push("--playlist-items".into());
                    args.push(
                        collection
                            .items
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
            }
            // Without this, one video from inside a playlist drags the whole
            // playlist down with it.
            None => args.push("--no-playlist".into()),
        }

        match &self.selection {
            Selection::Video(quality) => {
                flags(&mut args, &["-f", quality.selector()]);
                // A merged file is only playable if the container can hold both
                // streams; mkv can hold anything.
                flags(&mut args, &["--merge-output-format", "mkv/mp4"]);
            }
            Selection::Audio(format) => args.extend(format.args()),
            Selection::Exact(format_id) => flags(&mut args, &["-f", format_id]),
        }

        if let Cookies::FromBrowser(browser) = &self.cookies {
            flags(&mut args, &["--cookies-from-browser", browser]);
        }

        if let Some(rate) = &self.rate_limit {
            flags(&mut args, &["--limit-rate", rate]);
        }

        if let Some(argument) = self.js_runtime_argument() {
            flags(&mut args, &["--js-runtimes", &argument]);
        }

        args.push(self.url.clone());
        args
    }

    /// `name:/absolute/path` for the runtime that was found.
    ///
    /// The path is given, not just the name, for two reasons. yt-dlp enables only
    /// `deno` by default, so node and bun have to be named to be used at all; and
    /// a runtime installed by a version manager — fnm, nvm, asdf — lives on a
    /// `PATH` that exists in the user's shell and not in the environment a desktop
    /// launcher hands the application. Naming the file settles both.
    fn js_runtime_argument(&self) -> Option<String> {
        let path = self.js_runtime.as_ref()?;
        let name = path.file_name()?.to_str()?;
        Some(format!("{name}:{}", path.display()))
    }

    /// The filename template, which differs for a collection because the order
    /// of a playlist is part of what the user asked for.
    ///
    /// `.150B` truncates by *bytes*, not characters: ext4 caps a filename at
    /// 255 bytes, and a title of Japanese characters spends three bytes each.
    fn output_template(&self) -> &'static str {
        match self.collection {
            Some(_) => "%(playlist_index)03d - %(title).150B.%(ext)s",
            None => "%(title).200B.%(ext)s",
        }
    }
}

fn flags(args: &mut Vec<String>, values: &[&str]) {
    args.extend(values.iter().map(|value| value.to_string()));
}

/// Arguments that ask yt-dlp for one video's metadata and nothing else.
pub fn info_argv(url: &str, collection: bool) -> Vec<String> {
    let mut args: Vec<String> = ["--no-color", "--ignore-config", "--dump-single-json"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if collection {
        // Without `--flat-playlist`, asking about a 200-video playlist fetches
        // 200 videos' metadata and takes minutes.
        args.push("--flat-playlist".into());
        args.push("--yes-playlist".into());
    } else {
        args.push("--no-playlist".into());
    }
    args.push(url.to_string());
    args
}

/// Turn a playlist title into a folder name that is safe and readable.
///
/// Stricter than Linux requires: only `/` and NUL are actually illegal on ext4,
/// but a Downloads folder is a folder people copy onto a USB stick, and a colon
/// or a question mark makes a directory that FAT and NTFS refuse. Playlist titles
/// are full of colons. Accented characters and emoji are left alone — those are
/// fine everywhere and mangling them would be vandalism, not caution.
pub fn folder_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();

    let mut collapsed = String::with_capacity(cleaned.len());
    let mut last_was_space = false;
    for c in cleaned.chars() {
        let is_space = c.is_whitespace();
        if !(is_space && last_was_space) {
            collapsed.push(if is_space { ' ' } else { c });
        }
        last_was_space = is_space;
    }

    // Truncate on a character boundary, under the 255-byte filename cap.
    let mut name = String::new();
    for c in collapsed.trim().chars() {
        if name.len() + c.len_utf8() > 120 {
            break;
        }
        name.push(c);
    }
    let name = name.trim_end().trim_start_matches('.').to_string();

    if name.is_empty() {
        "Playlist".to_string()
    } else {
        name
    }
}

const DOWNLOAD_TEMPLATE: &str = concat!(
    "download:\u{1f}magpie\tdownload\t",
    "%(progress.status)s\t",
    "%(progress.downloaded_bytes)s\t",
    "%(progress.total_bytes)s\t",
    "%(progress.total_bytes_estimate)s\t",
    "%(progress.speed)s\t",
    "%(progress.eta)s\t",
    "%(info.playlist_index)s\t",
    "%(info.n_entries)s\t",
    // Both, because they answer different questions and disagree the moment
    // anything is filtered out: `playlist_index` is where the item sits in the
    // playlist and what names its file, `playlist_autonumber` is how far into
    // the download queue it is. See `progress::Snapshot`.
    "%(info.playlist_autonumber)s"
);

const POSTPROCESS_TEMPLATE: &str = concat!(
    "postprocess:\u{1f}magpie\tpostprocess\t",
    "%(progress.status)s\t",
    "%(progress.postprocessor)s"
);

/// A per-job scratch file for `--print-to-file`, under the cache directory.
pub fn sink_path(cache_dir: &Path, job_id: u64) -> PathBuf {
    cache_dir.join(format!("job-{job_id}.paths"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            url: "https://youtu.be/dQw4w9WgXcQ".into(),
            destination: PathBuf::from("/home/matty/Videos"),
            selection: Selection::default(),
            collection: None,
            cookies: Cookies::None,
            rate_limit: None,
            js_runtime: None,
            filepath_sink: PathBuf::from("/home/matty/.cache/magpie/job-1.paths"),
        }
    }

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    #[test]
    fn the_url_is_last_so_a_link_starting_with_a_dash_cannot_become_a_flag() {
        let args = request().argv();
        assert_eq!(
            args.last().map(String::as_str),
            Some(request().url.as_str())
        );
    }

    #[test]
    fn the_finished_path_is_reported_by_yt_dlp_not_scraped() {
        // The whole reason the old application mis-reported every MP3 it made.
        let args = request().argv();
        assert_eq!(
            value_after(&args, "--print-to-file"),
            Some("after_move:filepath")
        );
    }

    #[test]
    fn a_single_video_never_drags_its_playlist_along() {
        let args = request().argv();
        assert!(args.contains(&"--no-playlist".to_string()));
    }

    #[test]
    fn a_collection_downloads_into_its_own_folder_in_order() {
        let mut req = request();
        req.collection = Some(Collection {
            folder: "Bach cantatas".into(),
            items: vec![2, 3, 5],
        });
        let args = req.argv();

        assert_eq!(
            value_after(&args, "-P"),
            Some("/home/matty/Videos/Bach cantatas")
        );
        assert_eq!(value_after(&args, "--playlist-items"), Some("2,3,5"));
        assert!(value_after(&args, "-o").unwrap().contains("playlist_index"));
    }

    #[test]
    fn taking_every_item_of_a_collection_passes_no_item_filter() {
        let mut req = request();
        req.collection = Some(Collection {
            folder: "Everything".into(),
            items: vec![],
        });
        assert_eq!(value_after(&req.argv(), "--playlist-items"), None);
    }

    #[test]
    fn an_exact_format_id_is_passed_through_untouched() {
        // The escape hatch has to be an escape hatch: whatever `--dump-json`
        // called the format is what yt-dlp gets back.
        let mut req = request();
        req.selection = Selection::Exact("616+251".into());
        assert_eq!(value_after(&req.argv(), "-f"), Some("616+251"));
    }

    #[test]
    fn merged_video_lands_in_a_container_that_can_hold_it() {
        // vp9 video plus opus audio has no legal mp4 muxing; without this the
        // merge fails after the download has already finished.
        let args = request().argv();
        assert_eq!(value_after(&args, "--merge-output-format"), Some("mkv/mp4"));
    }

    #[test]
    fn progress_is_asked_for_explicitly_because_this_is_not_a_terminal() {
        let args = request().argv();
        assert!(args.contains(&"--progress".to_string()));
        assert!(args.contains(&"--no-color".to_string()));
        assert!(args.iter().any(|a| a.starts_with("download:\u{1f}magpie")));
    }

    #[test]
    fn the_template_asks_where_the_item_sits_and_how_far_in_it_is() {
        // `--playlist-items 20,30` downloads two videos, and yt-dlp calls the
        // second one index 30 of 2 entries. Without the autonumber the row says
        // "30 of 2".
        let template = request()
            .argv()
            .into_iter()
            .find(|a| a.starts_with("download:"))
            .expect("a download template");
        assert!(template.contains("%(info.playlist_index)s"), "{template}");
        assert!(
            template.contains("%(info.playlist_autonumber)s"),
            "{template}"
        );
    }

    #[test]
    fn cookies_and_a_rate_limit_are_only_present_when_asked_for() {
        let plain = request().argv();
        assert!(!plain.iter().any(|a| a.contains("cookies")));
        assert!(!plain.contains(&"--limit-rate".to_string()));

        let mut req = request();
        req.cookies = Cookies::FromBrowser("firefox".into());
        req.rate_limit = Some("2M".into());
        let args = req.argv();
        assert_eq!(
            value_after(&args, "--cookies-from-browser"),
            Some("firefox")
        );
        assert_eq!(value_after(&args, "--limit-rate"), Some("2M"));
    }

    #[test]
    fn a_javascript_runtime_is_named_with_its_full_path() {
        // yt-dlp enables only `deno` by default, so node and bun must be named or
        // they are not used at all — and a runtime installed by fnm, nvm or asdf
        // sits on a PATH the user's shell has and a desktop launcher does not.
        // Naming the file settles both. Without it yt-dlp warns that extraction
        // is deprecated and "some formats may be missing" — a risk yt-dlp
        // documents, not one observed here; see `tools::Tool::JsRuntime`.
        let mut req = request();
        assert_eq!(
            value_after(&req.argv(), "--js-runtimes"),
            None,
            "nothing claimed when nothing was found"
        );

        req.js_runtime = Some(PathBuf::from(
            "/run/user/1000/fnm_multishells/744991_1785587718332/bin/node",
        ));
        assert_eq!(
            value_after(&req.argv(), "--js-runtimes"),
            Some("node:/run/user/1000/fnm_multishells/744991_1785587718332/bin/node")
        );

        req.js_runtime = Some(PathBuf::from("/usr/bin/deno"));
        assert_eq!(
            value_after(&req.argv(), "--js-runtimes"),
            Some("deno:/usr/bin/deno"),
            "named even though it is the default, so a deno off PATH still works"
        );
    }

    #[test]
    fn asking_about_a_playlist_does_not_fetch_every_video_in_it() {
        // `--dump-single-json` without `--flat-playlist` resolves all 200
        // entries, which takes minutes and looks like a hang.
        let args = info_argv("https://youtube.com/playlist?list=PL1", true);
        assert!(args.contains(&"--flat-playlist".to_string()));
        assert!(!info_argv("https://youtu.be/abc", false).contains(&"--flat-playlist".to_string()));
    }

    #[test]
    fn a_folder_name_survives_anything_a_playlist_title_can_contain() {
        assert_eq!(folder_name("Best of 2024/2025"), "Best of 2024-2025");
        assert_eq!(folder_name("  spaced   out  "), "spaced out");
        // Not illegal on ext4, but a folder people copy to a USB stick should
        // still copy. Playlist titles are full of colons and question marks.
        assert_eq!(folder_name("Bach: cantatas"), "Bach- cantatas");
        assert_eq!(folder_name("Why? How?"), "Why- How-");
        // Left alone: legal everywhere, and mangling them would be vandalism.
        assert_eq!(folder_name("Café — Naïve 🎵"), "Café — Naïve 🎵");
        assert_eq!(folder_name(""), "Playlist");
        assert_eq!(folder_name("..."), "Playlist");
        // A hidden folder is not what the user asked for.
        assert!(!folder_name(".hidden").starts_with('.'));
        // Multi-byte titles truncate on a character boundary, not mid-codepoint.
        let long = folder_name(&"あ".repeat(200));
        assert!(long.len() <= 120 && long.chars().all(|c| c == 'あ'));
    }
}
