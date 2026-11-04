//! Drops spoken fillers from a transcript, as dictation apps do, so pasted quotes read as
//! written text (`removeFillers`, default on). Hesitations ("um", "uh", drawn out as "ummm" or
//! cut off as "uh-") always go. "Like" goes where the transcriber's commas mark it as an aside
//! ("it's, like, nice" or a leading "Like,"), because "it looks like rain" needs it, and without
//! commas where it can only be filler (`bare_like`).

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

/// A cut-off hesitation ("uh-", "um—") keeps its dash, so dashes trail like punctuation.
const TRAILING: &[char] = &[',', '.', '?', '!', ';', ':', '…', '-', '–', '—'];
const SENTENCE_END: &[char] = &['.', '?', '!', '…'];
/// How close a segment's first word must start to it to count as the clause boundary
/// `keyframes` looks for.
const BOUNDARY_SLACK_MS: u64 = 20;

pub fn clean(transcript: &mut TranscriptData) {
    transcript.text = clean_text(&transcript.text);
    clean_segments(&mut transcript.segments);
    let dropped = clean_words(&mut transcript.words);
    for segment in &mut transcript.segments {
        realign_start(segment, &transcript.words, &dropped);
    }
}

/// Text with nothing to drop comes back byte for byte, line breaks included.
fn clean_text(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let kept = clean_tokens(&tokens);
    if unchanged(&kept, &tokens) {
        return text.to_owned();
    }
    join(kept)
}

/// Segments are cleaned as one stream, as words are, so a filler at a segment's edge reads
/// the same in both.
fn clean_segments(segments: &mut Vec<TranscriptSegment>) {
    let per_segment: Vec<Vec<&str>> = segments
        .iter()
        .map(|segment| segment.text.split_whitespace().collect())
        .collect();
    let stream: Vec<&str> = per_segment.iter().flatten().copied().collect();
    let mut kept = clean_tokens(&stream).into_iter();
    let texts: Vec<Option<String>> = per_segment
        .iter()
        .map(|tokens| {
            let mine: Vec<_> = kept.by_ref().take(tokens.len()).collect();
            (!unchanged(&mine, tokens)).then(|| join(mine))
        })
        .collect();
    for (segment, text) in segments.iter_mut().zip(texts) {
        if let Some(text) = text {
            segment.text = text;
        }
    }
    segments.retain(|segment| !segment.text.is_empty());
}

/// Drops filler words and returns when each dropped one started.
fn clean_words(words: &mut Vec<WordTiming>) -> Vec<u64> {
    let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
    let kept = clean_tokens(&texts);
    let mut dropped = Vec::new();
    *words = std::mem::take(words)
        .into_iter()
        .zip(kept)
        .filter_map(
            |(mut word, text)| match text.filter(|text| !text.is_empty()) {
                Some(text) => {
                    word.text = text;
                    Some(word)
                }
                None => {
                    dropped.push(word.t0);
                    None
                }
            },
        )
        .collect();
    dropped
}

/// A segment whose opening filler was dropped starts at its first kept word, so `keyframes`
/// still sees the clause boundary there.
fn realign_start(segment: &mut TranscriptSegment, words: &[WordTiming], dropped: &[u64]) {
    let from = segment.t0.saturating_sub(BOUNDARY_SLACK_MS);
    let Some(first) = words
        .iter()
        .find(|word| word.t0 >= from && word.t0 < segment.t1)
    else {
        return;
    };
    if dropped.iter().any(|&t0| t0 >= from && t0 < first.t0) {
        segment.t0 = segment.t0.max(first.t0);
    }
}

