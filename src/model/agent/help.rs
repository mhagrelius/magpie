//! What the surface says about itself.
//!
//! One table of verbs feeds three things: the prose help, the machine-readable
//! `describe` output, and the list of verbs the parser will accept. They cannot
//! drift apart, because there is nothing to drift — a verb that is not in this
//! table cannot be parsed, and one that is has documentation by construction.
//!
//! The help is written for a model rather than for a person at a terminal. That
//! mostly means the same things good help always meant, with two additions.
//! Every verb states what it returns, so a caller can tell whether it needs a
//! second call. And `transcribe` states what it *costs* — minutes of CPU, and a
//! model download the first time — because the one thing a caller cannot
//! discover by trying it is how long it will be waiting.

use serde::Serialize;

/// One argument of one verb.
#[derive(Debug, Clone, Serialize)]
pub struct Argument {
    pub name: &'static str,
    pub required: bool,
    pub description: &'static str,
}

/// One verb, as both prose and schema.
#[derive(Debug, Clone, Serialize)]
pub struct Verb {
    pub name: &'static str,
    #[serde(skip_serializing_if = "<[&str]>::is_empty")]
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub summary: &'static str,
    /// Whether running it changes anything on disk. A caller that gates writes
    /// behind an approval prompt reads this rather than keeping its own list.
    pub mutates: bool,
    /// Whether it can take minutes. The two flags are different questions: this
    /// one decides whether a caller needs a long timeout or a background run.
    pub slow: bool,
    pub arguments: &'static [Argument],
    /// What comes back, so a caller knows whether it needs a second call.
    pub returns: &'static str,
    #[serde(skip_serializing_if = "<[&str]>::is_empty")]
    pub examples: &'static [&'static str],
}

const JOB_REFERENCE: Argument = Argument {
    name: "download",
    required: true,
    description: "A download, named by its id or by any part of its title or \
                  link. Text matching more than one comes back as an \
                  `ambiguous` error listing the candidates with their ids.",
};

