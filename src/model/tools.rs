//! Finding the programs Magpie runs, and judging whether they will work.
//!
//! Magpie ships none of these. The old application downloaded `yt-dlp`, a Deno
//! runtime and a self-hosted static `whisper-cli` from GitHub into
//! `~/.local/share`, unpacked them with `unzip`, and marked them executable —
//! a self-updating binary outside the package manager, which is wrong in a
//! `.deb` and impossible in a Flatpak. So this file's job is detection and
//! honest reporting instead.
//!
//! Nothing here runs anything: the search is a pure function over a `PATH`
//! string and an "does this file exist and is it executable" predicate that
//! `ui/` supplies. That is what makes the ordering rules testable.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

/// A program Magpie runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Ffprobe,
    Whisper,
    /// A JavaScript engine, which YouTube extraction now needs.
    ///
    /// This entry corrects an earlier mistake. The application Magpie replaces
    /// downloaded and managed a Deno runtime, and the first cut of this file
    /// dismissed that as pointless on the grounds that yt-dlp finds a runtime by
    /// itself. It does — but when there is none it prints "YouTube extraction
    /// without a JS runtime has been deprecated, and some formats may be missing"
    /// to stderr and carries on.
    ///
    /// Measured, not assumed: on the videos this was tested against the format
    /// list was *identical* with and without a runtime, so the lost formats are
    /// documented by yt-dlp rather than observed here. What naming a runtime
    /// definitely does is silence a deprecation warning and remove the risk. That
    /// is worth one argument, and it is not worth a banner — see
    /// `ui::toolbox::Report::banner`.
    ///
    /// Magpie detects a runtime and tells yt-dlp where it is. It does not install
    /// one, which is the part the old application got wrong.
    JsRuntime,
}

impl Tool {
    /// The command names to try, in order.
    ///
    /// Deliberately *not* including a bare `whisper`: on Linux that is almost
    /// always OpenAI's Python implementation, whose command line shares none of
    /// whisper.cpp's flags. The old application probed for it and would have
    /// produced a baffling argument error on any machine that had it. A missing
    /// tool is a clear message; a wrong tool is a bug report.
    pub fn commands(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp => &["yt-dlp"],
            Tool::Ffmpeg => &["ffmpeg"],
            Tool::Ffprobe => &["ffprobe"],
            Tool::Whisper => &["whisper-cli", "whisper-cpp"],
            // Deno first because it is the only one yt-dlp enables by default;
            // the others work but have to be pointed at explicitly. Order is
            // preference, so a machine with all three gets the one needing no
            // extra argument.
            Tool::JsRuntime => &["deno", "node", "bun"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "FFmpeg",
            Tool::Ffprobe => "FFprobe",
            Tool::Whisper => "whisper.cpp",
            Tool::JsRuntime => "JavaScript runtime",
        }
    }

