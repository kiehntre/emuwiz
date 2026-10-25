//! Conservative parsing of TOSEC release-name annotations.
//!
//! These names are useful *after* a DAT hash match has identified a member.
//! They are deliberately never used to look up a DAT, select a candidate, or
//! establish an identity on their own.  TOSEC country and language tokens are
//! retained verbatim: this module does not reinterpret them using another
//! catalogue's regional vocabulary.

/// Release and dump annotations carried in a classic TOSEC entry name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TosecFilenameMetadata {
    /// The title before the first recognised TOSEC annotation.
    pub title: String,
    pub year: Option<String>,
    pub publisher: Option<String>,
    pub countries: Vec<String>,
    pub languages: Vec<String>,
    pub version: Option<String>,
    pub revision: Option<String>,
    pub flags: TosecDumpFlags,
}

/// Dump-state annotations from square-bracket TOSEC tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TosecDumpFlags {
    pub cracked: bool,
    pub trainer: bool,
    pub hacked: bool,
    pub alternate: bool,
    pub fixed: bool,
    pub modified: bool,
    pub pirated: bool,
    pub bad_dump: bool,
    pub overdump: bool,
    pub underdump: bool,
    pub virus: bool,
    pub verified_good: bool,
}

impl TosecDumpFlags {
    /// Whether TOSEC explicitly marks this dump as defective or unsafe.
    pub fn quality_downgraded(self) -> bool {
        self.bad_dump || self.overdump || self.underdump || self.virus
    }

    /// Stable human-readable labels for the annotations that are present.
    pub fn labels(self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.cracked {
            labels.push("cracked");
        }
        if self.trainer {
            labels.push("trainer");
        }
        if self.hacked {
            labels.push("hack");
        }
        if self.alternate {
            labels.push("alternate");
        }
        if self.fixed {
            labels.push("fixed");
        }
        if self.modified {
            labels.push("modified");
        }
        if self.pirated {
            labels.push("pirated");
        }
        if self.bad_dump {
            labels.push("bad dump");
        }
        if self.overdump {
            labels.push("overdump");
        }
        if self.underdump {
            labels.push("underdump");
        }
        if self.virus {
            labels.push("virus");
        }
        if self.verified_good {
            labels.push("verified good");
        }
        labels
    }
}

/// Parses useful classic TOSEC naming tokens from an already-matched DAT
/// entry. Unknown annotations are intentionally left uninterpreted.
pub fn parse_tosec_filename_metadata(name: &str) -> TosecFilenameMetadata {
    let mut metadata = TosecFilenameMetadata {
        title: title_prefix(name),
        ..Default::default()
    };

    for token in parenthesized_tokens(name) {
        let trimmed = token.trim();
        let lower = trimmed.to_ascii_lowercase();
        if metadata.year.is_none() && is_year(trimmed) {
            metadata.year = Some(trimmed.to_string());
        } else if metadata.version.is_none() && is_version(&lower) {
            metadata.version = Some(trimmed.to_string());
        } else if metadata.revision.is_none() && is_revision(&lower) {
            metadata.revision = Some(trimmed.to_string());
        } else if is_language_token(trimmed) {
            metadata.languages.push(trimmed.to_string());
        } else if is_country_token(trimmed) {
            metadata.countries.push(trimmed.to_string());
        } else if metadata.publisher.is_none() {
            // In classic TOSEC names the publisher is the remaining first
            // parenthesised release token. Preserve it exactly rather than
            // mapping it through a different catalogue's publisher registry.
            metadata.publisher = Some(trimmed.to_string());
        }
    }

    for token in bracketed_tokens(name) {
        apply_dump_token(&mut metadata.flags, token);
    }
    metadata
}

fn title_prefix(name: &str) -> String {
    let cut = name
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '(' | '[').then_some(index))
        .unwrap_or(name.len());
    name[..cut].trim_end().to_string()
}

fn parenthesized_tokens(name: &str) -> Vec<&str> {
    delimited_tokens(name, '(', ')')
}

fn bracketed_tokens(name: &str) -> Vec<&str> {
    delimited_tokens(name, '[', ']')
}

fn delimited_tokens(name: &str, open: char, close: char) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, ch) in name.char_indices() {
        if ch == open {
            start = Some(index + ch.len_utf8());
        } else if ch == close
            && let Some(token_start) = start.take()
        {
            tokens.push(&name[token_start..index]);
        }
    }
    tokens
}

fn is_year(token: &str) -> bool {
    token.len() == 4
        && token.starts_with(['1', '2'])
        && token.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_version(lower: &str) -> bool {
    lower.starts_with('v')
        && lower.strip_prefix('v').is_some_and(|rest| {
            rest.bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_digit())
        })
}

fn is_revision(lower: &str) -> bool {
    lower.starts_with("rev") || lower.starts_with("revision") || lower.starts_with("r ")
}

fn is_language_token(token: &str) -> bool {
    let language_codes = [
        "ar", "cs", "da", "de", "el", "en", "es", "fi", "fr", "he", "hu", "it", "ja", "ko", "nl",
        "no", "pl", "pt", "ru", "sv", "tr", "zh",
    ];
    token
        .split([',', '+', '-'])
        .all(|part| language_codes.contains(&part.trim().to_ascii_lowercase().as_str()))
}

fn is_country_token(token: &str) -> bool {
    let lower = token.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "australia"
            | "brazil"
            | "canada"
            | "denmark"
            | "europe"
            | "france"
            | "germany"
            | "italy"
            | "japan"
            | "korea"
            | "netherlands"
            | "new zealand"
            | "norway"
            | "poland"
            | "russia"
            | "spain"
            | "sweden"
            | "uk"
            | "united kingdom"
            | "usa"
            | "world"
            | "us"
            | "eu"
            | "jp"
    )
}

fn apply_dump_token(flags: &mut TosecDumpFlags, token: &str) {
    let token = token.trim().to_ascii_lowercase();
    let marker = token.split_whitespace().next().unwrap_or_default();
    match marker {
        "cr" => flags.cracked = true,
        "t" => flags.trainer = true,
        "h" => flags.hacked = true,
        "a" => flags.alternate = true,
        "f" => flags.fixed = true,
        "m" => flags.modified = true,
        "p" => flags.pirated = true,
        "b" => flags.bad_dump = true,
        "o" => flags.overdump = true,
        "u" => flags.underdump = true,
        "v" => flags.virus = true,
        "!" => flags.verified_good = true,
        _ => {}
    }
}
