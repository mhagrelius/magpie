//! The speaker pass, end to end, against output both tools really produced.
//!
//! The strings below are not invented. They were captured on 2026-08-01 by
//! running whisper.cpp and sherpa-onnx v1.13.4 over
//! `1-two-speakers-en.wav` — sherpa-onnx's own two-speaker sample — with exactly
//! the arguments `model::transcript::argv` and `model::diarize::argv` build. That
//! is the point of this file: the unit tests check the pieces against a format
//! this crate believes in, and this one checks that belief against reality.
//!
//! No display, no models and no audio needed to run it, because everything from
//! the tools' stdout onwards is pure.

use magpie::model::diarize;
use magpie::model::speakers;
use magpie::model::transcript::Format;

/// whisper.cpp's `--output-srt`, ggml-small, verbatim.
const WHISPER_SRT: &str = "1\n\
    00:00:00,000 --> 00:00:09,600\n\
    \x20A pencil with black lead writes best, the lamp shone with a steady green flame.\n\
    \n\
    2\n\
    00:00:09,600 --> 00:00:14,640\n\
    \x20Clothes and lodging are free to new men, the glow deepened in the eyes of the sweet girl.\n";

/// `sherpa-onnx-offline-speaker-diarization` stdout, verbatim, with the
/// configuration banner it prints before the turns.
const SHERPA_STDOUT: &str = "OfflineSpeakerDiarizationConfig(segmentation=OfflineSpeakerSegmentationModelConfig(pyannote=OfflineSpeakerSegmentationPyannoteModelConfig(model=\"/m/segmentation.onnx\"), num_threads=1, debug=False, provider=\"cpu\"), embedding=SpeakerEmbeddingExtractorConfig(model=\"/m/embedding.onnx\", num_threads=1, debug=False, provider=\"cpu\"), clustering=FastClusteringConfig(num_clusters=-1, threshold=0.5), min_duration_on=0.3, min_duration_off=0.5)\n\
    Started\n\
    1.583 -- 3.406 speaker_00\n\
    4.402 -- 6.443 speaker_00\n\
    9.346 -- 11.472 speaker_03\n\
    12.164 -- 14.645 speaker_03\n";

fn turns() -> Vec<diarize::Turn> {
    SHERPA_STDOUT
        .lines()
        .filter_map(diarize::parse_turn)
        .collect()
}

#[test]
fn the_banner_is_skipped_and_only_the_turns_are_read() {
    let turns = turns();
    assert_eq!(turns.len(), 4, "{turns:?}");
    assert_eq!(turns[0].start, 1.583);
    assert_eq!(turns[3].speaker, 3);
}

#[test]
fn two_people_talking_come_out_as_two_speakers() {
    // The headline claim. Note the raw cluster ids are 0 and 3 — this really is
    // what sherpa-onnx returned for a file with two people in it — so anything
    // that printed the ids would report speakers 1 and 4.
    let turns = turns();
    assert_eq!(diarize::speaker_count(&turns), 2);

    let cues = speakers::parse_cues(WHISPER_SRT);
    assert_eq!(cues.len(), 2);

    let lines = speakers::align(cues, &turns);
    let cast = speakers::cast(&lines);

    assert_eq!(cast.len(), 2);
    assert_eq!(cast.labels(), vec!["Speaker 1", "Speaker 2"]);
    assert_eq!(speakers::summary(&cast), "2 speakers");
}

#[test]
fn each_line_of_the_real_transcript_lands_on_the_right_voice() {
    // whisper cut the audio into two cues and sherpa-onnx into four turns, on
    // completely different boundaries — 0.0-9.6 against 1.583-3.406 and
    // 4.402-6.443. Overlap is what reconciles them.
    let lines = speakers::align(speakers::parse_cues(WHISPER_SRT), &turns());
    let cast = speakers::cast(&lines);

    let text = speakers::render(&lines, &cast, Format::Text);
    assert_eq!(
        text,
        "Speaker 1: A pencil with black lead writes best, the lamp shone with a steady green flame.\
         \n\nSpeaker 2: Clothes and lodging are free to new men, the glow deepened in the eyes of \
         the sweet girl.\n"
    );
}

#[test]
fn the_subtitle_form_keeps_whispers_timings_untouched() {
    let lines = speakers::align(speakers::parse_cues(WHISPER_SRT), &turns());
    let cast = speakers::cast(&lines);

    let srt = speakers::render(&lines, &cast, Format::Srt);
    // Re-reading the file Magpie wrote must give back the timings whisper
    // produced, or the transcript has drifted out of sync with the video.
    let round_tripped = speakers::parse_cues(&srt);
    let original = speakers::parse_cues(WHISPER_SRT);
    assert_eq!(round_tripped.len(), original.len());
    for (after, before) in round_tripped.iter().zip(&original) {
        assert_eq!(after.start, before.start);
        assert_eq!(after.end, before.end);
    }
    assert!(srt.contains("Speaker 1: A pencil"), "{srt}");
}

#[test]
fn a_run_that_found_nothing_leaves_the_transcript_alone() {
    // Silence, or audio with no speech the segmentation model would accept.
    // There is no cast, so there is nothing to label and the caller keeps the
    // plain transcript rather than writing an empty one over it.
    let lines = speakers::align(speakers::parse_cues(WHISPER_SRT), &[]);
    let cast = speakers::cast(&lines);
    assert!(cast.is_empty());
    assert_eq!(speakers::summary(&cast), "No speakers identified");
}
