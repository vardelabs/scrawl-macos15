//! Drops spoken fillers from a transcript, as dictation apps do, so pasted quotes read as
//! written text (`removeFillers`, default on). Hesitations ("um", "uh") always go. "Like" goes
//! only where the transcriber's commas mark it as an aside ("it's, like, nice" or a leading
//! "Like,"), because "it looks like rain" needs it.

use crate::model::TranscriptData;

const HESITATIONS: &[&str] = &[
    "um", "umm", "ummm", "uh", "uhh", "uhm", "er", "erm", "hmm", "mm",
];
const TRAILING: &[char] = &[',', '.', '?', '!', ';', ':', '…'];

pub fn clean(transcript: &mut TranscriptData) {
    transcript.text = clean_text(&transcript.text);
    for segment in &mut transcript.segments {
        segment.text = clean_text(&segment.text);
    }
    transcript
        .segments
        .retain(|segment| !segment.text.is_empty());
    let texts: Vec<&str> = transcript
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect();
    let kept = clean_tokens(&texts);
    transcript.words = std::mem::take(&mut transcript.words)
        .into_iter()
        .zip(kept)
        .filter_map(|(mut word, text)| {
            word.text = text?;
            Some(word)
        })
        .collect();
}

/// Text with nothing to drop comes back byte for byte, line breaks included.
fn clean_text(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let kept = clean_tokens(&tokens);
    if kept
        .iter()
        .zip(&tokens)
        .all(|(kept, token)| kept.as_deref() == Some(*token))
    {
        return text.to_owned();
    }
    kept.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// Each token's text after cleaning, or `None` where it was a filler. A dropped filler hands its
/// sentence-ending mark to the word before it and its sentence start to the word after it.
fn clean_tokens(tokens: &[&str]) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = tokens
        .iter()
        .map(|token| Some((*token).to_owned()))
        .collect();
    let mut last_kept: Option<usize> = None;
    let mut capitalize_next = false;
    for (i, token) in tokens.iter().enumerate() {
        let (word, trail) = split_trailing(token);
        let previous = last_kept.and_then(|k| out[k].as_deref());
        let sentence_start = previous.map_or(true, ends_sentence);
        let aside_like = word.eq_ignore_ascii_case("like")
            && trail == ","
            && (sentence_start || previous.is_some_and(|p| p.ends_with(',')));
        if !(aside_like || HESITATIONS.contains(&word.to_lowercase().as_str())) {
            if std::mem::take(&mut capitalize_next) {
                out[i] = Some(capitalize(token));
            }
            last_kept = Some(i);
            continue;
        }
        out[i] = None;
        capitalize_next |= sentence_start;
        let Some(k) = last_kept else { continue };
        let kept = out[k].as_mut().expect("last_kept is kept");
        if trail.starts_with(['.', '?', '!', '…']) {
            kept.truncate(kept.trim_end_matches([',', ';', ':']).len());
            if !ends_sentence(kept) {
                kept.push_str(trail);
            }
        } else if aside_like && kept.ends_with(',') {
            kept.pop();
        }
    }
    out
}

fn split_trailing(token: &str) -> (&str, &str) {
    let word = token.trim_end_matches(TRAILING);
    (word, &token[word.len()..])
}

fn ends_sentence(token: &str) -> bool {
    token.ends_with(['.', '?', '!', '…'])
}

fn capitalize(token: &str) -> String {
    let mut chars = token.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TranscriptSegment, WordTiming};

    #[test]
    fn hesitations_go_anywhere() {
        assert_eq!(
            clean_text("it might be nice to have it uh close"),
            "it might be nice to have it close"
        );
        assert_eq!(clean_text("close. Um and yeah."), "close. And yeah.");
        assert_eq!(clean_text("Um, so, uh, this works"), "So, this works");
    }

    #[test]
    fn a_dropped_filler_hands_on_its_full_stop() {
        assert_eq!(clean_text("it goes away, um. Next"), "it goes away. Next");
        assert_eq!(clean_text("Um."), "");
    }

    #[test]
    fn like_goes_only_as_a_comma_aside() {
        assert_eq!(clean_text("it's, like, kind of nice"), "it's kind of nice");
        assert_eq!(
            clean_text("Like, if you click it again"),
            "If you click it again"
        );
        assert_eq!(clean_text("it looks like rain"), "it looks like rain");
        assert_eq!(
            clean_text("things like, say, buttons"),
            "things like, say, buttons"
        );
        assert_eq!(clean_text("Like if you click it"), "Like if you click it");
    }

    #[test]
    fn untouched_text_keeps_its_spacing() {
        assert_eq!(clean_text("line one\nline  two"), "line one\nline  two");
    }

    #[test]
    fn words_keep_their_times_and_segments_left_empty_go() {
        let word = |t0, text: &str| WordTiming {
            t0,
            t1: t0 + 100,
            text: text.into(),
        };
        let mut transcript = TranscriptData {
            text: "Um, close it. Uh.".into(),
            segments: vec![
                TranscriptSegment {
                    t0: 0,
                    t1: 900,
                    text: "Um, close it.".into(),
                },
                TranscriptSegment {
                    t0: 900,
                    t1: 1200,
                    text: "Uh.".into(),
                },
            ],
            words: vec![
                word(0, "Um,"),
                word(200, "close"),
                word(400, "it."),
                word(900, "Uh."),
            ],
            ..Default::default()
        };
        clean(&mut transcript);
        assert_eq!(transcript.text, "Close it.");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].text, "Close it.");
        assert_eq!(transcript.words, vec![word(200, "Close"), word(400, "it.")]);
    }
}
