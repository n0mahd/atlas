//! A small JSONC reader and in-place editor, for `OpenCode`'s config files.
//!
//! `OpenCode` reads `opencode.json` and `opencode.jsonc` with `jsonc-parser`, allowing `//`
//! and `/* */` comments and trailing commas. Atlas must keep the owner's comments, so it
//! doesn't re-serialise the file: it parses it into a tree of byte spans and splices only
//! the members it adds or removes. Everything else stays byte for byte.
//!
//! Bounded: one pass over the text, nesting capped at [`MAX_DEPTH`].

use crate::FormatError;

/// Deeper nesting than this is refused, so a hostile file can't exhaust the stack.
const MAX_DEPTH: usize = 128;

/// A parsed value and where it sits in the text.
#[derive(Debug)]
pub(super) struct Value {
    pub end: usize,
    pub kind: Kind,
}

#[derive(Debug)]
pub(super) enum Kind {
    Object(Object),
    String(String),
    /// Arrays, numbers, `true`, `false`, `null`: never edited, only stepped over.
    Other,
}

#[derive(Debug)]
pub(super) struct Object {
    /// Offset of `{`.
    pub open: usize,
    /// Offset of `}`.
    pub close: usize,
    pub members: Vec<Member>,
}

#[derive(Debug)]
pub(super) struct Member {
    pub key: String,
    /// Offset of the key's opening quote.
    pub start: usize,
    pub value: Value,
    /// Offset of the comma after the value, if there is one.
    pub comma: Option<usize>,
    /// A `//` comment on the same line straight after the value (and its comma).
    pub note: Option<(usize, usize)>,
}

impl Value {
    pub fn as_object(&self) -> Option<&Object> {
        match &self.kind {
            Kind::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            Kind::String(s) => Some(s),
            _ => None,
        }
    }
}

impl Object {
    /// The member a reader would see for `key`: the last one, as `jsonc-parser` keeps.
    pub fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| m.key == key)
    }

    pub fn position(&self, key: &str) -> Option<usize> {
        self.members.iter().rposition(|m| m.key == key)
    }

    /// No members and nothing but whitespace between the braces.
    pub fn is_bare(&self, text: &str) -> bool {
        self.members.is_empty() && text[self.open + 1..self.close].trim().is_empty()
    }
}

impl Member {
    /// Where the member's own text ends: after its value, comma and note.
    fn end(&self) -> usize {
        let mut end = self.value.end;
        if let Some(c) = self.comma {
            end = end.max(c + 1);
        }
        if let Some((_, e)) = self.note {
            end = end.max(e);
        }
        end
    }

    pub fn note<'t>(&self, text: &'t str) -> Option<&'t str> {
        self.note.map(|(s, e)| text[s..e].trim_end())
    }
}

