//! Removal of the private details EmuWiz recognises, before a report is
//! shown or exported.
//!
//! Emulator output and file paths carry things that are nobody else's
//! business: the name of the user's home folder, the user name itself,
//! and now and then a credential an emulator or a wrapper printed. A report
//! kept in memory needs the real paths (to re-check that a file has not
//! changed); a report that is displayed or exported does not.
//!
//! # What this is, and what it is not
//!
//! This is a short list of literal rules. It removes what those rules
//! describe and nothing else. It is **not** a guarantee that the result
//! holds nothing private: emulator output is free text, and a secret
//! printed without a label, an unusual credential format, another person's
//! name or a network address passes through unchanged. Running the rules a
//! second time proves only that the rules ran.
//!
//! For that reason redacted text is never stored. The launch history
//! ([`super::history`]) keeps structured summaries built from fixed lists
//! and does not contain emulator output at all, redacted or otherwise.
//! Redacted text is used in two places only: on screen, for the person
//! whose computer it is, and in an export that person asked for, previewed
//! and was warned about ([`super::export`]).
//!
//! [`Redactor::report`] returns a copy of a report with every text field
//! redacted. The rules:
//!
//! - the home folder becomes `~`, and the name in any other `/home/<name>`
//!   or `/Users/<name>` becomes `[user]`;
//! - the user name, as a whole path component, becomes `[user]`;
//! - the value of anything named like a secret (`password=`, `token:`,
//!   `api_key = "..."`) becomes `[redacted]`;
//! - credentials in a URL (`scheme://user:pass@host`) and after `Bearer` or
//!   `Basic` become `[redacted]`;
//! - the value of an environment variable named like a secret becomes
//!   `[redacted]`.
//!
//! Redaction is idempotent. It never invents text, and it removes rather
//! than preserves when unsure.

use serde_json::Value;

use super::rules::DiagnosisReport;
use super::text;

pub const REDACTED: &str = "[redacted]";
const USER: &str = "[user]";

/// User names that are also ordinary words in emulator output. Redacting
/// them everywhere would blank out the report; the home-folder rules still
/// hide them where they identify a person's folder.
const GENERIC_USER_NAMES: &[&str] = &[
    "root",
    "user",
    "users",
    "home",
    "games",
    "game",
    "admin",
    "retroarch",
    "emuwiz",
    "steam",
    "deck",
    "player",
    "guest",
    "ubuntu",
    "linux",
];

/// Names that mark the value next to them as a secret, matched without
/// regard to case anywhere inside a key.
const SECRET_WORDS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "auth",
    "cookie",
    "session",
    "credential",
    "private_key",
];

fn names_a_secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| key.contains(word))
}

/// Knows what to hide for one user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Redactor {
    home: Option<String>,
    user: Option<String>,
}

fn is_path_boundary(character: Option<char>) -> bool {
    match character {
        None => true,
        Some(character) => !(character.is_alphanumeric() || matches!(character, '_' | '-' | '.')),
    }
}

/// Replace `needle` where it stands as a whole run of path characters:
/// the text before and after it must not continue a name.
fn replace_whole(input: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return input.to_string();
    }
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find(needle) {
        let before = rest[..index].chars().next_back();
        let after = rest[index + needle.len()..].chars().next();
        output.push_str(&rest[..index]);
        if is_path_boundary(before) && is_path_boundary(after) {
            output.push_str(replacement);
        } else {
            output.push_str(needle);
        }
        rest = &rest[index + needle.len()..];
    }
    output.push_str(rest);
    output
}

/// The name in `/home/<name>` and `/Users/<name>` becomes `[user]`.
fn redact_other_homes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let found = ["/home/", "/Users/"]
            .iter()
            .filter_map(|prefix| rest.find(prefix).map(|index| (index, *prefix)))
            .min_by_key(|(index, _)| *index);
        let Some((index, prefix)) = found else {
            break;
        };
        // Only at the start of a path: not in the middle of another name.
        let before = rest[..index].chars().next_back();
        let name_start = index + prefix.len();
        let name_len = rest[name_start..]
            .find(|character: char| {
                character == '/' || character.is_whitespace() || character == '"'
            })
            .unwrap_or(rest.len() - name_start);
        output.push_str(&rest[..name_start]);
        let name = &rest[name_start..name_start + name_len];
        if before.is_some_and(|character| character.is_alphanumeric()) || name.is_empty() {
            output.push_str(name);
        } else {
            output.push_str(USER);
        }
        rest = &rest[name_start + name_len..];
    }
    output.push_str(rest);
    output
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