/// Each token's text after cleaning, or `None` where it was a filler. A punctuation-only token
/// belongs to the word before it. A dropped filler hands its sentence-ending mark to the word
/// before it, takes a comma pair with it ("it's, um, nice" reads "it's nice"), and hands its
/// sentence start to the word after it.
fn clean_tokens(tokens: &[&str]) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = tokens
        .iter()
        .map(|token| Some((*token).to_owned()))
        .collect();
    let mut capitalize_next = false;
    let mut i = 0;
    while i < tokens.len() {
        let mut end = i + 1;
        while end < tokens.len() && is_punctuation(tokens[end]) {
            end += 1;
        }
        let (word, own_trail) = split_trailing(tokens[i]);
        let trail: String = std::iter::once(own_trail)
            .chain(tokens[i + 1..end].iter().copied())
            .collect();
        // A comma token emptied by an earlier filler no longer marks anything.
        let last_kept = (0..i)
            .rev()
            .find(|&k| out[k].as_deref().is_some_and(|t| !t.is_empty()));
        let previous = last_kept.and_then(|k| out[k].as_deref());
        let sentence_start = previous.map_or(true, |p| p.ends_with(SENTENCE_END));
        let is_like = matches!(word, "like" | "Like");
        let aside_like = is_like
            && trail.starts_with(',')
            && (sentence_start || previous.is_some_and(|p| p.ends_with(',')));
        let bare = is_like
            && bare_like(
                (!sentence_start).then_some(previous).flatten(),
                tokens.get(end).copied(),
            );
        if !(aside_like || bare || is_hesitation(word)) {
            if std::mem::take(&mut capitalize_next) {
                out[i] = Some(capitalize(tokens[i]));
            }
            i = end;
            continue;
        }
        out[i..end].fill(None);
        capitalize_next |= sentence_start;
        if let Some(kept) = last_kept.and_then(|k| out[k].as_mut()) {
            hand_back(kept, &trail);
        }
        i = end;
    }
    out
}

fn hand_back(kept: &mut String, trail: &str) {
    let mark: String = trail.chars().filter(|c| SENTENCE_END.contains(c)).collect();
    if !mark.is_empty() {
        kept.truncate(kept.trim_end_matches([',', ';', ':']).len());
        if !kept.ends_with(SENTENCE_END) {
            kept.push_str(&mark);
        }
    } else if trail.starts_with(',') && kept.ends_with(',') {
        kept.pop();
    }
}

/// "um", "uh", "uhm", "er", "erm" and "hmm", however drawn out ("ummm", "uhhh", "hmmmm"),
/// lowercase or capitalized, so "ER" and "UM" as written words survive. "mm" is left out because
/// it is also millimetres, and "uh-huh" and "mhm" because they answer yes.
fn is_hesitation(word: &str) -> bool {
    let lower = word.to_lowercase();
    if word != lower && word != capitalize(&lower) {
        return false;
    }
    let runs = runs(&lower);
    let letters: String = runs.iter().map(|(c, _)| *c).collect();
    match letters.as_str() {
        "uh" | "um" | "uhm" | "hm" => true,
        // "er" and "erm", but not "err", which is a word.
        "er" | "erm" => runs[1].1 == 1,
        _ => false,
    }
}

/// Each letter with how many times it repeats in a row: "ummm" is `[('u', 1), ('m', 3)]`.
fn runs(word: &str) -> Vec<(char, usize)> {
    let mut runs: Vec<(char, usize)> = Vec::new();
    for c in word.chars() {
        match runs.last_mut() {
            Some((last, n)) if *last == c => *n += 1,
            _ => runs.push((c, 1)),
        }
    }
    runs
}

/// Words that join on a filler "like" ("and like then", "so like if").
const DISCOURSE: &[&str] = &[
    "and", "but", "so", "or", "yeah", "well", "okay", "ok", "oh", "because", "cause",
];
/// After those, "like" is a verb or a preposition: "and like it", "but like the old one".
const OBJECTS: &[&str] = &[
    "it",
    "them",
    "him",
    "her",
    "me",
    "you",
    "us",
    "that",
    "this",
    "these",
    "those",
    "the",
    "a",
    "an",
    "to",
    "my",
    "your",
    "his",
    "their",
    "our",
    "its",
    "what",
    "how",
    "i",
    "we",
    "they",
    "he",
    "she",
    "some",
    "any",
    "all",
    "each",
    "every",
    "one",
    "something",
    "anything",
];
/// "it's like really slow", but "it's like a car" compares.
const COPULAS: &[&str] = &[
    "is", "was", "are", "were", "be", "been", "it's", "that's", "he's", "she's", "they're",
    "we're", "you're", "i'm", "there's",
];
const INTENSIFIERS: &[&str] = &[
    "really",
    "very",
    "so",
    "super",
    "totally",
    "literally",
    "actually",
    "kind",
    "kinda",
    "sort",
    "sorta",
    "basically",
    "just",
    "pretty",
    "completely",
    "way",
    "too",
    "almost",
    "honestly",
    "not",
    "always",
    "never",
];
/// What a sentence-opening "Like" leads into when it is filler ("Like if you click it"), not a
/// comparison ("Like the old one") or "Like I said".
const OPENED: &[&str] = &[
    "if",
    "when",
    "so",
    "maybe",
    "honestly",
    "basically",
    "literally",
    "really",
    "right",
    "yeah",
    "okay",
    "ok",
    "then",
    "there's",
    "it's",
    "this",
    "is",
    "can",
    "could",
    "should",
    "why",
];

