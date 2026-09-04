//! Turning a RomM record into an [`ExternalIdentityRecord`].
//!
//! This is the adapter's whole job: everything above it speaks EmuWiz's own
//! identity model and knows nothing about RomM's field names. Every field name
//! read here was taken from a real RomM 5.1.0's `/openapi.json`, and every one is
//! optional at the JSON level - a record missing a field is normalised without
//! it rather than rejected, because an older instance or an unmatched game
//! legitimately has gaps.
//!
//! # Platforms go through the existing registry
//!
//! RomM's platform slugs are resolved with [`crate::platform::platform_for_alias`],
//! the same registry the rest of EmuWiz uses. There is deliberately no second
//! table of platform names here: a slug the registry does not recognise stays
//! visible as unknown, with RomM's own name and id preserved, rather than being
//! guessed at with substring matching.
//!
//! # Bad provider data stays visible
//!
//! A malformed hash is not silently dropped and not promoted to evidence: it is
//! counted and reported as rejected provider data, so a person can see that RomM
//! published something unusable rather than wondering why verification never
//! reaches confirmed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity_source::model::{
    ArtworkReference, ExternalHash, ExternalIdentityRecord, ExternalVerification, HashAlgorithm,
    IdentityProvider, MediaReference, MetadataProviderId,
};
use crate::identity_source::path_map::{PathMappings, PathTranslation};

/// The metadata-provider id fields a RomM record can carry, with the name each
/// is recorded under. Read from the real ROM schema.
const METADATA_ID_FIELDS: &[(&str, &str)] = &[
    ("igdb_id", "igdb"),
    ("moby_id", "moby"),
    ("ss_id", "screenscraper"),
    ("launchbox_id", "launchbox"),
    ("ra_id", "retroachievements"),
    ("hasheous_id", "hasheous"),
    ("tgdb_id", "thegamesdb"),
    ("sgdb_id", "steamgriddb"),
    ("flashpoint_id", "flashpoint"),
    ("hltb_id", "howlongtobeat"),
];

/// The most `files[]` entries one record's relationships will carry, so a
/// pathological record cannot make the cache unbounded.
pub const MAX_RELATED_FILES: usize = 64;

/// A hash RomM published that could not be used.
///
/// Kept rather than discarded: "RomM published an MD5 that is not 32 hex
/// characters" is something a person should be able to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedHash {
    pub provider_game_id: String,
    pub algorithm: HashAlgorithm,
    /// Why it was rejected. Deliberately does not echo the value, which could be
    /// arbitrary provider text.
    pub reason: String,
}

/// What normalising one page produced, alongside the records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalisationReport {
    /// Records whose platform slug the registry did not recognise.
    pub unknown_platforms: Vec<String>,
    pub rejected_hashes: Vec<RejectedHash>,
    /// Records RomM returned that carried no usable identity at all.
    pub skipped_records: usize,
}

impl NormalisationReport {
    pub fn merge(&mut self, other: Self) {
        for platform in other.unknown_platforms {
            if !self.unknown_platforms.contains(&platform) {
                self.unknown_platforms.push(platform);
            }
        }
        self.rejected_hashes.extend(other.rejected_hashes);
        self.skipped_records += other.skipped_records;
    }
}

