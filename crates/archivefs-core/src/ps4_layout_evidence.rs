//! Pure, read-only PlayStation 4 extracted-game layout evidence: the
//! `sce_sys/param.sfo` metadata file (parsed through
//! [`crate::param_sfo`]), and nothing else. Never a decrypted PKG, never an
//! executed or interpreted `eboot.bin`, never a recursive crawl of a whole
//! PS4 game directory.
//!
//! # Scope
//!
//! This module observes and validates. It performs **no I/O itself** - a
//! caller supplies the already-read `param.sfo` bytes and the two boolean
//! layout facts. The bounded directory read that feeds it lives in
//! [`crate::game_identity`], next to the equivalent PS3 folder path, and
//! reuses [`crate::param_sfo::MAX_SFO_BYTES`] as its ceiling.
//!
//! # What proves "PlayStation 4"
//!
//! A `param.sfo` file alone proves nothing: it is the shared Sony-ecosystem
//! PSF container, used by PSP, PS3, PS Vita and PS4 with different keys and
//! different conventional layouts. PS Vita in particular *also* stores its
//! `param.sfo` under `sce_sys/`. The PS4-specific discriminators this
//! module relies on, together:
//!
//! * the `sce_sys/param.sfo` relative layout (PS3 uses `PS3_GAME/PARAM.SFO`,
//!   PSP uses `PSP_GAME/PARAM.SFO`); **and**
//! * a `TITLE_ID` in the PS4 `CUSA` application-ID family - four ASCII
//!   letters `CUSA` followed by five digits. PS Vita uses `PCSx` region
//!   codes; PS3 uses `BLxx` / `BCxx` / `NPxx`. None of those pass
//!   [`normalize_ps4_title_id`].
//!
//! Neither signal on its own is treated as PS4 identity; both are required
//! by [`crate::game_identity`]'s PS4 folder inspection.
//!
//! # Content ID
//!
//! When `CONTENT_ID` is present it is parsed separately (see
//! [`parse_ps4_content_id`]). It is **not** assumed to equal `TITLE_ID`: it
//! embeds a title-ID component in its middle segment, and
//! [`title_id_agreement`] compares the two and reports a disagreement
//! rather than silently preferring one.
//!
//! # What this never does
//!
//! No decryption, no PKG parsing, no executable execution, no launch
//! planning, no filesystem mutation, and no claim about which canonical
//! release or region a validated title ID belongs to - exact release
//! identity stays DAT/hash-driven exactly as for every other platform.

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use crate::param_sfo::SfoObservation;

/// The only relative paths this observer looks at inside a candidate PS4
/// game directory. Deliberately not a crawl: exactly the metadata file and
/// its parent.
pub const PS4_LAYOUT_RELATIVE_PATHS: &[&str] = &["sce_sys", "sce_sys/param.sfo"];

/// The `sce_sys/param.sfo` relative path, as a single constant so the
/// bounded reader and this module never drift.
pub const PS4_PARAM_SFO_RELATIVE_PATH: &str = "sce_sys/param.sfo";

/// The `sce_sys` directory name.
pub const PS4_SCE_SYS_DIR: &str = "sce_sys";

/// PARAM.SFO keys read for a PS4 layout. Only `TITLE_ID` is treated as
/// verified identity; the rest are descriptive.
pub const PS4_SFO_TITLE_ID_KEY: &str = "TITLE_ID";
pub const PS4_SFO_CONTENT_ID_KEY: &str = "CONTENT_ID";
pub const PS4_SFO_TITLE_KEY: &str = "TITLE";
pub const PS4_SFO_APP_VER_KEY: &str = "APP_VER";
pub const PS4_SFO_VERSION_KEY: &str = "VERSION";
pub const PS4_SFO_CATEGORY_KEY: &str = "CATEGORY";

/// A stable, PS4-exclusive boot-structure marker for
/// [`crate::content_evidence`] consumers: emitted by [`observe_ps4_evidence`]
/// **only** when a valid `sce_sys/param.sfo` layout carries a `CUSA`-family
/// `TITLE_ID`, so its mere presence already encodes the two-signal test.
pub const PS4_LAYOUT_EVIDENCE_MARKER: &str = "sce_sys/param.sfo+CUSA";

