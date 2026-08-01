//! Why a download stopped, and what the person in front of the screen can do
//! about it.
//!
//! The old application's entire error model was `stderr.includes('ERROR')`,
//! which meant every failure surfaced as a wall of yt-dlp's own output. Almost
//! all of those failures have exactly one remedy, and the remedy is the useful
//! part. Classification is substring matching against yt-dlp's messages, which
//! is unavoidable — it does not emit error codes — but the substrings are kept
//! in one place and each one is paired with the sentence that fixes it.

/// Why a job ended badly.
///
/// Serialisable so that a failed job still says why after a restart, rather
/// than coming back as a row with a red icon and no explanation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    /// yt-dlp is not installed, or not where it was last seen.
    ToolMissing,
    /// The site refused to serve without proof of a signed-in human. On
    /// YouTube this is the "confirm you're not a bot" wall.
    SignInRequired,
    AgeRestricted,
    /// Removed, private, or never public.
    Unavailable,
    GeoBlocked,
    /// Extraction broke in the way a stale yt-dlp breaks.
    ExtractorOutOfDate,
    /// The requested format does not exist for this video.
    FormatUnavailable,
    /// Merging or converting needed ffmpeg, and there is none.
    FfmpegMissing,
    NoSpace,
    PermissionDenied,
    Network,
    /// Stopped on purpose.
    Cancelled,
    /// Anything else. Carries the last thing yt-dlp said, for the details
    /// expander — never for the headline.
    Unknown(String),
}

impl Failure {
    /// The one-line status shown on the row. Sentence case, no full stop.
    pub fn title(&self) -> &'static str {
        match self {
            Failure::ToolMissing => "yt-dlp is not installed",
            Failure::SignInRequired => "The site asked for a signed-in account",
            Failure::AgeRestricted => "This video is age restricted",
            Failure::Unavailable => "This video is not available",
            Failure::GeoBlocked => "Not available in your country",
            Failure::ExtractorOutOfDate => "yt-dlp could not read the page",
            Failure::FormatUnavailable => "That quality is not available",
            Failure::FfmpegMissing => "FFmpeg is not installed",
            Failure::NoSpace => "The disk is full",
            Failure::PermissionDenied => "Magpie cannot write to that folder",
            Failure::Network => "No connection",
            Failure::Cancelled => "Cancelled",
            Failure::Unknown(_) => "Download failed",
        }
    }

    /// What to do about it. Shown under the title in the error dialog.
    pub fn guidance(&self) -> &'static str {
        match self {
            Failure::ToolMissing => {
                // No command named here on purpose. The Tools page knows which
                // installer this machine has and can offer to run it; a second
                // copy of that advice in a dialog would be the one that goes
                // stale.
                "Magpie downloads through yt-dlp, which is not installed. \
                 Preferences → Tools will set it up."
            }
            Failure::SignInRequired => {
                "Turn on “Use cookies from a browser” in Preferences and pick the browser \
                 you are signed in with."
            }
            Failure::AgeRestricted => {
                "Age restricted videos need a signed-in account. Turn on “Use cookies from a \
                 browser” in Preferences and pick the browser you are signed in with."
            }
            Failure::Unavailable => {
                "The video may have been removed, made private, or never published. \
                 Opening the link in a browser will say which."
            }
            Failure::GeoBlocked => "The uploader has restricted this video to certain countries.",
            Failure::ExtractorOutOfDate => {
                "This usually means yt-dlp is too old for the site’s current pages. \
                 Check its version on the Tools page in Preferences."
            }
            Failure::FormatUnavailable => {
                "Try “Best available” instead, or pick a specific format from the list.                  If the quality you wanted is not offered at all, check that a                  JavaScript engine is listed on the Tools page — YouTube needs one to                  reveal every format."
            }
            Failure::FfmpegMissing => {
                "Merging high quality video and converting audio both need FFmpeg. \
                 Install it with “sudo apt install ffmpeg”, or choose “Best available” audio, \
                 which needs no conversion."
            }
            Failure::NoSpace => "Free some space, or choose another folder, then try again.",
            Failure::PermissionDenied => {
                "Choose another folder in Preferences, or change the folder’s permissions."
            }
            Failure::Network => "Check your connection and try again.",
            Failure::Cancelled => "",
            Failure::Unknown(_) => "The details below are yt-dlp’s own report.",
        }
    }

    /// Whether trying the same thing again could plausibly work.
    ///
    /// Retry is offered for these and withheld for the rest, because a Retry
    /// button that cannot succeed is a button that wastes the user's time
    /// twice.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Failure::Network | Failure::Unknown(_) | Failure::Cancelled | Failure::NoSpace
        )
    }

    /// yt-dlp's own words, when there are any worth showing.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Failure::Unknown(detail) => Some(detail),
            _ => None,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title())
    }
}

