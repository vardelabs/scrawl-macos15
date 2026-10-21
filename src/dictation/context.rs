//! Filenames in dictated text, as Wispr Flow writes them: "dot md" becomes ".md", a spoken
//! filename that matches one on screen ("product's design dot MD" with `PRODUCT-DESIGN.md` in
//! the window) is written as it is there, and filenames and `snake_case` go in backticks. Pure,
//! so it is tested without a Mac; `ax::window_text` gathers the screen text.
//!
//! Prose is the common case, so every rule needs a spoken "dot" or a written `.` or `_`: names,
//! brand names, camelCase and phrasal verbs ("sign in" with `signIn` on screen) stay as spoken.

use std::collections::HashMap;

/// File extensions that make a word a filename.
const EXTENSIONS: &[&str] = &[
    "md", "rs", "swift", "json", "toml", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "sh",
    "txt", "html", "css", "go", "rb", "java", "kt", "c", "h", "cpp", "sql", "csv", "png", "jpg",
    "svg", "pdf", "lock", "env", "plist", "xml", "mov", "mp4", "wav",
];
/// Extensions that are also everyday words or letters ("on the dot go", "Washington D.C."), so
/// neither a spoken "dot" nor backticks take them.
const AMBIGUOUS: &[&str] = &["go", "c", "h", "sh", "env", "lock"];
/// Words a spoken "dot" never joins onto ("the dot", "a dot").
const NOT_A_STEM: &[&str] = &["the", "a", "an"];
/// Most screen filenames kept.
const MAX_TERMS: usize = 400;
/// Screen text read for filenames; a huge document stops here.
const MAX_SCREEN_CHARS: usize = 200_000;
/// The most spoken words one filename can stand for ("scrawl macos fifteen dot md").
const MAX_SPAN_WORDS: usize = 6;

/// Joins a spoken "dot" (lower case, so the name "Dot" stays) and a known extension onto the
/// word before: "design dot MD" becomes "design.md".
pub fn spoken_extensions(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let ext = words.get(i + 1).map(|next| split_trailing(next));
        let joins = words[i] == "dot"
            && out.last().is_some_and(|prev| {
                prev.ends_with(|c: char| c.is_alphanumeric())
                    && !NOT_A_STEM.contains(&prev.to_lowercase().as_str())
            })
            && ext.is_some_and(|(core, _)| unambiguous_extension(core));
        match (joins, ext) {
            (true, Some((core, rest))) => {
                let prev = out.last_mut().expect("checked above");
                prev.push('.');
                prev.push_str(&core.to_lowercase());
                prev.push_str(rest);
                i += 2;
            }
            _ => {
                out.push(words[i].to_owned());
                i += 1;
            }
        }
    }
    out.join(" ")
}

fn unambiguous_extension(ext: &str) -> bool {
    let ext = ext.to_lowercase();
    EXTENSIONS.contains(&ext.as_str()) && !AMBIGUOUS.contains(&ext.as_str())
}

/// "md." → ("md", ".").
fn split_trailing(word: &str) -> (&str, &str) {
    let end = word
        .char_indices()
        .find(|(_, c)| !c.is_alphanumeric())
        .map_or(word.len(), |(i, _)| i);
    word.split_at(end)
}

/// The filenames in screen text: `PRODUCT-DESIGN.md`, and a path's last part.
pub fn terms(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut read = 0;
    for text in texts {
        read += text.len();
        if read > MAX_SCREEN_CHARS {
            break;
        }
        for raw in text.split(|c: char| !(c.is_alphanumeric() || "._-/".contains(c))) {
            let word = raw.trim_matches(|c: char| !c.is_alphanumeric());
            let name = word.rsplit('/').next().unwrap_or(word);
            if filename(name) && !out.iter().any(|t| t == name) {
                out.push(name.to_owned());
                if out.len() >= MAX_TERMS {
                    return out;
                }
            }
        }
    }
    out
}

/// A stem of two or more letters or digits, a dot, and an unambiguous known extension.
fn filename(word: &str) -> bool {
    word.chars().count() <= 80
        && word.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))
        && word.rsplit_once('.').is_some_and(|(stem, ext)| {
            stem.chars().filter(|c| c.is_alphanumeric()).count() >= 2 && unambiguous_extension(ext)
        })
}

/// `Node.js`, `Vue.js`: a capitalised name with `.js` is a product, not a file.
fn brand_js(word: &str) -> bool {
    word.rsplit_once('.').is_some_and(|(stem, ext)| {
        ext.eq_ignore_ascii_case("js")
            && stem.chars().next().is_some_and(char::is_uppercase)
            && stem.chars().skip(1).all(char::is_lowercase)
    })
}

