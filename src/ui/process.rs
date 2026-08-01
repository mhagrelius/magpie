//! The seam where Magpie actually runs something.
//!
//! Everything below `model/` is a decision about what to run; this is the only
//! file that runs it. There is no thread and no tokio: `gio::Subprocess` reads
//! its pipes through the GLib main loop, so the lines arrive on the same thread
//! the widgets live on and there is nothing to hand across.
//!
//! Without a `STDIN_INHERIT` flag GLib gives the child `/dev/null` for stdin,
//! which is what we want — a child that asks a question waits forever otherwise,
//! and ffmpeg asks whether to overwrite files.
//!
//! Children are spawned through a `SubprocessLauncher` rather than
//! `Subprocess::newv` for one reason: [`spawn`] asks the kernel to send the child
//! a `SIGTERM` when this process dies. Without it, a Magpie killed on logout — or
//! crashed, or `SIGKILL`ed — leaves a yt-dlp running that goes on writing into the
//! user's Downloads folder with no window watching it and no way to stop it but
//! `pkill`.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::model::progress::LineBuffer;

/// Which pipe a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Exited zero.
    Success,
    /// Exited non-zero or died on a signal, with whatever it last said on
    /// stderr.
    Failed { stderr: String },
    /// Stopped by [`Handle::cancel`].
    Cancelled,
}

/// Lines of stderr kept for the error report.
///
/// yt-dlp can produce a great deal of stderr on a bad day — one line per failed
/// fragment — and the useful part is always at the end. Keeping the tail bounds
/// the memory and loses nothing that would have been read.
const STDERR_TAIL_LINES: usize = 40;

/// A running child process.
///
/// Dropping the handle does **not** kill the process: a download outlives the
/// row that started it being rebuilt. Stopping is always explicit.
#[derive(Debug, Clone)]
pub struct Handle {
    subprocess: gio::Subprocess,
    cancelled: Rc<Cell<bool>>,
    paused: Rc<Cell<bool>>,
}

impl Handle {
    /// Stop the process without ending it, with `SIGSTOP`.
    ///
    /// The connection can still time out server-side across a long pause, which
    /// is why a job that fails after being resumed is retried from its partial
    /// file rather than reported.
    pub fn pause(&self) {
        if !self.paused.replace(true) {
            self.subprocess.send_signal(libc::SIGSTOP);
        }
    }

    pub fn resume(&self) {
        if self.paused.replace(false) {
            self.subprocess.send_signal(libc::SIGCONT);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.get()
    }

    /// End the process. `SIGTERM` first so yt-dlp can close its `.part` file
    /// cleanly, then `SIGKILL` if it has not gone in five seconds.
    ///
    /// A paused process cannot act on `SIGTERM` until it is running again, so
    /// resuming first is not optional — without it every cancelled pause waits
    /// out the full five seconds before dying.
    pub fn cancel(&self) {
        if self.cancelled.replace(true) {
            return;
        }
        self.resume();
        self.subprocess.send_signal(libc::SIGTERM);

        let subprocess = self.subprocess.clone();
        glib::timeout_add_seconds_local_once(5, move || {
            if subprocess.identifier().is_some() {
                subprocess.force_exit();
            }
        });
    }

    pub fn was_cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

/// Launch a child that will not outlive this process.
///
/// `set_child_setup` runs in the forked child between `fork` and `exec`, which is
/// the only moment `PR_SET_PDEATHSIG` can be set for it. The rules for that window
/// are strict — async-signal-safe calls only, no allocation — and one `prctl` is
/// exactly that.
fn spawn(program: &Path, args: &[String]) -> Result<gio::Subprocess, glib::Error> {
    let mut argv: Vec<&std::ffi::OsStr> = vec![program.as_os_str()];
    argv.extend(args.iter().map(|arg| std::ffi::OsStr::new(arg.as_str())));

    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE,
    );
    launcher.set_child_setup(|| {
        // SAFETY: one variadic prctl with an integer argument. Signal-safe, and
        // a failure here is not worth acting on — the worst case is the
        // behaviour we had before, an orphan.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
        }
    });
    launcher.spawn(&argv)
}