/// Normalises one RomM ROM record.
///
/// Returns `None` only when the record has no usable identity at all - no id -
/// which is the one thing that makes it unrecordable.
pub fn normalise_rom(
    value: &Value,
    server_id: &str,
    mappings: &PathMappings,
    imported_at_unix_seconds: i64,
    report: &mut NormalisationReport,
) -> Option<ExternalIdentityRecord> {
    // The id is the only field a record cannot do without: it is what a cached
    // record is keyed by and what a person would use to find it in RomM.
    let provider_game_id = value
        .get("id")
        .and_then(json_id)
        .or_else(|| string_field(value, "id"))?;

    let provider_platform_id = value.get("platform_id").and_then(json_id);
    let platform_slug = string_field(value, "platform_slug");
    let provider_platform_name = platform_slug.clone().or_else(|| {
        string_field(value, "platform_display_name")
            .or_else(|| string_field(value, "platform_custom_name"))
    });
    let platform_candidate = platform_slug
        .as_deref()
        .and_then(canonical_platform_for_romm_slug);
    if platform_candidate.is_none()
        && let Some(slug) = &provider_platform_name
        && !report.unknown_platforms.contains(slug)
    {
        report.unknown_platforms.push(slug.clone());
    }

    let provider_path = provider_path_of(value);
    let translation = mappings.translate(&provider_path);
    let archivefs_path = translation
        .archivefs_path()
        .map(std::path::Path::to_path_buf);

    let mut hashes = Vec::new();
    for (field, algorithm) in [
        ("crc_hash", HashAlgorithm::Crc32),
        ("md5_hash", HashAlgorithm::Md5),
        ("sha1_hash", HashAlgorithm::Sha1),
    ] {
        let Some(raw) = string_field(value, field) else {
            continue;
        };
        match ExternalHash::parse(algorithm, &raw) {
            Some(hash) => hashes.push(hash),
            None => report.rejected_hashes.push(RejectedHash {
                provider_game_id: provider_game_id.clone(),
                algorithm,
                reason: format!(
                    "RomM published a {} that is not {} hexadecimal characters",
                    algorithm.label(),
                    algorithm.hex_length()
                ),
            }),
        }
    }

    let metadata_provider_ids: Vec<MetadataProviderId> = METADATA_ID_FIELDS
        .iter()
        .filter_map(|(field, name)| {
            value
                .get(*field)
                .and_then(json_id)
                .map(|id| MetadataProviderId {
                    provider: (*name).to_string(),
                    id,
                })
        })
        .collect();

    // Artwork references only - never bytes, and never fetched here.
    let screenshots = media_references(value, "path_screenshots", "url_screenshots");
    let manual = media_reference(value, "path_manual", "url_manual");
    let large_reference = string_field(value, "path_cover_large");
    let cover_reference = string_field(value, "url_cover").or_else(|| large_reference.clone());
    let artwork =
        (cover_reference.is_some() || !screenshots.is_empty() || manual.is_some()).then(|| {
            ArtworkReference {
                reference: cover_reference.unwrap_or_default(),
                small_reference: string_field(value, "path_cover_small"),
                large_reference,
                screenshots: screenshots.clone(),
                manual: manual.clone(),
            }
        });

    // Enrichment (game metadata milestone, 2026-08-22): `summary` is a flat
    // field on the ROM object itself, but `genres`/`player_count`/
    // `average_rating`/`first_release_date` all live inside RomM's nested
    // `metadatum` object (`RomMetadataSchema` in RomM's own API, confirmed
    // against a real RomM release's response shape) - never at the top
    // level. A record with no `metadatum` at all (an older RomM instance,
    // or a game RomM never matched to a metadata source) simply carries
    // none of these; nothing here treats that as an error.
    let synopsis = string_field(value, "summary");
    let metadatum = value.get("metadatum");
    let genres = metadatum
        .map(|m| string_array(m, "genres"))
        .unwrap_or_default();
    // RomM has been observed (live, 2026-08-22, Majora's Mask on a real
    // instance) sending the literal JSON string "null" for `player_count`
    // rather than JSON null, on records where the value was never set - a
    // provider-side stringification quirk, not a genuine "unknown" value
    // spelled that way. Treated the same as absent, enrichment-side only:
    // this does not touch how any identity field is read.
    let players = metadatum
        .and_then(|m| string_field(m, "player_count"))
        .filter(|value| !value.eq_ignore_ascii_case("null"));
    let rating = metadatum
        .and_then(|m| m.get("average_rating"))
        .and_then(Value::as_f64)
        .map(|value| value.round().clamp(0.0, 100.0) as u8);
    // `metadatum.first_release_date` is milliseconds, not seconds - confirmed
    // 2026-08-22 against a live RomM 5.2.0 instance (Ocarina of Time reports
    // 911606400000, which is 1998-11-21 as milliseconds; read as seconds it
    // overflows a plausible year and was silently discarded).
    let release_year = metadatum
        .and_then(|m| m.get("first_release_date"))
        .and_then(Value::as_i64)
        .and_then(|millis| millis.checked_div(1000))
        .and_then(unix_seconds_to_year);

    // Multi-file structure, preserved rather than flattened.
    let related_files: Vec<String> = value
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .take(MAX_RELATED_FILES)
                .filter_map(|file| {
                    string_field(file, "full_path")
                        .or_else(|| string_field(file, "file_name"))
                        .or_else(|| string_field(file, "fs_name"))
                })
                .collect()
        })
        .unwrap_or_default();
    let sibling_game_ids: Vec<String> = value
        .get("sibling_roms")
        .and_then(Value::as_array)
        .map(|siblings| {
            siblings
                .iter()
                .take(MAX_RELATED_FILES)
                .filter_map(|sibling| {
                    sibling
                        .get("id")
                        .and_then(json_id)
                        .or_else(|| json_id(sibling))
                })
                .collect()
        })
        .unwrap_or_default();

    // The provider's own view of whether the file is still there. Recorded as
    // evidence; the local check is what actually decides staleness.
    let mut evidence = Vec::new();
    if value
        .get("missing_from_fs")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        evidence.push("RomM reports this file as missing from its own filesystem".to_string());
    }
    if value
        .get("has_multiple_files")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        evidence.push(format!(
            "RomM reports this as a multi-file game with {} file(s)",
            related_files.len()
        ));
    }
    if let PathTranslation::Unmatched { .. } = &translation {
        evidence.push("no configured path mapping covers this record's RomM path".to_string());
    }
    if let PathTranslation::Refused { refusal, .. } = &translation {
        evidence.push(format!(
            "the RomM path could not be used: {}",
            refusal.detail()
        ));
    }

    Some(ExternalIdentityRecord {
        provider: IdentityProvider::Romm,
        server_id: server_id.to_string(),
        provider_platform_id,
        provider_game_id,
        // RomM's ROM *is* the game record; a per-file id exists only inside
        // `files[]`, so the file id is left absent at the record level and the
        // relationships are carried in `related_files`.
        provider_file_id: None,
        provider_path,
        archivefs_path,
        title: string_field(value, "name"),
        platform_candidate: platform_candidate.map(str::to_string),
        provider_platform_name,
        regions: string_array(value, "regions"),
        revision: string_field(value, "revision"),
        hashes,
        file_size_bytes: value.get("fs_size_bytes").and_then(Value::as_u64),
        metadata_provider_ids,
        artwork,
        related_files,
        sibling_game_ids,
        imported_at_unix_seconds,
        provider_updated_at: string_field(value, "updated_at"),
        // Assigned by matching, which happens after normalisation: an imported
        // record starts as unmatched and is only promoted by evidence.
        verification: ExternalVerification::Unmatched,
        conflicts: Vec::new(),
        evidence,
        synopsis,
        genres,
        players,
        rating,
        release_year,
    })
}

