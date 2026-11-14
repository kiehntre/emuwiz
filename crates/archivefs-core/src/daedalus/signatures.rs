//! Fixed log signatures and the scanner that applies them.
//!
//! A signature is a literal pattern, anchored to the start of a line, with
//! an identifier, a version and a meaning. There are no regular
//! expressions and no fuzzy matching: a line either is one of the exact
//! messages a named program is known to print, or it is ignored.
//!
//! Every signature in this crate was taken from output captured from the
//! real program, and its doc comment says which version. A message that
//! was not observed is not here.
//!
//! Matching a signature proves only that the text contains that line. The
//! text is untrusted ([`Trust::UntrustedText`]): the same line could have
//! been printed by a game through its emulator. What a match may be used
//! for is decided by the rules, and bounded by the engine.

use super::evidence::{
    EvidenceArea, EvidenceId, EvidenceSet, Fact, LogCoverage, LogEvent, LogMeaning, LogStream,
    Trust,
};
use super::text::{self, Examined};

/// How a signature recognises a line. Every form is anchored at the first
/// byte of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matcher {
    /// The whole line.
    Exact(&'static str),
    /// The start of the line.
    Prefix(&'static str),
    /// The start of the line; the value is the first double-quoted string
    /// after the prefix.
    PrefixQuoted(&'static str),
    /// The start of the line; the value is the rest of the line.
    PrefixRest(&'static str),
    /// The start of the line; the value is the text between the prefix and
    /// the first occurrence of `until`, which must be present.
    PrefixUntil {
        prefix: &'static str,
        until: &'static str,
    },
    /// The start of the line; the value is the text between the first
    /// `after` following the prefix and the next `until`.
    PrefixBetween {
        prefix: &'static str,
        after: &'static str,
        until: &'static str,
    },
    /// `marker` somewhere after a program name; the value is the text
    /// between `marker` and the first following `until`. Used only for the
    /// dynamic loader, whose message starts with the path of the program.
    AfterMarker {
        marker: &'static str,
        until: &'static str,
    },
}

/// One known message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub id: &'static str,
    pub version: u32,
    pub meaning: LogMeaning,
    pub matcher: Matcher,
}

impl Signature {
    /// `Some(value)` when the line matches; the value is `None` for
    /// matchers that extract nothing.
    fn matches<'a>(&self, line: &'a str) -> Option<Option<&'a str>> {
        match self.matcher {
            Matcher::Exact(expected) => (line == expected).then_some(None),
            Matcher::Prefix(prefix) => line.starts_with(prefix).then_some(None),
            Matcher::PrefixQuoted(prefix) => text::quoted_after(line, prefix).map(Some),
            Matcher::PrefixRest(prefix) => line.strip_prefix(prefix).map(Some),
            Matcher::PrefixUntil { prefix, until } => {
                let rest = line.strip_prefix(prefix)?;
                let end = rest.find(until)?;
                Some(Some(&rest[..end]))
            }
            Matcher::PrefixBetween {
                prefix,
                after,
                until,
            } => {
                let rest = line.strip_prefix(prefix)?;
                let start = rest.find(after)? + after.len();
                let end = rest[start..].find(until)?;
                Some(Some(&rest[start..start + end]))
            }
            Matcher::AfterMarker { marker, until } => {
                let start = line.find(marker)? + marker.len();
                let end = line[start..].find(until)?;
                Some(Some(&line[start..start + end]))
            }
        }
    }

    /// Text that identifies this signature for the rule-set digest.
    pub(super) fn digest_text(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}",
            self.id, self.version, self.meaning, self.matcher
        )
    }
}

/// Messages printed by the host rather than by any emulator.
pub const HOST_EMULATOR: &str = "host";

/// Host signatures. Captured on Ubuntu 24.04 (glibc 2.39, Flatpak 1.14.6,
/// bubblewrap 0.9.0), 2026-11-13.
pub const HOST_SIGNATURES: &[Signature] = &[
    // `./needs_lib: error while loading shared libraries: libx.so.1: cannot
    // open shared object file: No such file or directory`, exit status 127.
    Signature {
        id: "host.loader.shared_library_missing",
        version: 1,
        meaning: LogMeaning::SharedLibraryMissing,
        matcher: Matcher::AfterMarker {
            marker: ": error while loading shared libraries: ",
            until: ": cannot open shared object file",
        },
    },
    // `error: app/org.example.App/x86_64/master not installed`, exit status 1.
    Signature {
        id: "host.flatpak.app_not_installed",
        version: 1,
        meaning: LogMeaning::SandboxRefused,
        matcher: Matcher::PrefixUntil {
            prefix: "error: app/",
            until: " not installed",
        },
    },
    // `bwrap: Can't find source path /x: No such file or directory`, exit
    // status 1. Every bubblewrap setup error starts with `bwrap: `.
    Signature {
        id: "host.bwrap.setup_failed",
        version: 1,
        meaning: LogMeaning::SandboxRefused,
        matcher: Matcher::PrefixRest("bwrap: "),
    },
];

/// Result of scanning one stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    pub events: Vec<LogEvent>,
    pub coverage: LogCoverage,
}

/// Match every examined line against `signatures`, first match wins, and
/// keep at most [`text::LOG_EVENTS_LIMIT`] events. Reaching that limit
/// marks the coverage as truncated: later lines were not classified.
pub fn scan(
    examined: &Examined,
    signatures: &[Signature],
    emulator: &str,
    stream: LogStream,
) -> Scan {
    let mut events = Vec::new();
    let mut truncated = examined.truncated;
    for line in examined.lines() {
        let Some((signature, value)) = signatures
            .iter()
            .find_map(|signature| signature.matches(line.text).map(|value| (signature, value)))
        else {
            continue;
        };
        if events.len() == text::LOG_EVENTS_LIMIT {
            truncated = true;
            break;
        }
        events.push(LogEvent {
            emulator: emulator.to_string(),
            signature: signature.id.to_string(),
            signature_version: signature.version,
            meaning: signature.meaning,
            stream,
            line: line.number,
            excerpt: text::sanitise(line.text, text::EXCERPT_LIMIT),
            value: value.map(|value| text::sanitise(value, text::VALUE_LIMIT)),
        });
    }
    Scan {
        events,
        coverage: LogCoverage {
            emulator: emulator.to_string(),
            stream,
            lines_examined: examined.line_count() + examined.lines_ignored,
            bytes_examined: examined.bytes_examined,
            truncated,
            lines_ignored: examined.lines_ignored,
            // Decided by the emulator's adapter, which knows what an
            // informational line looks like.
            verbose: false,
        },
    }
}

/// Add a scan to an evidence set: one coverage item, then one item per
/// event. Returns the ids of the event items.
pub fn record_scan(set: &mut EvidenceSet, scan: Scan, source: &str) -> Vec<EvidenceId> {
    set.observe(
        EvidenceArea::LogEvents,
        // How much text was examined is EmuWiz's own measurement.
        Trust::Measured,
        source,
        Fact::LogCoverage(scan.coverage),
    );
    scan.events
        .into_iter()
        .filter_map(|event| {
            set.observe(
                EvidenceArea::LogEvents,
                Trust::UntrustedText,
                source,
                Fact::LogEvent(event),
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "signatures/tests.rs"]
mod tests;