/// Start `program` with `args`, streaming its output.
///
/// `on_line` is called for every complete line from either pipe, `on_done`
/// exactly once when the process ends.
pub fn run<L, D>(
    program: &Path,
    args: &[String],
    on_line: L,
    on_done: D,
) -> Result<Handle, glib::Error>
where
    L: Fn(Stream, &str) + 'static,
    D: FnOnce(Outcome) + 'static,
{
    let subprocess = spawn(program, args)?;

    let handle = Handle {
        subprocess: subprocess.clone(),
        cancelled: Rc::new(Cell::new(false)),
        paused: Rc::new(Cell::new(false)),
    };

    let on_line = Rc::new(on_line);
    let stderr_tail = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));

    if let Some(pipe) = subprocess.stdout_pipe() {
        let on_line = on_line.clone();
        glib::spawn_future_local(async move {
            drain(pipe, move |line| on_line(Stream::Stdout, line)).await;
        });
    }

    if let Some(pipe) = subprocess.stderr_pipe() {
        let on_line = on_line.clone();
        let stderr_tail = stderr_tail.clone();
        glib::spawn_future_local(async move {
            drain(pipe, move |line| {
                let mut tail = stderr_tail.borrow_mut();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.remove(0);
                }
                tail.push(line.to_string());
                drop(tail);
                on_line(Stream::Stderr, line);
            })
            .await;
        });
    }

    let waited = handle.clone();
    glib::spawn_future_local(async move {
        let result = waited.subprocess.wait_check_future().await;
        let outcome = if waited.was_cancelled() {
            Outcome::Cancelled
        } else if result.is_ok() {
            Outcome::Success
        } else {
            Outcome::Failed {
                stderr: stderr_tail.borrow().join("\n"),
            }
        };
        on_done(outcome);
    });

    Ok(handle)
}

/// Read a pipe to the end, handing over complete lines as they arrive.
async fn drain<F: Fn(&str)>(pipe: gio::InputStream, on_line: F) {
    let mut buffer = LineBuffer::new();
    loop {
        match pipe.read_bytes_future(8192, glib::Priority::DEFAULT).await {
            Ok(bytes) if bytes.is_empty() => break,
            Ok(bytes) => {
                for line in buffer.push(&bytes) {
                    on_line(&line);
                }
            }
            // A closed or cancelled pipe is how this normally ends.
            Err(_) => break,
        }
    }
    if let Some(rest) = buffer.flush() {
        on_line(&rest);
    }
}

/// Read a whole program's stdout, for the short one-shot calls: `--dump-json`
/// and `--version`.
///
/// `communicate_utf8_future` would reject a video title that is not valid UTF-8;
/// the bytes form and a lossy conversion cannot.
pub async fn capture(program: &Path, args: &[String]) -> Result<Capture, glib::Error> {
    let subprocess = spawn(program, args)?;
    let (stdout, stderr) = subprocess.communicate_future(None).await?;
    Ok(Capture {
        stdout: stdout
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default(),
        stderr: stderr
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default(),
    })
}

/// What a one-shot call produced.
#[derive(Debug, Clone)]
pub struct Capture {
    pub stdout: String,
    pub stderr: String,
}

/// Whether a path is a file this process could execute.
///
/// The predicate `model::tools::locate` is given. `gio::File` rather than
/// `std::fs` so that a Flatpak's document portal paths behave the same way.
pub fn is_executable(path: &Path) -> bool {
    let file = gio::File::for_path(path);
    let Ok(info) = file.query_info(
        "standard::type,access::can-execute",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) else {
        return false;
    };
    info.file_type() != gio::FileType::Directory
        && info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_EXECUTE)
}
