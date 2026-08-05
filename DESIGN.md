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
all ticked, above a line saying what that adds up to — `All 107 items · 58 hours`
— and the files land in a subfolder named after the playlist. Untick everything
and Download greys out rather than looking ready and doing nothing.

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
│   │ ▤  Bach — the complete cantatas                   │     │
│   │    Downloading 3 of 6 · 1.1 MB/s · 14 min  ⏸ ✕ ⌄ │     │
│   │    1  BWV 4 — Christ lag in Todesbanden   Saved ✓ │     │
│   │    2  BWV 8 — Liebster Gott               Saved ✓ │     │
│   │    3  BWV 12 — Weinen, Klagen        Downloading ◌│     │
│   │    4  BWV 21 — Ich hatte viel Bekümmernis   26:51 │     │
│   ├───────────────────────────────────────────────────┤     │
│   │ ▤  Wind ensemble rehearsal, take 4                │     │
│   │    Saved to Videos · transcript ready       ⌂  ✕  │     │
│   └───────────────────────────────────────────────────┘     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### A playlist is one row and a hundred files

Two scales are in play and the row used to mix them. `Downloading · 8 of 107 ·
100% · Almost done` was four true facts about the eighth file and one false
impression about the afternoon: the percentage, the speed and the time left all
belonged to the item in hand.

So a collection's row talks about the collection. The bar and the time left
count items — how many have landed, plus how far into the current one — where a
single video's still count bytes. The per-file detail moves to where it belongs:
the row opens.

The expanded list is derived, never stored. The files that have landed are the
record of what is done, matched to their items by the `008 - ` that yt-dlp's own
output template puts at the front of each name; the item in hand comes from the
progress line; everything else is still to come. Nothing is kept in step with
anything, so nothing can fall out of step — including across a restart, when the
downloaded files are still there and the subprocess is not.

The entries themselves are remembered on the job when the playlist is first
listed, so the rows have titles. A job that never had them — one queued by an
earlier Magpie — still opens: finished items are named by their files, the rest
are numbered, and opening the row asks yt-dlp for the names in the background.

Two figures on the progress line rather than one. `playlist_index` is where an
item sits in the playlist and what names its file; `playlist_autonumber` is how
far into the download queue it is. They agree until someone unticks a box —
`--playlist-items 20,30,40` makes the second item index 30 of 3 entries — and
the row wants the second while the file wants the first.

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

A playlist is transcribed item by item, one whisper at a time. One at a time is
not a limitation to lift later: whisper saturates the CPU on its own, and two of
them produce two transcripts in twice the time while making the machine
unusable.

The pass is offered but never assumed. The Add dialog shows the switch for a
playlist with what it costs written under it — *Every item, one after another —
this takes hours* — and leaves it off however **Transcribe by default** is set.
That preference is applied when a link is pasted, so it skips anything that looks
like a playlist, and a link that turns out to be one later has the presumed wish
taken back. A hundred and seven whisper runs is a decision; a switch meant for
single videos did not make it.

**Transcribe** on a finished row is where the decision is made afterwards. It
looks at the files, not at what was asked for at the time, so it covers both "I
downloaded this last week and now I want the words" and "I have the hundred and
seven files and none of the transcripts". Pressing it clears any earlier failures,
because pressing it is someone saying to try again.

What has words is derived from two lists on the job — the media it produced and
the transcripts written so far — matched by filename, which is safe because a
transcript *is* the media file with a different extension. So a pass that is
stopped, quit, and resumed the next day picks up at the first item without words
rather than starting the hundred again. **Stop Transcribing** exists for the same
reason: three hours of CPU needs a better way to stop than removing the row.

One item that whisper cannot read — four seconds of silence, audio the model
refuses — is recorded against that item and the pass moves on. Without the
record it would be retried for ever and the other hundred and six would never
start, which is the same shape of bug as the one `queue.rs` exists to prevent.
The row's tally says so afterwards: `104 of 107 transcribed`.

### Who is speaking

A transcript of a conversation with no attribution is a wall of text in which
nobody can find who said what. With **Identify speakers** on, a finished
transcript is handed to sherpa-onnx's diarizer, and every line comes back
labelled.