/// `snake_case`: parts of two or more letters or digits, one of them three or more.
fn snake_case(word: &str) -> bool {
    let parts: Vec<&str> = word.split('_').collect();
    parts.len() >= 2
        && parts.iter().all(|p| p.chars().count() >= 2 && p.chars().all(char::is_alphanumeric))
        && parts.iter().any(|p| p.chars().count() >= 3)
}

/// Writes spoken filenames that match one on screen as it is there, and puts filenames and
/// `snake_case` in backticks when `backticks` is on. Text already in backticks is left alone.
pub fn apply(text: &str, terms: &[String], backticks: bool) -> String {
    let mut tokens = tokenize(text);
    let keys: HashMap<String, &str> = terms.iter().map(|t| (key(t, false), t.as_str())).collect();
    let mut i = 0;
    while i < tokens.len() {
        if let Some((end, term)) = longest_match(&tokens, i, &keys) {
            tokens.splice(i..=end, [term.to_owned()]);
        }
        i += 1;
    }
    let text = tokens.concat();
    if backticks { backtick_code(&text) } else { text }
}

/// Words, separators that may sit inside a filename (`.`, `-`, `_`, `/`), and everything else
/// one character at a time.
fn tokenize(text: &str) -> Vec<String> {
    let wordy = |c: char| c.is_alphanumeric() || c == '\'' || c == '’';
    let mut tokens: Vec<String> = Vec::new();
    for ch in text.chars() {
        match tokens.last_mut() {
            Some(last) if wordy(ch) && last.chars().next().is_some_and(wordy) => last.push(ch),
            _ => tokens.push(ch.to_string()),
        }
    }
    tokens
}

fn is_word(token: &str) -> bool {
    token.chars().next().is_some_and(char::is_alphanumeric)
}

fn is_joiner(token: &str) -> bool {
    matches!(token, " " | "." | "-" | "_" | "/")
}

/// The longest run of words from `start` that a screen filename stands for, as its last token's
/// index and the filename. The run must end in "." and an extension, as a spoken "dot" leaves it,
/// so plain words ("note text" with `note.txt` on screen) never match.
fn longest_match<'a>(
    tokens: &[String],
    start: usize,
    keys: &HashMap<String, &'a str>,
) -> Option<(usize, &'a str)> {
    if !is_word(&tokens[start]) {
        return None;
    }
    let mut ends = Vec::new();
    let mut j = start;
    let mut words = 1;
    while words < MAX_SPAN_WORDS && j + 2 < tokens.len() {
        if !is_joiner(&tokens[j + 1]) || !is_word(&tokens[j + 2]) {
            break;
        }
        j += 2;
        words += 1;
        if tokens[j - 1] == "." {
            ends.push(j);
        }
    }
    ends.into_iter().rev().find_map(|end| {
        let words: Vec<&String> = tokens[start..=end].iter().filter(|t| is_word(t)).collect();
        let (last, stem) = words.split_last()?;
        // The extension keeps its suffix, so "README.md's broken" never loses its "'s".
        let tail = key(last, false);
        [false, true].iter().find_map(|&strip| {
            let spoken: String = stem.iter().map(|w| key(w, strip)).chain([tail.clone()]).collect();
            keys.get(&spoken).map(|term| (end, *term))
        })
    })
}

/// Lower-case letters and digits only; with `strip`, a possessive "'s" comes off first, since
/// "product's design" is how `PRODUCT-DESIGN` sounds.
fn key(word: &str, strip: bool) -> String {
    let word = if strip {
        ["'s", "’s", "s'", "s’"].iter().find_map(|s| word.strip_suffix(s)).unwrap_or(word)
    } else {
        word
    };
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Wraps each filename and `snake_case` word in backticks, outside text already in backticks.
fn backtick_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (i, part) in text.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        if i % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let words: Vec<String> = part
            .split(' ')
            .map(|word| {
                let (lead, core, rest) = split_code_punctuation(word);
                if (filename(core) && !brand_js(core)) || snake_case(core) {
                    format!("{lead}`{core}`{rest}")
                } else {
                    word.to_owned()
                }
            })
            .collect();
        out.push_str(&words.join(" "));
    }
    out
}