/// Replace the value after a secret-looking key in `key=value`,
/// `key: value` and `key = "value"`.
fn redact_secret_values(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    let bytes = input.as_bytes();
    while index < input.len() {
        // Find the next separator that could follow a key.
        let Some(offset) = input[index..].find(['=', ':']) else {
            break;
        };
        let separator = index + offset;
        let key_start = input[..separator]
            .char_indices()
            .rev()
            .skip_while(|(_, character)| *character == ' ' || *character == '"')
            .take_while(|(_, character)| is_key_character(*character))
            .last()
            .map(|(position, _)| position);
        let key_end = input[..separator].trim_end_matches([' ', '"']).len();
        let key = key_start.map(|start| &input[start..key_end]).unwrap_or("");
        let mut value_start = separator + 1;
        // `scheme://` is not a key and a value.
        let is_url_scheme =
            bytes.get(separator) == Some(&b':') && input[value_start..].starts_with("//");
        if key.is_empty() || !names_a_secret(key) || is_url_scheme {
            output.push_str(&input[index..value_start]);
            index = value_start;
            continue;
        }
        while input[value_start..].starts_with(' ') {
            value_start += 1;
        }
        let quoted = input[value_start..].starts_with('"');
        if quoted {
            value_start += 1;
        }
        let value_len = if quoted {
            input[value_start..]
                .find('"')
                .unwrap_or(input.len() - value_start)
        } else {
            input[value_start..]
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, '&' | ',' | ';' | '"' | '\'')
                })
                .unwrap_or(input.len() - value_start)
        };
        output.push_str(&input[index..value_start]);
        if value_len > 0 {
            output.push_str(REDACTED);
        }
        index = value_start + value_len;
    }
    output.push_str(&input[index..]);
    output
}

/// `scheme://user:pass@host` keeps the scheme and host only.
fn redact_url_credentials(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find("://") {
        let authority_start = index + 3;
        let authority_len = rest[authority_start..]
            .find(|character: char| {
                character == '/' || character.is_whitespace() || character == '"'
            })
            .unwrap_or(rest.len() - authority_start);
        let authority = &rest[authority_start..authority_start + authority_len];
        output.push_str(&rest[..authority_start]);
        match authority.rfind('@') {
            Some(at) => {
                output.push_str(REDACTED);
                output.push_str(&authority[at..]);
            }
            None => output.push_str(authority),
        }
        rest = &rest[authority_start + authority_len..];
    }
    output.push_str(rest);
    output
}

/// The word after `Bearer ` or `Basic ` is a credential.
fn redact_authorization(input: &str) -> String {
    let mut output = input.to_string();
    for scheme in ["Bearer ", "Basic ", "bearer ", "basic "] {
        let mut result = String::with_capacity(output.len());
        let mut rest = output.as_str();
        while let Some(index) = rest.find(scheme) {
            let start = index + scheme.len();
            let length = rest[start..]
                .find(|character: char| character.is_whitespace() || character == '"')
                .unwrap_or(rest.len() - start);
            result.push_str(&rest[..start]);
            if length > 0 {
                result.push_str(REDACTED);
            }
            rest = &rest[start + length..];
        }
        result.push_str(rest);
        output = result;
    }
    output
}