    /// What Magpie needs it for, shown under the name on the Tools page.
    pub fn purpose(self) -> &'static str {
        match self {
            Tool::YtDlp => "Required. Every download goes through it",
            Tool::Ffmpeg => "Needed to merge high quality video and to convert audio",
            Tool::Ffprobe => "Comes with FFmpeg. Used to measure audio before transcribing",
            Tool::Whisper => "Optional. Needed only for transcripts",
            Tool::JsRuntime => {
                "YouTube needs one to reach every format. Without it, the higher \
                 qualities may be missing"
            }
        }
    }

    /// The command to run, ready to paste into a terminal.
    ///
    /// Tailored to what the user already has. A hint that names a tool they would
    /// first have to install is two chores presented as one, and the most likely
    /// outcome is that they do neither and conclude the app is broken.
    pub fn install_command(self, installers: Installers) -> String {
        match self {
            // apt's yt-dlp is frequently months behind, and months behind is the
            // difference between working and not on YouTube — so a route to a
            // current one wins wherever the user has one.
            //
            // `uv tool install`, never `uvx`: uvx runs a tool without leaving
            // anything on PATH, so someone who followed a `uvx yt-dlp` hint would
            // come straight back here and still be told it is not installed.
            Tool::YtDlp if installers.uv => "uv tool install yt-dlp".into(),
            Tool::YtDlp if installers.pipx => "pipx install yt-dlp".into(),
            Tool::YtDlp => "sudo apt install yt-dlp".into(),
            Tool::Ffmpeg | Tool::Ffprobe => "sudo apt install ffmpeg".into(),
            // No package on Ubuntu, so Magpie ships a script that builds one.
            // Not offered as a button: it needs cmake and a compiler, takes a
            // couple of minutes, and a GUI button that silently starts a C++
            // build is a button whose failure nobody can read.
            Tool::Whisper => "./install.sh --with-whisper".into(),
            // Not in Ubuntu's archive, and not something to fetch with a shell
            // pipeline on the user's behalf. Node is the one most machines
            // already have, so it is named first.
            Tool::JsRuntime => "sudo apt install nodejs — or deno.land".into(),
        }
    }

    /// The command to run to get a *newer* one, when the installed copy is too
    /// old rather than absent.
    pub fn upgrade_command(self, installers: Installers) -> Option<String> {
        match self {
            Tool::YtDlp if installers.uv => Some("uv tool upgrade yt-dlp".into()),
            Tool::YtDlp if installers.pipx => Some("pipx upgrade yt-dlp".into()),
            // Nothing useful to offer: apt's copy is as new as the archive has,
            // and telling someone to `apt upgrade` a package that is already at
            // its latest version is advice that cannot work.
            Tool::YtDlp => None,
            _ => None,
        }
    }

    /// The sentence shown under the tool's name when it is missing.
    pub fn install_hint(self, installers: Installers) -> String {
        let command = self.install_command(installers);
        match self {
            Tool::Whisper => format!("Build it from Magpie's source tree — {command}"),
            // Said out loud, because someone who installs the distribution's
            // package and then hits a failure deserves to have been warned.
            Tool::YtDlp if !installers.uv && !installers.pipx => format!(
                "{command} — though the packaged version is often too old for YouTube. \
                 Installing uv gets a current one."
            ),
            _ => command,
        }
    }

    /// Whether a download can happen at all without it.
    pub fn is_required(self) -> bool {
        matches!(self, Tool::YtDlp)
    }

    /// The argument that makes it print its version.
    pub fn version_argv(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp => &["--version"],
            Tool::Ffmpeg | Tool::Ffprobe => &["-version"],
            // whisper.cpp has no --version; -h exits zero and prints a banner,
            // which is enough to prove the binary runs.
            Tool::Whisper => &["-h"],
            Tool::JsRuntime => &["--version"],
        }
    }
}

/// Which tool installers the user already has.
///
/// Used only to phrase advice. Magpie names a route the user is already set up
/// for, because a hint that names a tool they would first have to install is two
/// chores presented as one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Installers {
    pub uv: bool,
    pub pipx: bool,
}

impl Installers {
    /// Whether Magpie can run this exact command itself.
    ///
    /// Decided by the program it names, not by what the string looks like. An
    /// earlier version asked "does it start with `sudo` or `http`", which quietly
    /// approved `./install.sh --with-whisper` the moment that became whisper's
    /// hint — offering an Install button for a command the runner then refused.
    ///
    /// Only the user-local installers qualify. `sudo apt install` is excluded
    /// because it needs a password, and an application that raises a privilege
    /// prompt to install something is asking for a trust it does not need;
    /// everything else is excluded because Magpie is not a shell.
    ///
    /// This is the same rule `ui::toolbox::run_installer` enforces, and it is here
    /// so the button and the runner cannot disagree.
    pub fn can_run(self, command: &str) -> bool {
        match command.split_whitespace().next() {
            Some("uv") => self.uv,
            Some("pipx") => self.pipx,
            _ => false,
        }
    }
}

/// Tools whose own installer uses a private directory and edits the shell profile
/// to reach it. A desktop launcher reads no shell profile, so these are searched
/// directly.
const HOME_INSTALL_DIRECTORIES: [(&str, &str); 2] = [("bun", ".bun/bin"), ("deno", ".deno/bin")];

/// Where a Magpie package puts a tool it had to bundle.
///
/// `/usr/lib/magpie` is the `.deb`'s, `/app/lib/magpie` the Flatpak's. Neither is
/// on `PATH`, deliberately: these are Magpie's copies and nothing else's.
const PRIVATE_DIRECTORIES: [&str; 2] = ["/usr/lib/magpie", "/app/lib/magpie"];