impl std::error::Error for Failure {}

/// Read a cause out of everything yt-dlp said.
///
/// Order matters: the specific causes are tested before the general ones, so
/// "unable to download webpage: HTTP Error 403" caused by a bot check is not
/// reported as a network problem.
pub fn classify(stderr: &str) -> Failure {
    let text = stderr.to_ascii_lowercase();
    let contains_any = |needles: &[&str]| needles.iter().any(|n| text.contains(n));

    if contains_any(&[
        "confirm you're not a bot",
        "confirm you’re not a bot",
        "sign in to confirm",
    ]) {
        // "Sign in to confirm your age" also matches here, so age has to be
        // separated out rather than falling through.
        if text.contains("your age") {
            return Failure::AgeRestricted;
        }
        return Failure::SignInRequired;
    }
    if contains_any(&[
        "age-restricted",
        "age restricted",
        "inappropriate for some users",
    ]) {
        return Failure::AgeRestricted;
    }
    if contains_any(&[
        "private video",
        "members-only",
        "this channel's members",
        "join this channel",
        "sign in if you've been granted access",
    ]) {
        return Failure::SignInRequired;
    }
    if contains_any(&[
        // YouTube's phrasing is "has not made this video available in your
        // country", so the needle cannot start with "not available".
        "available in your country",
        "not available from your location",
        "geo restricted",
        "geo-restricted",
        "blocked it in your country",
    ]) {
        return Failure::GeoBlocked;
    }
    if contains_any(&[
        "video unavailable",
        "has been removed",
        "this video is unavailable",
        "account associated with this video has been terminated",
        "video has been removed by the uploader",
        "this video is no longer available",
    ]) {
        return Failure::Unavailable;
    }
    if contains_any(&[
        "ffmpeg not found",
        "ffmpeg is not installed",
        "ffprobe and ffmpeg not found",
        "ffmpeg could not be found",
    ]) {
        return Failure::FfmpegMissing;
    }
    if contains_any(&[
        "requested format is not available",
        "requested format not available",
    ]) {
        return Failure::FormatUnavailable;
    }
    if contains_any(&[
        "nsig extraction failed",
        "unable to extract",
        "please report this issue",
        "update to the latest version",
        "signature extraction failed",
        "player response",
    ]) {
        return Failure::ExtractorOutOfDate;
    }
    if contains_any(&["no space left", "errno 28"]) {
        return Failure::NoSpace;
    }
    if contains_any(&["permission denied", "errno 13", "read-only file system"]) {
        return Failure::PermissionDenied;
    }
    if contains_any(&[
        "unable to download webpage",
        "name or service not known",
        "temporary failure in name resolution",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "timed out",
        "errno -2",
        "errno -3",
        "ssl",
    ]) {
        return Failure::Network;
    }

    Failure::Unknown(last_error_line(stderr))
}

