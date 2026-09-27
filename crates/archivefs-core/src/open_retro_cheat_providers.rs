//! Legal/open cheat-provider metadata and conservative local projections.
//!
//! This module does not download, execute, or install cheat payloads.  It
//! provides provider declarations plus bounded parsers for the documented ZX
//! `.pok` text format and WHDLoad slave custom-option declarations.  Existing
//! identity matching and emulator-specific installers remain authoritative.

use serde::{Deserialize, Serialize};

use crate::patch_manager::{CheatDocument, CheatOperation, CheatPlatform, CheatSourceFormat};

pub const ZXDB_URL: &str = "https://github.com/zxdb/ZXDB";
pub const LIBRETRO_CHEAT_URL: &str = "https://github.com/libretro/libretro-database";
pub const WHDLOAD_OPTIONS_URL: &str = "https://www.whdload.net/docs/en/opt.html";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCheatProviderMode {
    Bundled,
    Downloadable,
    MetadataIndex,
    UserImport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCheatTrustState {
    OpenLicenceDeclared,
    AttributionRequired,
    LicenceUnclear,
    LocalUserSupplied,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCheatProvider {
    pub id: String,
    pub name: String,
    pub source_url: String,
    pub licence: String,
    pub redistribution_status: String,
    pub update_method: String,
    pub platform_coverage: Vec<String>,
    pub format: String,
    pub provenance: String,
    pub last_update: Option<String>,
    pub mode: OpenCheatProviderMode,
    pub trust: OpenCheatTrustState,
}

/// Providers whose terms are clear enough to describe. Payloads are not
/// bundled here: ZXDB is an index/metadata source and Libretro remains an
/// explicit user-requested downloadable snapshot.
pub fn open_cheat_provider_catalogue() -> Vec<OpenCheatProvider> {
    vec![
        OpenCheatProvider {
            id: "zxdb".into(),
            name: "ZXDB metadata index".into(),
            source_url: ZXDB_URL.into(),
            licence: "ODbL 1.0 / attribution and open-derivative terms as declared upstream".into(),
            redistribution_status:
                "Do not bundle derived database content without preserving terms".into(),
            update_method: "Explicit user-requested metadata/index update".into(),
            platform_coverage: vec!["ZX Spectrum".into()],
            format: "ZXDB metadata; .pok references where available".into(),
            provenance: "ZXDB upstream project and its published licence guidance".into(),
            last_update: None,
            mode: OpenCheatProviderMode::MetadataIndex,
            trust: OpenCheatTrustState::AttributionRequired,
        },
        OpenCheatProvider {
            id: "libretro-cheats".into(),
            name: "Libretro cheat database".into(),
            source_url: LIBRETRO_CHEAT_URL.into(),
            licence: "Upstream repository licence and per-source provenance must be retained"
                .into(),
            redistribution_status: "Not bundled; explicit downloadable snapshot only".into(),
            update_method: "Existing pinned HTTPS snapshot workflow".into(),
            platform_coverage: vec!["Multiple libretro systems".into()],
            format: "RetroArch .cht".into(),
            provenance: "libretro/libretro-database; community-contributed records".into(),
            last_update: None,
            mode: OpenCheatProviderMode::Downloadable,
            trust: OpenCheatTrustState::AttributionRequired,
        },
        OpenCheatProvider {
            id: "whdload-local".into(),
            name: "Installed WHDLoad slave options".into(),
            source_url: WHDLOAD_OPTIONS_URL.into(),
            licence: "Options are read from the user’s installed slave/package".into(),
            redistribution_status: "Never redistributed by EmuWiz".into(),
            update_method: "Read-only local inspection".into(),
            platform_coverage: vec!["Amiga".into()],
            format: "WHDLoad slave custom-option declaration".into(),
            provenance: "Installed slave path and SHA-256".into(),
            last_update: None,
            mode: OpenCheatProviderMode::UserImport,
            trust: OpenCheatTrustState::LocalUserSupplied,
        },
        OpenCheatProvider {
            id: "c64-user-import".into(),
            name: "Commodore 64 local cheat import".into(),
            source_url: "".into(),
            licence: "No redistribution source established".into(),
            redistribution_status: "User import only".into(),
            update_method: "Local file selection".into(),
            platform_coverage: vec!["Commodore 64".into()],
            format: "Provider-specific/user-supplied".into(),
            provenance: "Local user file".into(),
            last_update: None,
            mode: OpenCheatProviderMode::UserImport,
            trust: OpenCheatTrustState::LicenceUnclear,
        },
        OpenCheatProvider {
            id: "atari-st-user-import".into(),
            name: "Atari ST local cheat import".into(),
            source_url: "".into(),
            licence: "No redistribution source established".into(),
            redistribution_status: "User import only".into(),
            update_method: "Local file selection".into(),
            platform_coverage: vec!["Atari ST".into()],
            format: "Provider-specific/user-supplied".into(),
            provenance: "Local user file".into(),
            last_update: None,
            mode: OpenCheatProviderMode::UserImport,
            trust: OpenCheatTrustState::LicenceUnclear,
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZxPokOperation {
    pub bank: u8,
    pub address: u16,
    pub value: u8,
    pub original_value: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZxPokTrainer {
    pub title: String,
    pub operations: Vec<ZxPokOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZxPokError {
    Empty,
    InvalidLine { line: usize, detail: String },
    MissingTrainer { line: usize },
    MissingTerminator,
    TooManyLines,
}

pub fn parse_zx_pok(input: &str) -> Result<Vec<ZxPokTrainer>, ZxPokError> {
    let mut trainers = Vec::new();
    let mut current: Option<ZxPokTrainer> = None;
    let mut terminated = false;
    for (index, raw) in input.lines().enumerate() {
        if index >= 4096 {
            return Err(ZxPokError::TooManyLines);
        }
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (kind, rest) = line.split_at(1);
        match kind {
            "N" => {
                if let Some(trainer) = current.take() {
                    trainers.push(trainer);
                }
                current = Some(ZxPokTrainer {
                    title: rest.trim().to_string(),
                    operations: Vec::new(),
                });
            }
            "M" | "Z" => {
                let trainer = current
                    .as_mut()
                    .ok_or(ZxPokError::MissingTrainer { line: index + 1 })?;
                let numbers: Vec<_> = rest.split_whitespace().collect();
                if numbers.len() != 4 {
                    return Err(ZxPokError::InvalidLine {
                        line: index + 1,
                        detail: "POKE requires bank, address, value and original value".into(),
                    });
                }
                let bank = parse_bounded(numbers[0], 8, index + 1)?;
                let address = parse_bounded(numbers[1], u16::MAX as u32, index + 1)? as u16;
                let value = parse_bounded(numbers[2], u8::MAX as u32, index + 1)? as u8;
                let original = parse_bounded(numbers[3], u8::MAX as u32, index + 1)? as u8;
                trainer.operations.push(ZxPokOperation {
                    bank: bank as u8,
                    address,
                    value,
                    original_value: Some(original),
                });
                if kind == "Z" {
                    trainers.push(current.take().expect("current trainer exists"));
                }
            }
            "Y" => {
                terminated = true;
                break;
            }
            _ => {
                return Err(ZxPokError::InvalidLine {
                    line: index + 1,
                    detail: "only N, M, Z and Y records are supported".into(),
                });
            }
        }
    }
    if !terminated {
        return Err(ZxPokError::MissingTerminator);
    }
    if trainers.is_empty() {
        return Err(ZxPokError::Empty);
    }
    Ok(trainers)
}

pub fn normalize_zx_pok(trainer: &ZxPokTrainer) -> CheatDocument {
    CheatDocument {
        title: trainer.title.clone(),
        platform: CheatPlatform::Other("ZX Spectrum".into()),
        source_format: CheatSourceFormat::Other("ZX .pok".into()),
        operations: trainer
            .operations
            .iter()
            .map(|operation| CheatOperation::Write8 {
                address: operation.address as u64,
                value: operation.value,
            })
            .collect(),
        issues: Vec::new(),
        provenance: vec!["ZX POK format; bank retained in source projection".into()],
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhdloadCustomOption {
    pub key: String,
    pub option_type: String,
    pub description: String,
    pub default_value: Option<String>,
    /// The raw documented `Spec` portion after the label.  It is retained so
    /// an adapter can interpret lists and bit ranges without guessing from a
    /// trainer name.
    #[serde(default)]
    pub spec: Option<String>,
    pub documented: bool,
}

/// Parses the documented `C1:X:Description:default;` style declaration from
/// a WHDLoad slave. It does not infer that an option is a trainer or mutate a
/// slave; callers must retain the slave path/hash as provenance.
/// Official ws_config types are B/L/M/X. N is retained only as EmuWiz's
/// internal numeric compatibility representation. Other one-letter records
/// remain visible as opaque declarations but are never applyable.
pub fn parse_whdload_custom_options(config: &str) -> Vec<WhdloadCustomOption> {
    config
        .split(';')
        .filter_map(|raw| {
            let fields: Vec<_> = raw.splitn(4, ':').collect();
            if fields.len() < 3 || !fields[0].starts_with('C') {
                return None;
            }
            let key = fields[0].to_string();
            let option_type = fields[1].to_string();
            if !matches!(key.as_str(), "C1" | "C2" | "C3" | "C4" | "C5") || option_type.len() != 1 {
                return None;
            }
            let spec = fields
                .get(3)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let documented = matches!(option_type.as_str(), "B" | "L" | "M" | "X" | "N");
            Some(WhdloadCustomOption {
                key,
                option_type,
                description: fields[2].trim().to_string(),
                default_value: spec.clone(),
                spec,
                documented,
            })
        })
        .collect()
}

fn parse_bounded(value: &str, maximum: u32, line: usize) -> Result<u32, ZxPokError> {
    let parsed = value.parse::<u32>().map_err(|_| ZxPokError::InvalidLine {
        line,
        detail: format!("invalid decimal value {value}"),
    })?;
    if parsed > maximum {
        return Err(ZxPokError::InvalidLine {
            line,
            detail: format!("value {parsed} exceeds {maximum}"),
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_zx_pok_and_normalizes_without_execution() {
        let trainers = parse_zx_pok("NInfinite Lives\nM 8 45523 5 0\nZ 8 45525 0 25\nY\n").unwrap();
        assert_eq!(trainers[0].operations.len(), 2);
        assert_eq!(normalize_zx_pok(&trainers[0]).operations.len(), 2);
    }

    #[test]
    fn malformed_poke_is_rejected() {
        assert!(matches!(
            parse_zx_pok("NBad\nZ 8 nope 0 0\nY"),
            Err(ZxPokError::InvalidLine { .. })
        ));
        assert!(matches!(
            parse_zx_pok("NBad\nZ 8 1 2 3"),
            Err(ZxPokError::MissingTerminator)
        ));
    }

    #[test]
    fn whdload_options_are_projected_only_when_documented() {
        let options = parse_whdload_custom_options(
            "C2:X:Activate Trainer:0;C3:B:Infinite lives:1;C4:Q:unknown:0;",
        );
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].key, "C2");
        assert_eq!(options[1].default_value.as_deref(), Some("1"));
        assert!(!options[2].documented);
    }

    #[test]
    fn provider_catalogue_refuses_unclear_sources_to_bundled_mode() {
        let c64 = open_cheat_provider_catalogue()
            .into_iter()
            .find(|p| p.id == "c64-user-import")
            .unwrap();
        assert_eq!(c64.mode, OpenCheatProviderMode::UserImport);
        assert_eq!(c64.trust, OpenCheatTrustState::LicenceUnclear);
    }
}
