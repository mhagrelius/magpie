//! What is installed on this machine, and fetching whisper models.
//!
//! `model::tools` holds the rules — search order, which command names count, how
//! to read a version, when a yt-dlp is old enough to blame. This file is the
//! part that touches the filesystem and asks each program its version, which is
//! why it lives on the `ui/` side of the line.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use soup::prelude::*;

use crate::model::diarize::Asset;
use crate::model::tools::{self, Found, Freshness, Installers, Tool};
use crate::model::transcript::Model;

use super::process;

/// Everything Magpie knows about the programs it needs.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub ytdlp: Option<Found>,
    pub ffmpeg: Option<Found>,
    pub ffprobe: Option<Found>,
    pub whisper: Option<Found>,
    pub freshness: Freshness,
    /// What the user has to install things with, so the advice can name one of
    /// them. Also where `uv` itself was found, for running the command.
    pub installers: Installers,
    pub uv_path: Option<PathBuf>,
    pub pipx_path: Option<PathBuf>,
    /// A JavaScript engine for yt-dlp's YouTube extractor, if there is one.
    pub js_runtime: Option<Found>,
    /// sherpa-onnx's diarizer, for marking who is speaking.
    pub diarizer: Option<Found>,
}

impl Report {
    pub fn found(&self, tool: Tool) -> Option<&Found> {
        match tool {
            Tool::YtDlp => self.ytdlp.as_ref(),
            Tool::Ffmpeg => self.ffmpeg.as_ref(),
            Tool::Ffprobe => self.ffprobe.as_ref(),
            Tool::Whisper => self.whisper.as_ref(),
            Tool::JsRuntime => self.js_runtime.as_ref(),
            Tool::Diarizer => self.diarizer.as_ref(),
        }
    }

    pub fn ytdlp_path(&self) -> Option<&PathBuf> {
        self.ytdlp.as_ref().map(|found| &found.path)
    }

    pub fn has_ffmpeg(&self) -> bool {
        self.ffmpeg.is_some()
    }

    pub fn has_whisper(&self) -> bool {
        self.whisper.is_some()
    }

    /// Whether identifying speakers can be offered at all.
    ///
    /// The tool alone, not the models: the models are a download Magpie can
    /// start from preferences, whereas a missing binary is a install the user
    /// has to do. Those are different sentences and the UI says the right one.
    pub fn has_diarizer(&self) -> bool {
        self.diarizer.is_some()
    }

    /// The banner to show at the top of the window, if any.
    ///
    /// Only two things earn a banner: yt-dlp missing, which stops everything,
    /// and yt-dlp being stale, which is the most likely cause of a download
    /// failing for reasons the error message will not name. A missing ffmpeg is
    /// not a banner — it only matters once someone asks for something that needs
    /// it, and the Add dialog is where that is said.
    pub fn banner(&self) -> Option<(String, &'static str)> {
        match (&self.ytdlp, self.freshness) {
            (None, _) => Some((
                "yt-dlp is not installed. Magpie needs it to download anything.".to_string(),
                "Show Me How",
            )),
            (Some(_), Freshness::Stale { days }) => Some((
                format!(
                    "The installed yt-dlp is {} months old. Sites change faster than that.",
                    (days / 30).max(1)
                ),
                "Details",
            )),
            // A missing JavaScript engine deliberately does *not* earn a banner.
            // yt-dlp calls extraction without one deprecated and warns that "some
            // formats may be missing", but on the videos this was tested against
            // the format list was identical with and without one — so the harm is
            // documented rather than observed, and a persistent banner for a
            // maybe is the kind of nagging that teaches people to ignore banners.
            // It appears on the Tools page, and the two failures it could plausibly
            // cause name it in their guidance.
            _ => None,
        }
    }
}

