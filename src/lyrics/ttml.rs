//! TTML lyrics: `<p>` for lines, timed `<span>` for words or syllables.

use roxmltree::{Document, Node};

use super::{Line, Word};

const TTML_METADATA: &str = "http://www.w3.org/ns/ttml#metadata";

pub fn parse(content: &str) -> Option<Vec<Line>> {
    let document = Document::parse(content).ok()?;
    let mut lines: Vec<Line> = document
        .descendants()
        .filter(|node| node.has_tag_name("p") || node.tag_name().name() == "p")
        .filter_map(parse_paragraph)
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines.sort_by_key(|line| line.start_ms);
    Some(lines)
}

fn parse_paragraph(paragraph: Node) -> Option<Line> {
    let mut words = Vec::new();
    let mut loose = String::new();
    collect(paragraph, &mut words, &mut loose);
    if let Some(last) = words.last_mut() {
        last.text.truncate(last.text.trim_end().len());
    }
    let begin = paragraph.attribute("begin").and_then(parse_time);
    let end = paragraph.attribute("end").and_then(parse_time);
    let start_ms = begin.or_else(|| words.first().map(|w: &Word| w.start_ms))?;
    let end_ms = end
        .or_else(|| words.last().map(|w| w.end_ms))
        .unwrap_or(start_ms);
    let text = if words.is_empty() {
        loose.trim().to_owned()
    } else {
        words.iter().map(|w| w.text.as_str()).collect()
    };
    Some(Line {
        start_ms,
        end_ms,
        text,
        words,
    })
}

fn collect(parent: Node, words: &mut Vec<Word>, loose: &mut String) {
    for child in parent.children() {
        if child.is_text() {
            let text = child.text().unwrap_or_default();
            if let Some(last) = words.last_mut() {
                // A space after a span separates words; syllables have none.
                if text.chars().any(char::is_whitespace) {
                    last.text.push(' ');
                }
            } else {
                loose.push_str(text);
            }
        } else if child.is_element() {
            if is_translation(child) {
                continue;
            }
            let timing = child
                .attribute("begin")
                .and_then(parse_time)
                .zip(child.attribute("end").and_then(parse_time));
            let has_elements = child.children().any(|node| node.is_element());
            match timing {
                Some((start_ms, end_ms)) if !has_elements => {
                    let text: String = child.children().filter_map(|node| node.text()).collect();
                    if !text.is_empty() {
                        words.push(Word {
                            text,
                            start_ms,
                            end_ms,
                        });
                    }
                }
                _ => collect(child, words, loose),
            }
        }
    }
}

fn is_translation(node: Node) -> bool {
    matches!(
        node.attribute((TTML_METADATA, "role")),
        Some("x-translation" | "x-roman")
    )
}

/// Accepts `hh:mm:ss.fff`, `mm:ss.fff`, `ss.fff` and `12s`, `1500ms`, `2m`, `1h`.
fn parse_time(value: &str) -> Option<u64> {
    let value = value.trim();
    let seconds = |text: &str| text.parse::<f64>().ok().filter(|n| *n >= 0.0);
    let ms = if value.contains(':') {
        let mut total = 0.0;
        for part in value.split(':') {
            total = total * 60.0 + seconds(part)?;
        }
        total * 1000.0
    } else if let Some(number) = value.strip_suffix("ms") {
        seconds(number)?
    } else if let Some(number) = value.strip_suffix('s') {
        seconds(number)? * 1000.0
    } else if let Some(number) = value.strip_suffix('m') {
        seconds(number)? * 60_000.0
    } else if let Some(number) = value.strip_suffix('h') {
        seconds(number)? * 3_600_000.0
    } else {
        seconds(value)? * 1000.0
    };
    Some(ms.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(body: &str) -> String {
        format!(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body><div>{body}</div></body></tt>"#
        )
    }

    #[test]
    fn parses_a_line_without_spans() {
        let lines = parse(&wrap(r#"<p begin="1.5s" end="3s">Bonjour</p>"#)).unwrap();
        assert_eq!(lines[0].text, "Bonjour");
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (1500, 3000));
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn spaces_between_spans_separate_words() {
        let lines = parse(&wrap(
            r#"<p begin="00:01.000" end="00:02.000"><span begin="00:01.000" end="00:01.500">Bon</span> <span begin="00:01.500" end="00:02.000">jour</span></p>"#,
        ))
        .unwrap();
        assert_eq!(lines[0].text, "Bon jour");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[1].start_ms, 1500);
    }

    #[test]
    fn syllables_without_spaces_stay_joined() {
        let lines = parse(&wrap(
            r#"<p begin="1s" end="2s"><span begin="1s" end="1.5s">chan</span><span begin="1.5s" end="2s">son</span></p>"#,
        ))
        .unwrap();
        assert_eq!(lines[0].text, "chanson");
        assert_eq!(lines[0].words.len(), 2);
    }

    #[test]
    fn skips_translations_and_keeps_background_vocals() {
        let lines = parse(&wrap(
            r#"<p begin="1s" end="3s"><span begin="1s" end="2s">Hello</span> <span ttm:role="x-translation">Bonjour</span><span ttm:role="x-bg"><span begin="2s" end="3s">oh</span></span></p>"#,
        ))
        .unwrap();
        assert_eq!(lines[0].text, "Hello oh");
    }

    #[test]
    fn takes_line_timing_from_words_when_missing() {
        let lines = parse(&wrap(r#"<p><span begin="4s" end="5s">Fin</span></p>"#)).unwrap();
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (4000, 5000));
    }

    #[test]
    fn sorts_lines_and_rejects_invalid_documents() {
        let lines = parse(&wrap(
            r#"<p begin="5s" end="6s">B</p><p begin="1s" end="2s">A</p>"#,
        ))
        .unwrap();
        assert_eq!(lines[0].text, "A");
        assert_eq!(parse("<tt><body"), None);
        assert_eq!(parse(&wrap("")), None);
    }

    #[test]
    fn parses_time_formats() {
        assert_eq!(parse_time("01:02:03.5"), Some(3_723_500));
        assert_eq!(parse_time("02:03.25"), Some(123_250));
        assert_eq!(parse_time("12.5"), Some(12_500));
        assert_eq!(parse_time("12.5s"), Some(12_500));
        assert_eq!(parse_time("1500ms"), Some(1_500));
        assert_eq!(parse_time("2m"), Some(120_000));
        assert_eq!(parse_time("n'importe"), None);
    }
}
