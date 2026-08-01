# Magpie — design for review

## Scope

A video downloader for the GNOME desktop. Paste a link, get the file, keep a
record of what you took. Optionally get a transcript alongside it.

Magpie is a front end to `yt-dlp`. It does not parse a single web page itself,
does not sign requests, and does not ship a copy of anything it did not write.
Everything it knows about the network, it learned by running `yt-dlp` and
reading its output. That is the whole trick, and it is deliberate: the part of
this problem that rots is the extractor, and the extractor is somebody else's
full-time job.

This replaces an Electron/React application of the same purpose. Nothing
carried over but the behaviour worth keeping; §"What the rewrite fixes" lists
where the two deliberately differ.

## What it does

### Taking a link

The link bar sits at the top of the window and is always available. Paste, press
Enter, and Magpie runs `yt-dlp --dump-json` in the background while an
**Add Download** dialog opens with a spinner. When the metadata lands the dialog
fills in: thumbnail, title, uploader, duration, and the format choices. Press
Download and the job joins the queue.

If the link is a playlist, the dialog grows a list of entries with check boxes,
all ticked, and the files land in a subfolder named after the playlist.

```
┌─ Magpie ───────────────────────────────────────────── ⌂  ☰ ─┐
│                                                             │
│   ┌───────────────────────────────────────────────────┐     │
│   │ Paste a video or playlist link          ⧉  │ Add │ │     │
│   └───────────────────────────────────────────────────┘     │
│                                                             │
│   ┌───────────────────────────────────────────────────┐     │
│   │ ▤  Blackbird singing in the dead of night         │     │
│   │    Downloading · 47% · 3.2 MB/s · 1 min left  ⏸ ✕ │     │
│   ├───────────────────────────────────────────────────┤     │
│   │ ▤  How to solder without crying                   │     │
│   │    Waiting                                    ⏸ ✕ │     │
│   ├───────────────────────────────────────────────────┤     │
│   │ ▤  Wind ensemble rehearsal, take 4                │     │
│   │    Saved to Videos · transcript ready       ⌂  ✕  │     │
│   └───────────────────────────────────────────────────┘     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Choosing a format

Four presets and an escape hatch:

| Preset | `yt-dlp -f` |
|---|---|
| Best available | `bestvideo*+bestaudio/best` |
| Up to 1080p | `bestvideo*[height<=?1080]+bestaudio/best[height<=?1080]` |
| Up to 720p | `bestvideo*[height<=?720]+bestaudio/best[height<=?720]` |
| Up to 480p | `bestvideo*[height<=?480]+bestaudio/best[height<=?480]` |
| Audio only | see below |

The escape hatch is **Choose a specific format**, which lists every format
`--dump-json` reported, video-only ones included, and passes the raw format id
through. The old application filtered the list down to formats that carry their
own audio, which on YouTube means 360p and nothing above it; the presets above
are `height<=?` rather than `height<=` so a video that only exists at 1440p
still downloads rather than failing the selector.

Audio only has its own three:

| Audio format | `yt-dlp` arguments | Result |
|---|---|---|
| Best available | `-f bestaudio` | Original stream, no transcode, fastest |
| MP3 | `-f bestaudio -x --audio-format mp3 --audio-quality 0` | ffmpeg transcode, plays anywhere |
| M4A | `-f bestaudio[ext=m4a]/bestaudio -x --audio-format m4a` | AAC, no transcode when the source is already m4a |

### Watching it happen

`yt-dlp` is asked for machine-readable progress rather than the human progress
bar, with a sentinel so a line from Magpie's template can never be confused with
a line of yt-dlp's own chatter:

```
--progress-template  download:magpie	%(progress.status)s	%(progress.downloaded_bytes)s	…
```

Raw numbers, not `_str` fields — parsing `1.23MiB` back into an integer is work
the template can be told not to create. Speed is smoothed over a ten-sample
window and the time remaining is computed from the smoothed figure, because
yt-dlp's own ETA jitters by a factor of three between updates and a number that
jumps is worse than no number.

A row is Waiting, Downloading, Paused, Saved or Failed. Pause is `SIGSTOP` and
resume is `SIGCONT`; a long pause can still lose the connection server-side, so
a paused job that fails on resume retries from the partial file rather than
reporting an error.

### Where the file went

`--print-to-file after_move:filepath` writes the final path to a temporary file,
one line per download, read when the process exits. The old application scraped
`[download] Destination:` out of stdout, which names the *pre*-conversion file —
so every MP3 it produced reported a `.webm` path that was no longer on disk.

### Transcripts

A finished download can be handed to `whisper-cli`, which Magpie builds and ships
because Ubuntu does not package it — see "Not built" for why that is the one tool
treated this way. Anything that is not already `.wav`/`.mp3`/`.flac`/`.ogg` goes
through `ffmpeg -ar 16000 -ac 1` first, into the cache directory rather than next
to the user's file, and the scratch copy is deleted afterwards. Output is `txt`,
`srt` or `vtt`, written beside the media.

Models are downloaded on demand from Hugging Face, with the size shown before the
download starts and a button to delete one afterwards. Four sizes, `small` the
default — the same four and the same default as the application this replaces,
which is where the choice was already proven. A model is data, and downloading
data the user asked for is ordinary.

Only for a single download, never for a collection: transcribing forty playlist
items is an afternoon of CPU nobody asked for by flipping one switch, so the
switch is not offered for a playlist at all.

### The library

Every finished download is recorded in `~/.local/share/magpie/library.json`:
url, title, the file it produced, the transcript if there is one, and when. The
queue survives a restart — a job that was downloading when the app quit comes
back as Waiting, and resumes from its partial file.

## What the rewrite fixes

The old application's behaviour was worth keeping. These specific parts were
not, and are listed because each is a decision someone might otherwise reverse
by accident:

- **Downloaded output path was wrong for MP3.** Scraped from `Destination:`;
  now `--print-to-file after_move:filepath`.
- **1080p was unreachable.** The format list was filtered to muxed formats.
  Now DASH video is merged with a separate audio stream, which is what
  `bestvideo*+bestaudio` means.
- **The quality preference did nothing.** `1080p` in settings only had the
  effect of *not* skipping the format picker; it was never turned into a
  selector. Now it is one.
- **`audioQuality` was inert.** Declared, stored, never passed. Now MP3 passes
  `--audio-quality 0`.
- **A failed playlist item stalled the rest.** The queue advanced only on
  success, so one private video left forty behind it Pending forever. The queue
  now advances on any terminal outcome.
- **Nothing was persisted.** A SQLite `history` table was created at startup
  and never read or written, and the queue lived in renderer memory. Closing
  the window lost everything.
- **Progress parsing dropped lines split across reads.** Chunks were split on
  `\n` with no buffer for the remainder.
- **Errors were `stderr.includes('ERROR')`.** Now classified into the handful
  of causes a user can act on, each with the sentence that says what to do.
- **No line buffering, no cookies, no rate limiting.** Cookies-from-browser and
  a rate limit are now settings, because "it works in my browser but not in the
  app" has exactly one fix and it is a flag.

## Architecture

Two halves, and the rule is the same one the sister projects keep: `model/`
links no GTK and spawns no process, so `cargo test` exercises it with no display
and no network. `ui/` is the only half that knows a window exists, and the only
half that runs anything.

The seam between them is that **`model/` builds argument vectors and parses
lines; `ui/` runs the process**. A download is a pure function from a request to
`Vec<String>`, plus a pure function from a line of output to a progress event.
That is what makes the interesting parts testable without a network, and it is
what will make an MCP server cheap later — see "Deferred".

```
src/
  main.rs             Eight lines: name the app, run it.
  lib.rs              Module declarations and APP_ID.

  model/
    url.rs            What is a link, is it a playlist, what is its id.
    quality.rs        Preset -> yt-dlp format selector. Audio format -> args.
    request.rs        A download request -> the full yt-dlp argument vector.
    progress.rs       A line of yt-dlp output -> a progress event. Line buffer.
    failure.rs        Exit status + stderr -> a cause the user can act on.
    media.rs          --dump-json -> Media, Format, Playlist.
    job.rs            One download's state machine.
    queue.rs          Ordering, parallelism, advance-on-any-outcome.
    library.rs        The record on disk. Atomic write, recover a corrupt file.
    settings.rs       config.json. Load never writes.
    tools.rs          Which external tools exist, where, how old.
    transcript.rs     whisper-cli argument vector and progress parsing.

  ui/
    application.rs    MagpieApplication. Owns state; the only thing that writes.
    window.rs         MagpieWindow. Header bar, banner, link bar, the list.
    link_bar.rs       The entry, the paste button, the Add button.
    add_dialog.rs     Preview, format choices, playlist picker.
    job_row.rs        One row: thumbnail, status line, progress, controls.
    preferences.rs    AdwPreferencesDialog, three pages.
    process.rs        The gio::Subprocess seam. Lines out, signals in.
    thumbnail.rs      Fetch and cache the poster image.
    style.css