/// A tool that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// As the program reported it, when it was asked and answered.
    pub version: Option<String>,
}

/// Where to look for a command, most-preferred first.
///
/// `~/.local/bin` and the per-tool virtualenv directories come **before**
/// `PATH`, which is a deliberate inversion for one reason: a user who installed
/// yt-dlp with `uv tool install` because the distribution's copy was too old
/// should get the one they installed, even though `/usr/bin` may come first in
/// their `PATH`. The alternative is an application that tells them to update a
/// tool they already updated.
///
/// `~/.local/bin` is where both `uv tool install` and `pipx install` put their
/// shims, so it covers the ordinary case on its own. The venv directories below
/// it are for the user who pointed `UV_TOOL_BIN_DIR` somewhere off `PATH`.
pub fn candidates(command: &str, path_var: &str, home: &Path) -> Vec<PathBuf> {
    let mut preferred = vec![
        home.join(".local/bin"),
        home.join(".local/share/uv/tools").join(command).join("bin"),
        home.join(".local/share/pipx/venvs")
            .join(command)
            .join("bin"),
    ];

    // A few tools install to a directory of their own that their own installer
    // adds to the shell's PATH — which a desktop launcher does not inherit. Left
    // out, a `bun` sitting exactly where bun.sh puts it is invisible to Magpie
    // when launched from the applications grid and visible when launched from a
    // terminal, which is a difference nobody could diagnose.
    for (name, directory) in HOME_INSTALL_DIRECTORIES {
        if command == name {
            preferred.push(home.join(directory));
        }
    }

    let mut directories: Vec<PathBuf> = preferred;
    directories.extend(
        path_var
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from),
    );
    // Magpie's own private directories, searched **last**. This is where a
    // package puts a tool it had to bundle because the distribution has none —
    // `whisper-cli`, built by packaging/build-whisper.sh. Last, because anything
    // the user or the distribution installed later should win over our copy, and
    // because a private binary must never shadow a system one.
    directories.extend(PRIVATE_DIRECTORIES.iter().map(PathBuf::from));

    let mut seen = Vec::new();
    let mut paths = Vec::new();
    for directory in directories {
        if seen.contains(&directory) {
            continue;
        }
        seen.push(directory.clone());
        paths.push(directory.join(command));
    }
    paths
}

/// Find a tool by trying each of its command names in each candidate directory.
///
/// `is_executable` is passed in so this stays a pure function; `ui/` supplies
/// the one that touches the filesystem.
pub fn locate(
    tool: Tool,
    path_var: &str,
    home: &Path,
    is_executable: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    tool.commands().iter().find_map(|command| {
        candidates(command, path_var, home)
            .into_iter()
            .find(|path| is_executable(path))
    })
}

/// How much to worry about the installed yt-dlp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Freshness {
    /// Recent enough that its age is not the problem.
    Fresh,
    /// Nothing has been asked yet, or the version string was not a date.
    #[default]
    Unknown,
    /// Old enough to mention, not old enough to warn about.
    Ageing { days: i64 },
    /// Old enough that it is the first thing to suspect.
    Stale { days: i64 },
}

impl Freshness {
    /// The sentence for the Tools page, or `None` when there is nothing worth
    /// saying.
    pub fn advice(self) -> Option<&'static str> {
        match self {
            Freshness::Fresh | Freshness::Unknown => None,
            Freshness::Ageing { .. } => {
                Some("Newer releases handle site changes that this one may not.")
            }
            Freshness::Stale { .. } => Some(
                "Sites change faster than this. If downloads are failing, updating yt-dlp \
                 is the first thing to try.",
            ),
        }
    }

    pub fn is_stale(self) -> bool {
        matches!(self, Freshness::Stale { .. })
    }
}