/// Whether a "like" with no comma around it is filler: doubled ("like like"), joined on with
/// "and", "so" or "but" before anything but an object, between a copula and an intensifier, or
/// opening a sentence before a clause. `previous` is the kept word before it, `None` at a
/// sentence start; `next` the token after it.
fn bare_like(previous: Option<&str>, next: Option<&str>) -> bool {
    let bare = |token: &str| {
        token
            .trim_end_matches(TRAILING)
            .replace('’', "'")
            .to_lowercase()
    };
    let Some(next) = next.map(bare).filter(|next| !next.is_empty()) else {
        return false;
    };
    if next == "like"
        || is_hesitation(&next)
            && previous.is_none_or(|p| DISCOURSE.contains(&bare(p).as_str()))
    {
        return true;
    }
    let Some(raw) = previous else {
        return OPENED.contains(&next.as_str());
    };
    let previous = bare(raw);
    (DISCOURSE.contains(&previous.as_str()) && !OBJECTS.contains(&next.as_str()))
        || (COPULAS.contains(&previous.as_str()) && INTENSIFIERS.contains(&next.as_str()))
}

fn is_punctuation(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| TRAILING.contains(&c))
}

fn split_trailing(token: &str) -> (&str, &str) {
    let word = token.trim_end_matches(TRAILING);
    (word, &token[word.len()..])
}