```

**Widgets emit intent; the application acts.** A row's Cancel button emits
`cancel-requested`; it does not kill a process. `MagpieApplication` is the only
object that touches the queue, the library file, or a subprocess.

**No tokio.** GLib's main loop already runs futures, and every wait in this
application is I/O: `gio::Subprocess` with `read_line_async` on stdout,
`gio::File` for the library, `glib::spawn_future_local` to sequence them.
Cancellation is `gio::Cancellable` and a signal to the child.

**Errors are typed enums per module**, `Display` + `std::error::Error`, no
`thiserror` and no `anyhow`. A failure that a user can act on carries the
sentence that says what to do, because the alternative is a dialog that shows
them a stack of yt-dlp's stderr.

## Testing

`model/` is tested inline, in `#[cfg(test)] mod tests` at the bottom of each
file. The tests that matter are the ones about the outside world's shapes:
a progress line arriving in two reads, a format selector for a video that only
exists at 1440p, a `--dump-json` payload missing every optional field, a
`library.json` truncated by a power cut.

`tests/session.rs` drives the whole model half end to end over recorded
fixtures — real `--dump-json` output, real progress streams — into a
`tempfile::tempdir()`, then reopens it from disk. No display, no network.

`tests/widgets.rs` is one `#[test]` containing a list of cases, because GTK is
thread-affine and `--test-threads=1` serialises without making tests share a
thread. Windows are constructed and never presented.