/// Parses JSONC text whose top level is any value.
///
/// # Errors
/// The text isn't JSONC, or nests deeper than [`MAX_DEPTH`].
pub(super) fn parse(text: &str) -> Result<Value, FormatError> {
    let mut p = Parser {
        text,
        b: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    p.trivia()?;
    let v = p.value()?;
    p.trivia()?;
    if p.i != p.b.len() {
        return Err(p.error("the end of the file"));
    }
    Ok(v)
}

struct Parser<'t> {
    text: &'t str,
    b: &'t [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, expected: &'static str) -> FormatError {
        let before = &self.text[..floor_char(self.text, self.i)];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        FormatError::InvalidJsonc {
            line,
            column,
            expected,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Skips whitespace and comments.
    fn trivia(&mut self) -> Result<(), FormatError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.i += 1,
                Some(b'/') if self.b.get(self.i + 1) == Some(&b'/') => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.i += 1;
                    }
                }
                Some(b'/') if self.b.get(self.i + 1) == Some(&b'*') => {
                    let from = self.i + 2;
                    match self.text[from..].find("*/") {
                        Some(at) => self.i = from + at + 2,
                        None => return Err(self.error("the end of a /* comment */")),
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self) -> Result<Value, FormatError> {
        let kind = match self.peek() {
            Some(b'{') => Kind::Object(self.object()?),
            Some(b'[') => {
                self.array()?;
                Kind::Other
            }
            Some(b'"') => Kind::String(self.string()?),
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Kind::Other
            }
            Some(b't') => self.literal("true")?,
            Some(b'f') => self.literal("false")?,
            Some(b'n') => self.literal("null")?,
            _ => return Err(self.error("a value")),
        };
        Ok(Value { end: self.i, kind })
    }

    fn enter(&mut self) -> Result<(), FormatError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("nesting no deeper than 128"));
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Object, FormatError> {
        self.enter()?;
        let open = self.i;
        self.i += 1;
        let mut members = Vec::new();
        loop {
            self.trivia()?;
            match self.peek() {
                Some(b'}') => break,
                Some(b'"') => {}
                _ => return Err(self.error("a key in quotes or '}'")),
            }
            let start = self.i;
            let key = self.string()?;
            self.trivia()?;
            if self.peek() != Some(b':') {
                return Err(self.error("':'"));
            }
            self.i += 1;
            self.trivia()?;
            let value = self.value()?;
            let after = self.i;
            self.trivia()?;
            let comma = (self.peek() == Some(b',')).then_some(self.i);
            let note = same_line_note(self.text, after);
            members.push(Member {
                key,
                start,
                value,
                comma,
                note,
            });
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => break,
                _ => return Err(self.error("',' or '}'")),
            }
        }
        let close = self.i;
        self.i += 1;
        self.depth -= 1;
        Ok(Object {
            open,
            close,
            members,
        })
    }

    fn array(&mut self) -> Result<(), FormatError> {
        self.enter()?;
        self.i += 1;
        loop {
            self.trivia()?;
            if self.peek() == Some(b']') {
                break;
            }
            self.value()?;
            self.trivia()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => break,
                _ => return Err(self.error("',' or ']'")),
            }
        }
        self.i += 1;
        self.depth -= 1;
        Ok(())
    }

    fn literal(&mut self, word: &'static str) -> Result<Kind, FormatError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(Kind::Other)
        } else {
            Err(self.error("a value"))
        }
    }

    fn digits(&mut self) -> usize {
        let from = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - from
    }

    fn number(&mut self) -> Result<(), FormatError> {
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("a digit")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.error("a digit"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("a digit"));
            }
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, FormatError> {
        let digits = self
            .text
            .get(self.i..self.i + 4)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("four hex digits"))?;
        self.i += 4;
        u32::from_str_radix(digits, 16).map_err(|_| self.error("four hex digits"))
    }

    fn string(&mut self) -> Result<String, FormatError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let run = self.i;
            while !matches!(self.peek(), None | Some(b'"' | b'\\' | 0..0x20)) {
                self.i += 1;
            }
            out.push_str(&self.text[run..self.i]);
            match self.peek() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.i += 1;
                            let c = self.unicode_escape()?;
                            out.push(c);
                            continue;
                        }
                        _ => return Err(self.error("an escape such as \\n")),
                    };
                    self.i += 1;
                    out.push(c);
                }
                _ => return Err(self.error("the end of the string")),
            }
        }
    }

    /// After `\u`: four hex digits, or a surrogate pair written as two escapes.
    fn unicode_escape(&mut self) -> Result<char, FormatError> {
        let hi = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&hi) {
            if !self.b[self.i..].starts_with(b"\\u") {
                return Err(self.error("the second half of a surrogate pair"));
            }
            self.i += 2;
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err(self.error("the second half of a surrogate pair"));
            }
            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
        } else {
            hi
        };
        char::from_u32(code).ok_or_else(|| self.error("a character, not half a surrogate pair"))
    }
}

fn floor_char(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A `//` comment that starts on the same line after `from`, skipping spaces, tabs and
/// one comma.
fn same_line_note(text: &str, from: usize) -> Option<(usize, usize)> {
    let rest = &text[from..];
    let skipped = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let mut at = from + skipped;
    if text[at..].starts_with(',') {
        at += 1;
        let rest = &text[at..];
        at += rest.len() - rest.trim_start_matches([' ', '\t']).len();
    }
    text[at..]
        .starts_with("//")
        .then(|| (at, line_end(text, at)))
}

/// Offset of the start of the line holding `at`.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |n| n + 1)
}

