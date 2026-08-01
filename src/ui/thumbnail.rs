//! Poster images.
//!
//! A thumbnail is worth fetching and not worth waiting for: a row shows a
//! placeholder immediately and swaps in the picture whenever it arrives, and if
//! it never arrives the placeholder is the answer. Nothing here reports an
//! error, because there is no error here a user could act on.
//!
//! Images are cached under `~/.cache/magpie/thumbnails` so that reopening the
//! window does not re-fetch the whole history, and so that a finished download
//! keeps its picture after the site has forgotten the video.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use soup::prelude::*;

/// Fetches, caches and hands out textures.
///
/// One per application. Holds the in-memory map so that forty rows showing the
/// same channel's art decode it once.
#[derive(Clone)]
pub struct Cache {
    directory: PathBuf,
    session: soup::Session,
    textures: Rc<RefCell<HashMap<String, gdk::Texture>>>,
    /// URLs already being fetched, so that a list rebuild does not start the
    /// same request a second time.
    in_flight: Rc<RefCell<Vec<String>>>,
}

impl Cache {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            directory: cache_dir.join("thumbnails"),
            session: soup::Session::new(),
            textures: Rc::new(RefCell::new(HashMap::new())),
            in_flight: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// A texture for `url` if one is already decoded, without fetching.
    pub fn peek(&self, url: &str) -> Option<gdk::Texture> {
        self.textures.borrow().get(url).cloned()
    }

    /// Ask for a texture, calling `deliver` now or later, once, if it arrives.
    ///
    /// Order of attempts: the decoded map, then the disk cache, then the
    /// network.
    pub fn load<F: Fn(gdk::Texture) + 'static>(&self, url: &str, deliver: F) {
        if let Some(texture) = self.peek(url) {
            deliver(texture);
            return;
        }

        let path = self.path_for(url);
        if let Some(texture) = self.decode(url, &path) {
            deliver(texture);
            return;
        }

        if self.in_flight.borrow().iter().any(|other| other == url) {
            return;
        }
        self.in_flight.borrow_mut().push(url.to_string());

        let Ok(message) = soup::Message::new("GET", url) else {
            self.forget_in_flight(url);
            return;
        };

        let cache = self.clone();
        let url = url.to_string();
        let sent = message.clone();
        self.session.send_and_read_async(
            &message,
            glib::Priority::LOW,
            gio::Cancellable::NONE,
            move |result| {
                cache.forget_in_flight(&url);
                let Ok(bytes) = result else { return };
                if !(200..300).contains(&(sent.status_code() as u16)) || bytes.is_empty() {
                    return;
                }

                // Write the cache copy before decoding: a picture that decodes
                // is a picture worth keeping, and if the write fails the decode
                // still works for this session.
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, &bytes);

                if let Some(texture) = cache.decode(&url, &path) {
                    deliver(texture);
                }
            },
        );
    }

    /// Decode a cached file and remember it, or `None` if there is no usable
    /// file there.
    fn decode(&self, url: &str, path: &Path) -> Option<gdk::Texture> {
        let texture = gdk::Texture::from_filename(path).ok()?;
        self.textures
            .borrow_mut()
            .insert(url.to_string(), texture.clone());
        Some(texture)
    }

    fn forget_in_flight(&self, url: &str) {
        self.in_flight.borrow_mut().retain(|other| other != url);
    }

    /// A stable filename for a URL.
    ///
    /// The URL itself cannot be the filename — it is longer than 255 bytes often
    /// enough, and contains slashes always — so it is hashed. This is a cache
    /// key, not a security boundary, so GLib's string hash is the right size of
    /// tool.
    fn path_for(&self, url: &str) -> PathBuf {
        self.directory.join(format!("{:016x}", fnv(url)))
    }
}

/// FNV-1a, sixty-four bit.
///
/// Eight lines instead of a dependency, for a filename in a cache directory.
fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The widget that shows a poster, or a symbolic stand-in until there is one.
///
/// A `GtkPicture` inside a fixed-size frame: the image is cropped to fill rather
/// than letterboxed, because a row of thumbnails with different aspect ratios
/// and grey bars reads as broken.
pub fn poster(width: i32, height: i32) -> (gtk::Widget, gtk::Picture, gtk::Image) {
    let placeholder = gtk::Image::builder()
        .icon_name("video-x-generic-symbolic")
        .pixel_size(24)
        .build();
    placeholder.add_css_class("dimmed");

    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .visible(false)
        .build();

    let overlay = gtk::Overlay::builder()
        .width_request(width)
        .height_request(height)
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .child(&placeholder)
        .build();
    overlay.add_overlay(&picture);
    overlay.add_css_class("poster");

    (overlay.upcast(), picture, placeholder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_url_always_hashes_to_the_same_name_and_different_ones_do_not() {
        // A cache filename has to be stable across runs, or nothing is ever a
        // hit; and it has to be a filename, which a URL is not.
        let a = fnv("https://i.ytimg.com/vi/aaaaaaaaaaa/hqdefault.jpg");
        let b = fnv("https://i.ytimg.com/vi/bbbbbbbbbbb/hqdefault.jpg");
        assert_eq!(a, fnv("https://i.ytimg.com/vi/aaaaaaaaaaa/hqdefault.jpg"));
        assert_ne!(a, b);
        assert!(!format!("{a:016x}").contains('/'));
    }
}