`examples/preview.rs` paints the real widget tree offscreen to a PNG, because
GNOME will not give a screenshot to a non-interactive caller.

`./test.sh` runs `cargo fmt --check`, then `cargo clippy --all-targets -D
warnings`, then the tests; `./test.sh --headless` wraps them in `xvfb-run -a
dbus-run-session`.

## Dependencies

**gtk4 / libadwaita.** GTK 4.22 and libadwaita 1.9, the versions in Ubuntu's
GNOME 50. Same floors as the sister projects, for the same reason: `AdwSidebar`
and `AdwShortcutsDialog` are 1.9 and 1.8, and writing around them to support an
older libadwaita would cost more than it buys on a desktop that ships 1.9.

**gio, explicitly, at `v2_80`.** `Subprocess::send_signal` is what pause and
resume are, and the level gtk4 pulls in does not expose it.

**serde / serde\_json.** yt-dlp's `--dump-json` and the library file. The
metadata is read out of a `Value` by hand rather than derived, because a struct
with `#[derive(Deserialize)]` over yt-dlp's output is a struct that breaks the
week yt-dlp renames a field it never promised to keep.

**chrono.** Two jobs: timestamps in the library, and the age of the installed
yt-dlp. The second is not decoration — a yt-dlp more than a couple of months old
is the single most common reason a YouTube download fails, and saying so is
more useful than relaying the extractor's error.

**libsoup.** Two HTTP calls of Magpie's own: a poster image and a whisper model.
`gio::File` over an `https` URI would cover both, but only where gvfs' http
backend is installed and reachable, which it is not inside a Flatpak sandbox.
libsoup is GNOME's own client and is already in the runtime.

**libc, directly.** Signal numbers, and one `prctl(PR_SET_PDEATHSIG)` in the
child between `fork` and `exec`. This is what stops a Magpie killed on logout
from leaving a yt-dlp running that goes on writing into the user's Downloads
folder with no window watching it. There is no GLib equivalent —
`g_unix_signal_add` is unbound in glib-rs 0.22 — and no signal handler could
cover `SIGKILL`, which the kernel-level parent-death signal does. libc is
already compiled as part of glib's own tree, so this is a direct use of
something that was going to be built regardless rather than a new dependency.

Rejected: `tokio` (GLib's loop is already there), `reqwest` (libsoup covers the
two HTTP calls), `rusqlite` (the library is a few hundred rows of JSON; SQLite is
a C dependency bought for nothing), `clap` (no CLI), `thiserror`/`anyhow` (four
error enums do not need a macro), `uuid` (a monotonic counter is a unique job id
on one machine), a Markdown or HTML parser (nothing here parses a page — that is
what yt-dlp is for).

