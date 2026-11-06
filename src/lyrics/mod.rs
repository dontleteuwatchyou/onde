//! Lyrics model and timing. Parsing is in `lrc`/`ttml`, loading in `source`.

mod lrc;
pub mod source;
mod ttml;

use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// Includes its trailing space, so a line's words concatenate to its text.
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// Empty when the source only has line timing.
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lyrics {
    Synced(Vec<Line>),
    Plain(Vec<String>),
    Instrumental,
}

/// Recognises TTML or LRC; text without timestamps becomes plain lyrics.
pub fn parse(content: &str) -> Option<Lyrics> {
    let content = content.trim_start_matches('\u{feff}').trim();
    if content.is_empty() {
        return None;
    }
    if content.starts_with('<') {
        return ttml::parse(content).map(Lyrics::Synced);
    }
    if let Some(lines) = lrc::parse(content) {
        return Some(Lyrics::Synced(lines));
    }
    Some(Lyrics::Plain(content.lines().map(str::to_owned).collect()))
}

/// The line being sung: the last one that has started.
pub fn active_line(lines: &[Line], ms: u64) -> Option<usize> {
    lines
        .partition_point(|line| line.start_ms <= ms)
        .checked_sub(1)
}

/// How much of a word has been sung, from 0 to 1.
pub fn word_fill(word: &Word, ms: u64) -> f32 {
    if ms <= word.start_ms {
        0.0
    } else if ms >= word.end_ms {
        1.0
    } else {
        (ms - word.start_ms) as f32 / (word.end_ms - word.start_ms) as f32
    }
}

/// Byte ranges of each word in the line's text, with how sung they are.
pub fn line_fills(line: &Line, ms: u64) -> Vec<(Range<usize>, f32)> {
    let mut start = 0;
    line.words
        .iter()
        .map(|word| {
            let range = start..start + word.text.len();
            start = range.end;
            (range, word_fill(word, ms))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start_ms: u64) -> Line {
        Line {
            start_ms,
            end_ms: start_ms + 1000,
            text: String::new(),
            words: Vec::new(),
        }
    }

    #[test]
    fn active_line_is_the_last_one_started() {
        let lines = [line(1000), line(2000), line(3000)];
        assert_eq!(active_line(&lines, 500), None);
        assert_eq!(active_line(&lines, 1000), Some(0));
        assert_eq!(active_line(&lines, 2500), Some(1));
        assert_eq!(active_line(&lines, 99_000), Some(2));
    }

    #[test]
    fn word_fill_goes_from_zero_to_one() {
        let word = Word {
            text: "a".into(),
            start_ms: 100,
            end_ms: 300,
        };
        assert_eq!(word_fill(&word, 50), 0.0);
        assert_eq!(word_fill(&word, 200), 0.5);
        assert_eq!(word_fill(&word, 400), 1.0);
    }

    #[test]
    fn a_word_without_duration_does_not_divide_by_zero() {
        let word = Word {
            text: "a".into(),
            start_ms: 100,
            end_ms: 100,
        };
        assert_eq!(word_fill(&word, 100), 0.0);
        assert_eq!(word_fill(&word, 101), 1.0);
    }

    #[test]
    fn fills_use_byte_ranges_for_accented_text() {
        let word = |text: &str, start_ms, end_ms| Word {
            text: text.into(),
            start_ms,
            end_ms,
        };
        let line = Line {
            start_ms: 0,
            end_ms: 200,
            text: "été là".into(),
            words: vec![word("été ", 0, 100), word("là", 100, 200)],
        };
        let fills = line_fills(&line, 150);
        assert_eq!(fills[0], (0..6, 1.0));
        assert_eq!(fills[1], (6..9, 0.5));
        assert!(line.text.is_char_boundary(6));
    }

    #[test]
    fn parse_detects_each_kind() {
        assert!(matches!(parse("[00:01.00]Salut"), Some(Lyrics::Synced(_))));
        assert!(matches!(parse("Juste\ndu texte"), Some(Lyrics::Plain(_))));
        assert_eq!(parse("  \n "), None);
        assert!(matches!(
            parse(
                r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="1s" end="2s">Salut</p></div></body></tt>"#
            ),
            Some(Lyrics::Synced(_))
        ));
    }
}