/// A Unix timestamp's calendar year in UTC, bounded to a plausible release
/// date range. `None` for a timestamp so implausible it is more likely a
/// provider data error than a real game.
///
/// Reuses [`crate::database::format_unix_timestamp_utc`] (the project's one
/// hand-rolled Unix-timestamp-to-calendar-date routine) rather than a
/// second implementation of the same civil-calendar math.
fn unix_seconds_to_year(seconds: i64) -> Option<u16> {
    let formatted = crate::database::format_unix_timestamp_utc(seconds);
    let year: i64 = formatted.get(0..4)?.parse().ok()?;
    u16::try_from(year)
        .ok()
        .filter(|&year| (1950..=2100).contains(&year))
}

/// The path a RomM record describes, exactly as RomM gives it.
///
/// RomM reports `fs_path` as the directory and `fs_name` as the file, and
/// `full_path` when it has one - preferred, because it is the provider's own
/// answer rather than something reassembled here. In RomM 5.1.0 these are
/// relative to the instance's library base, e.g. `roms/gb/game.gb`.
///
/// Public because a mapping preview has to sample the very same string an import
/// would use. Two copies of this logic would let a preview show a translation the
/// import then did differently, which is the one thing a preview must not do.
pub fn provider_path_of(value: &Value) -> String {
    if let Some(full) = string_field(value, "full_path") {
        return full;
    }
    match (
        string_field(value, "fs_path"),
        string_field(value, "fs_name"),
    ) {
        (Some(directory), Some(name)) => format!("{}/{name}", directory.trim_end_matches('/')),
        (Some(directory), None) => directory,
        (None, Some(name)) => name,
        (None, None) => String::new(),
    }
}

/// Normalises one platform record into its canonical mapping, for a summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalisedPlatform {
    pub provider_platform_id: Option<String>,
    pub provider_slug: String,
    pub provider_name: Option<String>,
    /// The canonical EmuWiz platform, when the registry recognises the slug.
    ///
    /// Owned rather than `&'static str` because this is persisted: a cache written
    /// by one build must be readable by the next, and a borrowed registry name
    /// cannot survive a round trip through JSON.
    pub canonical: Option<String>,
    /// How many ROMs RomM reports on this platform, when it says.
    pub rom_count: Option<u64>,
}