Two models, because it is two problems: pyannote segmentation 3.0 finds the
stretches of speech, and a WeSpeaker embedding turns each stretch into a vector.
Clustering those vectors is what makes the count an *answer* rather than a
setting — the number of clusters is the number of people. The user can override
it when they know, and saying "two" is meaningfully more reliable than letting
the threshold decide.

This is a second tool rather than a whisper flag because whisper has nothing that
does it. `--diarize` compares the loudness of a stereo file's two channels, which
says nothing about a mono download; `--tinydiarize` marks where the speaker
changes but never says whether a returning voice is one already heard, so it
cannot count. Neither answers the question.

Three parts, and only the middle one runs a process:

1. whisper is asked for a subtitle file as well as the chosen format, because
   plain text has no timestamps to align against. When the user wanted subtitles
   anyway, that file *is* their output and there is no extra one.
2. sherpa-onnx reads the same 16 kHz scratch wav the transcript used and prints
   `1.583 -- 3.406 speaker_00` lines. Asking for speakers therefore forces the
   ffmpeg conversion even for audio whisper would have taken as it stood — the
   diarizer reads 16 kHz WAV and refuses everything else.
3. `model::speakers` joins the two, giving each cue the voice that spoke most of
   it by total overlap, and renders the result.

Clustering returns sparse labels — a real two-speaker file came back as
`speaker_00` and `speaker_03` — so speakers are renumbered by who talks first.
Printing the raw ids would report a two-hander between Speaker 1 and Speaker 4.

Names are then guessed from what people call each other: "I'm Alice" names the
speaker, "thanks, Priya" names whoever just finished, "over to you, Marcus" names
whoever starts next. It is a heuristic and is treated as one — a name has to win
a vote, no two voices can share one, and the fallback is always the honest
number. The preferences page says so in as many words, because a wrong name is
worse than "Speaker 2": a reader cannot tell that it is wrong.

Failure here never costs the transcript. The words are already written and took
ten minutes; the attribution takes seconds and can be missing. Every way this
stage can fail ends with the plain transcript kept and a toast saying why it has
no names on it.

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
    collection.rs     A playlist job -> a line per item and where each got to.
    job.rs            One download's state machine.
    queue.rs          Ordering, parallelism, advance-on-any-outcome.
    library.rs        The record on disk. Atomic write, recover a corrupt file.
    settings.rs       config.json. Load never writes.
    tools.rs          Which external tools exist, where, how old.
    transcript.rs     whisper-cli argument vector and progress parsing.
    diarize.rs        sherpa-onnx argument vector, turn and progress parsing.
    speakers.rs       Turns + subtitle cues -> a transcript with names on it.
    agent/            `magpie agent`: verbs, refusals, JSON, its own help.

  ui/
    application.rs    MagpieApplication. Owns state; the only thing that writes.
    window.rs         MagpieWindow. Header bar, banner, link bar, the list.
    link_bar.rs       The entry, the paste button, the Add button.
    add_dialog.rs     Preview, format choices, playlist picker.
    job_row.rs        One row: thumbnail, status line, progress, controls, and
                      a playlist's items under a disclosure.
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

## An agent interface

`magpie agent <verb>` — `model/agent/`, built after the milestones above, for an
assistant to transcribe a video without a window. Six verbs: `help`, `describe`,
`tools`, `transcribe`, `list`, `show`. Five decisions worth recording.

**It rides the existing command line rather than a new D-Bus interface.** The
application gains `HANDLES_COMMAND_LINE`, so a second invocation is forwarded to
the running instance by GApplication, which is exactly the property this needs:
that process holds the queue in memory and rewrites `library.json` on every
change, so a separate process writing that file would be overwritten. Forwarding
makes the running app answer, which also puts the download in the window where
the user can watch it or cancel it. With nothing running, the invoked process
becomes primary and does the work itself. A custom D-Bus interface would have
bought a second copy of that plumbing and nothing else.

