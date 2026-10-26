//! Spoken edit commands: "scratch that" drops the last phrase, "new
//! line" and "new paragraph" break the text, "comma", "period" and "question mark" write their
//! mark, and "at the rate kai" writes `@kai`. Pure, so it is tested without a Mac.
//!
//! The design follows `VoiceCommands.swift` in opennookorg/betterwispr (Apache-2.0); no code is
//! copied from it.

/// A command word glued to another word ("comma-separated") is a different word, so only whole
/// words match, with the transcriber's own punctuation at their edges ignored.
const EDGE_PUNCTUATION: &[char] = &['.', ',', '!', '?', ';', ':', '…', '"', '“', '”'];
const PHRASE_MARKS: &[char] = &['.', ',', '!', '?', ';', ':', '…'];
const SENTENCE_ENDS: &[char] = &['.', '!', '?', '…', '\n'];
/// A command word after one of these is the thing named, not the command ("a comma", "the new
/// line", "an Oxford comma").
const NOUN_MARKERS: &[&str] = &["a", "an", "the", "this", "that", "oxford"];
/// "at the rate of five percent" is prose: these never become a handle.
const NOT_A_HANDLE: &[&str] = &[
    "of", "per", "a", "an", "the", "we", "you", "they", "it", "i", "he", "she", "that", "which",
];