/// Look for every tool, then ask each one its version.
///
/// `on_ready` is called once with the finished report. The version calls run in
/// parallel and are each allowed to fail: a program that will not answer
/// `--version` still counts as present, because the path is what matters for
/// running it.
pub fn survey<F: Fn(Report) + 'static>(ytdlp_override: Option<PathBuf>, on_ready: F) {
    let path_var = glib::getenv("PATH")
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".to_string());
    let home = glib::home_dir();

    let locate = |tool: Tool| tools::locate(tool, &path_var, &home, &process::is_executable);

    // An explicit path from preferences wins, and is checked rather than
    // trusted: a stale setting pointing at a deleted file should look like "not
    // installed", not like a mysterious failure to launch.
    let ytdlp = ytdlp_override
        .filter(|path| process::is_executable(path))
        .or_else(|| locate(Tool::YtDlp));

    let paths = [
        (Tool::YtDlp, ytdlp),
        (Tool::Ffmpeg, locate(Tool::Ffmpeg)),
        (Tool::Ffprobe, locate(Tool::Ffprobe)),
        (Tool::Whisper, locate(Tool::Whisper)),
        (Tool::JsRuntime, locate(Tool::JsRuntime)),
        (Tool::Diarizer, locate(Tool::Diarizer)),
    ];

    // The installers themselves, so the advice can name one the user already
    // has and the Tools page can offer to run it.
    let installer = |command: &str| {
        tools::candidates(command, &path_var, &home)
            .into_iter()
            .find(|path| process::is_executable(path))
    };
    let uv_path = installer("uv");
    let pipx_path = installer("pipx");
    // Only checked so the advice can say `snap install deno` where that would
    // work. Magpie never runs it — it needs a password.
    let has_snap = installer("snap").is_some();

    glib::spawn_future_local(async move {
        let mut report = Report {
            installers: Installers {
                uv: uv_path.is_some(),
                pipx: pipx_path.is_some(),
                snap: has_snap,
            },
            uv_path,
            pipx_path,
            ..Report::default()
        };
        for (tool, path) in paths {
            let Some(path) = path else { continue };
            let version = version_of(tool, &path).await;
            let found = Found {
                path,
                version: version.clone(),
            };
            match tool {
                Tool::YtDlp => {
                    report.freshness = version
                        .as_deref()
                        .map(|version| tools::freshness(version, chrono::Local::now().date_naive()))
                        .unwrap_or(Freshness::Unknown);
                    report.ytdlp = Some(found);
                }
                Tool::Ffmpeg => report.ffmpeg = Some(found),
                Tool::Ffprobe => report.ffprobe = Some(found),
                Tool::Whisper => report.whisper = Some(found),
                Tool::JsRuntime => report.js_runtime = Some(found),
                Tool::Diarizer => report.diarizer = Some(found),
            }
        }
        on_ready(report);
    });
}

async fn version_of(tool: Tool, path: &Path) -> Option<String> {
    let args: Vec<String> = tool.version_argv().iter().map(|a| a.to_string()).collect();
    let capture = process::capture(path, &args).await.ok()?;
    tools::parse_version(tool, &capture.stdout)
}