/// Length of a PS4 `CUSA`-family title ID: `CUSA` + 5 digits.
const PS4_TITLE_ID_LEN: usize = 9;
const PS4_TITLE_ID_PREFIX: &str = "CUSA";

/// What was observed about a PS4-style extracted game directory - never a
/// platform decision on its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ps4LayoutObservation {
    /// `sce_sys/` present as a real directory.
    pub sce_sys_dir_present: bool,
    /// `sce_sys/param.sfo` present as a real file.
    pub param_sfo_present: bool,
    /// The parsed `param.sfo`, when it was present and parsed within bounds.
    pub param_sfo: Option<SfoObservation>,
}

impl Ps4LayoutObservation {
    /// The raw `TITLE_ID` text, exactly as stored (not normalized).
    pub fn raw_title_id(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_TITLE_ID_KEY)
    }

    /// The raw `CONTENT_ID` text, exactly as stored.
    pub fn raw_content_id(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_CONTENT_ID_KEY)
    }

    /// The human-readable `TITLE`, descriptive only - never exact identity.
    pub fn title(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_TITLE_KEY)
    }

    /// `APP_VER` (e.g. `"01.00"`), descriptive only.
    pub fn app_version(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_APP_VER_KEY)
    }

    /// `VERSION` (e.g. `"01.00"`), descriptive only.
    pub fn version(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_VERSION_KEY)
    }

    /// `CATEGORY` (e.g. `"gd"` for a game application), descriptive only.
    pub fn category(&self) -> Option<&str> {
        self.param_sfo.as_ref()?.get_text(PS4_SFO_CATEGORY_KEY)
    }

    /// The normalized, validated PS4 `CUSA`-family title ID, or `None` when
    /// the key is missing or not in that family.
    pub fn ps4_title_id(&self) -> Option<String> {
        normalize_ps4_title_id(self.raw_title_id()?)
    }

    /// The parsed PS4 Content ID, or `None` when the key is missing or does
    /// not match the Sony content-ID grammar with a `CUSA`-family title.
    pub fn ps4_content_id(&self) -> Option<Ps4ContentId> {
        parse_ps4_content_id(self.raw_content_id()?)
    }

    /// Whether both PS4 discriminators are satisfied: the `sce_sys/param.sfo`
    /// layout is present *and* the `TITLE_ID` is a valid `CUSA`-family id.
    pub fn is_valid_ps4_layout(&self) -> bool {
        self.sce_sys_dir_present && self.param_sfo_present && self.ps4_title_id().is_some()
    }
}

/// Normalizes and validates a PS4 `TITLE_ID`.
///
/// Real PS4 application title IDs are the `CUSA` family: the four ASCII
/// letters `CUSA` followed by exactly five digits (`CUSA00001`). This is
/// deliberately *not* the looser "four letters + five digits" shape used
/// for PS3 (`BLUS30000`, `NPEB00342`) or PS Vita (`PCSE00001`), so a PS3 or
/// Vita SFO can never be promoted to a PS4 identity by this check.
///
/// Input is trimmed and upper-cased before validation. Returns the
/// normalized `CUSAxxxxx` string, or `None` for anything else - never a
/// best-effort guess.
pub fn normalize_ps4_title_id(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_uppercase();
    if value.len() == PS4_TITLE_ID_LEN
        && value.starts_with(PS4_TITLE_ID_PREFIX)
        && value.as_bytes()[PS4_TITLE_ID_PREFIX.len()..]
            .iter()
            .all(u8::is_ascii_digit)
    {
        Some(value)
    } else {
        None
    }
}