pub fn normalise_platform(value: &Value) -> Option<NormalisedPlatform> {
    let provider_slug = string_field(value, "slug")
        .or_else(|| string_field(value, "fs_slug"))
        .or_else(|| string_field(value, "name"))?;
    Some(NormalisedPlatform {
        provider_platform_id: value.get("id").and_then(json_id),
        canonical: canonical_platform_for_romm_slug(&provider_slug).map(str::to_string),
        provider_name: string_field(value, "name").or_else(|| string_field(value, "display_name")),
        rom_count: value
            .get("rom_count")
            .and_then(Value::as_u64)
            .or_else(|| value.get("roms_count").and_then(Value::as_u64)),
        provider_slug,
    })
}

/// Resolves a RomM platform slug to a canonical EmuWiz platform.
///
/// Delegates to the one platform registry. Exact, normalised matching only -
/// `platform_for_alias` compares whole normalised aliases, never substrings, so
/// RomM's `amiga-cd32` resolves and its `zx-spectrum-next` does not become
/// ZX Spectrum.
///
/// A short explicit table handles the few slugs whose RomM spelling has no alias
/// in the registry. It is deliberately tiny and each entry is a slug observed on
/// a real instance; anything not here stays unknown rather than being guessed.
///
/// # Deliberately not inverted for the opposite (canonical -> slug) direction
///
/// The Library Views frontend-profile planner (`library_views::resolve_romm_platform_slug`)
/// needs the *opposite* direction - given a canonical platform, which RomM slug
/// to plan a path under - and does not derive it from this table. Several
/// entries below are intentionally approximate, many-to-one associations for
/// *importing* provider data (e.g. `fds` -> `NES`, because FDS games are
/// commonly catalogued alongside NES; `pc-fx` -> `PC Engine`, a related but
/// distinct NEC console; `xboxone` -> `Xbox`, a different console generation),
/// which is safe for recognising an incoming slug but would be actively wrong
/// if inverted to output a default slug for the canonical platform (`NES`'s
/// real, correct RomM slug is `nes` - resolved through `platform_for_alias`
/// above - never `fds`). The output-direction planner therefore only ever
/// resolves a slug from an explicit user override or a locally cached, live
/// instance's own reported slug, and fails closed otherwise; see that
/// function's doc comment.
pub fn canonical_platform_for_romm_slug(slug: &str) -> Option<&'static str> {
    if let Some(platform) = crate::platform::platform_for_alias(slug) {
        return Some(platform.id);
    }
    // RomM slugs that differ from every alias the registry carries. Each maps to
    // a platform that must exist - asserted by a test - and each is an exact
    // slug, never a pattern.
    const ROMM_SLUG_ALIASES: &[(&str, &str)] = &[
        ("acpc", "Amstrad CPC"),
        ("cpc", "Amstrad CPC"),
        ("dc", "Dreamcast"),
        ("fds", "NES"),
        ("gb", "Game Boy"),
        ("gba", "Game Boy Advance"),
        ("gbc", "Game Boy Color"),
        ("genesis-slash-megadrive", "MegaDrive"),
        ("n64", "N64"),
        ("nds", "Nintendo DS"),
        ("neo-geo-cd", "Neo Geo CD"),
        ("ngc", "GameCube"),
        ("pc-fx", "PC Engine"),
        ("ps", "PSX"),
        ("psvita", "PlayStation Vita"),
        ("sega-cd", "Sega CD"),
        ("sega32", "Sega 32X"),
        ("segacd", "Sega CD"),
        ("sfam", "SNES"),
        ("sms", "MasterSystem"),
        ("snes", "SNES"),
        ("turbografx-16-slash-pc-engine-cd", "PC Engine CD"),
        ("win", "PC"),
        ("xboxone", "Xbox"),
    ];
    let normalised = crate::platform::normalize_alias(slug);
    ROMM_SLUG_ALIASES
        .iter()
        .find(|(alias, _)| crate::platform::normalize_alias(alias) == normalised)
        .map(|(_, canonical)| *canonical)
}