/// Leaves a word that already has a capital ("iOS", "Okay") as it is.
fn capitalize(token: &str) -> String {
    if token.chars().any(char::is_uppercase) {
        return token.to_owned();
    }
    let mut chars = token.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn unchanged(kept: &[Option<String>], tokens: &[&str]) -> bool {
    kept.iter()
        .zip(tokens)
        .all(|(kept, token)| kept.as_deref() == Some(*token))
}

fn join(kept: Vec<Option<String>>) -> String {
    kept.into_iter()
        .flatten()
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_out_and_cut_off_hesitations_go() {
        assert_eq!(
            clean_text("so uhhh we click ummmm here"),
            "so we click here"
        );
        assert_eq!(clean_text("Hmmm, that works"), "That works");
        assert_eq!(
            clean_text("it's uh- the button um— there"),
            "it's the button there"
        );
        assert_eq!(clean_text("Erm, close it"), "Close it");
    }

    #[test]
    fn answers_and_words_that_look_like_hesitations_stay() {
        assert_eq!(
            clean_text("uh-huh, mhm, that's right"),
            "uh-huh, mhm, that's right"
        );
        assert_eq!(clean_text("to err is human"), "to err is human");
        assert_eq!(clean_text("the UMM build"), "the UMM build");
        assert_eq!(
            clean_text("a well-known — but slow — app"),
            "a well-known — but slow — app"
        );
    }

    #[test]
    fn bare_like_goes_where_it_can_only_be_filler() {
        assert_eq!(
            clean_text("and like then we click it"),
            "and then we click it"
        );
        assert_eq!(clean_text("so like if you open it"), "so if you open it");
        assert_eq!(clean_text("it's like really slow"), "it's really slow");
        assert_eq!(clean_text("it was like super laggy"), "it was super laggy");
        assert_eq!(clean_text("Like if you click it"), "If you click it");
        assert_eq!(
            clean_text("Done. Like when it loads"),
            "Done. When it loads"
        );
        assert_eq!(clean_text("it's like like broken"), "it's like broken");
        assert_eq!(clean_text("so um like then it loads"), "so then it loads");
        assert_eq!(clean_text("Like uh this one"), "This one");
    }

    #[test]
    fn bare_like_reads_the_word_before_it_however_it_was_written() {
        assert_eq!(
            clean_text("Um so like if you open it, it's like really slow, I like it."),
            "So if you open it, it's really slow, I like it."
        );
        assert_eq!(clean_text("So like if you open it"), "So if you open it");
        assert_eq!(clean_text("It's like really slow"), "It's really slow");
        assert_eq!(clean_text("It’s like really slow"), "It’s really slow");
        assert_eq!(clean_text("Yeah, like when it loads"), "Yeah, when it loads");
        assert_eq!(clean_text("So, um like then it loads"), "So, then it loads");
    }

    #[test]
    fn bare_like_stays_as_a_verb_or_comparison() {
        for text in [
            "it looks like rain",
            "I like it",
            "I'd like to see that",
            "and like it a lot",
            "but like the old one",
            "it's like a car",
            "Like I said, it works",
            "Like the old one",
            "feels like it's broken",
            "something like this",
            "I like really fast cars",
            "what is it like",
        ] {
            assert_eq!(clean_text(text), text);
        }
        assert_eq!(clean_text("I like uh pizza"), "I like pizza");
        assert_eq!(clean_text("so um like the button"), "so like the button");
    }

    fn word(t0: u64, text: &str) -> WordTiming {
        WordTiming {
            t0,
            t1: t0 + 100,
            text: text.into(),
        }
    }

    fn segment(t0: u64, t1: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            t0,
            t1,
            text: text.into(),
        }
    }

    #[test]
    fn hesitations_go_anywhere() {
        assert_eq!(
            clean_text("it might be nice to have it uh close"),
            "it might be nice to have it close"
        );
        assert_eq!(clean_text("close. Um and yeah."), "close. And yeah.");
        assert_eq!(clean_text("Um, so, uh, this works"), "So this works");
    }

    #[test]
    fn written_words_that_spell_a_filler_stay() {
        assert_eq!(clean_text("took him to the ER."), "took him to the ER.");
        assert_eq!(clean_text("make it 5 mm wider"), "make it 5 mm wider");
        assert_eq!(clean_text("Um, iOS crashes"), "iOS crashes");
    }

    #[test]
    fn a_dropped_filler_hands_on_its_full_stop() {
        assert_eq!(clean_text("it goes away, um. Next"), "it goes away. Next");
        assert_eq!(clean_text("Um."), "");
    }

    #[test]
    fn a_filler_takes_its_comma_pair_with_it() {
        assert_eq!(clean_text("it's, like, kind of nice"), "it's kind of nice");
        assert_eq!(clean_text("it's, um, kind of nice"), "it's kind of nice");
    }

    #[test]
    fn like_goes_as_a_comma_aside() {
        assert_eq!(
            clean_text("Like, if you click it again"),
            "If you click it again"
        );
        assert_eq!(clean_text("it looks like rain"), "it looks like rain");
        assert_eq!(
            clean_text("things like, say, buttons"),
            "things like, say, buttons"
        );
    }

    #[test]
    fn separate_punctuation_tokens_go_with_their_filler() {
        assert_eq!(
            clean_tokens(&["it's", ",", "like", ",", "nice"]),
            vec![
                Some("it's".into()),
                Some("".into()),
                None,
                None,
                Some("nice".into())
            ]
        );
        assert_eq!(
            join(clean_tokens(&["it's", ",", "um", ",", "uh", ",", "nice"])),
            "it's nice"
        );
    }

    #[test]
    fn untouched_text_keeps_its_spacing() {
        assert_eq!(clean_text("line one\nline  two"), "line one\nline  two");
    }

    #[test]
    fn segments_read_as_one_stream_like_words() {
        let mut segments = vec![
            segment(0, 900, "we click it,"),
            segment(900, 1800, "um, and then"),
        ];
        clean_segments(&mut segments);
        assert_eq!(segments[0].text, "we click it");
        assert_eq!(segments[1].text, "and then");
    }

    #[test]
    fn words_keep_their_times_and_segments_follow_their_first_word() {
        let mut transcript = TranscriptData {
            text: "Um, close it. Uh.".into(),
            segments: vec![segment(0, 900, "Um, close it."), segment(900, 1200, "Uh.")],
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
        assert_eq!(transcript.segments, vec![segment(200, 900, "Close it.")]);
        assert_eq!(transcript.words, vec![word(200, "Close"), word(400, "it.")]);
    }
}
