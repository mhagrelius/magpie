# Magpie

Bring videos home from the web.

Magpie is a video downloader for the GNOME desktop, in Rust with GTK 4 and
libadwaita. Paste a link, choose a quality, and the file lands in your Downloads
folder with a record of where it came from. Playlists arrive as a list to pick
from. The queue survives closing the window.

Downloading is done by [yt-dlp](https://github.com/yt-dlp/yt-dlp) and
transcribing by [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Magpie
bundles neither and downloads no executables of its own — both are installed
through your package manager and stay current with the rest of your system.

## Install

```bash
./install.sh                    # into ~/.local
./install.sh --with-whisper     # also build whisper.cpp, for transcripts
PREFIX=/usr/local sudo ./install.sh
```

or build a package:

```bash
packaging/build-deb.sh --install
packaging/build-deb.sh --with-whisper --install
packaging/build-flatpak.sh      # includes whisper.cpp; see the caveats below
```

### Requirements

| | |
|---|---|
| GTK 4.16 or newer, libadwaita 1.9 or newer | Ubuntu 25.10 / GNOME 50 and later |
| **yt-dlp** | Required. `uv tool install yt-dlp`, or `sudo apt install yt-dlp` |
| FFmpeg | For merging high quality video and converting audio. `sudo apt install ffmpeg` |
| whisper.cpp | Optional, only for transcripts. `./install.sh --with-whisper` builds it |
| `libsoup-3.0-dev` | To build |

**On yt-dlp's version.** The copy in a stable Ubuntu or Debian release is often
months behind, and months behind is frequently the difference between a download
working and failing with a message that blames something else. `uv tool install
yt-dlp` gets a current one, and Magpie prefers a `yt-dlp` in `~/.local/bin` over
one in `/usr/bin` for that reason.

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

### Where things are

| | |
|---|---|
| Downloads | Your Downloads folder, or wherever you point it |
| `~/.config/magpie/config.json` | Preferences |
| `~/.local/share/magpie/library.json` | The queue and the history |
| `~/.local/share/magpie/models/` | Whisper models |
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
what would make an MCP server cheap to add later. `DESIGN.md` has the whole
argument, including where this deliberately differs from the Electron
application it replaces.

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