/// Offset of the `\n` (or `\r\n`) ending the line holding `at`, or the end of the text.
fn line_end(text: &str, at: usize) -> usize {
    let end = text[at..].find('\n').map_or(text.len(), |n| at + n);
    if end > at && text.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// The whitespace a line starts with.
fn indent_of(text: &str, at: usize) -> &str {
    let line = &text[line_start(text, at)..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The file's indent step: the first indented line's whitespace, else two spaces.
pub(super) fn indent_unit(text: &str) -> String {
    text.lines()
        .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
        .find(|w| !w.is_empty())
        .map_or_else(
            || "  ".to_owned(),
            |w| {
                if w.starts_with('\t') {
                    "\t".to_owned()
                } else {
                    " ".repeat(w.len().clamp(1, 8))
                }
            },
        )
}

/// The line ending for new lines: `\r\n` if the file has one anywhere.
fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Text without `object.members[index]`. A member alone on its line takes the whole line
/// with it; a last member's removal also takes the comma it leaves trailing.
pub(super) fn remove_member(text: &str, object: &Object, index: usize) -> String {
    let m = &object.members[index];
    let (mut start, mut end) = (m.start, m.end());
    let ls = line_start(text, start);
    let le = line_end(text, end);
    let whole_line = text[ls..start].trim().is_empty() && text[end..le].trim().is_empty();
    if whole_line {
        start = ls;
        // The ending this line has, which in a file with both kinds may not be the
        // file's usual one.
        let rest = &text[le..];
        end = le
            + if rest.starts_with("\r\n") {
                2
            } else {
                usize::from(rest.starts_with('\n'))
            };
    } else {
        let rest = &text[end..];
        end += rest.len() - rest.trim_start_matches([' ', '\t']).len();
    }
    let mut out = String::with_capacity(text.len());
    let trailing = (m.comma.is_none() && index + 1 == object.members.len() && index > 0)
        .then(|| object.members[index - 1].comma)
        .flatten();
    match trailing {
        Some(c) => {
            out.push_str(&text[..c]);
            let between = &text[c + 1..start];
            out.push_str(if whole_line {
                between
            } else {
                between.trim_end_matches([' ', '\t'])
            });
        }
        None => out.push_str(&text[..start]),
    }
    out.push_str(&text[end..]);
    out
}

/// A member to add: its text (maybe several lines, indented from column 0 by the file's
/// own step) and an optional `//` note after it.
pub(super) struct NewMember {
    pub body: String,
    pub note: Option<String>,
}

/// Text with `items` added after the last member of `object`, one per line.
pub(super) fn append_members(text: &str, object: &Object, items: &[NewMember]) -> String {
    let unit = indent_unit(text);
    let nl = eol(text);
    let member_indent = object.members.first().map_or_else(
        || format!("{}{unit}", indent_of(text, object.open)),
        |m| {
            if text[line_start(text, m.start)..m.start].trim().is_empty() {
                indent_of(text, m.start).to_owned()
            } else {
                format!("{}{unit}", indent_of(text, object.open))
            }
        },
    );
    let close_indent = if text[line_start(text, object.close)..object.close]
        .trim()
        .is_empty()
    {
        indent_of(text, object.close).to_owned()
    } else {
        indent_of(text, object.open).to_owned()
    };

    // An owner who ends every member with a comma gets Atlas's last one ended the same way,
    // so taking it out again leaves their file as it was.
    let trailing = object.members.last().is_some_and(|m| m.comma.is_some());
    let mut block = String::new();
    for (k, item) in items.iter().enumerate() {
        block.push_str(nl);
        let mut lines = item.body.split('\n');
        if let Some(first) = lines.next() {
            block.push_str(&member_indent);
            block.push_str(first);
        }
        for line in lines {
            block.push_str(nl);
            block.push_str(&member_indent);
            block.push_str(line);
        }
        if k + 1 < items.len() || trailing {
            block.push(',');
        }
        if let Some(note) = &item.note {
            block.push(' ');
            block.push_str(note);
        }
    }

    // Where the new lines go: the end of the last member's line, or straight after `{`.
    let (after, comma_at) = match object.members.last() {
        Some(last) => (last.end(), last.comma.is_none().then_some(last.value.end)),
        None => (object.open + 1, None),
    };
    let rest = &text[after..];
    let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let at_line_end = matches!(text.as_bytes().get(after + gap), Some(b'\n' | b'\r'));
    let insert_at = if at_line_end { after + gap } else { after };
    if !at_line_end {
        block.push_str(nl);
        block.push_str(&close_indent);
    }

    let mut out = String::with_capacity(text.len() + block.len() + 1);
    match comma_at {
        Some(c) => {
            out.push_str(&text[..c]);
            out.push(',');
            out.push_str(&text[c..insert_at]);
        }
        None => out.push_str(&text[..insert_at]),
    }
    out.push_str(&block);
    out.push_str(&text[insert_at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(text: &str) -> Object {
        match parse(text).unwrap().kind {
            Kind::Object(o) => o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_comments_trailing_commas_and_escapes() {
        let text = "// head\n{ /* a */ \"a\\u00e9\\ud83d\\ude00\": [1, -2.5e3, true, null,], \"b\": {\"c\": \"d\",}, }\n";
        let o = root(text);
        assert_eq!(o.members[0].key, "a\u{e9}\u{1F600}");
        let b = o.get("b").unwrap().value.as_object().unwrap();
        assert_eq!(b.get("c").unwrap().value.as_str(), Some("d"));
    }

    #[test]
    fn refuses_what_isnt_jsonc() {
        for (text, line, column) in [
            ("{\"a\": 1 \"b\": 2}", 1, 9),
            ("{\"a\": 01}", 1, 8),
            ("{\n  \"a\": tru\n}", 2, 8),
            ("{\"a\": \"x\n\"}", 1, 9),
            ("{\"a\": 1} x", 1, 10),
            ("{/* open", 1, 2),
            ("{\"a\": \"\\ud800\"}", 1, 14),
            ("", 1, 1),
        ] {
            match parse(text) {
                Err(FormatError::InvalidJsonc {
                    line: l, column: c, ..
                }) => assert_eq!((l, c), (line, column), "{text}"),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn deep_nesting_is_refused() {
        let text = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(matches!(
            parse(&text),
            Err(FormatError::InvalidJsonc { .. })
        ));
    }

    #[test]
    fn notes_are_found_on_the_members_line_only() {
        let text = "{\n  \"a\": 1, // one\n  \"b\": 2 // two\n  // not b's\n}";
        let o = root(text);
        assert_eq!(o.members[0].note(text), Some("// one"));
        assert_eq!(o.members[1].note(text), Some("// two"));
    }

    #[test]
    fn removing_a_line_keeps_the_rest() {
        let text = "{\n  \"a\": 1, // keep\n  \"b\": 2, // go\n  \"c\": 3\n}\n";
        assert_eq!(
            remove_member(text, &root(text), 1),
            "{\n  \"a\": 1, // keep\n  \"c\": 3\n}\n"
        );
        // The last member leaves no trailing comma behind.
        assert_eq!(
            remove_member(text, &root(text), 2),
            "{\n  \"a\": 1, // keep\n  \"b\": 2 // go\n}\n"
        );
        let one_line = "{\"a\": 1, \"b\": 2}";
        assert_eq!(remove_member(one_line, &root(one_line), 1), "{\"a\": 1}");
        assert_eq!(remove_member(one_line, &root(one_line), 0), "{\"b\": 2}");
    }

    #[test]
    fn removing_a_line_takes_only_its_own_line_ending() {
        // A file with both endings: each line goes with the ending it actually has.
        for (text, index, want) in [
            (
                "{\r\n  \"a\": 1,\n  \"b\": 2\r\n}\r\n",
                0,
                "{\r\n  \"b\": 2\r\n}\r\n",
            ),
            ("{\n  \"a\": 1,\r\n  \"b\": 2\n}\n", 0, "{\n  \"b\": 2\n}\n"),
            // The member's line is the last before the closing brace at the end of the file.
            ("{\r\n  \"a\": 1,\n  \"b\": 2\n}", 1, "{\r\n  \"a\": 1\n}"),
            // The next line starts with a multi-byte character.
            (
                "{\r\n\"a\": 1,\n\"\u{e9}\": 2\r\n}",
                0,
                "{\r\n\"\u{e9}\": 2\r\n}",
            ),
        ] {
            let got = remove_member(text, &root(text), index);
            assert_eq!(got, want, "{text:?}");
            parse(&got).unwrap();
        }
    }

    #[test]
    fn appending_follows_the_files_layout() {
        let item = |b: &str| NewMember {
            body: b.into(),
            note: Some("// n".into()),
        };
        let text = "{\n    \"a\": 1 // keep\n}\n";
        assert_eq!(
            append_members(text, &root(text), &[item("\"x\": 2"), item("\"y\": 3")]),
            "{\n    \"a\": 1, // keep\n    \"x\": 2, // n\n    \"y\": 3 // n\n}\n"
        );
        let empty = "{}";
        assert_eq!(
            append_members(empty, &root(empty), &[item("\"x\": 2")]),
            "{\n  \"x\": 2 // n\n}"
        );
        let inline = "{\"a\": 1}";
        assert_eq!(
            append_members(inline, &root(inline), &[item("\"x\": 2")]),
            "{\"a\": 1,\n  \"x\": 2 // n\n}"
        );
        let trailing = "{\n  \"a\": 1,\n}";
        assert_eq!(
            append_members(trailing, &root(trailing), &[item("\"x\": 2")]),
            "{\n  \"a\": 1,\n  \"x\": 2, // n\n}"
        );
        let tabs = "{\r\n\t\"a\": {\r\n\t}\r\n}";
        let inner = root(tabs);
        let inner = inner.members[0].value.as_object().unwrap();
        assert_eq!(
            append_members(tabs, inner, &[item("\"x\": 2")]),
            "{\r\n\t\"a\": {\r\n\t\t\"x\": 2 // n\r\n\t}\r\n}"
        );
        for out in [
            append_members(text, &root(text), &[item("\"x\": 2")]),
            append_members(inline, &root(inline), &[item("\"x\": {\n  \"y\": 3\n}")]),
        ] {
            parse(&out).unwrap();
        }
    }
}
