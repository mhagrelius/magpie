//! Reading an argument list.
//!
//! The surface is positional words and `key=value` pairs, never `--flags`. That
//! is not a style preference: `GApplication` parses the command line with
//! `GOption` before the application ever sees it and refuses any option it was
//! not told about in advance, while an unrecognised *word* is passed straight
//! through to the handler. A `--flag` invented for one verb would therefore be
//! rejected by the launcher rather than reaching the code that understands it.
//!
//! Parsing is separate from doing because the two fail for unrelated reasons.
//! "I do not know that verb" and "`format=mp3` is not a transcript format" are
//! answerable with no network, no tools and no video, and answering them here
//! means the error can list what the right answers are.

use super::help;
use super::{AgentError, ErrorKind};
use crate::model::diarize;
use crate::model::transcript::{Format, Model, LANGUAGES};

/// How many downloads `list` returns when the caller does not say.
///
/// Lower than a task list's fifty: each entry carries two file paths and a
/// status sentence, and a caller looking for one transcript rarely needs to see
/// a year of history to find it. Nothing is dropped silently — a truncated
/// response says how many matched.
pub const DEFAULT_LIMIT: usize = 20;

/// One thing the assistant asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The whole surface, or one verb of it.
    Help {
        verb: Option<String>,
    },
    /// Every verb as JSON, for a caller generating tool definitions.
    Describe,
    Tools,
    Transcribe(Ask),
    List {
        query: Option<String>,
        limit: usize,
    },
    Show {
        job: String,
    },
}

/// A transcript, as asked for.
///
/// Every field but the link is optional, and `None` means "whatever
/// Preferences says" rather than a default invented here. Two sets of defaults
/// that can disagree is how the window and the command line become two
/// products.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub url: String,
    pub format: Option<Format>,
    /// The outer `Option` is whether the caller said anything; the inner one is
    /// whether they asked for automatic detection.
    pub language: Option<Option<String>>,
    pub model: Option<Model>,
    /// `Some(None)` is an explicit "no, do not identify speakers", which has to
    /// be distinguishable from silence when the preference says to.
    pub speakers: Option<Option<diarize::Wish>>,
    /// As typed. Resolved against the directory the command was run in, which
    /// only the caller of `plan` knows.
    pub directory: Option<String>,
}

impl Command {
    /// Whether running this writes anything.
    pub fn mutates(&self) -> bool {
        matches!(self, Self::Transcribe(_))
    }

    /// Whether answering this means running another program.
    ///
    /// The line the application splits on: everything else is answered from the
    /// list already in memory, in the time it takes to serialise it.
    pub fn runs_something(&self) -> bool {
        matches!(self, Self::Tools | Self::Transcribe(_))
    }
}

/// Read the arguments that followed `agent`.
///
/// No arguments at all is the help, not an error: a caller that runs the verb
/// with nothing after it is asking what it can do.
pub fn parse(args: &[String]) -> Result<Command, AgentError> {
    let Some((verb, rest)) = args.split_first() else {
        return Ok(Command::Help { verb: None });
    };

    let verb = verb.to_ascii_lowercase();
    // `help` after a verb reads the same way round as before it. An assistant
    // that has just been told a verb exists will try both.
    if rest.first().is_some_and(|word| word == "help") && verb != "help" {
        return Ok(Command::Help { verb: Some(verb) });
    }

    match help::canonical_verb(&verb) {
        Some("help") => Ok(Command::Help {
            verb: rest.first().map(|verb| verb.to_ascii_lowercase()),
        }),
        Some("describe") => Ok(Command::Describe),
        Some("tools") => Ok(Command::Tools),
        Some("transcribe") => transcribe(rest),
        Some("list") => {
            let (words, pairs) = split_pairs(rest);
            let limit = take_limit(&pairs, "list")?;
            let query = join(&words);
            Ok(Command::List {
                query: (!query.is_empty()).then_some(query),
                limit,
            })
        }
        Some("show") => {
            let job = join(rest);
            if job.is_empty() {
                return Err(missing("show", "a download to show"));
            }
            Ok(Command::Show { job })
        }
        _ => Err(AgentError {
            kind: ErrorKind::UnknownVerb,
            message: format!(
                "`{verb}` is not a verb. The verbs are: {}.",
                help::verb_names().join(", ")
            ),
            candidates: Vec::new(),
            hint: Some("Run `magpie agent help` for what each one does.".into()),
        }),
    }
}

