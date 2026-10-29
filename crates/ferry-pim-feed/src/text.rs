//! The text rules iCalendar (RFC 5545) and vCard (RFC 6350) share: a `TEXT` value is escaped and
//! a content line is folded. Pure, and byte for byte what the mirror has always written: it
//! hashes each file, so a change here re-fetches every item.

const CRLF: &str = "\r\n";

/// A `TEXT` value: backslash, semicolon, comma and newlines escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' => {}
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// A content line folded at 75 octets, never inside a character, and ended with CRLF.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0;
    let mut limit = 75;
    for c in line.chars() {
        let n = c.len_utf8();
        if width + n > limit && width > 0 {
            out.push_str(CRLF);
            out.push(' ');
            width = 0;
            limit = 74;
        }
        out.push(c);
        width += n;
    }
    out.push_str(CRLF);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_value_escapes_its_separators_and_drops_carriage_returns() {
        assert_eq!(escape("a\\b;c,d\r\ne\nf"), "a\\\\b\\;c\\,d\\ne\\nf");
    }

    #[test]
    fn a_long_line_is_folded_inside_75_octets_and_never_inside_a_character() {
        let line = format!("SUMMARY:{}", "é".repeat(100));
        let folded = fold(&line);
        for part in folded.split("\r\n") {
            assert!(part.len() <= 75, "{} octets: {part}", part.len());
        }
        assert_eq!(folded.replace("\r\n ", ""), format!("{line}\r\n"));
        assert_eq!(fold("SHORT:x"), "SHORT:x\r\n");
    }
}
