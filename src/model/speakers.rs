//! Putting names to the voices, and the voices to the words.
//!
//! Diarization answers "voice 3 spoke from 12.164 to 14.645". A transcript
//! answers "these words were said from 12.1 to 14.6". Neither is useful alone;
//! this file is the join, and it is pure string and arithmetic work, so all of it
//! is testable without models, audio, or a display.
//!
//! Three things happen here, in order:
//!
//! 1. **Alignment.** Each subtitle cue is given the voice that was speaking for
//!    most of it. Cheap, and more robust than matching boundaries, because the
//!    two tools segment on different criteria and their edges never line up.
//! 2. **Renumbering.** Clustering hands back sparse labels — a real two-speaker
//!    file came back as `speaker_00` and `speaker_03`, a four-speaker one as 0,
//!    1, 3 and 8. Presenting those as written would produce a two-hander between
//!    Speaker 1 and Speaker 4.
//! 3. **Naming.** People say each other's names constantly, and a transcript
//!    that says "Alice:" is worth more than one that says "Speaker 1:". This is
//!    the part that is a guess, and it is treated as one: a name has to be
//!    *earned* by evidence, and the fallback is always the honest number.

use std::collections::HashMap;

use super::diarize::Turn;
use super::transcript::Format;

/// One subtitle cue: a span of time and the words said in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Seconds out of an `HH:MM:SS,mmm` timestamp.
///
/// Accepts a full stop as well as a comma for the fraction. SRT specifies the
/// comma and WebVTT the stop, and reading both means the timing source can be
/// either without a second parser.
pub fn parse_timestamp(value: &str) -> Option<f64> {
    let value = value.trim().replace(',', ".");
    let mut parts = value.split(':');
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next());
    if parts.next().is_some() {
        return None;
    }

    // `MM:SS.mmm` is legal WebVTT, so the hour is optional and the pieces are
    // read from the right rather than the left.
    let (hours, minutes, seconds) = match c {
        Some(c) => (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?, c),
        None => (0.0, a.parse::<f64>().ok()?, b),
    };
    let seconds: f64 = seconds.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Seconds as `HH:MM:SS,mmm`, or with a stop for WebVTT.
pub fn format_timestamp(seconds: f64, decimal: char) -> String {
    let seconds = seconds.max(0.0);
    let total = seconds as u64;
    let millis = ((seconds - total as f64) * 1000.0).round() as u64;
    // Rounding 3.9996 up to a full second must carry, or the file gets a
    // `00:00:03,1000` that no player will read.
    let (total, millis) = if millis >= 1000 {
        (total + 1, 0)
    } else {
        (total, millis)
    };
    format!(
        "{:02}:{:02}:{:02}{decimal}{:03}",
        total / 3600,
        (total / 60) % 60,
        total % 60,
        millis
    )
}

/// Read the cues out of an SRT or WebVTT file.
///
/// Deliberately forgiving: it looks for lines containing ` --> `, takes the
/// timings from those, and treats everything up to the next blank line as the
/// text. Sequence numbers, `WEBVTT` headers, `NOTE` blocks and cue settings after
/// the end timestamp are all skipped by being ignored rather than by being
/// parsed, because the only thing wanted here is when-and-what.
pub fn parse_cues(source: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut lines = source.lines().peekable();

    while let Some(line) = lines.next() {
        let Some((start, rest)) = line.split_once(" --> ") else {
            continue;
        };
        // Cue settings (`align:start position:50%`) ride on the end timestamp.
        let end = rest.split_whitespace().next().unwrap_or(rest);
        let (Some(start), Some(end)) = (parse_timestamp(start), parse_timestamp(end)) else {
            continue;
        };

        let mut text = String::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(next.trim());
            lines.next();
        }

        let text = text.trim().to_string();
        if !text.is_empty() && end >= start {
            cues.push(Cue { start, end, text });
        }
    }

    cues
}

/// A cue and the voice it was spoken in.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub cue: Cue,
    /// `None` when no turn overlapped it at all — which happens, because the
    /// segmentation model drops stretches shorter than 0.3s and whisper does not.
    /// Inventing a speaker for those would be inventing evidence.
    pub speaker: Option<usize>,
}