#[derive(Clone, Copy)]
enum Command {
    Mark(char),
    Break(&'static str),
    Scratch,
}

/// "period" and "scratch that" are everyday words ("the trial period ends", "scratch that
/// itch"), so they count only where the transcriber's punctuation or a new sentence sets them
/// off. The others are rare enough in prose to count anywhere.
#[derive(Clone, Copy, PartialEq)]
enum Boundary {
    Anywhere,
    SetOff,
    /// "period" ends a phrase where the transcriber puts its own full stop, which almost every
    /// dictation gets at its end ("We extended the trial period."), so being set off says nothing.
    /// It counts when it stands alone between phrase ends ("Done. Period."), or when the speaker
    /// also spoke a mark that has no everyday meaning, which shows they are dictating punctuation.
    StandsAloneOrMarksSpoken,
}

const COMMANDS: &[(&[&str], Command, Boundary)] = &[
    (&["scratch", "that"], Command::Scratch, Boundary::SetOff),
    (&["new", "paragraph"], Command::Break("\n\n"), Boundary::Anywhere),
    (&["new", "line"], Command::Break("\n"), Boundary::Anywhere),
    (&["question", "mark"], Command::Mark('?'), Boundary::Anywhere),
    (&["comma"], Command::Mark(','), Boundary::Anywhere),
    (&["period"], Command::Mark('.'), Boundary::StandsAloneOrMarksSpoken),
];
const AT_THE_RATE: &[&str] = &["at", "the", "rate"];

/// Text with no command in it comes back byte for byte.
pub fn apply(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let marks_spoken = marks_spoken(&tokens);
    let mut written = Written::default();
    let mut changed = false;
    let mut i = 0;
    while i < tokens.len() {
        let spoken = if let Some((command, len)) = command_at(&tokens, i, marks_spoken) {
            written.command(command);
            len
        } else if let Some(handle) = handle_at(&tokens, i) {
            written.handle(handle);
            AT_THE_RATE.len() + 1
        } else {
            written.word(tokens[i]);
            i += 1;
            continue;
        };
        changed = true;
        i += spoken;
    }
    if changed { written.text.trim().to_owned() } else { text.to_owned() }
}

fn key(token: &str) -> String {
    token.trim_matches(EDGE_PUNCTUATION).to_lowercase()
}

fn ends_phrase(token: &str) -> bool {
    token.ends_with(PHRASE_MARKS)
}

/// The words of `phrase` from `start`, none but the last ending a phrase, so a command never
/// spans the transcriber's sentence break ("Any question? Mark said").
fn spoken_at(tokens: &[&str], start: usize, phrase: &[&str]) -> bool {
    let Some(run) = tokens.get(start..start + phrase.len()) else { return false };
    let inner_open = run[..run.len() - 1].iter().all(|t| !ends_phrase(t));
    inner_open && run.iter().zip(phrase).all(|(token, word)| key(token) == *word)
}

fn after_noun_marker(tokens: &[&str], i: usize) -> bool {
    i > 0 && !ends_phrase(tokens[i - 1]) && NOUN_MARKERS.contains(&key(tokens[i - 1]).as_str())
}

/// The start of a dictation doesn't count as the phrase end before a command: "Scratch that itch"
/// opening a dictation is prose.
fn set_off(tokens: &[&str], i: usize, len: usize) -> bool {
    let before = i > 0 && ends_phrase(tokens[i - 1]);
    let last = tokens[i + len - 1];
    let after = match tokens.get(i + len) {
        None => true,
        Some(next) => ends_phrase(last) || next.starts_with(char::is_uppercase),
    };
    before || after
}

fn stands_alone(tokens: &[&str], i: usize, len: usize) -> bool {
    let before = i == 0 || ends_phrase(tokens[i - 1]);
    before && (ends_phrase(tokens[i + len - 1]) || i + len == tokens.len())
}

/// Whether a mark or line break that counts anywhere was spoken (`Boundary::StandsAloneOrMarksSpoken`).
fn marks_spoken(tokens: &[&str]) -> bool {
    (0..tokens.len()).any(|i| {
        !after_noun_marker(tokens, i)
            && COMMANDS.iter().any(|&(phrase, command, boundary)| {
                boundary == Boundary::Anywhere
                    && matches!(command, Command::Mark(_) | Command::Break(_))
                    && spoken_at(tokens, i, phrase)
            })
    })
}

fn command_at(tokens: &[&str], i: usize, marks_spoken: bool) -> Option<(Command, usize)> {
    if after_noun_marker(tokens, i) {
        return None;
    }
    COMMANDS.iter().find_map(|&(phrase, command, boundary)| {
        let len = phrase.len();
        if !spoken_at(tokens, i, phrase) {
            return None;
        }
        let placed = match boundary {
            Boundary::Anywhere => true,
            Boundary::SetOff => set_off(tokens, i, len),
            Boundary::StandsAloneOrMarksSpoken => {
                stands_alone(tokens, i, len) || (marks_spoken && set_off(tokens, i, len))
            }
        };
        placed.then_some((command, len))
    })
}

/// The word after "at the rate", when it can be a handle or an email's domain.
fn handle_at<'a>(tokens: &[&'a str], i: usize) -> Option<&'a str> {
    if !spoken_at(tokens, i, AT_THE_RATE) || ends_phrase(tokens[i + AT_THE_RATE.len() - 1]) {
        return None;
    }
    let handle = *tokens.get(i + AT_THE_RATE.len())?;
    let name = key(handle);
    let plain = name.starts_with(char::is_alphanumeric) && !name.contains(['\'', '’']);
    (plain && !NOT_A_HANDLE.contains(&name.as_str())).then_some(handle)
}

#[derive(Default)]
struct Written {
    text: String,
    capitalize: bool,
}