fn transcribe(rest: &[String]) -> Result<Command, AgentError> {
    let (words, pairs) = split_pairs(rest);

    let Some((url, extra)) = words.split_first() else {
        return Err(missing("transcribe", "a link to transcribe"));
    };
    // Joining the loose words the way `show` does would silently swallow a
    // second link, or a stray word, into the URL — and yt-dlp would then fail
    // on an address nobody typed.
    if let Some(word) = extra.first() {
        return Err(AgentError::hinted(
            ErrorKind::BadValue,
            format!("A link is one argument, and `{word}` came after it."),
            "Everything after the link is `key=value`: format=srt, speakers=2, dir=.",
        ));
    }

    let mut ask = Ask {
        url: url.clone(),
        ..Ask::default()
    };

    for (key, value) in &pairs {
        let value = value.trim();
        match key.as_str() {
            "format" => ask.format = Some(format(value)?),
            "language" | "lang" => ask.language = Some(language(value)?),
            "model" => ask.model = Some(model(value)?),
            "speakers" | "diarize" => ask.speakers = Some(speakers(value)?),
            "dir" | "directory" | "to" => {
                if value.is_empty() {
                    return Err(AgentError::new(
                        ErrorKind::BadValue,
                        "`dir=` was given with nothing after it.",
                    ));
                }
                ask.directory = Some(value.to_string());
            }
            _ => {
                return Err(unknown_field(
                    key,
                    "transcribe",
                    &["format", "language", "model", "speakers", "dir"],
                ))
            }
        }
    }

    Ok(Command::Transcribe(ask))
}

fn format(value: &str) -> Result<Format, AgentError> {
    match value.to_ascii_lowercase().as_str() {
        "text" | "txt" | "plain" => Ok(Format::Text),
        "srt" | "subrip" => Ok(Format::Srt),
        "vtt" | "webvtt" => Ok(Format::Vtt),
        _ => Err(AgentError::hinted(
            ErrorKind::BadValue,
            format!("`format={value}` is not a transcript format."),
            "text for prose, srt or vtt for subtitles with timings.",
        )),
    }
}

fn model(value: &str) -> Result<Model, AgentError> {
    Model::ALL
        .into_iter()
        .find(|model| model.name() == value.to_ascii_lowercase())
        .ok_or_else(|| {
            AgentError::hinted(
                ErrorKind::BadValue,
                format!("`model={value}` is not a whisper model."),
                "tiny, base, small or medium — bigger is slower and more accurate.",
            )
        })
}

/// A language, as a code or as one of the names the window offers.
///
/// Names are accepted because "spanish" is what a user says and `es` is what
/// whisper wants; there is no reason for the caller to have to know the table.
/// Codes are not checked against that table, though — whisper knows ninety-nine
/// languages and the window lists sixteen, so refusing `cy` here would be this
/// file inventing a limit that does not exist.
fn language(value: &str) -> Result<Option<String>, AgentError> {
    let wanted = value.trim().to_ascii_lowercase();
    if wanted.is_empty() || wanted == "auto" || wanted == "detect" || wanted == "none" {
        return Ok(None);
    }
    if let Some((code, _)) = LANGUAGES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(&wanted))
    {
        return Ok(Some((*code).to_string()));
    }
    let is_code =
        (2..=3).contains(&wanted.len()) && wanted.chars().all(|c| c.is_ascii_alphabetic());
    if is_code {
        return Ok(Some(wanted));
    }
    Err(AgentError::hinted(
        ErrorKind::BadValue,
        format!("`language={value}` is not a language."),
        "An ISO code such as en, es or de, or leave it out and whisper detects it.",
    ))
}

fn speakers(value: &str) -> Result<Option<diarize::Wish>, AgentError> {
    let wanted = value.trim().to_ascii_lowercase();
    match wanted.as_str() {
        "no" | "off" | "false" | "none" | "0" => Ok(None),
        "" | "yes" | "on" | "true" | "auto" | "detect" => Ok(Some(diarize::Wish::default())),
        _ => match wanted.parse::<u8>() {
            Ok(count) if (1..=diarize::Count::MAX).contains(&count) => Ok(Some(diarize::Wish {
                count: diarize::Count::Fixed(count),
            })),
            _ => Err(AgentError::hinted(
                ErrorKind::BadValue,
                format!("`speakers={value}` is not a number of speakers."),
                format!(
                    "`yes` to work it out, or how many people there are, up to {}.",
                    diarize::Count::MAX
                ),
            )),
        },
    }
}

fn join(words: &[String]) -> String {
    words.join(" ").trim().to_string()
}