/// A parsed PS4 Content ID.
///
/// The Sony Content ID grammar, shared across the PlayStation ecosystem, is
/// `<label(2)><dist(4)>-<title-id(9)>_<content-type(2)>-<content-label>`
/// (e.g. `UP0001-CUSA00001_00-VALLYRIA00000000`). For PS4 the embedded
/// `title-id` segment is itself a `CUSA`-family id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps4ContentId {
    /// The full Content ID, upper-cased and trimmed.
    pub raw: String,
    /// The 6-character `<label><dist>` prefix (`"UP0001"`).
    pub prefix: String,
    /// The 2-digit content-type field (`"00"`).
    pub content_type: String,
    /// The `CUSA`-family title ID embedded in the middle segment.
    pub embedded_title_id: String,
}

/// Parses a PS4 Content ID by bounded, shape-only grammar matching -
/// exact segment counts and lengths, `CUSA`-family embedded title ID, no
/// claim about which canonical release it names. Returns `None` for
/// anything that does not match this exact shape.
pub fn parse_ps4_content_id(raw: &str) -> Option<Ps4ContentId> {
    let value = raw.trim().to_ascii_uppercase();
    // Bound: a real Content ID is 36 or 37 bytes. Refuse anything wildly
    // longer before splitting.
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let mut segments = value.split('-');
    let prefix = segments.next()?;
    let title_and_type = segments.next()?;
    let content_label = segments.next()?;
    if segments.next().is_some() {
        return None; // more than three '-'-delimited segments
    }
    if prefix.len() != 6 || !prefix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if content_label.is_empty()
        || content_label.len() > 16
        || !content_label.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    let mut middle = title_and_type.split('_');
    let title_id = middle.next()?;
    let content_type = middle.next()?;
    if middle.next().is_some() {
        return None;
    }
    if content_type.len() != 2 || !content_type.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let embedded_title_id = normalize_ps4_title_id(title_id)?;
    let prefix = prefix.to_string();
    let content_type = content_type.to_string();
    Some(Ps4ContentId {
        raw: value,
        prefix,
        content_type,
        embedded_title_id,
    })
}

/// The outcome of comparing a PARAM.SFO `TITLE_ID` against the title-ID
/// component of a `CONTENT_ID`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps4TitleIdAgreement {
    /// No usable `CONTENT_ID` to compare against - nothing is asserted.
    NotComparable,
    /// The `CONTENT_ID`'s embedded title ID equals the PARAM.SFO `TITLE_ID`.
    Agrees,
    /// The two disagree. Fails closed: the caller must not silently pick
    /// one.
    Disagrees {
        param_sfo_title_id: String,
        content_id_title_id: String,
    },
}

/// Compares a normalized PARAM.SFO `TITLE_ID` against an optional parsed
/// [`Ps4ContentId`].
pub fn title_id_agreement(
    param_sfo_title_id: &str,
    content_id: Option<&Ps4ContentId>,
) -> Ps4TitleIdAgreement {
    let Some(content_id) = content_id else {
        return Ps4TitleIdAgreement::NotComparable;
    };
    if content_id.embedded_title_id == param_sfo_title_id {
        Ps4TitleIdAgreement::Agrees
    } else {
        Ps4TitleIdAgreement::Disagrees {
            param_sfo_title_id: param_sfo_title_id.to_string(),
            content_id_title_id: content_id.embedded_title_id.clone(),
        }
    }
}