impl Redactor {
    /// A redactor for the given home folder and user name. Either may be
    /// absent; the pattern rules still apply.
    pub fn new(home: Option<&str>, user: Option<&str>) -> Self {
        let clean = |value: Option<&str>, minimum: usize| {
            value
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| value.len() >= minimum)
        };
        Self {
            // "/" or an empty home would turn every path into "~".
            home: clean(home, 2).filter(|home| home.starts_with('/')),
            // A one- or two-letter name, or one that is an ordinary word,
            // would match far too much.
            user: clean(user, 3)
                .filter(|user| !GENERIC_USER_NAMES.contains(&user.to_ascii_lowercase().as_str())),
        }
    }

    /// A redactor for the current user, from `HOME` and `USER`/`LOGNAME`.
    pub fn for_current_user() -> Self {
        let variable = |name: &str| std::env::var(name).ok();
        Self::new(
            variable("HOME").as_deref(),
            variable("USER").or_else(|| variable("LOGNAME")).as_deref(),
        )
    }

    /// Redact one piece of text.
    pub fn text(&self, input: &str) -> String {
        let mut text = input.to_string();
        if let Some(home) = &self.home {
            text = replace_whole(&text, home, "~");
        }
        text = redact_other_homes(&text);
        if let Some(user) = &self.user {
            text = replace_whole(&text, user, USER);
        }
        text = redact_url_credentials(&text);
        text = redact_authorization(&text);
        redact_secret_values(&text)
    }

    fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.value(item)),
            Value::Object(fields) => {
                // An environment variable named like a secret loses its
                // value whatever the value looks like.
                let secret_variable = fields.get("kind").and_then(Value::as_str)
                    == Some("environment_variable")
                    && fields
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(names_a_secret);
                for (key, field) in fields.iter_mut() {
                    if secret_variable && key == "value" && field.is_string() {
                        *field = Value::String(REDACTED.to_string());
                    } else {
                        self.value(field);
                    }
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    /// A copy of `report` with the private details this module recognises
    /// removed, for display or for an export a person asked for. Not for
    /// storage, and not a guarantee that nothing private remains. The
    /// original keeps the real paths, which freshness checks need.
    pub fn report(&self, report: &DiagnosisReport) -> DiagnosisReport {
        let Ok(mut value) = serde_json::to_value(report) else {
            return Self::withheld(report);
        };
        self.value(&mut value);
        serde_json::from_value(value).unwrap_or_else(|_| Self::withheld(report))
    }

    /// If a report cannot be redacted it is not shown: an empty report with
    /// one note takes its place.
    fn withheld(report: &DiagnosisReport) -> DiagnosisReport {
        let mut empty = report.clone();
        empty.causes.clear();
        empty.symptoms.clear();
        empty.conflicts.clear();
        empty.rejected.clear();
        empty.coverage.clear();
        empty.evidence = Default::default();
        empty.subject = Default::default();
        empty.notes = vec![
            "The diagnosis could not be prepared for display and has been withheld.".to_string(),
        ];
        empty
    }
}

/// Most bytes of captured output looked at for a displayed tail.
const DISPLAY_TAIL_BYTES: usize = 32 * 1024;

/// The end of captured emulator output, for a "technical details" box: at
/// most `max_lines` of its last lines, with the private details this module
/// recognises removed and every character made visible. A first line says
/// so when earlier output was left out.
///
/// For the screen of the person whose computer it is, and nothing else: it
/// is not a guarantee that nothing private remains, and it is never stored.
/// Only the end of the capture is examined, so this is cheap enough to call
/// while drawing.
pub fn output_tail_for_display(output: &[u8], redactor: &Redactor, max_lines: usize) -> String {
    let start = output.len().saturating_sub(DISPLAY_TAIL_BYTES);
    let mut tail = &output[start..];
    let mut cut = start > 0;
    if cut {
        // The first line of the window is probably only part of a line.
        tail = match tail.iter().position(|byte| *byte == b'\n') {
            Some(newline) => &tail[newline + 1..],
            None => &[],
        };
    }
    let decoded = String::from_utf8_lossy(tail);
    let lines: Vec<&str> = decoded.lines().collect();
    let skipped = lines.len().saturating_sub(max_lines);
    cut |= skipped > 0;
    let mut shown = String::new();
    if cut {
        shown.push_str("(earlier output is not shown)\n");
    }
    for line in &lines[skipped..] {
        shown.push_str(&text::sanitise(&redactor.text(line), text::LOG_LINE_LIMIT));
        shown.push('\n');
    }
    shown
}

#[cfg(test)]
#[path = "redact/tests.rs"]
mod tests;
