# Magpie

Bring videos home from the web.

Magpie is a video downloader for the GNOME desktop, in Rust with GTK 4 and
libadwaita. Paste a link, choose a quality, and the file lands in your Downloads
folder with a record of where it came from. Playlists arrive as a list to pick
from. The queue survives closing the window.

Downloading is done by [yt-dlp](https://github.com/yt-dlp/yt-dlp), transcribing
by [whisper.cpp](https://github.com/ggml-org/whisper.cpp), and telling voices
apart by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). yt-dlp is never
bundled and never self-updated — it is installed through your package manager and
stays current with the rest of your system, which matters because it is the one
of the three that rots when sites change. The two optional inference tools are
pinned and installed by an explicit flag to `./install.sh`, because Ubuntu
packages neither and local inference does not go stale.

## Features

**Downloading**

- Paste a link and go — the Add dialog fills in the title, thumbnail, duration
  and formats as soon as yt-dlp answers. `Ctrl+N` focuses the box; the paste
  button does it in one step.
- Quality presets from *Best available* down to a 480p ceiling, audio-only in the
  original format or converted to MP3/M4A, or any specific format id yt-dlp
  listed.
- Playlists and channels arrive as a checklist, all ticked, saved into a
  subfolder in playlist order. A `watch?v=…&list=…` link is treated as the one
  video you clicked.
- A queue that survives closing the window, with a configurable number of
  downloads running at once.
- Pause and resume (`SIGSTOP`/`SIGCONT`), cancel, retry, and resume from a
  part-finished file rather than starting again.
- Cookies from your browser for sites that want a signed-in account — read from
  the browser's own profile each time, never stored by Magpie.
- Optional rate limit, and a download folder you choose once.

**Transcripts**

- Transcribe a finished download to plain text, SRT or WebVTT, written beside the
  media file. Per download, or on by default.
- Four whisper models from Tiny to Medium, downloaded on demand with the size
  shown first and a button to delete one afterwards.
- Automatic language detection, or pick from sixteen.
- Audio is converted to 16 kHz mono in the cache directory, never next to your
  file, and the scratch copy is cleaned up.
- `magpie agent transcribe <url>` does the same thing from a terminal or an
  assistant and prints JSON, through the same queue the window uses.

**Identifying speakers**

- Works out **how many people are talking** by clustering voice embeddings —
  you are not asked to know in advance, though you can say so if you do.
- Every line attributed: `Speaker 1:` in text and SRT, a real `<v Speaker 1>`
  voice span in WebVTT, with subtitle timings untouched.
- Names picked up from what people call each other — "I'm Alice", "thanks,
  Priya", "over to you, Marcus" — treated as a guess that has to be earned, with
  an honest number as the fallback.
- The transcript is never lost to a failure here: if the tool, the models or the
  run is missing, you keep the plain transcript and get told why it has no names.

**Being honest about the tools**

- **Preferences → Tools** lists every program Magpie runs with its path, version
  and age, and offers Install, Update or Copy depending on what would actually
  work.
- A stale yt-dlp is named as the first thing to suspect, because months behind is
  frequently the difference between a download working and failing for reasons
  the error blames on something else.
- Failures state the cause and the fix rather than printing stderr, and Retry
  only appears where trying again could help.
- Nothing but yt-dlp is required to start. A missing tool is a banner or a greyed
  switch with an explanation, never a blank screen.

**The application itself**

- GTK 4 and libadwaita throughout, light and dark, and a layout that adapts down
  to a phone-width window.
- A record of every download in `library.json`, written atomically and recovered
  rather than lost if it is ever truncated.
- Undo for a removed download, and Clear Finished for the rest.

## Install

```bash
./install.sh                    # into ~/.local
./install.sh --with-whisper     # also build whisper.cpp, for transcripts
./install.sh --with-diarizer    # also fetch sherpa-onnx, to identify speakers
PREFIX=/usr/local sudo ./install.sh
```

or build a package:

```bash
packaging/build-deb.sh --install
packaging/build-deb.sh --with-whisper --with-diarizer --install
packaging/build-flatpak.sh      # includes both; see the caveats below
```

### Requirements

| | |
|---|---|
| GTK 4.16 or newer, libadwaita 1.9 or newer | Ubuntu 25.10 / GNOME 50 and later |
| **yt-dlp** | Required. `uv tool install "yt-dlp[default]"`, or `sudo apt install yt-dlp` |
| **A JavaScript engine** | Deno recommended. YouTube needs one to reveal every format |
| FFmpeg | For merging high quality video and converting audio. `sudo apt install ffmpeg` |
| whisper.cpp | Optional, only for transcripts. `./install.sh --with-whisper` builds it |
| sherpa-onnx | Optional, only to mark who is speaking. `./install.sh --with-diarizer` fetches it |
| `libsoup-3.0-dev` | To build |

**Two things about yt-dlp that are easy to get wrong**, and that Magpie's Tools
page will tell you about:

*Its version.* The copy in a stable Ubuntu or Debian release is often months
behind, and months behind is frequently the difference between a download working
and failing with a message that blames something else. `uv tool install
"yt-dlp[default]"` gets a current one, and Magpie prefers a `yt-dlp` in
`~/.local/bin` over one in `/usr/bin` for that reason.

*The `[default]` group, and a JavaScript engine.* YouTube now presents JavaScript
challenges, and yt-dlp solves them with an external engine plus a set of solver
scripts. The scripts ship in the `yt-dlp-ejs` package, which the bare PyPI install
leaves out — hence `"yt-dlp[default]"`. The engine you install yourself:

```bash
sudo snap install deno          # if you have snapd; Deno is what yt-dlp recommends
sudo apt install nodejs         # Ubuntu packages this one; needs 22.0.0 or newer
```

Deno's own installer puts it in `~/.deno/bin`, which Magpie searches directly —
so it is found even though that directory is only on your `PATH` inside a shell,
and a desktop launcher gives an app no shell. Without an engine yt-dlp still
works but warns that some formats may be missing; Magpie passes
`--js-runtimes <engine>:<absolute path>` for whichever it finds. Note `bun` is
supported but deprecated by yt-dlp, so it is the last one Magpie looks for.

You do not have to remember any of that. **Preferences → Tools** lists every
program Magpie uses with its path, version and age, and offers a button:

- Missing, and you have `uv` or `pipx` → **Install**, which runs
  `uv tool install yt-dlp` for you and shows what it said if it fails.
- Too old, same → **Update**, which runs `uv tool upgrade yt-dlp`.
- Needs a terminal (`sudo apt install ffmpeg`) or a compiler (whisper.cpp) →
  **Copy**, because Magpie will not ask you for a password and will not build C++
  behind your back.

Note `uv tool install`, not `uvx`: `uvx` runs a tool without leaving anything on
`PATH`, so Magpie would still report it missing.

Nothing but yt-dlp is required to start. A missing tool is a banner or a greyed
switch with an explanation, never a blank screen.

## Using it

### Taking a link

The link bar is at the top of the window and always available. Paste, press
Enter, and the **Add Download** dialog opens with the title, the thumbnail and
the format choices as soon as yt-dlp answers. `Ctrl+N` puts the cursor in the
box; the paste button adds a link in one step.

Turn off *Ask before each download* in Preferences to skip the dialog and start
immediately with your defaults.

### Quality

| Preset | What it does |
|---|---|
| Best available | The best video and the best audio, merged |
| Up to 1080p / 720p / 480p | A ceiling, not a requirement — a video published only at 1440p still downloads |
| Audio only | Original format, or converted to MP3 or M4A |
| Choose a specific format | Everything yt-dlp listed, passed through by id |

*Best available* and the capped presets combine separate video and audio streams,
which is how anything above 360p is served. That needs FFmpeg. Audio *Best
available* is a copy rather than a transcode, so it needs nothing.

### Playlists and channels

A playlist link shows every item with a check box, all ticked. Untick what you do
not want. The files go into a subfolder named after the playlist, numbered in
playlist order.

A link with both a video and a playlist in it — `watch?v=…&list=…` — is treated
as the one video you clicked.

### While it runs

Each row shows what is happening in words: `Downloading · 47% · 3.2 MB/s ·
1 minute left`. The speed is averaged over the last ten samples, because yt-dlp's
instantaneous figure swings by a factor of three and a number that jumps is worse
than no number.

Pause and resume use `SIGSTOP` and `SIGCONT`. Cancelling asks the download to
stop, then insists after five seconds. A part-finished download resumes from
where it stopped rather than starting again.

### When it fails

Almost every failure has one remedy, so the row states the cause and the dialog
states the fix — *The site asked for a signed-in account* points at the cookies
setting; *That quality is not available* suggests Best available. Retry is only
offered where trying again could work, so there is no button that cannot help.

For sites that want a signed-in account, turn on **Use cookies from a browser** in
Preferences and pick the browser you are signed in with. Magpie never stores the
cookies; yt-dlp reads them from the browser's own profile each time.

### Transcripts

A finished download can be transcribed to plain text, SRT or WebVTT, written
beside the media file. Turn on **Transcribe** in the Add dialog, or set it as the
default in Preferences.

This needs whisper.cpp, which Ubuntu does not package, so Magpie ships a script
that builds it — `./install.sh --with-whisper`, or `packaging/build-whisper.sh` on
its own. It pins whisper.cpp v1.9.1, takes about half a minute, and builds CPU-only
with `-DGGML_NATIVE=OFF` so the binary is not tuned to the machine that compiled
it. The Flatpak includes it already.

Then pick a model in Preferences → Transcripts and press Download:

| Model | Size | |
|---|---|---|
| Tiny | 75 MB | fastest, roughest |
| Base | 142 MB | quick, fine for clear speech |
| **Small** | **466 MB** | **the default, and a good balance** |
| Medium | 1.5 GB | slowest, most accurate |

Anything that is not already WAV, MP3, FLAC or OGG — which is most downloads — is
converted to 16 kHz mono first, in the cache directory rather than next to your
file, and the scratch copy is deleted afterwards.

### Who is speaking

Turn on **Identify speakers** as well and a transcript of a conversation comes
back attributed, instead of as a wall of text in which nobody can find who said
what:

```
Speaker 1: A pencil with black lead writes best, the lamp shone with a
steady green flame.

Speaker 2: Clothes and lodging are free to new men, the glow deepened in
the eyes of the sweet girl.
```

Magpie works out **how many people are talking** rather than asking you: voices
are turned into vectors and clustered, and the number of clusters is the number
of speakers. If you already know, say so in Preferences — a fixed count is more
reliable than a threshold deciding.

Where people say each other's names, those are used instead of numbers. "I'm
Alice" names the speaker, "thanks, Priya" names whoever just finished, "over to
you, Marcus" names whoever starts next. This is a guess and is treated as one: a
name has to be said more than once to beat a rival, no two speakers can end up
sharing one, and anything unproven stays "Speaker 2".

Subtitles get the same treatment with their timings untouched — SRT as a
`Speaker 1:` prefix, WebVTT as a real `<v Speaker 1>` voice span.

This needs sherpa-onnx and two models of its own (about 34 MB, downloaded from
Preferences → Transcripts). `./install.sh --with-diarizer` fetches the binary,
pinned to v1.13.4. If any of it is missing or the run fails, you still get the
plain transcript and a note saying why it has no names on it.

### From a script or an assistant

`magpie agent` transcribes a video without a window, printing JSON.

```sh
magpie agent tools                        # can a transcript be made here?
magpie agent transcribe https://youtu.be/dQw4w9WgXcQ
magpie agent transcribe https://youtu.be/dQw4w9WgXcQ format=srt speakers=2 dir=.
magpie agent list bees                    # find one made earlier
magpie agent show 7
```

`transcribe` downloads the audio, transcribes it, and answers when the words
exist — so it takes minutes, and wants a long timeout or a background run.
Progress goes to stderr; stdout is one JSON object naming the transcript file
and the audio beside it. Options not given come from Preferences → Transcripts,
so there is one place to set the model and the format rather than two. The
first run downloads the speech model, which stderr says before it starts.

It transcribes rather than downloads: audio only, one video, and a playlist link
is refused. Choosing 1080p or picking eleven items out of a playlist is what the
window is for.

When Magpie is running the command is handed to it over the same D-Bus channel a
second launch uses, so the download appears in the window and there is one list
rather than two copies falling out of step. With nothing running, the command
does the work itself and records it the same way.

`magpie agent help` documents every verb; `magpie agent describe` prints the
same thing as JSON, for a caller generating tool definitions.

### Where things are

| | |
|---|---|
| Downloads | Your Downloads folder, or wherever you point it |
| `~/.config/magpie/config.json` | Preferences |
| `~/.local/share/magpie/library.json` | The queue and the history |
| `~/.local/share/magpie/models/` | Speech and speaker models |
| `~/.cache/magpie/` | Thumbnails and scratch files |

## How it works

Two halves. `src/model/` links no GTK and spawns no process: it turns a request
into an argument vector and a line of yt-dlp's output back into an event.
`src/ui/` is the only half that knows a window exists, and
`ui::MagpieApplication` is the only thing that runs a subprocess or writes a
file.

That split is why the interesting parts are testable with no display and no
network — a progress line arriving in two reads, a format selector for a video
that only exists at 1440p, a `library.json` truncated by a power cut — and it is
what made `magpie agent` a small addition rather than a second application:
`src/model/agent/` decides what to refuse and what to report, and `ui/` runs the
same jobs the window runs. `DESIGN.md` has the whole argument, including where
this deliberately differs from the Electron application it replaces.

Widget trees are built in Rust. No `.ui` files, no Blueprint, no GResource. There
is no tokio: GLib's main loop already runs futures, and every wait here is I/O.

## Flatpak caveats

A sandbox cannot run the host's yt-dlp, so the Flatpak bundles one — pinned and
checksummed at build time, which means it is only as new as the last build. It does
build whisper.cpp, which does not have that problem. What it has no binary for is
ffmpeg, so MP3 conversion is unavailable in the sandbox; that degrades with an
explanation rather than failing, and "Best available" audio needs no conversion.
The `.deb` and `install.sh` are the supported route for v1. The manifest header
explains the whole trade-off.

## Development

```bash
cargo run
./test.sh                      # fmt, clippy -D warnings, then tests
./test.sh --headless           # the same under xvfb-run dbus-run-session
cargo run --example preview -- /tmp/preview        # render the window to a PNG
cargo run --example preview -- /tmp/preview dark
```

### Tests

Model tests are inline at the bottom of each file in `src/model/` and need no
display. `tests/session.rs` drives the whole model half end to end over recorded
yt-dlp output. `tests/widgets.rs` is one test containing a list of cases, because
GTK is thread-affine.

## Licence

GPL-3.0-or-later.

This replaces an Electron application of the same purpose that lived in this
repository under the MIT licence. Magpie is a rewrite sharing no code with it,
relicensed by its author to match its sibling projects.