/// Every verb, in the order the help lists them: cheap before slow.
pub const VERBS: &[Verb] = &[
    Verb {
        // No `--help` alias: GOption intercepts that before this code runs, and
        // documenting a spelling that never reaches us would be a lie. The
        // launcher's own `--help` points back here instead.
        name: "help",
        aliases: &[],
        usage: "magpie agent help [verb]",
        summary: "This text, or everything about one verb.",
        mutates: false,
        slow: false,
        arguments: &[Argument {
            name: "verb",
            required: false,
            description: "The verb to explain. Omit it for the whole surface.",
        }],
        returns: "Plain text, not JSON.",
        examples: &["magpie agent help transcribe"],
    },
    Verb {
        name: "describe",
        aliases: &[],
        usage: "magpie agent describe",
        summary: "Every verb as JSON, for generating tool definitions.",
        mutates: false,
        slow: false,
        arguments: &[],
        returns: "`{verbs: [{name, usage, summary, mutates, slow, arguments, returns}]}`.",
        examples: &["magpie agent describe"],
    },
    Verb {
        name: "tools",
        aliases: &["check"],
        usage: "magpie agent tools",
        summary: "What is installed, and whether a transcript can be made here.",
        mutates: false,
        slow: false,
        arguments: &[],
        returns: "`{tools, speech_models, speaker_models, ready}`. `ready.transcribe` \
                  is the one field worth branching on, and `ready.missing` says in \
                  sentences what is in the way. Call this first when a transcript \
                  matters: `transcribe` refuses in the same words, but only after \
                  the user has been told to wait.",
        examples: &["magpie agent tools"],
    },
    Verb {
        name: "transcribe",
        aliases: &[],
        usage: "magpie agent transcribe <url> [format=] [language=] [model=] \
                [speakers=] [dir=]",
        summary: "Download a video's audio and transcribe it. Waits for the \
                  words, then prints where they are.",
        mutates: true,
        slow: true,
        arguments: &[
            Argument {
                name: "url",
                required: true,
                description: "The link to one video, as a single argument. A \
                              playlist or channel link is refused rather than \
                              expanded — that is hours of CPU per link.",
            },
            Argument {
                name: "format",
                required: false,
                description: "`text`, `srt` or `vtt`. Defaults to whatever \
                              Preferences → Transcripts is set to, which is \
                              plain text unless the user changed it.",
            },
            Argument {
                name: "language",
                required: false,
                description: "An ISO code such as `en` or `es`. Omit it and \
                              whisper detects the language, which is usually \
                              right and is the default.",
            },
            Argument {
                name: "model",
                required: false,
                description: "`tiny`, `base`, `small` or `medium`. Bigger is \
                              slower and more accurate. A model that has not \
                              been downloaded yet is fetched first, and the \
                              default `small` is 466 MB.",
            },
            Argument {
                name: "speakers",
                required: false,
                description: "`yes` to work out who is speaking, or a number \
                              when you know how many people there are. Needs \
                              sherpa-onnx and two more models (34 MB). Failure \
                              here never costs the transcript.",
            },
            Argument {
                name: "dir",
                required: false,
                description: "Where to put the audio and the transcript beside \
                              it. Relative to the directory you ran this in. \
                              Defaults to the user's download folder, the same \
                              as the window.",
            },
        ],
        returns: "`{job}` with `transcript.path` — the file, already written. \
                  The words are not in the response: read the file. A download \
                  that worked without a transcript is an error, not a success, \
                  and names the audio file it left behind.",
        examples: &[
            "magpie agent transcribe https://youtu.be/dQw4w9WgXcQ",
            "magpie agent transcribe https://youtu.be/dQw4w9WgXcQ format=srt speakers=2",
            "magpie agent transcribe https://youtu.be/dQw4w9WgXcQ model=medium language=es dir=.",
        ],
    },
    Verb {
        name: "list",
        aliases: &["downloads"],
        usage: "magpie agent list [text] [limit=N]",
        summary: "Downloads Magpie has a record of, newest first.",
        mutates: false,
        slow: false,
        arguments: &[
            Argument {
                name: "text",
                required: false,
                description: "Match against the title and the link. Omitted, it \
                              lists everything.",
            },
            Argument {
                name: "limit",
                required: false,
                description: "How many to return. Defaults to 20. The response \
                              says how many matched, so a truncated list is \
                              visible rather than silent.",
            },
        ],
        returns: "`{jobs, count, matched, truncated}`. This is how to find a \
                  transcript made earlier — including in a previous session — \
                  rather than making it again.",
        examples: &["magpie agent list", "magpie agent list lecture limit=5"],
    },
    Verb {
        name: "show",
        aliases: &["job"],
        usage: "magpie agent show <download>",
        summary: "One download in full: the file, the transcript, the speakers.",
        mutates: false,
        slow: false,
        arguments: &[JOB_REFERENCE],
        returns: "`{job}`, the same shape `transcribe` returns.",
        examples: &["magpie agent show 7", "magpie agent show 'a talk about'"],
    },
];

/// Every verb's canonical name.
pub fn verb_names() -> Vec<&'static str> {
    VERBS.iter().map(|verb| verb.name).collect()
}

/// One verb by name or alias.
pub fn verb(word: &str) -> Option<&'static Verb> {
    VERBS
        .iter()
        .find(|verb| verb.name == word || verb.aliases.contains(&word))
}

/// The canonical name for a word, if it names a verb at all.
pub fn canonical_verb(word: &str) -> Option<&'static str> {
    verb(word).map(|verb| verb.name)
}

/// What a transcript actually costs, written once.
///
/// Here rather than only in `transcribe`'s own page because it is the thing a
/// caller most needs to know before the first call, and the overview is the
/// page anyone reads first.
const COST_HELP: &str = "\
WHAT IT COSTS
  `transcribe` runs to completion before it answers. It downloads the audio,
  converts it to 16 kHz mono, and runs whisper over it on the CPU. Reckon on
  a few minutes for a short video and considerably longer for an hour of
  conference audio, so give it a long timeout or run it in the background.

  The first run also downloads the speech model — 466 MB for the default
  `small`, 1.5 GB for `medium` — which is a one-off. `magpie agent tools`
  says which models are already here.

  Progress goes to stderr as it happens, one line per stage. stdout carries
  the single JSON object and nothing else, so a caller reading only stdout
  sees one answer whatever the stage lines said.

  Killing the command does not stop the work when Magpie is already running:
  the job belongs to the window, where it can be cancelled. When Magpie is
  not running the work dies with the command, leaving a part-finished
  download that the next attempt resumes from.";