/// Run one of the install commands Magpie offered, and report how it went.
///
/// Only the commands from `Tool::install_command`/`upgrade_command` reach here,
/// and only the unprivileged ones: the first word is looked up against the
/// installers actually found, and anything else is refused. There is no shell —
/// the string is split on spaces and handed to `exec` as an argument vector — so
/// this cannot become a way to run arbitrary text, however a future caller
/// misuses it.
///
/// It is still the user's environment being changed, so the caller is expected to
/// have shown them the exact command and had them press a button first.
pub fn run_installer<L, D>(
    report: &Report,
    command: &str,
    on_line: L,
    on_done: D,
) -> Result<(), String>
where
    L: Fn(&str) + 'static,
    D: FnOnce(Result<(), String>) + 'static,
{
    let mut words = command.split_whitespace();
    let program = words.next().ok_or("there is no command to run")?;
    let args: Vec<String> = words.map(str::to_string).collect();

    let path = match program {
        "uv" => report.uv_path.clone(),
        "pipx" => report.pipx_path.clone(),
        // `sudo apt install …` and a bare URL both land here. Neither is
        // Magpie's to run.
        _ => None,
    }
    .ok_or_else(|| format!("Magpie will not run “{program}” for you"))?;

    let tail = Rc::new(RefCell::new(Vec::<String>::new()));
    let collected = tail.clone();

    process::run(
        &path,
        &args,
        move |_, line| {
            let mut tail = collected.borrow_mut();
            if tail.len() == 20 {
                tail.remove(0);
            }
            tail.push(line.to_string());
            drop(tail);
            on_line(line);
        },
        move |outcome| match outcome {
            process::Outcome::Success => on_done(Ok(())),
            process::Outcome::Cancelled => on_done(Err("cancelled".into())),
            process::Outcome::Failed { stderr } => {
                // The installer's own last words, which for uv and pipx are
                // usually the whole explanation.
                let reported = if stderr.trim().is_empty() {
                    tail.borrow().join("\n")
                } else {
                    stderr
                };
                on_done(Err(reported));
            }
        },
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Where whisper models live.
pub fn models_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

/// Whether this model is already on disk, and how big the file is.
pub fn model_on_disk(models_dir: &Path, model: Model) -> Option<u64> {
    let path = model.path_in(models_dir);
    let metadata = std::fs::metadata(path).ok()?;
    // A truncated download left behind by a crash should read as absent, not as
    // a model that whisper will then fail to load.
    (metadata.len() > model.bytes() / 2).then_some(metadata.len())
}

/// Whether one of the diarization models is on disk and plausibly whole.
pub fn diarize_asset_on_disk(models_dir: &Path, asset: Asset) -> Option<u64> {
    let metadata = std::fs::metadata(asset.path_in(models_dir)).ok()?;
    (metadata.len() > asset.bytes() / 2).then_some(metadata.len())
}

/// Whether *both* models are there, which is what running needs.
pub fn diarize_models_on_disk(models_dir: &Path) -> bool {
    Asset::ALL
        .iter()
        .all(|asset| diarize_asset_on_disk(models_dir, *asset).is_some())
}

/// Fetch both diarization models as one action.
///
/// One action because they are useless apart — a user who has the segmentation
/// model and not the embedding one has nothing, and asking them to think about
/// two downloads would be exposing an implementation detail as a decision. The
/// progress fraction spans the pair, weighted by size, so the bar does not jump
/// back to zero in the middle.
pub fn download_diarize_models<P, D>(models_dir: &Path, on_progress: P, on_done: D) -> ModelDownload
where
    P: Fn(f64) + 'static,
    D: FnOnce(Result<(), String>) + 'static,
{
    let total = crate::model::diarize::total_bytes() as f64;
    let first = Asset::ALL[0];
    let second = Asset::ALL[1];
    let first_share = first.bytes() as f64 / total;

    let on_progress = Rc::new(on_progress);
    let on_done = Rc::new(RefCell::new(Some(on_done)));

    // The second download replaces the handle inside this cell, so cancelling
    // partway through cancels whichever is actually running.
    let inner: Rc<RefCell<Option<ModelDownload>>> = Rc::new(RefCell::new(None));
    let cancellable = gio::Cancellable::new();

    let models_dir = models_dir.to_path_buf();
    let started = download_file(
        first.download_url(),
        first.path_in(&models_dir),
        first.bytes(),
        {
            let on_progress = on_progress.clone();
            move |fraction| on_progress(fraction * first_share)
        },
        {
            let inner = inner.clone();
            let on_progress = on_progress.clone();
            let on_done = on_done.clone();
            let cancellable = cancellable.clone();
            move |result| {
                let finish = move |result: Result<(), String>| {
                    if let Some(on_done) = on_done.borrow_mut().take() {
                        on_done(result);
                    }
                };
                if let Err(error) = result {
                    finish(Err(error));
                    return;
                }
                if cancellable.is_cancelled() {
                    finish(Err("cancelled".into()));
                    return;
                }

                let next = download_file(
                    second.download_url(),
                    second.path_in(&models_dir),
                    second.bytes(),
                    move |fraction| on_progress(first_share + fraction * (1.0 - first_share)),
                    move |result| finish(result.map(|_| ())),
                );
                inner.replace(Some(next));
            }
        },
    );
    inner.replace(Some(started));

    ModelDownload {
        cancellable,
        running: Some(inner),
    }
}

/// A download in progress, cancellable.
pub struct ModelDownload {
    cancellable: gio::Cancellable,
    /// The download actually running, when this handle stands for a sequence of
    /// them. Cancelling has to reach whichever one that currently is, or pressing
    /// Cancel during the second of two files would only set a flag nobody reads.
    running: Option<Rc<RefCell<Option<ModelDownload>>>>,
}

impl ModelDownload {
    pub fn cancel(&self) {
        self.cancellable.cancel();
        if let Some(running) = &self.running {
            // `try_borrow` because cancelling from inside the callback that is
            // mid-way through replacing this cell would otherwise panic, and a
            // download that is already finishing needs no cancelling.
            if let Ok(running) = running.try_borrow() {
                if let Some(running) = running.as_ref() {
                    running.cancel();
                }
            }
        }
    }
}

/// Fetch a model, reporting progress.
///
/// Streamed to a `.part` file and renamed at the end rather than read into
/// memory: the medium model is a gigabyte and a half, and holding that as a
/// `glib::Bytes` before writing it is a gigabyte and a half of resident memory
/// for no reason. The `.part` name is also what makes a cancelled or crashed
/// download read as absent rather than as a corrupt model.
pub fn download_model<P, D>(
    models_dir: &Path,
    model: Model,
    on_progress: P,
    on_done: D,
) -> ModelDownload
where
    P: Fn(f64) + 'static,
    D: FnOnce(Result<PathBuf, String>) + 'static,
{
    download_file(
        model.download_url(),
        model.path_in(models_dir),
        model.bytes(),
        on_progress,
        on_done,
    )
}

/// Fetch one file to a path, reporting progress.
///
/// `expected` is only a fallback for the progress bar when the server sends no
/// content length; the file is whatever the server actually returns.
fn download_file<P, D>(
    url: impl Into<String>,
    final_path: PathBuf,
    expected: u64,
    on_progress: P,
    on_done: D,
) -> ModelDownload
where
    P: Fn(f64) + 'static,
    D: FnOnce(Result<PathBuf, String>) + 'static,
{
    let cancellable = gio::Cancellable::new();
    let download = ModelDownload {
        cancellable: cancellable.clone(),
        running: None,
    };

    let url = url.into();
    // `.part` appended rather than substituted: `set_extension` on
    // `diarize-segmentation.onnx` would replace `.onnx` and leave the finished
    // name unreachable.
    let part_path = final_path.with_file_name(format!(
        "{}.part",
        final_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string())
    ));
    let session = soup::Session::new();

    let on_done = Rc::new(RefCell::new(Some(on_done)));
    let finish = move |result: Result<PathBuf, String>| {
        if let Some(on_done) = on_done.borrow_mut().take() {
            on_done(result);
        }
    };

    glib::spawn_future_local(async move {
        if let Some(parent) = final_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                finish(Err(format!("cannot create {}: {error}", parent.display())));
                return;
            }
        }

        let Ok(message) = soup::Message::new("GET", &url) else {
            finish(Err("the model address is not a URL".into()));
            return;
        };

        let stream = match session.send_future(&message, glib::Priority::DEFAULT).await {
            Ok(stream) => stream,
            Err(error) => {
                finish(Err(error.to_string()));
                return;
            }
        };

        let status = message.status_code();
        if !(200..300).contains(&status) {
            finish(Err(format!("the server answered {status}")));
            return;
        }

        let total = message
            .response_headers()
            .map(|headers| headers.content_length())
            .filter(|length| *length > 0)
            .map(|length| length as u64)
            // Hugging Face redirects to a CDN that does send a length, but
            // falling back on the published size keeps the bar honest if it ever
            // stops.
            .unwrap_or(expected);

        let file = gio::File::for_path(&part_path);
        let output = match file
            .replace_future(
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
                glib::Priority::DEFAULT,
            )
            .await
        {
            Ok(output) => output,
            Err(error) => {
                finish(Err(error.to_string()));
                return;
            }
        };

        let mut written: u64 = 0;
        loop {
            // Checked here rather than by cancelling the futures: a half-written
            // `.part` file is deleted on the way out either way, and one place
            // to do that is easier to be sure of than three.
            if cancellable.is_cancelled() {
                let _ = std::fs::remove_file(&part_path);
                finish(Err("cancelled".into()));
                return;
            }

            let bytes = match stream
                .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
                .await
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = std::fs::remove_file(&part_path);
                    finish(Err(error.to_string()));
                    return;
                }
            };
            if bytes.is_empty() {
                break;
            }
            if let Err(error) = output
                .write_bytes_future(&bytes, glib::Priority::DEFAULT)
                .await
            {
                let _ = std::fs::remove_file(&part_path);
                finish(Err(error.to_string()));
                return;
            }
            written += bytes.len() as u64;
            on_progress((written as f64 / total as f64).clamp(0.0, 1.0));
        }

        let _ = output.close_future(glib::Priority::DEFAULT).await;

        if let Err(error) = std::fs::rename(&part_path, &final_path) {
            finish(Err(error.to_string()));
            return;
        }
        finish(Ok(final_path));
    });

    download
}