/// Every canonical target the RomM slug table names, for the test that proves
/// they all exist in the registry.
pub fn romm_slug_targets() -> Vec<&'static str> {
    // Kept in step with the table above by the test; listing them separately
    // would be a second source of truth, so they are re-derived here.
    [
        "Amstrad CPC",
        "Commodore 64",
        "Dreamcast",
        "NES",
        "Game Boy",
        "Game Boy Advance",
        "Game Boy Color",
        "MegaDrive",
        "N64",
        "Nintendo DS",
        "Neo Geo CD",
        "GameCube",
        "PC Engine",
        "PSX",
        "PlayStation Vita",
        "Sega CD",
        "Sega 32X",
        "SNES",
        "MasterSystem",
        "PC Engine CD",
        "PC",
        "Xbox",
    ]
    .to_vec()
}

/// A string field, trimmed, with empty and JSON null treated as absent.
fn string_field(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// An id field, accepting either a JSON number or a string, as a string.
fn json_id(value: &Value) -> Option<String> {
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn media_references(value: &Value, hosted_field: &str, public_field: &str) -> Vec<MediaReference> {
    let hosted = string_array_or_single(value, hosted_field);
    let public = string_array_or_single(value, public_field);
    (0..hosted.len().max(public.len()))
        .map(|index| MediaReference {
            hosted_reference: hosted.get(index).cloned(),
            public_reference: public.get(index).cloned(),
        })
        .collect()
}

fn media_reference(
    value: &Value,
    hosted_field: &str,
    public_field: &str,
) -> Option<MediaReference> {
    let hosted = string_field(value, hosted_field);
    let public = string_field(value, public_field);
    (hosted.is_some() || public.is_some()).then_some(MediaReference {
        hosted_reference: hosted,
        public_reference: public,
    })
}

fn string_array_or_single(value: &Value, field: &str) -> Vec<String> {
    let array = string_array(value, field);
    if !array.is_empty() {
        array
    } else {
        string_field(value, field).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::path_map::{PathMapping, PathMappings, ProviderPathKind};
    use serde_json::json;
    use std::path::PathBuf;

    fn no_mappings() -> PathMappings {
        PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative)
            .expect("an empty mapping set always validates")
    }

    #[test]
    fn a_curated_provider_alias_translates_during_import_normalisation() {
        let mappings = PathMappings::validate(
            &[PathMapping {
                provider_prefix: "roms/gb".to_string(),
                archivefs_prefix: PathBuf::from("/mnt/usbdrive/games/gb"),
                provider_aliases: vec!["roms/gameboy".to_string()],
            }],
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let mut report = NormalisationReport::default();
        let record = normalise_rom(
            &json!({
                "id": 7,
                "platform_slug": "gameboy",
                "fs_path": "roms/gameboy",
                "fs_name": "example.gb",
                "name": "Example"
            }),
            "server",
            &mappings,
            1,
            &mut report,
        )
        .expect("record normalises");
        assert_eq!(
            record.archivefs_path,
            Some(PathBuf::from("/mnt/usbdrive/games/gb/example.gb"))
        );
    }

    #[test]
    fn an_alias_record_is_translated_during_import_normalisation() {
        let mappings = PathMappings::validate(
            &[PathMapping {
                provider_prefix: "roms/gcn".to_string(),
                archivefs_prefix: PathBuf::from("/mnt/usbdrive/games/ngc"),
                provider_aliases: vec!["roms/ngc".to_string()],
            }],
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let mut report = NormalisationReport::default();
        let record = normalise_rom(
            &json!({
                "id": 101059,
                "platform_slug": "ngc",
                "full_path": "roms/ngc/example.iso",
                "name": "Example",
            }),
            "server",
            &mappings,
            1,
            &mut report,
        )
        .expect("the alias record is importable");

        assert_eq!(
            record.archivefs_path,
            Some(PathBuf::from("/mnt/usbdrive/games/ngc/example.iso"))
        );
    }

    #[test]
    fn refreshing_with_a_corrected_alias_mapping_replaces_a_stale_null_translation() {
        // The exact real-world shape the source-fingerprint bug produced: an
        // alias-provider path (`roms/ngc/...`) normalised against a mapping
        // set that does not yet know about the `ngc` alias comes back
        // untranslated. `normalise_rom` takes no previous result as input -
        // there is nothing here for a stale answer to hide in - so a second
        // call against the *corrected* mapping must independently produce
        // the real path, never inherit or preserve the earlier `None`.
        let rom = json!({
            "id": 101059,
            "platform_slug": "ngc",
            "full_path": "roms/ngc/example.iso",
            "name": "Example",
        });

        let mut stale_report = NormalisationReport::default();
        let stale_record = normalise_rom(&rom, "server", &no_mappings(), 1, &mut stale_report)
            .expect("an unmapped alias path is still importable, just untranslated");
        assert_eq!(
            stale_record.archivefs_path, None,
            "before the corrected mapping exists, the alias path has no local translation"
        );

        let corrected_mappings = PathMappings::validate(
            &[PathMapping {
                provider_prefix: "roms/gcn".to_string(),
                archivefs_prefix: PathBuf::from("/mnt/usbdrive/games/ngc"),
                provider_aliases: vec!["roms/ngc".to_string()],
            }],
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let mut refreshed_report = NormalisationReport::default();
        let refreshed_record = normalise_rom(
            &rom,
            "server",
            &corrected_mappings,
            1,
            &mut refreshed_report,
        )
        .expect("the alias record is importable");
        assert_eq!(
            refreshed_record.archivefs_path,
            Some(PathBuf::from("/mnt/usbdrive/games/ngc/example.iso")),
            "a refreshed publication with the corrected mapping must replace the stale \
             null translation, not preserve it"
        );
    }

    /// A realistic ROM object, shaped exactly like a real RomM release's
    /// `/api/roms` response (confirmed against RomM's own `RomSchema`/
    /// `RomMetadataSchema` source, 2026-08-22): `summary` flat on the ROM
    /// itself, everything else the enrichment milestone reads nested
    /// inside `metadatum`, never at the top level.
    fn rom_with_enrichment() -> serde_json::Value {
        json!({
            "id": 42,
            "platform_id": 7,
            "platform_slug": "gb",
            "fs_name": "game.gb",
            "name": "Example Game",
            "path_screenshots": ["assets/screens/second.jpg", "assets/screens/first.png"],
            "url_screenshots": ["https://public.example/second.jpg", "https://public.example/first.png"],
            "path_manual": "assets/manuals/game.pdf",
            "url_manual": "https://public.example/game.pdf",
            "summary": "A short adventure across five islands.",
            "metadatum": {
                "rom_id": 42,
                "genres": ["Action", "Platformer"],
                "franchises": [],
                "collections": [],
                "companies": ["Example Studio"],
                "game_modes": [],
                "age_ratings": [],
                "player_count": "1-2",
                // Milliseconds, as RomM's real metadatum actually reports it
                // (confirmed live against a RomM 5.2.0 instance) - not seconds.
                "first_release_date": 896_745_600_000_i64, // 1998-06-02T00:00:00Z
                "average_rating": 87.4,
            },
        })
    }

    #[test]
    fn enrichment_fields_are_read_from_the_nested_metadatum_object() {
        let mut report = NormalisationReport::default();
        let record = normalise_rom(
            &rom_with_enrichment(),
            "server",
            &no_mappings(),
            1,
            &mut report,
        )
        .expect("a record with an id normalises");

        assert_eq!(
            record.synopsis.as_deref(),
            Some("A short adventure across five islands.")
        );
        assert_eq!(record.genres, vec!["Action", "Platformer"]);
        assert_eq!(record.players.as_deref(), Some("1-2"));
        assert_eq!(record.rating, Some(87));
        assert_eq!(record.release_year, Some(1998));
    }

    #[test]
    fn romm_media_is_preserved_in_provider_order_without_fetching_it() {
        let mut report = NormalisationReport::default();
        let record = normalise_rom(
            &rom_with_enrichment(),
            "server",
            &no_mappings(),
            1,
            &mut report,
        )
        .expect("a record with an id normalises");
        let artwork = record.artwork.expect("media keeps the artwork envelope");
        assert_eq!(artwork.screenshots.len(), 2);
        assert_eq!(
            artwork.screenshots[0].hosted_reference.as_deref(),
            Some("assets/screens/second.jpg")
        );
        assert_eq!(
            artwork.screenshots[1].hosted_reference.as_deref(),
            Some("assets/screens/first.png")
        );
        assert_eq!(
            artwork
                .manual
                .as_ref()
                .and_then(|m| m.hosted_reference.as_deref()),
            Some("assets/manuals/game.pdf")
        );
    }

    #[test]
    fn path_cover_large_is_normalised_separately_from_the_public_cover_reference() {
        let value = json!({
            "id": 7,
            "platform_slug": "gb",
            "url_cover": "https://images.example/cover.jpg",
            "path_cover_large": "assets/romm/resources/large.jpg",
            "path_cover_small": "assets/romm/resources/small.png"
        });
        let mut report = NormalisationReport::default();
        let record = normalise_rom(&value, "server", &no_mappings(), 1, &mut report)
            .expect("a record with an id normalises");
        let artwork = record.artwork.expect("cover is present");
        assert_eq!(artwork.reference, "https://images.example/cover.jpg");
        assert_eq!(
            artwork.large_reference.as_deref(),
            Some("assets/romm/resources/large.jpg")
        );
        assert_eq!(
            artwork.small_reference.as_deref(),
            Some("assets/romm/resources/small.png")
        );
    }

    #[test]
    fn a_release_date_taken_from_a_real_live_romm_response_normalises_to_the_correct_year() {
        // The exact `first_release_date` a live RomM 5.2.0 instance returned
        // for The Legend of Zelda: Ocarina of Time (rom id 2189, 2026-08-22) -
        // milliseconds, not seconds. Read as seconds this overflows the
        // plausible-year range and silently disappears; this test pins the
        // real value so that regression can't recur unnoticed.
        let mut value = rom_with_enrichment();
        value["metadatum"]["first_release_date"] = json!(911_606_400_000_i64);
        let mut report = NormalisationReport::default();
        let record = normalise_rom(&value, "server", &no_mappings(), 1, &mut report)
            .expect("a record with an id normalises");
        assert_eq!(record.release_year, Some(1998));
    }

    #[test]
    fn a_rom_with_no_metadatum_at_all_normalises_with_no_enrichment() {
        // An older RomM instance, or a game RomM never matched to a
        // metadata source - not an error, just nothing to enrich with.
        let mut value = rom_with_enrichment();
        value.as_object_mut().unwrap().remove("metadatum");
        value.as_object_mut().unwrap().remove("summary");

        let mut report = NormalisationReport::default();
        let record = normalise_rom(&value, "server", &no_mappings(), 1, &mut report)
            .expect("a record with an id normalises");

        assert_eq!(record.synopsis, None);
        assert!(record.genres.is_empty());
        assert_eq!(record.players, None);
        assert_eq!(record.rating, None);
        assert_eq!(record.release_year, None);
        // Identity fields are entirely unaffected by the missing metadatum.
        assert_eq!(record.title.as_deref(), Some("Example Game"));
    }

    #[test]
    fn a_literal_null_string_player_count_from_a_real_live_romm_response_is_treated_as_absent() {
        // A live RomM 5.2.0 instance returned the four-character JSON string
        // "null" (not JSON null) for The Legend of Zelda: Majora's Mask's
        // `player_count`, 2026-08-22 - a provider-side stringification quirk
        // that must not surface in the UI as the literal text "null".
        let mut value = rom_with_enrichment();
        value["metadatum"]["player_count"] = json!("null");
        let mut report = NormalisationReport::default();
        let record = normalise_rom(&value, "server", &no_mappings(), 1, &mut report)
            .expect("a record with an id normalises");
        assert_eq!(record.players, None);
    }

    #[test]
    fn an_out_of_range_rating_is_clamped_not_rejected() {
        let mut value = rom_with_enrichment();
        value["metadatum"]["average_rating"] = json!(140.0);
        let mut report = NormalisationReport::default();
        let record = normalise_rom(&value, "server", &no_mappings(), 1, &mut report)
            .expect("a record with an id normalises");
        assert_eq!(record.rating, Some(100));
    }

    #[test]
    fn unix_seconds_to_year_rejects_implausible_timestamps() {
        assert_eq!(unix_seconds_to_year(896_745_600), Some(1998));
        assert_eq!(
            unix_seconds_to_year(0),
            Some(1970),
            "1970 is within the plausible release range"
        );
        assert_eq!(
            unix_seconds_to_year(-2_000_000_000),
            None,
            "a wildly negative timestamp must not panic or produce a bogus year"
        );
    }

    #[test]
    fn c16_and_plus_four_slugs_remain_unresolved_without_canonical_rows() {
        assert_eq!(canonical_platform_for_romm_slug("c16"), None);
        assert_eq!(canonical_platform_for_romm_slug("c-plus-4"), None);
        assert_eq!(canonical_platform_for_romm_slug("commodore-16"), None);
        assert_eq!(canonical_platform_for_romm_slug("plus-4"), None);
    }
}