const FILE_HELP: &str = "\
WHERE THINGS GO
  The audio lands in the user's download folder — the same one the window
  uses — and the transcript is written beside it with the matching extension.
  `dir=` puts both somewhere else.

  Nothing is deleted afterwards. The response names both files; removing the
  audio once the words are read is the caller's business, not Magpie's.

  Every download is recorded, so `magpie agent list` finds a transcript made
  weeks ago rather than making it again.";

/// The whole surface.
pub fn overview() -> String {
    let mut text = String::from(
        "\
magpie agent — transcribe a video from a script or an assistant.

USAGE
  magpie agent <verb> [arguments]

  Every verb prints one JSON object on stdout and exits 0, or prints
  {\"ok\": false, \"error\": ...} and exits 1. `help` prints text instead.

  Arguments are positional words and `key=value` pairs. There are no `--flags`:
  the launcher parses those before this code runs and would reject them.

  If Magpie is running, the command is handed to it, so the download appears in
  the window and there is no second copy of the list to fall out of step. If it
  is not, the command does the work itself and writes the same files.

VERBS
",
    );

    let width = VERBS.iter().map(|verb| verb.name.len()).max().unwrap_or(0);
    for verb in VERBS {
        let mark = if verb.slow {
            "~"
        } else if verb.mutates {
            "*"
        } else {
            " "
        };
        text.push_str(&format!(
            "  {mark} {:width$}  {}\n",
            verb.name,
            first_sentence(verb.summary),
            width = width
        ));
    }
    // Only the marks actually in the table above are explained. A legend for a
    // symbol nothing carries is a line that asks the reader to look for
    // something that is not there.
    text.push('\n');
    if VERBS.iter().any(|verb| verb.mutates && !verb.slow) {
        text.push_str("  * writes something.\n");
    }
    if VERBS.iter().any(|verb| verb.slow) {
        text.push_str("  ~ writes something, and takes minutes doing it.\n");
    }
    text.push_str(
        "\n  Run `magpie agent help <verb>` for arguments and examples, or\n  \
         `magpie agent describe` for the same thing as JSON.\n\n",
    );

    text.push_str(COST_HELP);
    text.push_str("\n\n");
    text.push_str(FILE_HELP);
    text.push_str(
        "\n\nNAMING A DOWNLOAD\n  \
         `show` takes an id, or any part of a title or link. Text matching\n  \
         several downloads is an `ambiguous` error listing them with their\n  \
         ids rather than a guess. Ids come back on everything, so pass the id\n  \
         you were already given.\n",
    );
    text
}

/// One verb, at length.
pub fn for_verb(name: &str) -> Option<String> {
    let found = verb(name)?;

    let mut text = format!(
        "{}\n\n{}\n\nUSAGE\n  {}\n",
        found.name,
        wrap(found.summary, "  "),
        found.usage
    );

    if !found.aliases.is_empty() {
        text.push_str(&format!("\nALSO CALLED\n  {}\n", found.aliases.join(", ")));
    }

    if !found.arguments.is_empty() {
        text.push_str("\nARGUMENTS\n");
        for argument in found.arguments {
            let required = if argument.required {
                "required"
            } else {
                "optional"
            };
            text.push_str(&format!(
                "  {} ({})\n{}\n",
                argument.name,
                required,
                wrap(argument.description, "      ")
            ));
        }
    }

    text.push_str(&format!("\nRETURNS\n{}\n", wrap(found.returns, "  ")));

    if !found.examples.is_empty() {
        text.push_str("\nEXAMPLES\n");
        for example in found.examples {
            text.push_str(&format!("  {example}\n"));
        }
    }

    if found.slow {
        text.push('\n');
        text.push_str(COST_HELP);
        text.push('\n');
        text.push_str(FILE_HELP);
        text.push('\n');
    }
    Some(text)
}

/// Take the first sentence of a summary, for the one-line verb table.
fn first_sentence(summary: &str) -> String {
    let collapsed = collapse(summary);
    match collapsed.find(". ") {
        Some(end) => collapsed[..=end].to_string(),
        None => collapsed,
    }
}

/// Squash the line breaks and indentation a `&'static str` in source carries.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rewrap prose to fit a terminal, under a fixed indent.
fn wrap(text: &str, indent: &str) -> String {
    const WIDTH: usize = 78;

    let mut lines = Vec::new();
    let mut line = String::from(indent);
    for word in collapse(text).split(' ') {
        if line.len() > indent.len() && line.len() + 1 + word.len() > WIDTH {
            lines.push(std::mem::replace(&mut line, String::from(indent)));
        } else if line.len() > indent.len() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_is_documented_well_enough_to_be_used() {
        for verb in VERBS {
            assert!(!verb.summary.is_empty(), "{} has no summary", verb.name);
            assert!(
                !verb.returns.is_empty(),
                "{} says nothing about what it returns",
                verb.name
            );
            assert!(
                verb.usage.starts_with("magpie agent "),
                "{} shows a usage line that cannot be run: {}",
                verb.name,
                verb.usage
            );
            // A verb with required arguments and no example is the one people
            // get wrong, so the example is not optional.
            if verb.arguments.iter().any(|argument| argument.required) {
                assert!(!verb.examples.is_empty(), "{} has no example", verb.name);
            }
        }
    }

    #[test]
    fn no_two_verbs_answer_to_the_same_word() {
        let mut seen = std::collections::HashSet::new();
        for verb in VERBS {
            for word in std::iter::once(&verb.name).chain(verb.aliases) {
                assert!(seen.insert(*word), "`{word}` names more than one verb");
            }
        }
    }

    #[test]
    fn a_verb_that_takes_minutes_says_so_in_the_schema() {
        // A caller generating a tool definition reads `slow` to decide on a
        // timeout. Getting it wrong means killing a transcript at nine minutes.
        assert!(verb("transcribe").expect("documented").slow);
        for name in ["help", "describe", "tools", "list", "show"] {
            assert!(!verb(name).expect("documented").slow, "{name}");
        }
    }

    #[test]
    fn an_alias_finds_the_verb_it_stands_for() {
        assert_eq!(canonical_verb("check"), Some("tools"));
        assert_eq!(canonical_verb("transcribe"), Some("transcribe"));
        assert_eq!(canonical_verb("nonsense"), None);
    }

    #[test]
    fn the_overview_lists_every_verb_and_what_a_transcript_costs() {
        let text = overview();
        for verb in VERBS {
            assert!(
                text.contains(verb.name),
                "{} is missing from the help",
                verb.name
            );
        }
        assert!(text.contains("WHAT IT COSTS"));
        assert!(text.contains("WHERE THINGS GO"));
    }

    #[test]
    fn the_slow_verb_carries_the_cost_and_the_quick_ones_are_not_padded_with_it() {
        let transcribe = for_verb("transcribe").expect("transcribe is documented");
        assert!(transcribe.contains("WHAT IT COSTS"));
        let list = for_verb("list").expect("list is documented");
        assert!(!list.contains("WHAT IT COSTS"));
    }

    #[test]
    fn help_for_something_that_is_not_a_verb_is_absent_rather_than_empty() {
        assert!(for_verb("frobnicate").is_none());
    }

    #[test]
    fn the_verb_table_fits_in_a_terminal() {
        // The table is the first thing anyone reads. A summary long enough to
        // wrap turns the column of verbs into a wall.
        for line in overview()
            .lines()
            .take_while(|line| !line.contains("WHAT IT COSTS"))
        {
            assert!(line.len() <= 88, "{} wide: {line:?}", line.len());
        }
    }

    #[test]
    fn wrapped_prose_stays_inside_a_terminal() {
        let long = "a ".repeat(200);
        for line in wrap(&long, "    ").lines() {
            assert!(line.len() <= 78, "{line:?} is {} wide", line.len());
        }
    }
}