**It waits, rather than returning a job id to poll.** A transcript takes
minutes; the honest way to say so is to take minutes and then answer, which is
what `g_application_command_line_done` and a hold on the application are for.
Returning immediately would have cost more than it saved: the caller would have
to guess when to look, and in a process with no window there is nothing to keep
the download alive after the answer — the command would have handed back an id
for a job it then killed. Progress goes to stderr, throttled to one line every
five seconds, and stdout carries one JSON object.

**It transcribes; it is not a downloader.** Audio only, one video, and a
playlist link is refused. The window is where someone chooses 1080p or picks
eleven items out of a playlist, and neither is a thing to do blind. What the
verb *does* offer is everything about the transcript itself — format, language,
model, speakers, where to put it — because those are the parts of the answer.

**Silence means Preferences.** An option not given is read from
`settings.transcript`, the same `Wish` the Add dialog starts from, rather than a
default invented for the command line. A user who set SRT and the medium model
gets them from both, and there is only one place to change them.

**It refuses before it starts, and it fetches what it must.** The link, the
directory, and every tool are checked in the first second rather than ten
minutes into a download — those checks are pure functions in `model/agent`, so
the sentence a caller sees on a machine with nothing installed is a unit test.
The one thing it does download unprompted is the speech model, because the
caller asked for a transcript and the model is the only way to make one; stderr
says which and how big before it starts. The window asks first because a user
who ticked a switch has not agreed to 466 MB; a caller that asked for words has.

**Positional arguments, no `--flags`.** Not a style choice: GOption parses the
command line before any of this code runs and rejects options it was not told
about, while unknown *words* pass through. `key=value` carries the same
information. The application declares exactly one option, `--version`, because
GOption does not look at the command line at all until there is one — and
without that, `--help` is not a help page but a word handed to `command_line`,
which opens a window. `--help` is therefore GOption's, and its summary points at
`magpie agent help`.

Deliberately out: downloading video, playlists, changing preferences, cancelling
(the window has a button, and killing the command stops it when there is no
window), and opening files. `../familiar/docs/magpie-cli.md` documents the
surface from the caller's side.

## Deferred

**MCP into Familiar.** Familiar has no MCP client today — its tools are
`FunctionDeclaration`s hardcoded in `model/tools.rs`, dispatched by a match arm
in `ui/runner.rs`, and its own DESIGN.md defers MCP to "the next project". So
Magpie ships no MCP server, and the integration is designed for rather than
built.

The agent command line is what makes that cheap now: a `magpie mcp` subcommand
would be a stdio JSON-RPC loop over `model::agent`, whose verb table already
carries the name, arguments, return shape and `mutates` flag a tool definition
needs — `magpie agent describe` emits exactly that as JSON. Nothing about it is
a second implementation of anything.

The tools left to expose if that day comes are the ones the CLI deliberately
does not do:

| Tool | Wraps |
|---|---|
| `video_info(url)` | `model::media::parse` over `--dump-json` |
| `download(url, quality, audio_only, destination)` | `model::request::Request::argv` |

The open question is not the server, it is Familiar's side: whether Familiar
grows a real MCP client, or whether Magpie is simply a fifth match arm in
`Runner::run` alongside `recall` and `web_search`. Spawning `magpie agent` is
the second, and it is now a day's work rather than a project. That decision
belongs to Familiar's next milestone, not to this document.

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
- **sherpa-onnx is fetched, not built, and that is the same argument applied
  honestly.** It has the same lack of rot as whisper.cpp — local inference
  against pinned model files — so it earns the same pinned-and-bundled treatment.
  The difference is that upstream already publishes a prebuilt Linux binary for
  every release, so compiling it here would mean requiring CMake, a C++ toolchain
  and ONNX Runtime to arrive at a file identical to one already sitting on a
  release page. whisper.cpp gets built only because nobody publishes a usable
  Linux `whisper-cli`.

  `packaging/fetch-diarizer.sh` pins v1.13.4 and installs into
  `lib/magpie/bin` with the shared libraries in a sibling `lib/`, because the
  binary is linked with `RPATH=$ORIGIN/../lib` and will not start from anywhere
  else. The script runs `--help` before declaring success: for a downloaded
  tarball the likeliest fault is a shared library that will not load, and that
  failure would otherwise stay invisible until someone's first transcript.
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