/// Neutral [`ContentEvidence`] for a PS4 layout observation, for a future
/// platform-fusion / scanner consumer. Emits:
///
/// * a [`ContentEvidenceKind::BootStructure`] fact
///   ([`PS4_LAYOUT_EVIDENCE_MARKER`], `Strong`) **only** when the full
///   two-signal PS4 test passes - a valid `sce_sys/param.sfo` layout with a
///   `CUSA`-family `TITLE_ID`;
/// * the normalized `CUSA` title ID as a `ProductCode` (`Corroborated`);
/// * the raw `CONTENT_ID`, when present and well-formed, as a second
///   independent `ProductCode` (`Corroborated`) - never collapsed into the
///   title ID.
///
/// Returns an empty vec for anything that is not a valid PS4 layout, so a
/// PS3 or Vita `param.sfo` produces no PS4 evidence here.
pub fn observe_ps4_evidence(observation: &Ps4LayoutObservation) -> Vec<ContentEvidence> {
    let mut evidence = Vec::new();
    let Some(title_id) = observation.ps4_title_id() else {
        return evidence;
    };
    if !(observation.sce_sys_dir_present && observation.param_sfo_present) {
        return evidence;
    }
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        PS4_LAYOUT_EVIDENCE_MARKER,
        ContentEvidenceConfidence::Strong,
        "sce_sys/param.sfo layout present with a CUSA-family TITLE_ID - the CUSA \
         application-ID family is PS4-exclusive (PS Vita uses PCSx, PS3 uses BLxx/NPxx)",
    ));
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::ProductCode,
        title_id,
        ContentEvidenceConfidence::Corroborated,
        "PS4 TITLE_ID read from sce_sys/param.sfo",
    ));
    if let Some(content_id) = observation.ps4_content_id() {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ProductCode,
            content_id.raw,
            ContentEvidenceConfidence::Corroborated,
            "PS4 CONTENT_ID read from sce_sys/param.sfo - a separate fact, not merged with TITLE_ID",
        ));
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param_sfo::{SfoEntry, SfoValue};

    fn sfo(pairs: &[(&str, &str)]) -> SfoObservation {
        SfoObservation {
            entries: pairs
                .iter()
                .map(|(key, value)| SfoEntry {
                    key: key.to_string(),
                    value: SfoValue::Text(value.to_string()),
                })
                .collect(),
        }
    }

    fn layout(pairs: &[(&str, &str)]) -> Ps4LayoutObservation {
        Ps4LayoutObservation {
            sce_sys_dir_present: true,
            param_sfo_present: true,
            param_sfo: Some(sfo(pairs)),
        }
    }

    #[test]
    fn valid_cusa_title_id_normalizes() {
        assert_eq!(
            normalize_ps4_title_id("cusa00001"),
            Some("CUSA00001".to_string())
        );
        assert_eq!(
            normalize_ps4_title_id("  CUSA12345 "),
            Some("CUSA12345".to_string())
        );
    }

    #[test]
    fn ps3_and_vita_title_ids_are_rejected_as_ps4() {
        // PS3 disc / PSN
        assert_eq!(normalize_ps4_title_id("BLUS30000"), None);
        assert_eq!(normalize_ps4_title_id("NPEB00342"), None);
        // PS Vita region codes
        assert_eq!(normalize_ps4_title_id("PCSE00001"), None);
        assert_eq!(normalize_ps4_title_id("PCSB00001"), None);
        // Wrong length / shape
        assert_eq!(normalize_ps4_title_id("CUSA0001"), None);
        assert_eq!(normalize_ps4_title_id("CUSA000001"), None);
        assert_eq!(normalize_ps4_title_id("CUSAABCDE"), None);
        assert_eq!(normalize_ps4_title_id(""), None);
    }

    #[test]
    fn content_id_parses_and_embeds_cusa_title() {
        let parsed = parse_ps4_content_id("UP0001-CUSA00001_00-VALLYRIA00000000").unwrap();
        assert_eq!(parsed.prefix, "UP0001");
        assert_eq!(parsed.content_type, "00");
        assert_eq!(parsed.embedded_title_id, "CUSA00001");
        assert_eq!(parsed.raw, "UP0001-CUSA00001_00-VALLYRIA00000000");
    }

    #[test]
    fn content_id_lowercase_is_normalized() {
        let parsed = parse_ps4_content_id("ep0001-cusa54321_00-abcdef0123456789").unwrap();
        assert_eq!(parsed.embedded_title_id, "CUSA54321");
    }

    #[test]
    fn content_id_with_ps3_style_embedded_title_is_rejected() {
        assert!(parse_ps4_content_id("UP0001-NPEB00342_00-CONTENT0000DLPKG").is_none());
    }

    #[test]
    fn content_id_malformed_shapes_are_rejected() {
        assert!(parse_ps4_content_id("CUSA00001").is_none());
        assert!(parse_ps4_content_id("UP0001-CUSA00001-LABEL").is_none()); // no _type
        assert!(parse_ps4_content_id("UP0001-CUSA00001_00-LABEL-EXTRA").is_none());
        assert!(parse_ps4_content_id("UP0001-CUSA00001_AA-LABEL").is_none()); // type not digits
        assert!(parse_ps4_content_id("").is_none());
        assert!(parse_ps4_content_id(&"A".repeat(200)).is_none());
    }

    #[test]
    fn agreement_reports_match_mismatch_and_not_comparable() {
        let content = parse_ps4_content_id("UP0001-CUSA00001_00-LABEL00000000000").unwrap();
        assert_eq!(
            title_id_agreement("CUSA00001", Some(&content)),
            Ps4TitleIdAgreement::Agrees
        );
        assert_eq!(
            title_id_agreement("CUSA99999", Some(&content)),
            Ps4TitleIdAgreement::Disagrees {
                param_sfo_title_id: "CUSA99999".to_string(),
                content_id_title_id: "CUSA00001".to_string(),
            }
        );
        assert_eq!(
            title_id_agreement("CUSA00001", None),
            Ps4TitleIdAgreement::NotComparable
        );
    }

    #[test]
    fn layout_accessors_read_descriptive_metadata() {
        let observation = layout(&[
            ("TITLE_ID", "CUSA00001"),
            ("CONTENT_ID", "UP0001-CUSA00001_00-LABEL00000000000"),
            ("TITLE", "Example PS4 Game"),
            ("APP_VER", "01.02"),
            ("VERSION", "01.00"),
            ("CATEGORY", "gd"),
        ]);
        assert_eq!(observation.ps4_title_id(), Some("CUSA00001".to_string()));
        assert_eq!(observation.title(), Some("Example PS4 Game"));
        assert_eq!(observation.app_version(), Some("01.02"));
        assert_eq!(observation.version(), Some("01.00"));
        assert_eq!(observation.category(), Some("gd"));
        assert!(observation.is_valid_ps4_layout());
        assert_eq!(
            observation.ps4_content_id().map(|c| c.embedded_title_id),
            Some("CUSA00001".to_string())
        );
    }

    #[test]
    fn evidence_only_emitted_for_a_valid_ps4_layout() {
        let good = layout(&[("TITLE_ID", "CUSA00001")]);
        let evidence = observe_ps4_evidence(&good);
        assert!(evidence.iter().any(|item| {
            item.kind == ContentEvidenceKind::BootStructure
                && item.value == PS4_LAYOUT_EVIDENCE_MARKER
                && item.confidence == ContentEvidenceConfidence::Strong
        }));
        assert!(evidence.iter().any(|item| {
            item.kind == ContentEvidenceKind::ProductCode && item.value == "CUSA00001"
        }));

        // A PS3-style TITLE_ID under sce_sys yields no PS4 evidence.
        let ps3ish = layout(&[("TITLE_ID", "BLUS30000")]);
        assert!(observe_ps4_evidence(&ps3ish).is_empty());

        // Layout flags missing -> no evidence even with a CUSA id.
        let no_layout = Ps4LayoutObservation {
            sce_sys_dir_present: false,
            param_sfo_present: false,
            param_sfo: Some(sfo(&[("TITLE_ID", "CUSA00001")])),
        };
        assert!(observe_ps4_evidence(&no_layout).is_empty());
    }

    #[test]
    fn evidence_keeps_content_id_as_a_separate_product_code() {
        let observation = layout(&[
            ("TITLE_ID", "CUSA00001"),
            ("CONTENT_ID", "UP0001-CUSA00001_00-LABEL00000000000"),
        ]);
        let evidence = observe_ps4_evidence(&observation);
        let product_codes: Vec<&str> = evidence
            .iter()
            .filter(|item| item.kind == ContentEvidenceKind::ProductCode)
            .map(|item| item.value.as_str())
            .collect();
        assert!(product_codes.contains(&"CUSA00001"));
        assert!(product_codes.contains(&"UP0001-CUSA00001_00-LABEL00000000000"));
    }
}