## Milestones

1. Crate, `model/` with its tests, no window.
2. Window, link bar, Add dialog, the queue list. Downloads work.
3. Pause, resume, cancel, retry. Library persists across a restart.
4. Transcripts: model management, and `packaging/build-whisper.sh` so the feature
   reaches someone who will not compile C++ themselves.
5. Preferences, tool detection, the missing-tool and stale-tool banners.
6. Packaging: `.deb`, Flatpak, `install.sh`.

## Deferred

**MCP into Familiar.** Familiar has no MCP client today — its tools are
`FunctionDeclaration`s hardcoded in `model/tools.rs`, dispatched by a match arm
in `ui/runner.rs`, and its own DESIGN.md defers MCP to "the next project". So
Magpie ships no MCP server in v1, and the integration is designed for rather
than built.

What makes it cheap when it happens: everything an agent would want is already a
pure function in `model/`, taking a request and returning either an argument
vector or a parsed result. A `magpie mcp` subcommand would be a stdio JSON-RPC
loop over that half, adding no dependency on `ui/` and no second implementation.
The tools it would expose map one-to-one onto what the dialog already asks for:

| Tool | Wraps |
|---|---|
| `video_info(url)` | `model::media::from_dump_json` over `--dump-json` |
| `download(url, quality, audio_only, destination)` | `model::request::Request::argv` |
| `transcribe(url \| path, format, language)` | `model::transcript` |
| `library_search(query)` | `model::library` |

The open question is not the server, it is Familiar's side: whether Familiar
grows a real MCP client, or whether Magpie is simply a fifth match arm in
`Runner::run` alongside `recall` and `web_search`. The second is a day's work
and the first is a project. That decision belongs to Familiar's next milestone,
not to this document.

## Not built, or built differently

- **No binaries fetched by Magpie itself.** The old application downloaded
  `yt-dlp`, `deno` and a self-hosted static `whisper-cli` from GitHub into
  `~/.local/share`, unpacked them by shelling out to `unzip`, and `chmod +x`'d
  them. That is a self-updating executable outside the package manager — wrong
  for a `.deb`, impossible in a Flatpak sandbox, and not something to hand a
  user who trusted an app store.

  The line is *who* is fetching, not whether anything gets fetched. Magpie will
  **drive the user's own installer** — Preferences → Tools shows the exact command
  and a button that runs it, for `uv` and `pipx` only. That is a package manager
  doing its job with the user's knowledge, and it leaves them with a `yt-dlp` they
  can update without Magpie. What Magpie will not do is fetch an executable
  itself, and it will not run anything needing a password: `sudo apt install
  ffmpeg` gets a Copy button, not an Install button, because an application that
  raises a privilege prompt to install something is asking for a trust it does not
  need.

  `uv tool install`, never `uvx`. `uvx` runs a tool without leaving anything on
  `PATH`, so a user who followed that advice would return to a Tools page still
  saying "not installed". Search order puts `~/.local/bin` — where both `uv` and
  `pipx` put shims — ahead of `PATH`, so a user-installed copy beats
  `/usr/bin`'s older one.

  **Why not bundle yt-dlp?** Because it rots faster than Magpie could ship. Its
  releases are roughly weekly and site breakage is the reason for most of them;
  any copy pinned into a Magpie release is stale within weeks, and the user would
  have no way to update it without waiting for us. Bundling would convert a
  problem they can fix in one command into one only a Magpie release can fix. The
  Flatpak has to bundle one anyway — a sandbox cannot run host binaries — and pays
  exactly that cost, which is why its manifest says so and why the Tools page
  reports the bundled version's age.

- **whisper.cpp *is* built and shipped, and that is not a contradiction.** The
  argument above is about rot: a bundled yt-dlp goes wrong on its own, without
  anyone touching it, because the sites it scrapes keep changing. whisper.cpp has
  no such exposure — it is local inference with no network — so a pinned version
  stays exactly as correct as the day it was built.

  Against that, Ubuntu has no package for it, which left transcription requiring
  the user to compile a C++ project before they could use a switch in a dialog.
  That is a feature that exists in the code and not in anyone's hands. So
  `packaging/build-whisper.sh` pins v1.9.1 and builds `whisper-cli`; the Flatpak
  builds it as a module, and the `.deb` includes it with `--with-whisper`.

  CPU only, and `-DGGML_NATIVE=OFF`. The GPU backends are faster and are many more
  ways to fail on a machine whose drivers are not what the build assumed, and a
  native-tuned build crashes with `SIGILL` on an older CPU than the builder's. A
  binary that works everywhere beats one that is quick where it was compiled.

  A bundled copy goes in `/usr/lib/magpie` or `/app/lib/magpie`, never on `PATH`,
  and `model::tools::candidates` searches those **last** — so a whisper.cpp the
  user or the distribution installs later always wins over ours.
