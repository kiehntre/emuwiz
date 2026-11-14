use super::*;

#[test]
fn sanitise_replaces_characters_that_could_repaint_or_hide_text() {
    let hostile = "ok\x1b[2J\x07\u{202E}evil\u{200B}\u{2028}end";
    let clean = sanitise(hostile, 100);
    assert!(!clean.chars().any(is_unsafe), "{clean:?}");
    assert!(clean.starts_with("ok"));
    assert!(clean.ends_with("end"));
    assert_eq!(clean.matches(char::REPLACEMENT_CHARACTER).count(), 5);
}

#[test]
fn sanitise_caps_length_and_marks_the_cut() {
    let clean = sanitise(&"a".repeat(1_000), 10);
    assert_eq!(clean.chars().count(), 11);
    assert!(clean.ends_with('\u{2026}'));
    assert_eq!(sanitise("short", 10), "short");
}

#[test]
fn examine_numbers_lines_and_drops_carriage_returns() {
    let examined = examine(b"first\r\nsecond\n\nfourth");
    let lines: Vec<(u32, &str)> = examined
        .lines()
        .map(|line| (line.number, line.text))
        .collect();
    assert_eq!(lines, [(1, "first"), (2, "second"), (3, ""), (4, "fourth")]);
    assert!(!examined.truncated);
    assert_eq!(examined.lines_ignored, 0);
}

#[test]
fn examine_skips_an_over_long_line_instead_of_matching_its_start() {
    let mut input = b"keep\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', LOG_LINE_LIMIT + 1));
    input.extend(b"\nalso keep\n");
    let examined = examine(&input);
    let lines: Vec<(u32, &str)> = examined
        .lines()
        .map(|line| (line.number, line.text))
        .collect();
    assert_eq!(lines, [(1, "keep"), (3, "also keep")]);
    assert_eq!(examined.lines_ignored, 1);
}

#[test]
fn examine_stops_at_the_byte_limit_and_never_keeps_a_partial_line() {
    let line = b"0123456789abcdef\n";
    let mut input = Vec::new();
    while input.len() <= LOG_BYTES_LIMIT + 1_000 {
        input.extend(line);
    }
    let examined = examine(&input);
    assert!(examined.truncated);
    assert!(examined.bytes_examined as usize <= LOG_BYTES_LIMIT);
    assert!(examined.lines().all(|line| line.text == "0123456789abcdef"));
}

#[test]
fn examine_stops_at_the_line_limit() {
    let input = "x\n".repeat(LOG_LINES_LIMIT + 5);
    let examined = examine(input.as_bytes());
    assert!(examined.truncated);
    assert_eq!(examined.line_count() as usize, LOG_LINES_LIMIT);
}

#[test]
fn examine_accepts_bytes_that_are_not_text() {
    let examined = examine(b"good\n\xff\xfe\x00bad\nlast\n");
    let lines: Vec<&str> = examined.lines().map(|line| line.text).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "good");
    assert_eq!(lines[2], "last");
}

#[test]
fn examine_of_nothing_is_empty_and_complete() {
    let examined = examine(b"");
    assert_eq!(examined.line_count(), 0);
    assert!(!examined.truncated);
}

#[test]
fn quoted_after_takes_the_first_quoted_string_after_the_prefix() {
    assert_eq!(quoted_after("key: \"value\" tail", "key: "), Some("value"));
    assert_eq!(quoted_after("key: \"a\" \"b\"", "key: "), Some("a"));
    assert_eq!(quoted_after("other: \"value\"", "key: "), None);
    assert_eq!(quoted_after("key: \"unterminated", "key: "), None);
    assert_eq!(quoted_after(" key: \"value\"", "key: "), None);
}