impl Written {
    fn word(&mut self, word: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push(' ');
        }
        let mut chars = word.chars();
        match chars.next() {
            Some(first) if self.capitalize => {
                self.text.extend(first.to_uppercase());
                self.text.push_str(chars.as_str());
            }
            _ => self.text.push_str(word),
        }
        self.capitalize = false;
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Mark(mark) => self.mark(mark),
            Command::Break(lines) => {
                self.trim_end(&[]);
                self.text.push_str(lines);
                self.capitalize = true;
            }
            Command::Scratch => self.scratch(),
        }
    }

    /// The transcriber often writes its own comma or full stop where the speaker paused before
    /// the command, so the spoken mark replaces it.
    fn mark(&mut self, mark: char) {
        self.trim_end(&[',', ';', ':', '.']);
        if mark != ',' {
            self.trim_end(&['!', '?']);
        }
        self.text.push(mark);
        self.capitalize = mark != ',';
    }

    /// Drops back to the last phrase end, so "Monday, scratch that, Tuesday" keeps what came
    /// before "Monday".
    fn scratch(&mut self) {
        self.trim_end(PHRASE_MARKS);
        let keep = self
            .text
            .char_indices()
            .rfind(|&(_, c)| PHRASE_MARKS.contains(&c) || c == '\n')
            .map_or(0, |(end, mark)| end + mark.len_utf8());
        self.text.truncate(keep);
        self.trim_end(&[]);
        self.capitalize = self.text.is_empty() || self.text.ends_with(SENTENCE_ENDS);
    }

    /// "kai at the rate gmail.com" is an email address, so a domain joins the word before it.
    fn handle(&mut self, handle: &str) {
        let domain = handle.trim_end_matches(EDGE_PUNCTUATION).contains('.');
        if domain && self.text.ends_with(char::is_alphanumeric) {
            self.text.push('@');
            self.text.push_str(handle);
            self.capitalize = false;
        } else {
            self.capitalize = false;
            self.word(&format!("@{handle}"));
        }
    }

    fn trim_end(&mut self, marks: &[char]) {
        let kept = self.text.trim_end_matches(|c: char| c == ' ' || marks.contains(&c)).len();
        self.text.truncate(kept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_commands_comes_back_unchanged() {
        for prose in [
            "Send me a comma-separated list.",
            "Export it as comma-separated values, then new-line-delimited JSON.",
            "Put a comma after the name and a period at the end.",
            "Is that a question mark or the new line character?",
            "The trial period ends on Friday.",
            "We extended the trial period.",
            "The trial period",
            "My trial period. Then the review.",
            "Send the invoice period then call me.",
            "Don't scratch that itch.",
            "Scratch that itch.",
            "Prices rose at the rate of five percent.",
            "We grew at the rate we're used to.",
            "Any question? Mark said no.",
            "It ends with an Oxford comma",
            "Line one\nline two",
        ] {
            assert_eq!(apply(prose), prose);
        }
    }

    #[test]
    fn spoken_punctuation_replaces_the_word() {
        assert_eq!(apply("Hello comma world"), "Hello, world");
        assert_eq!(apply("Hello, comma, world."), "Hello, world.");
        assert_eq!(apply("Hello. Comma world."), "Hello, world.");
        assert_eq!(apply("Is it ready question mark"), "Is it ready?");
        assert_eq!(apply("Is it ready question mark ship it"), "Is it ready? Ship it");
        assert_eq!(apply("That is done. Period. Next one."), "That is done. Next one.");
        assert_eq!(apply("Book the room comma then the flights period."), "Book the room, then the flights.");
        assert_eq!(apply("Is it ready question mark ship it period"), "Is it ready? Ship it.");
    }

    #[test]
    fn line_breaks_start_a_capitalised_line() {
        assert_eq!(apply("Dear Kai comma new line thanks for the fix"), "Dear Kai,\nThanks for the fix");
        assert_eq!(apply("First part. New paragraph. second part."), "First part.\n\nSecond part.");
    }

    #[test]
    fn scratch_that_drops_the_last_phrase() {
        assert_eq!(apply("Let's meet Monday. Scratch that. Let's meet Tuesday."), "Let's meet Tuesday.");
        assert_eq!(apply("I like apples, and oranges, scratch that, and pears."), "I like apples, and pears.");
        assert_eq!(apply("Ship it today. Send it Monday, scratch that"), "Ship it today.");
        assert_eq!(apply("Send it Monday scratch that Tuesday works"), "Tuesday works");
        assert_eq!(apply("One… two scratch that"), "One…");
        assert_eq!(apply("Scratch that."), "");
    }

    #[test]
    fn at_the_rate_writes_a_handle() {
        assert_eq!(apply("Ask at the rate kai about it"), "Ask @kai about it");
        assert_eq!(apply("Thanks, at the rate north."), "Thanks, @north.");
        assert_eq!(apply("Mail kai at the rate vardelabs.com today"), "Mail kai@vardelabs.com today");
    }
}