/// yt-dlp versions are release dates: `2025.07.21`, sometimes with a fourth
/// component for a same-day rebuild.
pub fn release_date(version: &str) -> Option<NaiveDate> {
    let mut parts = version.trim().split('.');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.trim_start_matches('0').parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Judge a version string against today.
pub fn freshness(version: &str, today: NaiveDate) -> Freshness {
    let Some(released) = release_date(version) else {
        return Freshness::Unknown;
    };
    let days = (today - released).num_days();
    match days {
        // A clock skew or a nightly build from the future is not staleness.
        d if d < 45 => Freshness::Fresh,
        d if d < 120 => Freshness::Ageing { days: d },
        d => Freshness::Stale { days: d },
    }
}

/// The first line of a `--version` output, tidied.
///
/// ffmpeg prints a paragraph beginning `ffmpeg version 6.1.1-3ubuntu5`, so the
/// interesting part is the third word; yt-dlp prints the version alone.
pub fn parse_version(tool: Tool, stdout: &str) -> Option<String> {
    let first = stdout.lines().find(|line| !line.trim().is_empty())?.trim();
    match tool {
        Tool::YtDlp => Some(first.to_string()),
        Tool::Ffmpeg | Tool::Ffprobe => first
            .split_whitespace()
            .nth(2)
            .map(|version| version.split('-').next().unwrap_or(version).to_string()),
        // The banner says nothing useful about a version, and inventing one
        // would be worse than admitting it.
        Tool::Whisper => None,
        // `deno 2.5.3`, `v22.11.0`, `1.1.38` — whichever engine it is, the last
        // whitespace-separated word is the version, with node's `v` dropped.
        Tool::JsRuntime => first
            .split_whitespace()
            .next_back()
            .map(|version| version.trim_start_matches('v').to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_user_installed_copy_wins_over_the_distributions_older_one() {
        // The point of the whole ordering rule. Someone who ran
        // `uv tool install yt-dlp` because apt's was too old must not be told to
        // update again. `~/.local/bin` is where both uv and pipx put shims.
        let home = Path::new("/home/matty");
        let paths = candidates("yt-dlp", "/usr/local/bin:/usr/bin", home);
        assert_eq!(paths[0], PathBuf::from("/home/matty/.local/bin/yt-dlp"));
        assert!(paths.contains(&PathBuf::from("/usr/bin/yt-dlp")));
        // And the venv directories, for a UV_TOOL_BIN_DIR pointed off PATH.
        assert!(paths.contains(&PathBuf::from(
            "/home/matty/.local/share/uv/tools/yt-dlp/bin/yt-dlp"
        )));
    }

    #[test]
    fn an_engine_in_its_own_installers_directory_is_found() {
        // bun.sh installs to ~/.bun/bin and puts that on PATH by editing the
        // shell profile. A desktop launcher reads no profile, so without this the
        // same machine finds bun from a terminal and not from the app grid.
        let home = Path::new("/home/matty");
        let bun = candidates("bun", "/usr/bin", home);
        assert!(
            bun.contains(&PathBuf::from("/home/matty/.bun/bin/bun")),
            "{bun:?}"
        );

        let deno = candidates("deno", "/usr/bin", home);
        assert!(deno.contains(&PathBuf::from("/home/matty/.deno/bin/deno")));

        // And the private directory is not offered to unrelated commands.
        let ytdlp = candidates("yt-dlp", "/usr/bin", home);
        assert!(!ytdlp.iter().any(|p| p.to_string_lossy().contains(".bun")));
    }

    #[test]
    fn a_bundled_copy_never_shadows_one_the_user_installed() {
        // The .deb may ship a whisper-cli because Ubuntu has no package for it.
        // Someone who later builds their own must get theirs, so the private
        // directory is searched after everything else.
        let paths = candidates("whisper-cli", "/usr/bin", Path::new("/home/matty"));
        let ours = paths
            .iter()
            .position(|p| p.starts_with("/usr/lib/magpie"))
            .expect("the private directory is searched");
        let theirs = paths
            .iter()
            .position(|p| p.starts_with("/home/matty/.local/bin"))
            .expect("and so is ~/.local/bin");
        let system = paths
            .iter()
            .position(|p| p.starts_with("/usr/bin"))
            .expect("and PATH");
        assert!(theirs < ours && system < ours, "{paths:?}");
    }

    #[test]
    fn a_directory_listed_twice_is_only_searched_once() {
        let paths = candidates("yt-dlp", "/usr/bin:/usr/bin:", Path::new("/home/matty"));
        let usr = paths.iter().filter(|p| p.starts_with("/usr/bin")).count();
        assert_eq!(usr, 1);
    }

    #[test]
    fn a_home_directory_entry_in_path_is_not_duplicated() {
        let paths = candidates(
            "yt-dlp",
            "/home/matty/.local/bin:/usr/bin",
            Path::new("/home/matty"),
        );
        let local = paths
            .iter()
            .filter(|p| p.starts_with("/home/matty/.local/bin"))
            .count();
        assert_eq!(local, 1);
    }

    #[test]
    fn plain_whisper_is_never_probed_for() {
        // `/usr/bin/whisper` on Linux is OpenAI's Python implementation, which
        // shares no flags with whisper.cpp. Finding it would turn "not
        // installed" into an unexplained argument error.
        assert!(!Tool::Whisper.commands().contains(&"whisper"));
        assert_eq!(Tool::Whisper.commands(), &["whisper-cli", "whisper-cpp"]);
    }

    #[test]
    fn the_first_command_name_that_exists_is_the_one_used() {
        let found = |path: &Path| path == Path::new("/usr/bin/whisper-cpp");
        assert_eq!(
            locate(Tool::Whisper, "/usr/bin", Path::new("/home/m"), &found),
            Some(PathBuf::from("/usr/bin/whisper-cpp"))
        );

        let nothing = |_: &Path| false;
        assert_eq!(
            locate(Tool::YtDlp, "/usr/bin", Path::new("/home/m"), &nothing),
            None
        );
    }

    #[test]
    fn a_yt_dlp_version_is_a_release_date() {
        assert_eq!(release_date("2025.07.21"), Some(date("2025-07-21")));
        assert_eq!(release_date("2025.06.09.232815"), Some(date("2025-06-09")));
        assert_eq!(release_date("nightly"), None);
        assert_eq!(release_date("2025.13.01"), None, "month 13 is not a date");
    }

    #[test]
    fn a_stale_yt_dlp_is_named_as_the_first_thing_to_suspect() {
        // The single most common cause of a YouTube download failing, and the
        // one the old application had no way to mention.
        let stale = freshness("2024.01.01", date("2026-08-01"));
        assert!(stale.is_stale());
        assert!(stale
            .advice()
            .is_some_and(|a| a.contains("updating yt-dlp")));
    }

    #[test]
    fn a_recent_yt_dlp_is_not_nagged_about() {
        let fresh = freshness("2026-07-20".replace('-', ".").as_str(), date("2026-08-01"));
        assert_eq!(fresh, Freshness::Fresh);
        assert_eq!(fresh.advice(), None);
    }

    #[test]
    fn a_version_from_the_future_is_a_clock_problem_not_a_stale_tool() {
        assert_eq!(
            freshness("2026.12.01", date("2026-08-01")),
            Freshness::Fresh
        );
    }

    #[test]
    fn an_unrecognised_version_string_produces_no_advice_rather_than_a_guess() {
        let unknown = freshness("some-git-build", date("2026-08-01"));
        assert_eq!(unknown, Freshness::Unknown);
        assert_eq!(unknown.advice(), None);
    }

    #[test]
    fn a_version_is_read_out_of_each_tools_own_shape_of_output() {
        assert_eq!(
            parse_version(Tool::YtDlp, "2025.07.21\n").as_deref(),
            Some("2025.07.21")
        );
        assert_eq!(
            parse_version(
                Tool::Ffmpeg,
                "ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023\nbuilt with gcc"
            )
            .as_deref(),
            Some("6.1.1")
        );
        assert_eq!(
            parse_version(Tool::Whisper, "usage: whisper-cli [options]"),
            None
        );
    }

    #[test]
    fn only_yt_dlp_is_required() {
        // Everything else degrades: no ffmpeg means no merging and no MP3, no
        // whisper means no transcripts. Neither is a reason to block the window.
        assert!(Tool::YtDlp.is_required());
        for tool in [Tool::Ffmpeg, Tool::Ffprobe, Tool::Whisper] {
            assert!(!tool.is_required(), "{tool:?}");
        }
    }

    #[test]
    fn every_tool_says_how_to_install_it_whatever_the_user_has() {
        for installers in [
            Installers::default(),
            Installers {
                uv: true,
                pipx: false,
            },
            Installers {
                uv: false,
                pipx: true,
            },
        ] {
            for tool in [Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe, Tool::Whisper] {
                assert!(!tool.install_hint(installers).is_empty(), "{tool:?}");
                assert!(!tool.install_command(installers).is_empty(), "{tool:?}");
                assert!(!tool.purpose().is_empty(), "{tool:?}");
            }
        }
    }

    #[test]
    fn the_advice_names_a_tool_the_user_already_has() {
        // Telling someone to install yt-dlp with pipx when they use uv is two
        // chores dressed up as one, and the likely outcome is neither.
        let uv = Installers {
            uv: true,
            pipx: false,
        };
        assert_eq!(Tool::YtDlp.install_command(uv), "uv tool install yt-dlp");
        assert_eq!(
            Tool::YtDlp.upgrade_command(uv).as_deref(),
            Some("uv tool upgrade yt-dlp")
        );

        let pipx = Installers {
            uv: false,
            pipx: true,
        };
        assert_eq!(Tool::YtDlp.install_command(pipx), "pipx install yt-dlp");
        assert_eq!(
            Tool::YtDlp.upgrade_command(pipx).as_deref(),
            Some("pipx upgrade yt-dlp")
        );
    }

    #[test]
    fn uvx_is_never_suggested_because_it_leaves_nothing_behind() {
        // `uvx yt-dlp` runs the tool without putting anything on PATH, so a user
        // who followed that advice would come back to a Tools page still saying
        // "not installed". `uv tool install` is the one that helps.
        for installers in [
            Installers::default(),
            Installers {
                uv: true,
                pipx: true,
            },
        ] {
            let command = Tool::YtDlp.install_command(installers);
            assert!(!command.contains("uvx"), "{command}");
        }
    }

    #[test]
    fn with_no_installer_the_packaged_version_is_offered_with_its_caveat() {
        // apt's yt-dlp is the only route left, and it is frequently too old for
        // YouTube. Handing it over without saying so sets up a failure the user
        // has no way to interpret.
        let nothing = Installers::default();
        assert_eq!(
            Tool::YtDlp.install_command(nothing),
            "sudo apt install yt-dlp"
        );
        assert!(Tool::YtDlp.install_hint(nothing).contains("too old"));
        // And there is no upgrade command, because there is nothing to upgrade
        // to — a button offering one could not work.
        assert_eq!(Tool::YtDlp.upgrade_command(nothing), None);
    }

    #[test]
    fn magpie_only_offers_to_run_the_installers_it_found() {
        let uv = Installers {
            uv: true,
            pipx: false,
        };
        assert!(uv.can_run("uv tool install yt-dlp"));
        // Named but not installed, so there is nothing to run.
        assert!(!uv.can_run("pipx install yt-dlp"));
        assert!(!Installers::default().can_run("uv tool install yt-dlp"));

        // A privilege prompt is a trust Magpie does not need, and Magpie is not a
        // shell. Both are refused by naming a program it will not run, rather than
        // by inspecting the shape of the string — an earlier prefix check approved
        // `./install.sh --with-whisper` the moment that became whisper's hint, and
        // produced an Install button the runner then refused.
        for command in [
            "sudo apt install ffmpeg",
            "./install.sh --with-whisper",
            "https://github.com/ggml-org/whisper.cpp",
            "rm -rf /",
            "",
        ] {
            assert!(!uv.can_run(command), "{command}");
        }
    }

    #[test]
    fn every_command_magpie_would_run_is_one_the_runner_accepts() {
        // The Install button and `run_installer` share `can_run`, so this asserts
        // the pairing holds for every tool and every combination of installers —
        // the invariant the earlier prefix check broke.
        for installers in [
            Installers::default(),
            Installers {
                uv: true,
                pipx: false,
            },
            Installers {
                uv: false,
                pipx: true,
            },
            Installers {
                uv: true,
                pipx: true,
            },
        ] {
            for tool in [Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe, Tool::Whisper] {
                let offered = [
                    Some(tool.install_command(installers)),
                    tool.upgrade_command(installers),
                ];
                for command in offered.into_iter().flatten() {
                    if installers.can_run(&command) {
                        let program = command.split_whitespace().next().unwrap();
                        assert!(
                            matches!(program, "uv" | "pipx"),
                            "{tool:?} offered {command:?}, which the runner refuses"
                        );
                    }
                }
            }
        }
    }
}