/// Give every cue the voice that was speaking for most of it.
///
/// Most, not first: a cue that starts in the tail of one turn and spends its
/// remaining four seconds in the next belongs to the next. Overlap is the only
/// measure that gets that right without special cases.
///
/// Totalled per voice rather than per turn, which matters in exactly the case
/// this is for. An interviewer interjecting three times across one long answer
/// produces three short turns against one long one; picking the single longest
/// turn would credit the whole cue to the interviewer if their three seconds
/// happened to be one four-second turn, where adding up each voice's seconds
/// gives it to the person who actually said most of it.
pub fn align(cues: Vec<Cue>, turns: &[Turn]) -> Vec<Line> {
    cues.into_iter()
        .map(|cue| {
            let mut totals: Vec<(usize, f64)> = Vec::new();
            for turn in turns {
                let overlap = turn.overlap(cue.start, cue.end);
                if overlap <= 0.0 {
                    continue;
                }
                match totals
                    .iter_mut()
                    .find(|(speaker, _)| *speaker == turn.speaker)
                {
                    Some((_, total)) => *total += overlap,
                    None => totals.push((turn.speaker, overlap)),
                }
            }
            // Ties go to the lower cluster id, so the same input always produces
            // the same transcript.
            let speaker = totals
                .into_iter()
                .max_by(|(a_speaker, a), (b_speaker, b)| {
                    a.total_cmp(b).then(b_speaker.cmp(a_speaker))
                })
                .map(|(speaker, _)| speaker);
            Line { cue, speaker }
        })
        .collect()
}

/// Who is in this recording, in the order they first speak.
///
/// The order matters: "Speaker 1" should be the person who talks first, which is
/// both what a reader expects and unrelated to the arbitrary integer clustering
/// happened to assign.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cast {
    /// Cluster ids in first-heard order.
    order: Vec<usize>,
    /// The name inferred for a cluster, where one was.
    names: HashMap<usize, String>,
}

impl Cast {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// What to write in front of the words.
    pub fn label(&self, cluster: usize) -> String {
        if let Some(name) = self.names.get(&cluster) {
            return name.clone();
        }
        match self.order.iter().position(|c| *c == cluster) {
            Some(index) => format!("Speaker {}", index + 1),
            None => "Speaker ?".to_string(),
        }
    }

    /// Every label, in first-heard order — for the summary the UI shows.
    pub fn labels(&self) -> Vec<String> {
        self.order.iter().map(|c| self.label(*c)).collect()
    }

    /// How many voices were given a name rather than a number.
    pub fn named(&self) -> usize {
        self.names.len()
    }
}

/// Work out the cast: who speaks, in what order, and what they seem to be called.
pub fn cast(lines: &[Line]) -> Cast {
    let mut order = Vec::new();
    for line in lines {
        if let Some(speaker) = line.speaker {
            if !order.contains(&speaker) {
                order.push(speaker);
            }
        }
    }
    let names = infer_names(lines, &order);
    Cast { order, names }
}

/// Which voice a spoken name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refers {
    /// "I'm Alice" — the person saying it.
    Speaker,
    /// "Thanks, Alice" — the person who just finished. You thank someone for
    /// what they have already said.
    Previous,
    /// "Over to you, Alice" — the person about to start.
    Next,
}

/// The phrases that introduce a name, and who that name belongs to.
///
/// Lower-cased and matched on word boundaries. Ordered longest-first within each
/// prefix so that "thank you" is tried before "thanks" could half-match, and so
/// "my name is" is not shadowed by "name is".
///
/// This list is short on purpose. Every entry is a phrase whose next word is
/// almost always a name; the temptation is to add "welcome to" and "with me is"
/// and a dozen more, and each one that is only *usually* a name buys a wrong
/// label somewhere. A wrong name is worse than "Speaker 2", because a reader
/// cannot tell it is wrong.
const CUES: [(&str, Refers); 12] = [
    ("my name is", Refers::Speaker),
    ("i'm", Refers::Speaker),
    ("i am", Refers::Speaker),
    ("this is", Refers::Speaker),
    ("thank you", Refers::Previous),
    ("thanks", Refers::Previous),
    ("over to you", Refers::Next),
    ("welcome back", Refers::Next),
    ("welcome", Refers::Next),
    ("joining me is", Refers::Next),
    ("joining us is", Refers::Next),
    ("go ahead", Refers::Next),
];