/// Split arguments into the leading words and the `key=value` pairs.
///
/// A token shaped like `key=` opens a pair, and the words after it belong to
/// its value until the next such token. That is what makes `dir=/home/matty/My
/// Videos` mean one thing whether or not the caller remembered to quote it.
///
/// The key must be lower-case ASCII, so an `=` inside a URL — which is where
/// most of them are — does not turn the rest of the line into a field.
fn split_pairs(args: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let mut leading = Vec::new();
    let mut pairs: Vec<(String, String)> = Vec::new();

    for argument in args {
        match key_of(argument) {
            Some((key, first)) => pairs.push((key, first.to_string())),
            None => match pairs.last_mut() {
                Some((_, value)) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(argument);
                }
                None => leading.push(argument.clone()),
            },
        }
    }
    (leading, pairs)
}

/// The key a `key=value` token opens, and the part of the value it carried.
///
/// A URL is the argument this has to get right: `https://youtu.be/x?t=90` has
/// an `=` in it, and reading `https://youtu.be/x?t` as a key would leave
/// yt-dlp with half a link. Requiring the key to be a bare lower-case word
/// settles it, since no key ever contains `:` or `/`.
fn key_of(argument: &str) -> Option<(String, &str)> {
    let (key, value) = argument.split_once('=')?;
    let shaped = !key.is_empty()
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    shaped.then(|| (key.to_string(), value))
}

fn take_limit(pairs: &[(String, String)], verb: &str) -> Result<usize, AgentError> {
    let mut limit = DEFAULT_LIMIT;
    for (key, value) in pairs {
        match key.as_str() {
            "limit" => {
                limit = value.trim().parse().map_err(|_| {
                    AgentError::new(
                        ErrorKind::BadValue,
                        format!("`limit={value}` is not a whole number."),
                    )
                })?
            }
            _ => return Err(unknown_field(key, verb, &["limit"])),
        }
    }
    Ok(limit)
}

fn missing(verb: &str, wanted: &str) -> AgentError {
    AgentError::hinted(
        ErrorKind::MissingArgument,
        format!("`{verb}` needs {wanted}."),
        format!("Run `magpie agent help {verb}` for the arguments."),
    )
}

