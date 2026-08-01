//! What counts as a link, and what kind of link it is.
//!
//! The old application accepted an eleven-character YouTube id and nothing
//! else, while the tool underneath supports some eighteen hundred sites. The
//! grammar here is deliberately loose: anything that looks like an `http(s)`
//! URL is offered to yt-dlp, and yt-dlp gets to be the one that says no. The
//! only thing worth recognising specifically is a playlist, because that
//! changes what the Add dialog shows.

/// What a link appears to point at, before anything has been fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One item.
    Single,
    /// A playlist, channel or other container of items.
    Collection,
}

/// A link that is at least shaped like something yt-dlp could take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub kind: Kind,
}

/// Trim a pasted string and decide whether it is worth handing to yt-dlp.
///
/// Returns `None` for anything that is not an absolute `http(s)` URL. A bare
/// `youtube.com/watch?v=…` with no scheme is common enough from a copied
/// address bar that it gets `https://` prepended rather than rejected.
pub fn parse(raw: &str) -> Option<Link> {
    let text = raw.trim();
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }

    let url = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_string()
    } else if looks_like_bare_host(text) {
        format!("https://{text}")
    } else {
        return None;
    };

    // A host and something after the scheme. `https://` on its own is not a
    // link, and neither is `https://.`
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.len() < 3 || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return None;
    }

    let kind = if is_collection(&url) {
        Kind::Collection
    } else {
        Kind::Single
    };
    Some(Link { url, kind })
}

/// Whether a URL points at a playlist rather than one item.
///
/// A `watch?v=…&list=…` link is treated as a **single** video, not a playlist.
/// The old application went the other way and sent anyone who clicked a video
/// from inside a playlist into the forty-item flow they did not ask for; the
/// `v=` parameter is the specific thing they clicked, so it wins. A link with
/// only `list=` has nothing else to mean.
pub fn is_collection(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if has_query_param(&lower, "v") {
        return false;
    }
    lower.contains("/playlist?")
        || has_query_param(&lower, "list")
        || lower.contains("/@")
        || lower.contains("/channel/")
        || lower.contains("/c/")
        || lower.contains("/user/")
}

/// The YouTube video id, when there is an obvious one.
///
/// Used only to guess a thumbnail URL before `--dump-json` has answered, so
/// that the Add dialog has something to show in its first half-second. A miss
/// costs nothing but a blank rectangle.
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.").to_ascii_lowercase();

    let candidate = if host == "youtu.be" {
        path.split(['?', '#']).next().unwrap_or("")
    } else if host.ends_with("youtube.com") || host.ends_with("youtube-nocookie.com") {
        if let Some(value) = query_param(rest, "v") {
            return valid_id(&value);
        }
        let path = path.split(['?', '#']).next().unwrap_or("");
        path.strip_prefix("shorts/")
            .or_else(|| path.strip_prefix("embed/"))
            .or_else(|| path.strip_prefix("live/"))
            .or_else(|| path.strip_prefix("v/"))
            .unwrap_or("")
    } else {
        return None;
    };

    valid_id(candidate)
}

/// A poster image for a YouTube link, guessable before any request is made.
pub fn guessed_thumbnail(url: &str) -> Option<String> {
    youtube_id(url).map(|id| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
}

fn valid_id(candidate: &str) -> Option<String> {
    let id = candidate.split('/').next().unwrap_or("");
    let ok = id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| id.to_string())
}

fn looks_like_bare_host(text: &str) -> bool {
    // Reject anything with a scheme we are not going to run, and anything that
    // is plainly a file path.
    !text.contains("://") && !text.starts_with('/') && text.contains('.') && !text.contains('\\')
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split(['&', ';']) {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(value.split('#').next().unwrap_or(value).to_string());
        }
    }
    None
}

fn has_query_param(url: &str, key: &str) -> bool {
    let Some(query) = url.split_once('?').map(|(_, q)| q) else {
        return false;
    };
    query
        .split(['&', ';'])
        .filter_map(|pair| pair.split_once('='))
        .any(|(name, value)| name == key && !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_address_bar_without_a_scheme_is_still_a_link() {
        // Copying from Firefox's address bar drops `https://`. Rejecting that
        // would fail the single most common way a link arrives.
        let link = parse("youtube.com/watch?v=dQw4w9WgXcQ").expect("a link");
        assert_eq!(link.url, "https://youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn a_link_from_any_site_is_accepted() {
        // yt-dlp supports ~1800 sites. Deciding which ones are real is its job,
        // not a regex's.
        for url in [
            "https://vimeo.com/76979871",
            "https://www.bbc.co.uk/programmes/b006q2x0",
            "https://example.museum/talk/17",
        ] {
            assert_eq!(parse(url).map(|l| l.kind), Some(Kind::Single), "{url}");
        }
    }

    #[test]
    fn prose_and_paths_are_not_links() {
        for text in [
            "",
            "   ",
            "how do I download a video",
            "/home/matty/clip.mp4",
            "file:///home/matty/clip.mp4",
            "https://",
            "https://.",
            "https://localhost",
        ] {
            assert_eq!(parse(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn a_video_inside_a_playlist_is_a_video() {
        // The old application routed `watch?v=…&list=…` into the playlist flow,
        // so clicking one episode offered to download the whole series. The
        // thing the user clicked is the `v=`.
        let link = parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL1234").expect("link");
        assert_eq!(link.kind, Kind::Single);
    }

    #[test]
    fn a_bare_list_link_is_a_collection() {
        for url in [
            "https://www.youtube.com/playlist?list=PLabc",
            "https://www.youtube.com/@someone",
            "https://www.youtube.com/channel/UC123",
        ] {
            assert_eq!(parse(url).map(|l| l.kind), Some(Kind::Collection), "{url}");
        }
    }

    #[test]
    fn a_youtube_id_is_found_in_every_shape_youtube_uses() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
            "https://www.youtube.com/watch?list=PL1&v=dQw4w9WgXcQ&t=3",
        ] {
            assert_eq!(youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
        }
    }

    #[test]
    fn a_thumbnail_guess_is_absent_rather_than_wrong_for_other_sites() {
        // A blank rectangle for half a second is fine; a broken image is not.
        assert_eq!(guessed_thumbnail("https://vimeo.com/76979871"), None);
        assert!(guessed_thumbnail("https://youtu.be/dQw4w9WgXcQ").is_some());
    }
}