- **No Deno management, but the runtime is not ignored either.** This started as
  a flat dismissal — "yt-dlp finds a JavaScript runtime itself" — which is true and
  was the wrong conclusion. Run a current yt-dlp against YouTube with no engine
  installed and it says:

      YouTube extraction without a JS runtime has been deprecated, and some
      formats may be missing

  Silently offering fewer formats is the failure this rewrite exists to prevent,
  so ignoring that warning would have reintroduced it by the back door.

  Magpie therefore *detects* an engine — `deno`, `node` or `bun`, in that order —
  and passes `--js-runtimes <name>:<absolute path>`. The absolute path matters
  twice: yt-dlp enables only `deno` by default so the others must be named at all,
  and an engine installed by fnm, nvm or asdf lives on a `PATH` the user's shell
  has and a desktop launcher does not.

  It still installs nothing, which is the part the old application got wrong.
  And measured honestly: on the videos this was tested against the format list was
  identical with and without an engine, so the missing formats are yt-dlp's
  documented risk rather than something observed here. That is why it appears on
  the Tools page and in the guidance for the two failures it could cause, and not
  as a banner — a persistent warning about a maybe teaches people to ignore
  banners.

  Engine preference is yt-dlp's own, from its EJS setup guide: `deno` first
  (recommended, and the only one enabled without being named), then `node`, then
  `quickjs`, with `bun` last because yt-dlp has deprecated it. The advice prefers a
  system package — `snap install deno` where snapd exists, `apt install nodejs`
  otherwise, since Ubuntu packages node and not deno — because a system package
  updates with everything else. Neither is a command Magpie runs: both need a
  password.

- **`yt-dlp[default]`, not `yt-dlp`.** The other half of the same problem, and the
  less obvious one. yt-dlp solves YouTube's JavaScript challenges with an engine
  *and* a set of solver scripts, and those scripts live in a companion package,
  `yt-dlp-ejs`. The official standalone binaries bundle it; the PyPI package does
  not unless you ask for the `default` dependency group. So a plain
  `uv tool install yt-dlp` produces a yt-dlp that warns about missing formats
  however many engines are installed — which is exactly what the first install on
  the author's own machine did.

  Every command Magpie offers for a Python install therefore names
  `"yt-dlp[default]"`, and the upgrade command is a forced reinstall rather than an
  upgrade, so a yt-dlp first installed without the group gains the scripts instead
  of upgrading around them forever. A unit test asserts no offered command can lack
  it.
- **No first-run setup wizard.** There is nothing to set up. A missing tool is
  an `AdwBanner`, not a gate — the library and preferences still work without
  `yt-dlp`, and gating the whole window on a download is how the old application
  turned a missing dependency into a blank screen.
- **No SQLite.** JSON, written tmp → fsync → rename.
- **No i18n in v1.** Same call the sister projects made; strings are not marked
  and there is no `po/`.
- **Not YouTube-only.** The old URL grammar accepted eleven-character YouTube
  ids and nothing else, while the tool underneath supports some eighteen hundred
  sites. Magpie accepts any `http(s)` URL and lets yt-dlp decide, which is why
  the app is not named after one website.

## Settled

- App id `us.hagreli.Magpie`, binary `magpie`, GObject classes `Magpie*`.
- Licence GPL-3.0-or-later. The Electron original was MIT; this is a rewrite
  sharing no code with it, relicensed by its author to match the family.
- Config `~/.config/magpie/config.json`. Library and models
  `~/.local/share/magpie/`. Scratch files `~/.cache/magpie/`.
- Widget trees are built in Rust. No `.ui`, no Blueprint, no GResource.
- `model/` links no GTK and spawns no process.
- Downloads default to `XDG_DOWNLOAD_DIR`, not a folder Magpie invents.