fn unknown_field(key: &str, verb: &str, allowed: &[&str]) -> AgentError {
    AgentError::hinted(
        ErrorKind::UnknownField,
        format!(
            "`{verb}` has no `{key}` field. It takes: {}.",
            allowed.join(", ")
        ),
        format!("Run `magpie agent help {verb}`."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn parsed(line: &str) -> Command {
        parse(&args(line)).expect("a command")
    }

    fn ask(line: &str) -> Ask {
        match parsed(line) {
            Command::Transcribe(ask) => ask,
            other => panic!("expected a transcribe, got {other:?}"),
        }
    }

    const URL: &str = "https://youtu.be/dQw4w9WgXcQ";

    #[test]
    fn no_arguments_at_all_is_a_request_for_help() {
        assert_eq!(parse(&[]).unwrap(), Command::Help { verb: None });
    }

    #[test]
    fn help_reads_the_same_before_or_after_a_verb() {
        let expected = Command::Help {
            verb: Some("transcribe".into()),
        };
        assert_eq!(parsed("help transcribe"), expected);
        assert_eq!(parsed("transcribe help"), expected);
    }

    #[test]
    fn an_unknown_verb_is_told_what_the_verbs_are() {
        let error = parse(&args("frobnicate")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnknownVerb);
        assert!(error.message.contains("transcribe"), "{}", error.message);
    }

    #[test]
    fn a_link_alone_asks_for_every_default() {
        let ask = ask(&format!("transcribe {URL}"));
        assert_eq!(ask.url, URL);
        assert_eq!(ask.format, None, "silence means the user's preference");
        assert_eq!(ask.model, None);
        assert_eq!(ask.speakers, None);
        assert_eq!(ask.language, None);
    }

    #[test]
    fn a_query_string_is_not_mistaken_for_a_field() {
        // The argument this has to get right: a timestamped YouTube link has an
        // `=` in it, and splitting on the first one leaves half a URL.
        let ask = ask("transcribe https://www.youtube.com/watch?v=abc&t=90");
        assert_eq!(ask.url, "https://www.youtube.com/watch?v=abc&t=90");
    }

    #[test]
    fn a_second_loose_word_is_refused_rather_than_glued_to_the_link() {
        let error = parse(&args(&format!("transcribe {URL} please"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadValue);
        assert!(error.message.contains("please"), "{}", error.message);
    }

    #[test]
    fn a_link_is_required() {
        let error = parse(&args("transcribe format=srt")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::MissingArgument);
    }

    #[test]
    fn each_option_is_read_into_the_thing_it_names() {
        let ask = ask(&format!(
            "transcribe {URL} format=srt model=medium language=es speakers=3 dir=/tmp/out"
        ));
        assert_eq!(ask.format, Some(Format::Srt));
        assert_eq!(ask.model, Some(Model::Medium));
        assert_eq!(ask.language, Some(Some("es".into())));
        assert_eq!(
            ask.speakers,
            Some(Some(diarize::Wish {
                count: diarize::Count::Fixed(3)
            }))
        );
        assert_eq!(ask.directory.as_deref(), Some("/tmp/out"));
    }

    #[test]
    fn an_unquoted_directory_keeps_its_spaces() {
        let ask = ask(&format!("transcribe {URL} dir=/home/matty/My Videos"));
        assert_eq!(ask.directory.as_deref(), Some("/home/matty/My Videos"));
    }

    #[test]
    fn a_language_may_be_named_rather_than_coded() {
        // `es` is what whisper wants and "Spanish" is what a person says. There
        // is no reason the caller should have to know the table.
        assert_eq!(
            ask(&format!("transcribe {URL} language=Spanish")).language,
            Some(Some("es".into()))
        );
        // A code the window does not list is still passed through: whisper
        // knows ninety-nine languages and the combo row shows sixteen.
        assert_eq!(
            ask(&format!("transcribe {URL} language=cy")).language,
            Some(Some("cy".into()))
        );
        // And asking for detection explicitly is not the same as saying nothing.
        assert_eq!(
            ask(&format!("transcribe {URL} language=auto")).language,
            Some(None)
        );
    }

    #[test]
    fn speakers_takes_a_count_or_an_answer_to_yes_or_no() {
        assert_eq!(
            ask(&format!("transcribe {URL} speakers=yes")).speakers,
            Some(Some(diarize::Wish::default()))
        );
        assert_eq!(
            ask(&format!("transcribe {URL} speakers=no")).speakers,
            Some(None),
            "an explicit no must survive a preference that says yes"
        );

        let error = parse(&args(&format!("transcribe {URL} speakers=lots"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadValue);
        // Past the point where naming a number beats detecting one.
        let error = parse(&args(&format!("transcribe {URL} speakers=40"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadValue);
    }

    #[test]
    fn a_value_that_is_not_one_of_the_choices_lists_them() {
        let error = parse(&args(&format!("transcribe {URL} format=mp3"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadValue);
        assert!(error.hint.unwrap_or_default().contains("srt"));

        let error = parse(&args(&format!("transcribe {URL} model=huge"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadValue);
        assert!(error.hint.unwrap_or_default().contains("medium"));
    }

    #[test]
    fn an_unknown_field_lists_the_ones_that_exist() {
        let error = parse(&args(&format!("transcribe {URL} quality=1080"))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnknownField);
        assert!(error.message.contains("speakers"), "{}", error.message);
    }

    #[test]
    fn a_list_keeps_its_words_and_takes_its_limit_out_of_them() {
        assert_eq!(
            parsed("list a talk about bees limit=5"),
            Command::List {
                query: Some("a talk about bees".into()),
                limit: 5,
            }
        );
        assert_eq!(
            parsed("list"),
            Command::List {
                query: None,
                limit: DEFAULT_LIMIT,
            }
        );
    }

    #[test]
    fn a_download_may_be_named_in_several_loose_words() {
        assert_eq!(
            parsed("show a talk about bees"),
            Command::Show {
                job: "a talk about bees".into(),
            }
        );
    }

    #[test]
    fn every_verb_the_help_advertises_can_be_parsed() {
        // The help table and the parser are two lists of verbs. This is what
        // stops one growing an entry the other has never heard of.
        for verb in help::verb_names() {
            let command = parse(&[verb.to_string(), "help".to_string()]);
            assert!(command.is_ok(), "`{verb}` is documented but not parsed");
        }
    }

    #[test]
    fn only_the_verb_that_downloads_says_it_writes_or_that_it_is_slow() {
        assert!(parsed(&format!("transcribe {URL}")).mutates());
        assert!(parsed(&format!("transcribe {URL}")).runs_something());
        assert!(
            parsed("tools").runs_something(),
            "it asks each tool its version"
        );
        for line in ["list", "show 3", "describe", "help"] {
            assert!(!parsed(line).mutates(), "{line}");
        }
        for line in ["list", "show 3", "describe", "help"] {
            assert!(!parsed(line).runs_something(), "{line}");
        }
    }
}
