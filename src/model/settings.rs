//! What the user chose, and where it lives.
//!
//! Three buckets, and the difference matters. **Settings** are what the
//! preferences dialog writes: they persist, and they are the defaults every new
//! download starts from. **Window geometry** persists too but is not a
//! preference — nobody sets it deliberately. **A job's choices** are neither:
//! they belong to the job, are copied out of the settings when it is created,
//! and do not change when the settings do.
//!
//! Loading never writes. See [`super::store`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::quality::{AudioFormat, Quality};
use super::queue::DEFAULT_PARALLELISM;
use super::transcript;

/// `~/.config/magpie/config.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    /// Where downloads go. `None` means the XDG download directory, resolved
    /// each time rather than baked in — so a user who later points
    /// `XDG_DOWNLOAD_DIR` somewhere else is followed rather than overridden.
    pub download_directory: Option<PathBuf>,
    pub quality: Quality,
    pub audio_format: AudioFormat,
    /// Whether the Add dialog opens with Audio only already on.
    pub audio_only: bool,
    /// Whether adding a link opens the dialog at all, or just starts.
    pub confirm_each_download: bool,
    pub simultaneous_downloads: usize,
    /// A browser name for `--cookies-from-browser`, or `None`.
    ///
    /// This is the one setting that fixes the most common failure there is, so
    /// the error dialog for a sign-in wall names it directly.
    pub cookies_from_browser: Option<String>,
    /// A yt-dlp rate string such as `2M`.
    pub rate_limit: Option<String>,
    pub transcript: transcript::Wish,
    /// Whether the Transcribe switch starts on.
    pub transcribe_by_default: bool,
    /// An explicit yt-dlp to use instead of whatever is on `PATH`.
    pub ytdlp_path: Option<PathBuf>,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_directory: None,
            quality: Quality::default(),
            audio_format: AudioFormat::default(),
            audio_only: false,
            // On, because the format and the folder are worth a glance and
            // because the dialog is where a playlist gets narrowed down. The
            // switch is there for people who always want the same thing.
            confirm_each_download: true,
            simultaneous_downloads: DEFAULT_PARALLELISM,
            cookies_from_browser: None,
            rate_limit: None,
            transcript: transcript::Wish::default(),
            transcribe_by_default: false,
            ytdlp_path: None,
            window_width: 720,
            window_height: 640,
            window_maximized: false,
        }
    }
}

impl Settings {
    /// Where the file lives.
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("config.json")
    }

    /// The download directory, resolved.
    ///
    /// `fallback` is the XDG download directory, which `ui/` looks up because
    /// `model/` does not link GLib.
    pub fn resolved_download_directory(&self, fallback: &Path) -> PathBuf {
        self.download_directory
            .clone()
            .unwrap_or_else(|| fallback.to_path_buf())
    }

    /// Clamp anything a hand-edited file could have got wrong.
    ///
    /// A config file is a text file, and a text file gets edited. A zero here
    /// would be a queue that never starts anything.
    pub fn sanitised(mut self) -> Self {
        self.simultaneous_downloads = self
            .simultaneous_downloads
            .clamp(1, super::queue::MAX_PARALLELISM);
        // A non-positive size is not a small window, it is a size that was never
        // recorded — GTK reports 0 for a window that is already unmapped, which is
        // what shutdown used to read. Clamping that to the minimum produced a
        // 360x294 window on the next launch and looked like Magpie had forgotten
        // how big it should be. Fall back to the default instead, which also
        // repairs a config file already written with zeroes.
        let default = Self::default();
        if self.window_width <= 0 {
            self.window_width = default.window_width;
        }
        if self.window_height <= 0 {
            self.window_height = default.window_height;
        }
        self.window_width = self.window_width.clamp(360, 10_000);
        self.window_height = self.window_height.clamp(294, 10_000);
        self.rate_limit = self.rate_limit.filter(|rate| is_rate(rate));
        self.cookies_from_browser = self
            .cookies_from_browser
            .filter(|browser| BROWSERS.contains(&browser.as_str()));
        self
    }

    /// The `Cookies` value for a request.
    pub fn cookies(&self) -> super::request::Cookies {
        match &self.cookies_from_browser {
            Some(browser) => super::request::Cookies::FromBrowser(browser.clone()),
            None => super::request::Cookies::None,
        }
    }
}

