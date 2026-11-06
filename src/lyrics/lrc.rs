//! LRC, including the enhanced form with `<mm:ss.xx>` word timestamps.

use super::{Line, Word};

/// How long the last line stays timed when nothing follows it.
const LAST_LINE_MS: u64 = 5_000;

/// Returns `None` when the text has no timestamped line.
pub fn parse(content: &str) -> Option<Vec<Line>> {
    let mut offset = 0_i64;
    let mut lines = Vec::new();
    for raw in content.lines() {
        let mut rest = raw.trim();
        let mut starts = Vec::new();
        while let Some(tag) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let (inner, after) = tag;
            match parse_time(inner) {
                Some(ms) => starts.push(ms),
                None => {
                    if let Some(value) = inner.strip_prefix("offset:") {
                        offset = value.trim().parse().unwrap_or(offset);
                    }
                }
            }
            rest = after;
        }
        for start in starts {
            lines.push(parse_body(rest.trim(), start));
        }
    }
    if lines.is_empty() {
        return None;
    }
    // A positive offset means the lyrics come earlier.
    let shift = |ms: u64| ms.saturating_add_signed(-offset);
    for line in &mut lines {
        line.start_ms = shift(line.start_ms);
        for word in &mut line.words {
            word.start_ms = shift(word.start_ms);
            word.end_ms = word.end_ms.checked_add_signed(-offset).unwrap_or(0);
        }
    }
    lines.sort_by_key(|line| line.start_ms);
    let starts: Vec<u64> = lines.iter().map(|line| line.start_ms).collect();
    for (index, line) in lines.iter_mut().enumerate() {
        let end = starts
            .get(index + 1)
            .copied()
            .unwrap_or(line.start_ms + LAST_LINE_MS);
        line.end_ms = end;
        if let Some(last) = line.words.last_mut()
            && last.end_ms == OPEN_END
        {
            last.end_ms = end;
        }
    }
    Some(lines)
}

/// Marks a final word whose end is the next line's start, not yet known.
const OPEN_END: u64 = u64::MAX;

fn parse_body(body: &str, line_start: u64) -> Line {
    let mut words: Vec<Word> = Vec::new();
    let mut text = String::new();
    let mut rest = body;
    let mut start = line_start;
    loop {
        let tag = rest.find('<').and_then(|open| {
            let close = open + rest[open..].find('>')?;
            Some((open, close, parse_time(&rest[open + 1..close])?))
        });
        let Some((open, close, time)) = tag else {
            break;
        };
        push_word(&mut words, &mut text, &rest[..open], start, time);
        start = time;
        rest = &rest[close + 1..];
    }
    if words.is_empty() {
        // No word timing at all: `rest` is the whole body.
        return Line {
            start_ms: line_start,
            end_ms: 0,
            text: rest.to_owned(),
            words,
        };
    }
    push_word(&mut words, &mut text, rest, start, OPEN_END);
    Line {
        start_ms: line_start,
        end_ms: 0,
        text,
        words,
    }
}

fn push_word(words: &mut Vec<Word>, text: &mut String, raw: &str, start_ms: u64, end_ms: u64) {
    if raw.is_empty() {
        return;
    }
    text.push_str(raw);
    if raw.trim().is_empty() {
        // Spaces between tags belong to the previous word.
        if let Some(previous) = words.last_mut() {
            previous.text.push_str(raw);
        } else {
            text.truncate(text.len() - raw.len());
        }
        return;
    }
    words.push(Word {
        text: raw.to_owned(),
        start_ms,
        end_ms,
    });
}

/// Accepts `mm:ss`, `mm:ss.xx`, `mm:ss.xxx` and `mm:ss:xx`.
fn parse_time(value: &str) -> Option<u64> {
    let (minutes, rest) = value.split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let seconds: u64 = seconds.parse().ok()?;
    if seconds >= 60 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut ms = 0;
    for (index, digit) in fraction.bytes().take(3).enumerate() {
        ms += u64::from(digit - b'0') * [100, 10, 1][index];
    }
    Some((minutes * 60 + seconds) * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_timed_lyrics_and_ignores_metadata() {
        let lines = parse("[ar:Quelqu'un]\n[00:01.50]Première\n[00:03.00]Deuxième").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_ms, 1500);
        assert_eq!(lines[0].end_ms, 3000);
        assert_eq!(lines[0].text, "Première");
        assert!(lines[0].words.is_empty());
        assert_eq!(lines[1].end_ms, 8000);
    }

    #[test]
    fn repeats_a_line_with_several_timestamps_in_order() {
        let lines = parse("[00:05.00]B\n[00:01.00][00:03.00]A").unwrap();
        let order: Vec<_> = lines
            .iter()
            .map(|l| (l.start_ms, l.text.as_str()))
            .collect();
        assert_eq!(order, [(1000, "A"), (3000, "A"), (5000, "B")]);
    }

    #[test]
    fn parses_word_timestamps() {
        let lines =
            parse("[00:01.00]<00:01.00>Bon <00:01.50>jour<00:02.00>\n[00:04.00]Fin").unwrap();
        let words = &lines[0].words;
        assert_eq!(lines[0].text, "Bon jour");
        assert_eq!(words.len(), 2);
        assert_eq!((words[0].start_ms, words[0].end_ms), (1000, 1500));
        assert_eq!((words[1].start_ms, words[1].end_ms), (1500, 2000));
    }

    #[test]
    fn a_last_word_without_closing_tag_ends_with_the_line() {
        let lines = parse("[00:01.00]<00:01.00>Bon <00:01.50>jour\n[00:04.00]Fin").unwrap();
        assert_eq!(lines[0].words[1].end_ms, 4000);
    }

    #[test]
    fn spaces_between_tags_join_the_previous_word() {
        let lines = parse("[00:01.00]<00:01.00>Bon<00:01.50> <00:01.60>jour<00:02.00>").unwrap();
        assert_eq!(lines[0].text, "Bon jour");
        assert_eq!(lines[0].words[0].text, "Bon ");
        assert_eq!(lines[0].words.len(), 2);
    }

    #[test]
    fn keeps_angle_brackets_that_are_not_timestamps() {
        let lines = parse("[00:01.00]a <b> c").unwrap();
        assert_eq!(lines[0].text, "a <b> c");
    }

    #[test]
    fn applies_the_offset() {
        let lines = parse("[offset:500]\n[00:02.00]A").unwrap();
        assert_eq!(lines[0].start_ms, 1500);
    }

    #[test]
    fn a_negative_offset_delays_the_lyrics() {
        let lines = parse("[offset:-500]\n[00:02.00]A").unwrap();
        assert_eq!(lines[0].start_ms, 2500);
    }

    #[test]
    fn text_without_timestamps_is_not_lrc() {
        assert_eq!(parse("Une chanson\n[Refrain]\nsans minutage"), None);
    }

    #[test]
    fn parses_time_formats() {
        assert_eq!(parse_time("01:02"), Some(62_000));
        assert_eq!(parse_time("01:02.5"), Some(62_500));
        assert_eq!(parse_time("01:02.34"), Some(62_340));
        assert_eq!(parse_time("01:02.345"), Some(62_345));
        assert_eq!(parse_time("01:02:34"), Some(62_340));
        assert_eq!(parse_time("ar:Nom"), None);
        assert_eq!(parse_time("01:75"), None);
    }
}