/// Capitalised words that begin sentences and are not people.
///
/// Without this, "So, I'm going to..." names a speaker "So" and every transcript
/// acquires a cast member who does not exist.
const NOT_NAMES: [&str; 46] = [
    "a", "an", "and", "but", "so", "the", "then", "there", "this", "that", "these", "those", "i",
    "we", "you", "he", "she", "it", "they", "my", "your", "our", "not", "no", "yes", "ok", "okay",
    "well", "now", "today", "tonight", "just", "really", "very", "going", "here", "what", "when",
    "where", "why", "how", "who", "if", "all", "one", "sure",
];

/// Pull a name out of the words following a cue phrase.
///
/// A name is one or two consecutive capitalised words. Two, so that "Alice
/// Cooper" survives; not three, because by then it is a title and not a name.
fn name_after(rest: &str) -> Option<String> {
    let mut words = Vec::new();

    // The name is almost always separated from the phrase by punctuation —
    // "thanks, Priya", "over to you, Marcus" — and the comma is its own token
    // once split on whitespace. Dropping it here rather than tolerating empty
    // tokens in the loop keeps "break" meaning "the name has ended".
    let rest = rest.trim_start_matches(|c: char| !c.is_alphanumeric());

    for word in rest.split_whitespace() {
        // Trailing punctuation is part of the sentence, not the name. Leading
        // too, for the opening quote in `thanks, "Alice"`.
        let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-');
        if word.len() < 2 {
            break;
        }
        let first = word.chars().next()?;
        if !first.is_uppercase() {
            break;
        }
        if NOT_NAMES.contains(&word.to_lowercase().as_str()) {
            break;
        }
        // An all-caps shout is emphasis, not a surname.
        if word
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase)
            && word.len() > 3
        {
            break;
        }
        words.push(word.to_string());
        if words.len() == 2 {
            break;
        }
    }

    (!words.is_empty()).then(|| words.join(" "))
}

/// Find the cue phrases in one line and say which voice each names.
///
/// Returns the name and who it refers to; the caller resolves that to a cluster,
/// because only it knows who spoke before and after.
fn mentions(text: &str) -> Vec<(String, Refers)> {
    let lower = text.to_lowercase();
    let mut found = Vec::new();

    for (phrase, refers) in CUES {
        let mut from = 0;
        while let Some(at) = lower[from..].find(phrase) {
            let start = from + at;
            let end = start + phrase.len();
            from = end;

            // Must be a whole word, or "thanks" matches inside "thanksgiving"
            // and "i am" inside "miami among".
            let before_ok = start == 0
                || !lower[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric);
            // `is_none_or` would read better and is newer than this crate's MSRV.
            let after_ok = !lower[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric());
            if !before_ok || !after_ok {
                continue;
            }

            if let Some(name) = name_after(&text[end..]) {
                found.push((name, refers));
            }
        }
    }

    found
}

/// Decide what each voice is called, on the balance of what was said.
///
/// A vote rather than first-match: a name that comes up four times is more likely
/// right than one that came up once, and single stray matches are exactly what
/// the heuristic gets wrong. A voice keeps its number unless some name clearly
/// won.
fn infer_names(lines: &[Line], order: &[usize]) -> HashMap<usize, String> {
    let mut votes: HashMap<(usize, String), usize> = HashMap::new();

    for (index, line) in lines.iter().enumerate() {
        for (name, refers) in mentions(&line.cue.text) {
            let target = match refers {
                Refers::Speaker => line.speaker,
                Refers::Previous => neighbour(lines, index, false),
                Refers::Next => neighbour(lines, index, true),
            };
            if let Some(target) = target {
                *votes.entry((target, name)).or_default() += 1;
            }
        }
    }

    // Strongest claims first, so that when two voices are both called "Alice"
    // the one with more evidence keeps her and the other falls back to a number.
    let mut ranked: Vec<((usize, String), usize)> = votes.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut names: HashMap<usize, String> = HashMap::new();
    let mut taken: Vec<String> = Vec::new();
    for ((cluster, name), _) in ranked {
        if !order.contains(&cluster) || names.contains_key(&cluster) || taken.contains(&name) {
            continue;
        }
        taken.push(name.clone());
        names.insert(cluster, name);
    }
    names
}

