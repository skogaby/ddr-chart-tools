//! MSD tokenizer — shared between SSC and SM parsers.
//!
//! MSD is the text format used by StepMania's simfile formats.
//! Each value is written as `#PARAM0:PARAM1:PARAM2;`:
//!
//! - `#` starts a new value
//! - `:` separates params within a value
//! - `;` terminates the value
//! - `//` begins a line comment (consumed silently)
//! - If a `#` appears at the start of a new line while a value is still
//!   open, we recover as if the missing `;` were present (some real
//!   files author without the terminator; StepMania tolerates this).
//! - Backslash escapes the next character (enabled by default).

/// One parsed MSD value. `params[0]` is conventionally the tag name
/// (e.g. `"TITLE"`, `"NOTES"`, `"NOTEDATA"`); later entries are the
/// value's sub-fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsdValue {
    pub params: Vec<String>,
}

impl MsdValue {
    /// Tag name (uppercase-canonicalized), or empty if the value had
    /// no params.
    #[must_use]
    pub fn tag(&self) -> &str {
        self.params.first().map(String::as_str).unwrap_or("")
    }

    /// Nth sub-field of the value, or `""` if absent.
    #[must_use]
    pub fn param(&self, n: usize) -> &str {
        self.params.get(n).map(String::as_str).unwrap_or("")
    }
}

/// Tokenize the full input string into a list of MSD values.
#[must_use]
pub fn tokenize(input: &str) -> Vec<MsdValue> {
    let bytes = input.as_bytes();
    let mut out: Vec<MsdValue> = Vec::new();
    let mut i = 0usize;
    let mut reading_value = false;
    // Current param being accumulated; `None` means "no param started
    // yet since the last control byte".
    let mut cur: Option<String> = None;

    while i < bytes.len() {
        // Line comment — skip to end of line.
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        let c = bytes[i];

        if !reading_value {
            if c == b'#' {
                out.push(MsdValue { params: Vec::new() });
                reading_value = true;
                cur = None;
                i += 1;
                continue;
            }
            // Everything outside a value is ignored (whitespace, stray
            // bytes, comments already handled above).
            i += 1;
            continue;
        }

        // Missing-`;` recovery: a `#` at the start of a line while
        // still inside a value closes the current value.
        if c == b'#' && is_first_non_whitespace_on_line(&cur) {
            finalize_param(out.last_mut().expect("value exists"), &mut cur);
            trim_trailing_whitespace_from_last_param(out.last_mut().expect("value exists"));
            reading_value = false;
            continue;
        }

        if c == b':' {
            finalize_param(out.last_mut().expect("value exists"), &mut cur);
            i += 1;
            continue;
        }

        if c == b';' {
            finalize_param(out.last_mut().expect("value exists"), &mut cur);
            reading_value = false;
            i += 1;
            continue;
        }

        // Backslash escape: consume next byte verbatim.
        if c == b'\\' && i + 1 < bytes.len() {
            cur.get_or_insert_with(String::new)
                .push(bytes[i + 1] as char);
            i += 2;
            continue;
        }

        cur.get_or_insert_with(String::new).push(c as char);
        i += 1;
    }

    // Flush any unterminated value.
    if reading_value {
        if let Some(value) = out.last_mut() {
            if cur.is_some() {
                finalize_param(value, &mut cur);
            }
        }
    }

    out
}

fn finalize_param(value: &mut MsdValue, cur: &mut Option<String>) {
    let param = cur.take().unwrap_or_default();
    value.params.push(param);
}

/// True when `cur` (the current param being built) is empty or
/// contains only trailing whitespace since the last newline. Used to
/// decide whether a `#` should trigger missing-`;` recovery.
fn is_first_non_whitespace_on_line(cur: &Option<String>) -> bool {
    let Some(s) = cur else { return true };
    for c in s.chars().rev() {
        if c == '\n' || c == '\r' {
            return true;
        }
        if c.is_whitespace() {
            continue;
        }
        return false;
    }
    // We reached the start of the whole param without hitting a
    // newline; treat as the first char on its line.
    true
}

fn trim_trailing_whitespace_from_last_param(value: &mut MsdValue) {
    if let Some(last) = value.params.last_mut() {
        while let Some(c) = last.chars().next_back() {
            if c.is_whitespace() {
                last.pop();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tag_value() {
        let v = tokenize("#TITLE:Example;");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].tag(), "TITLE");
        assert_eq!(v[0].param(1), "Example");
    }

    #[test]
    fn multiple_params_separated_by_colon() {
        let v = tokenize("#NOTES:dance-single::Beginner:3:0.5,0.5,0.5:0000\n0000\n;");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].tag(), "NOTES");
        assert_eq!(v[0].param(1), "dance-single");
        assert_eq!(v[0].param(2), ""); // empty description field
        assert_eq!(v[0].param(3), "Beginner");
        assert_eq!(v[0].param(4), "3");
        assert_eq!(v[0].param(5), "0.5,0.5,0.5");
        assert_eq!(v[0].param(6), "0000\n0000\n");
    }

    #[test]
    fn multiple_top_level_values() {
        let v = tokenize("#TITLE:A;\n#ARTIST:B;\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].tag(), "TITLE");
        assert_eq!(v[0].param(1), "A");
        assert_eq!(v[1].tag(), "ARTIST");
        assert_eq!(v[1].param(1), "B");
    }

    #[test]
    fn line_comment_is_stripped() {
        let v = tokenize("#TITLE:Hello // this is a comment\nWorld;");
        // The comment-through-newline is stripped; the value continues on
        // the next line.
        assert_eq!(v[0].param(1), "Hello \nWorld");
    }

    #[test]
    fn backslash_escapes_special_chars() {
        let v = tokenize(r"#TITLE:a\:b\;c;");
        assert_eq!(v[0].param(1), "a:b;c");
    }

    #[test]
    fn missing_semicolon_recovers_on_hash_at_line_start() {
        // First value is missing its `;`. `#ARTIST` at start of next line
        // closes it.
        let v = tokenize("#TITLE:A\n#ARTIST:B;");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].tag(), "TITLE");
        assert_eq!(v[0].param(1), "A");
        assert_eq!(v[1].tag(), "ARTIST");
        assert_eq!(v[1].param(1), "B");
    }

    #[test]
    fn hash_mid_value_is_literal() {
        // A `#` that's not at the start of a line is treated as content.
        let v = tokenize("#TITLE:A#B;");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].param(1), "A#B");
    }

    #[test]
    fn empty_input() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn whitespace_between_values() {
        let v = tokenize("#A:1;\n\n   \n#B:2;");
        assert_eq!(v.len(), 2);
    }
}