/// "(design.md)," → ("(", "design.md", "),"): a sentence's punctuation and quotes stay outside
/// the backticks.
fn split_code_punctuation(word: &str) -> (&str, &str, &str) {
    let core = word.trim_start_matches(|c: char| "(\"'“‘[".contains(c));
    let lead = &word[..word.len() - core.len()];
    let end = core.trim_end_matches(|c: char| ",.;:!?)\"'”’]".contains(c)).len();
    let (core, rest) = core.split_at(end);
    (lead, core, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms_of(text: &str) -> Vec<String> {
        terms(&[text.to_owned()])
    }

    fn dictate(spoken: &str, screen: &str) -> String {
        apply(&spoken_extensions(spoken), &terms_of(screen), true)
    }

    #[test]
    fn a_spoken_dot_joins_a_known_extension() {
        assert_eq!(spoken_extensions("open design dot MD."), "open design.md.");
        assert_eq!(spoken_extensions("see main dot rs and lib dot rs"), "see main.rs and lib.rs");
        for prose in [
            "connect the dot later",
            "dot md",
            "open the dot md file",
            "be there on the dot go home",
            "polka dot sh and Jay dot go",
            "three dot five",
            "Tell Dot JSON is fine",
        ] {
            assert_eq!(spoken_extensions(prose), prose);
        }
    }

    #[test]
    fn screen_text_gives_filenames_only() {
        let found = terms_of(
            "Read PRODUCT-DESIGN.md and src/dictation/session.rs, then call press_shortcut or \
             signIn. Washington D.C., main.go, Node FOLLOW-UP.",
        );
        assert_eq!(found, ["PRODUCT-DESIGN.md", "session.rs"]);
    }

    #[test]
    fn kais_spoken_filename_is_written_as_it_is_on_screen() {
        let screen = "Follow PRODUCT-DESIGN.md for the pill.";
        let spoken = spoken_extensions("Update the product's design dot MD.");
        assert_eq!(apply(&spoken, &terms_of(screen), false), "Update the PRODUCT-DESIGN.md.");
        assert_eq!(dictate("Update the product's design dot MD.", screen), "Update the `PRODUCT-DESIGN.md`.");
        assert_eq!(dictate("Update the product’s design dot MD.", screen), "Update the `PRODUCT-DESIGN.md`.");
    }

    #[test]
    fn prose_beside_a_code_screen_is_left_alone() {
        let screen = "const signIn = () => isLoading; checkOut(user_name, first_name, sign_in); \
                      note.txt FOLLOW-UP SIGN-UP NON-PROFIT PowerPoint SharePoint main.go";
        for prose in [
            "Users can sign in with Google while the page is loading.",
            "Check out the docs and check our plan.",
            "What is your first name and user names? Please sign in.",
            "Can you follow up after the sign up for the non profit?",
            "Write a note text to me, and check go later.",
            "Send the power points and share points.",
        ] {
            assert_eq!(dictate(prose, screen), prose);
        }
    }

    #[test]
    fn a_suffix_after_a_filename_stays_and_runs_stop_at_six_words() {
        let screen = "README.md please-update-the-big-old-scrawl-mac-os-notes.md";
        assert_eq!(
            dictate("The read me dot md's fine.", screen),
            "The read me.md's fine."
        );
        assert_eq!(dictate("open read me dot md now", screen), "open `README.md` now");
        let long = "please update the big old scrawl mac os notes dot md";
        assert_eq!(dictate(long, screen), "please update the big old scrawl mac os `notes.md`");
    }

    #[test]
    fn names_brands_abbreviations_and_hyphenated_prose_get_no_backticks() {
        let text = "Kai deSilva met Neil deGrasse Tyson in Washington D.C. and N.C. The A.C. is \
                    broken at 120 mmHg. I love Node.js and Vue.js. I got a new iPhone, watched YouTube on macOS, and use \
                    useState. Self-driving cars, a 24-hour turnaround, a T-shirt and COVID-19.";
        assert_eq!(apply(text, &[], true), text);
    }

    #[test]
    fn filenames_and_snake_case_get_backticks_with_punctuation_outside() {
        assert_eq!(apply("edit main.rs, then `lib.rs`.", &[], true), "edit `main.rs`, then `lib.rs`.");
        assert_eq!(apply("edit (main.rs) and \"lib.rs\", now", &[], true), "edit (`main.rs`) and \"`lib.rs`\", now");
        assert_eq!(apply("call press_shortcut", &[], true), "call `press_shortcut`");
        assert_eq!(apply("edit main.rs", &[], false), "edit main.rs");
    }
}