/// Browsers yt-dlp can read cookies out of.
///
/// A fixed list rather than a text entry, because the failure mode of a typo is
/// a download that fails with a message about the cookie store rather than about
/// the typo.
pub const BROWSERS: [&str; 8] = [
    "firefox", "chrome", "chromium", "brave", "edge", "opera", "vivaldi", "whale",
];

/// A yt-dlp rate limit: digits, optionally fractional, optionally `K`/`M`/`G`.
fn is_rate(text: &str) -> bool {
    let (digits, suffix) = match text.strip_suffix(['K', 'M', 'G', 'k', 'm', 'g']) {
        Some(digits) => (digits, true),
        None => (text, false),
    };
    !digits.is_empty()
        && digits.parse::<f64>().is_ok_and(|n| n > 0.0)
        // A bare number is bytes per second, so "50" is a limit nobody means.
        && (suffix || digits.parse::<f64>().is_ok_and(|n| n >= 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_work_on_a_machine_with_nothing_installed() {
        // The default audio format is a copy rather than a transcode and the
        // default video preset merges with ffmpeg only when it has to, so a
        // first run needs yt-dlp and nothing else.
        let settings = Settings::default();
        assert!(!settings.audio_format.needs_ffmpeg());
        assert!(!settings.audio_only);
        assert_eq!(settings.cookies_from_browser, None);
    }

    #[test]
    fn the_download_folder_is_resolved_each_time_rather_than_stored() {
        // Storing the resolved path would freeze a user's Downloads folder at
        // whatever it was on first run, and ignore them moving it.
        let settings = Settings::default();
        assert_eq!(settings.download_directory, None);
        assert_eq!(
            settings.resolved_download_directory(Path::new("/home/matty/Downloads")),
            PathBuf::from("/home/matty/Downloads")
        );
    }

    #[test]
    fn a_hand_edited_file_cannot_stall_the_queue() {
        let settings = Settings {
            simultaneous_downloads: 0,
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(settings.simultaneous_downloads, 1);
    }

    #[test]
    fn a_hand_edited_file_cannot_produce_an_unusable_window() {
        let settings = Settings {
            window_width: 1,
            window_height: 5,
            ..Settings::default()
        }
        .sanitised();
        assert!(settings.window_width >= 360 && settings.window_height >= 294);
    }

    #[test]
    fn a_size_that_was_never_recorded_comes_back_as_the_default() {
        // GTK reports 0 for an unmapped window, which is what shutdown read; the
        // result was a config full of zeroes and a 360x294 window on next launch.
        // Zero means "unknown", not "tiny".
        let settings = Settings {
            window_width: 0,
            window_height: 0,
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(settings.window_width, Settings::default().window_width);
        assert_eq!(settings.window_height, Settings::default().window_height);
    }

    #[test]
    fn a_nonsense_browser_name_is_dropped_rather_than_passed_on() {
        // yt-dlp rejects an unknown browser with an error about cookie stores,
        // which reads as a bug in Magpie.
        let settings = Settings {
            cookies_from_browser: Some("netscape".into()),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(settings.cookies_from_browser, None);

        let settings = Settings {
            cookies_from_browser: Some("firefox".into()),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(settings.cookies_from_browser.as_deref(), Some("firefox"));
    }

    #[test]
    fn a_rate_limit_is_kept_only_if_yt_dlp_would_understand_it() {
        for good in ["2M", "500K", "1.5M", "4G", "2048"] {
            assert!(is_rate(good), "{good}");
        }
        for bad in ["", "fast", "2MB", "-1M", "0", "50"] {
            assert!(!is_rate(bad), "{bad}");
        }
    }

    #[test]
    fn an_unknown_key_in_the_file_does_not_lose_the_rest_of_it() {
        // A config written by a later version, or by hand with a typo. Failing
        // the parse would reset every other preference the user set.
        let json = r#"{"quality": "up-to-720", "favourite-colour": "blue"}"#;
        let settings: Settings = serde_json::from_str(json).expect("parses");
        assert_eq!(settings.quality, Quality::UpTo720);
        assert_eq!(settings.window_width, Settings::default().window_width);
    }

    #[test]
    fn an_empty_file_is_every_default() {
        let settings: Settings = serde_json::from_str("{}").expect("parses");
        assert_eq!(settings, Settings::default());
    }
}