/// The nearest line before or after `index` spoken by somebody else.
fn neighbour(lines: &[Line], index: usize, forwards: bool) -> Option<usize> {
    let here = lines[index].speaker;
    let range: Box<dyn Iterator<Item = usize>> = if forwards {
        Box::new(index + 1..lines.len())
    } else {
        Box::new((0..index).rev())
    };
    range
        .filter_map(|i| lines[i].speaker)
        .find(|speaker| Some(*speaker) != here)
}

/// Write the transcript out with the speakers marked.
pub fn render(lines: &[Line], cast: &Cast, format: Format) -> String {
    match format {
        Format::Text => render_text(lines, cast),
        Format::Srt => render_subtitles(lines, cast, format),
        Format::Vtt => render_subtitles(lines, cast, format),
    }
}

/// Plain text, as a script: one paragraph per turn.
///
/// Consecutive cues from one voice are joined rather than each getting their own
/// prefix. whisper cuts a cue every few seconds, so labelling each one turns a
/// two-minute answer into thirty lines that all say "Alice:".
fn render_text(lines: &[Line], cast: &Cast) -> String {
    let mut out = String::new();
    let mut current: Option<Option<usize>> = None;

    for line in lines {
        if current != Some(line.speaker) {
            if current.is_some() {
                out.push_str("\n\n");
            }
            out.push_str(&match line.speaker {
                Some(cluster) => format!("{}: ", cast.label(cluster)),
                // Said by nobody the segmentation model was sure about. Marked,
                // rather than silently attached to whoever spoke last.
                None => "—: ".to_string(),
            });
            current = Some(line.speaker);
        } else {
            out.push(' ');
        }
        out.push_str(&line.cue.text);
    }

    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Subtitles, with the timings kept exactly and the name on the text.
///
/// WebVTT gets a real `<v Alice>` voice span, which players and screen readers
/// understand as speaker attribution rather than as part of the dialogue. SRT has
/// no such thing, so it gets the prefix.
fn render_subtitles(lines: &[Line], cast: &Cast, format: Format) -> String {
    let vtt = matches!(format, Format::Vtt);
    let decimal = if vtt { '.' } else { ',' };
    let mut out = String::new();

    if vtt {
        out.push_str("WEBVTT\n\n");
    }

    for (index, line) in lines.iter().enumerate() {
        if !vtt {
            out.push_str(&format!("{}\n", index + 1));
        }
        out.push_str(&format!(
            "{} --> {}\n",
            format_timestamp(line.cue.start, decimal),
            format_timestamp(line.cue.end, decimal)
        ));

        match line.speaker {
            Some(cluster) if vtt => out.push_str(&format!(
                "<v {}>{}</v>\n\n",
                cast.label(cluster),
                line.cue.text
            )),
            Some(cluster) => {
                out.push_str(&format!("{}: {}\n\n", cast.label(cluster), line.cue.text))
            }
            None => out.push_str(&format!("{}\n\n", line.cue.text)),
        }
    }

    out
}

/// The sentence the UI shows when it is done.
pub fn summary(cast: &Cast) -> String {
    match cast.len() {
        0 => "No speakers identified".to_string(),
        1 => "One speaker".to_string(),
        n => {
            let named = cast.named();
            if named > 0 {
                format!("{n} speakers · {}", cast.labels().join(", "))
            } else {
                format!("{n} speakers")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speaker: usize) -> Turn {
        Turn {
            start,
            end,
            speaker,
        }
    }

    fn cue(start: f64, end: f64, text: &str) -> Cue {
        Cue {
            start,
            end,
            text: text.into(),
        }
    }

    const SRT: &str = "1\n\
        00:00:01,583 --> 00:00:03,406\n\
        Good morning, and welcome.\n\
        \n\
        2\n\
        00:00:09,346 --> 00:00:11,472\n\
        Thanks for having me.\n";

    #[test]
    fn whispers_own_srt_is_read_back() {
        let cues = parse_cues(SRT);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 1.583);
        assert_eq!(cues[0].end, 3.406);
        assert_eq!(cues[0].text, "Good morning, and welcome.");
        assert_eq!(cues[1].start, 9.346);
    }

    #[test]
    fn a_cue_split_over_two_lines_becomes_one_cue() {
        // whisper wraps long segments, and a transcript that treats each visual
        // line as a separate utterance labels half-sentences.
        let cues =
            parse_cues("1\n00:00:00,000 --> 00:00:04,000\nthe first half\nand the second half\n\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "the first half and the second half");
    }

    #[test]
    fn webvtt_timings_and_cue_settings_are_read_too() {
        let cues = parse_cues(
            "WEBVTT\n\n00:00:02.500 --> 00:00:04.000 align:start position:50%\nHello\n\n",
        );
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, 2.5);
        assert_eq!(cues[0].end, 4.0);
        assert_eq!(cues[0].text, "Hello");
    }

    #[test]
    fn a_timestamp_without_an_hour_is_still_a_timestamp() {
        assert_eq!(parse_timestamp("01:30.500"), Some(90.5));
        assert_eq!(parse_timestamp("00:01:30,500"), Some(90.5));
        assert_eq!(parse_timestamp("nonsense"), None);
    }

    #[test]
    fn a_rounded_up_millisecond_carries_into_the_second() {
        // 3.9996 rounds to 1000ms, and `00:00:03,1000` is not a timestamp any
        // player will accept.
        assert_eq!(format_timestamp(3.9996, ','), "00:00:04,000");
        assert_eq!(format_timestamp(0.0, ','), "00:00:00,000");
        assert_eq!(format_timestamp(3661.25, '.'), "01:01:01.250");
    }

    #[test]
    fn a_cue_belongs_to_whoever_spoke_for_most_of_it() {
        // Starts inside speaker 0's turn but spends most of itself in speaker 1's.
        // Matching on the start alone gets this wrong.
        let turns = [turn(0.0, 5.2, 0), turn(5.2, 12.0, 1)];
        let lines = align(vec![cue(5.0, 11.0, "...")], &turns);
        assert_eq!(lines[0].speaker, Some(1));
    }

    #[test]
    fn a_cue_goes_to_whoever_spoke_most_in_total_not_in_one_stretch() {
        // Speaker 1 interjects three times across speaker 0's answer. Speaker 1's
        // longest single turn (2.0s) beats any one of speaker 0's (1.5s), but
        // speaker 0 said more of the cue (4.5s against 2.6s).
        let turns = [
            turn(0.0, 1.5, 0),
            turn(1.5, 1.8, 1),
            turn(1.8, 3.3, 0),
            turn(3.3, 3.6, 1),
            turn(3.6, 5.1, 0),
            turn(5.1, 7.1, 1),
        ];
        let lines = align(vec![cue(0.0, 7.1, "...")], &turns);
        assert_eq!(lines[0].speaker, Some(0));
    }

    #[test]
    fn a_cue_no_turn_covers_is_left_unattributed() {
        // The segmentation model drops speech under 0.3s; whisper does not. An
        // invented attribution is worse than an honest gap.
        let lines = align(vec![cue(30.0, 31.0, "hm")], &[turn(0.0, 5.0, 0)]);
        assert_eq!(lines[0].speaker, None);
    }

    #[test]
    fn speakers_are_numbered_by_who_talks_first_not_by_cluster_id() {
        // Real output from sherpa-onnx v1.13.4 on a two-speaker file was
        // `speaker_00` and `speaker_03`. Printing the raw ids gives a two-hander
        // between "Speaker 1" and "Speaker 4".
        let turns = [turn(0.0, 5.0, 3), turn(5.0, 9.0, 0)];
        let lines = align(
            vec![cue(1.0, 4.0, "first"), cue(6.0, 8.0, "second")],
            &turns,
        );
        let cast = cast(&lines);

        assert_eq!(cast.len(), 2);
        assert_eq!(cast.label(3), "Speaker 1");
        assert_eq!(cast.label(0), "Speaker 2");
    }

    #[test]
    fn a_self_introduction_names_the_person_saying_it() {
        let turns = [turn(0.0, 5.0, 0)];
        let lines = align(vec![cue(0.0, 5.0, "Hello, I'm Alice Chen.")], &turns);
        assert_eq!(cast(&lines).label(0), "Alice Chen");
    }

    #[test]
    fn thanking_someone_names_the_person_who_just_spoke() {
        // You thank someone for what they have already said, so the name points
        // backwards.
        let turns = [turn(0.0, 5.0, 0), turn(5.0, 9.0, 1)];
        let lines = align(
            vec![
                cue(0.0, 5.0, "...and that is the whole story."),
                cue(5.0, 9.0, "Thanks, Priya."),
            ],
            &turns,
        );
        let cast = cast(&lines);
        assert_eq!(cast.label(0), "Priya");
        assert_eq!(cast.label(1), "Speaker 2");
    }

    #[test]
    fn handing_over_names_the_person_about_to_speak() {
        let turns = [turn(0.0, 5.0, 0), turn(5.0, 9.0, 1)];
        let lines = align(
            vec![
                cue(0.0, 5.0, "Over to you, Marcus."),
                cue(5.0, 9.0, "Right, so the numbers."),
            ],
            &turns,
        );
        assert_eq!(cast(&lines).label(1), "Marcus");
    }

    #[test]
    fn a_sentence_opener_is_not_a_person() {
        // "So, I'm going to explain" must not produce a speaker called "So", and
        // "I'm going" must not produce one called "Going".
        for text in [
            "So I'm going to explain the thing.",
            "I'm Not sure about that.",
            "I am The first to admit it.",
            "Thanks, Everyone.",
        ] {
            let lines = align(vec![cue(0.0, 5.0, text)], &[turn(0.0, 5.0, 0)]);
            assert_eq!(cast(&lines).label(0), "Speaker 1", "{text}");
        }
    }

    #[test]
    fn a_cue_phrase_inside_a_longer_word_does_not_count() {
        // "thanksgiving" contains "thanks"; "Miami" contains "i am".
        let lines = align(
            vec![cue(0.0, 5.0, "At Thanksgiving Dinner we talked.")],
            &[turn(0.0, 5.0, 0)],
        );
        assert_eq!(cast(&lines).label(0), "Speaker 1");
    }

    #[test]
    fn the_better_evidenced_name_wins_when_two_voices_claim_it() {
        // Both voices get called Alice; only one can be. The other keeps a number
        // rather than the transcript having two people with one name.
        let turns = [turn(0.0, 5.0, 0), turn(5.0, 10.0, 1)];
        let lines = align(
            vec![
                cue(0.0, 2.0, "I'm Alice."),
                cue(2.0, 5.0, "Yes, I'm Alice."),
                cue(5.0, 10.0, "I'm Alice too."),
            ],
            &turns,
        );
        let cast = cast(&lines);
        assert_eq!(cast.label(0), "Alice");
        assert_eq!(cast.label(1), "Speaker 2");
        assert_eq!(cast.named(), 1);
    }

    #[test]
    fn an_unnamed_recording_still_produces_a_usable_transcript() {
        let turns = [turn(0.0, 5.0, 0), turn(5.0, 9.0, 1)];
        let lines = align(vec![cue(0.0, 5.0, "One."), cue(5.0, 9.0, "Two.")], &turns);
        let cast = cast(&lines);
        assert_eq!(
            render(&lines, &cast, Format::Text),
            "Speaker 1: One.\n\nSpeaker 2: Two.\n"
        );
    }

    #[test]
    fn consecutive_cues_from_one_voice_become_one_paragraph() {
        // whisper cuts a cue every few seconds. Labelling each turns one answer
        // into thirty lines that all say the same name.
        let turns = [turn(0.0, 12.0, 0), turn(12.0, 16.0, 1)];
        let lines = align(
            vec![
                cue(0.0, 4.0, "First part"),
                cue(4.0, 8.0, "second part"),
                cue(8.0, 12.0, "third part."),
                cue(12.0, 16.0, "My turn."),
            ],
            &turns,
        );
        let cast = cast(&lines);
        let text = render(&lines, &cast, Format::Text);
        assert_eq!(
            text,
            "Speaker 1: First part second part third part.\n\nSpeaker 2: My turn.\n"
        );
    }

    #[test]
    fn subtitles_keep_their_timings_exactly() {
        let turns = [turn(0.0, 5.0, 0)];
        let lines = align(vec![cue(1.583, 3.406, "Hello.")], &turns);
        let cast = cast(&lines);

        let srt = render(&lines, &cast, Format::Srt);
        assert!(srt.contains("00:00:01,583 --> 00:00:03,406"), "{srt}");
        assert!(srt.contains("Speaker 1: Hello."), "{srt}");
        assert!(srt.starts_with("1\n"), "{srt}");

        // Round-tripping the rendered file must give back the same timings, which
        // is the property that matters for a subtitle track.
        let back = parse_cues(&srt);
        assert_eq!(back[0].start, 1.583);
        assert_eq!(back[0].end, 3.406);
    }

    #[test]
    fn webvtt_uses_a_real_voice_span_rather_than_a_prefix() {
        // `<v Alice>` is attribution players and screen readers understand;
        // "Alice:" is dialogue that happens to contain a colon.
        let turns = [turn(0.0, 5.0, 0)];
        let lines = align(vec![cue(0.0, 5.0, "Hello.")], &turns);
        let cast = cast(&lines);
        let vtt = render(&lines, &cast, Format::Vtt);

        assert!(vtt.starts_with("WEBVTT\n"), "{vtt}");
        assert!(vtt.contains("00:00:00.000 --> 00:00:05.000"), "{vtt}");
        assert!(vtt.contains("<v Speaker 1>Hello.</v>"), "{vtt}");
    }

    #[test]
    fn an_unattributed_cue_is_not_given_to_whoever_spoke_last() {
        let turns = [turn(0.0, 5.0, 0)];
        let lines = align(
            vec![cue(0.0, 5.0, "Mine."), cue(40.0, 41.0, "Nobody's.")],
            &turns,
        );
        let cast = cast(&lines);
        let srt = render(&lines, &cast, Format::Srt);
        assert!(srt.contains("Speaker 1: Mine."), "{srt}");
        // No name at all rather than a wrong one.
        assert!(srt.contains("\nNobody's.\n"), "{srt}");
    }

    #[test]
    fn the_summary_says_what_was_found() {
        assert_eq!(summary(&Cast::default()), "No speakers identified");

        let turns = [turn(0.0, 5.0, 0), turn(5.0, 9.0, 1)];
        let lines = align(
            vec![cue(0.0, 5.0, "I'm Alice."), cue(5.0, 9.0, "Hello.")],
            &turns,
        );
        assert_eq!(summary(&cast(&lines)), "2 speakers · Alice, Speaker 2");

        let solo = align(vec![cue(0.0, 5.0, "Just me.")], &[turn(0.0, 5.0, 0)]);
        assert_eq!(summary(&cast(&solo)), "One speaker");
    }

    #[test]
    fn an_empty_transcript_does_not_panic_or_invent_a_cast() {
        let lines = align(Vec::new(), &[]);
        let cast = cast(&lines);
        assert!(cast.is_empty());
        assert_eq!(render(&lines, &cast, Format::Text), "");
        assert_eq!(render(&lines, &cast, Format::Srt), "");
    }
}