/// The last `ERROR:` line, or the last line of any kind.
///
/// yt-dlp prints warnings before the error that stopped it, and the last one is
/// the one that did.
fn last_error_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find(|line| line.starts_with("ERROR:"))
        .or_else(|| lines.last())
        .map(|line| line.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| "yt-dlp exited without saying why".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bot_wall_sends_you_to_the_setting_that_fixes_it() {
        // By far the most common YouTube failure, and the one where relaying
        // yt-dlp's text helps least.
        let stderr = "ERROR: [youtube] abc: Sign in to confirm you're not a bot. \
                      Use --cookies-from-browser or --cookies";
        assert_eq!(classify(stderr), Failure::SignInRequired);
        assert!(classify(stderr).guidance().contains("cookies"));
    }

    #[test]
    fn an_age_wall_is_not_reported_as_a_generic_sign_in() {
        // Both start with "Sign in to confirm", so the general branch would
        // swallow this one if it were tested first.
        let stderr = "ERROR: [youtube] abc: Sign in to confirm your age. \
                      This video may be inappropriate for some users.";
        assert_eq!(classify(stderr), Failure::AgeRestricted);
    }

    #[test]
    fn a_bot_wall_behind_an_http_error_is_not_reported_as_a_network_problem() {
        // "unable to download webpage" appears in the same stderr, and telling
        // someone to check their connection when their connection is fine is
        // the specific failure this ordering prevents.
        let stderr = "WARNING: unable to download webpage: HTTP Error 403: Forbidden\n\
                      ERROR: [youtube] abc: Sign in to confirm you're not a bot.";
        assert_eq!(classify(stderr), Failure::SignInRequired);
    }

    #[test]
    fn each_cause_the_user_can_fix_says_how() {
        for failure in [
            Failure::ToolMissing,
            Failure::SignInRequired,
            Failure::AgeRestricted,
            Failure::FfmpegMissing,
            Failure::NoSpace,
            Failure::PermissionDenied,
            Failure::Network,
            Failure::FormatUnavailable,
        ] {
            assert!(
                !failure.guidance().is_empty(),
                "{failure:?} has no remedy to offer"
            );
        }
    }

    #[test]
    fn retry_is_offered_only_where_it_could_work() {
        // A private video will still be private in ten seconds.
        assert!(!Failure::Unavailable.is_retryable());
        assert!(!Failure::SignInRequired.is_retryable());
        assert!(!Failure::FfmpegMissing.is_retryable());
        assert!(Failure::Network.is_retryable());
    }

    #[test]
    fn the_recognised_causes_are_recognised() {
        let cases = [
            ("ERROR: [youtube] abc: Video unavailable", Failure::Unavailable),
            (
                "ERROR: [youtube] abc: The uploader has not made this video available in your country",
                Failure::GeoBlocked,
            ),
            (
                "ERROR: You have requested merging of multiple formats but ffmpeg is not installed",
                Failure::FfmpegMissing,
            ),
            (
                "ERROR: [youtube] abc: Requested format is not available",
                Failure::FormatUnavailable,
            ),
            (
                "WARNING: [youtube] abc: nsig extraction failed: Some formats may be missing",
                Failure::ExtractorOutOfDate,
            ),
            (
                "ERROR: unable to open for writing: [Errno 28] No space left on device",
                Failure::NoSpace,
            ),
            (
                "ERROR: unable to open for writing: [Errno 13] Permission denied",
                Failure::PermissionDenied,
            ),
            (
                "ERROR: unable to download webpage: <urlopen error [Errno -3] \
                 Temporary failure in name resolution>",
                Failure::Network,
            ),
        ];
        for (stderr, expected) in cases {
            assert_eq!(classify(stderr), expected, "{stderr}");
        }
    }

    #[test]
    fn an_unrecognised_failure_keeps_the_line_that_caused_it() {
        // The headline stays generic; the detail expander gets yt-dlp's words.
        let stderr = "WARNING: something harmless\n\
                      ERROR: the flux capacitor is misaligned\n\
                      cleaning up";
        let failure = classify(stderr);
        assert_eq!(
            failure.detail(),
            Some("the flux capacitor is misaligned"),
            "the last ERROR line is the one that stopped it"
        );
    }

    #[test]
    fn silence_is_still_reported_as_something() {
        assert!(matches!(classify(""), Failure::Unknown(_)));
        assert!(!classify("").title().is_empty());
    }
}
