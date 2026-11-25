//! ALCHEMY Q1-A tests. Synthetic bytes only; no game data. Everything is
//! written inside a private temporary directory: the configured diagnostics
//! folder lives there, and tests never fall back to the real data directory
//! or a real filesystem root (mount roots are simulated).

use super::diagnostics::DIAGNOSTICS_DIR_NAME;
use super::*;
use crate::recovery::prepared_media::FREE_SPACE_OVERRIDE;
use flate2::write::ZlibEncoder;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

// ---- fixtures -------------------------------------------------------------

/// The deterministic synthetic ISO the maxcso fixture was made from: every
/// third sector constant, every third text, every third a SHA-256 chain
/// (incompressible, so it is stored plain).
fn synthetic_iso() -> Vec<u8> {
    let mut out = Vec::new();
    let mut chain: [u8; 32] = Sha256::digest(b"alchemy-q1-synthetic").into();
    for i in 0..96usize {
        match i % 3 {
            0 => out.extend(std::iter::repeat_n((i % 251) as u8, CSO_BLOCK_SIZE)),
            1 => out.extend_from_slice(
                &format!("PSP SYNTHETIC SECTOR {i:04} ")
                    .repeat(80)
                    .as_bytes()[..CSO_BLOCK_SIZE],
            ),
            _ => {
                let mut sector = Vec::with_capacity(CSO_BLOCK_SIZE + 32);
                while sector.len() < CSO_BLOCK_SIZE {
                    chain = Sha256::digest(chain).into();
                    sector.extend_from_slice(&chain);
                }
                out.extend_from_slice(&sector[..CSO_BLOCK_SIZE]);
            }
        }
    }
    out
}

const SYNTHETIC_ISO_SHA256: &str =
    "69150feecd8d0e65d7c884f8efa6d50d8f7e3cfb3dd1690f2fb6d903f6710801";
/// Made by maxcso v1.13.0 (`maxcso --block=2048`) from `synthetic_iso()`;
/// see tests/fixtures/psp_cso/README.md.
const MAXCSO_FIXTURE: &[u8] =
    include_bytes!("../../tests/fixtures/psp_cso/maxcso-1.13.0-synthetic.cso");
const ISO_LEN: u64 = 96 * CSO_BLOCK_SIZE as u64;

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A private scratch tree: `<tmp>/games` (a source folder) and `<tmp>/out`.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir(root.join("games")).unwrap();
        fs::create_dir(root.join("out")).unwrap();
        // The configured diagnostics folder (preference 1) exists already,
        // as a configured folder must; EmuWiz never creates it.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(".emuwiz-diagnostics"))
            .unwrap();
        Self { _dir: dir, root }
    }
    fn source(&self) -> (PathBuf, Vec<u8>) {
        let path = self.root.join("games").join("PSP game with spaces.iso");
        let bytes = synthetic_iso();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    }
    fn output(&self) -> PathBuf {
        self.root.join("out").join("result.cso")
    }
    fn placement(&self) -> DiagnosticsPlacement {
        // Tests never fall back to the real data directory or mount root.
        DiagnosticsPlacement {
            source_folders: vec![self.root.join("games")],
            configured_dirs: vec![self.root.join(".emuwiz-diagnostics")],
            data_dir: None,
            allow_mount_root: false,
        }
    }
    fn diagnostics(&self) -> PathBuf {
        self.root.join(".emuwiz-diagnostics").join("psp-cso")
    }
    fn entries(&self, sub: &str) -> Vec<PathBuf> {
        let mut found: Vec<_> = fs::read_dir(self.diagnostics().join(sub))
            .map(|read| read.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        found.sort();
        found
    }
    fn manifest_lines(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.diagnostics().join("manifest.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn reference() -> IndependentCsoValidator {
    IndependentCsoValidator {
        kind: ValidatorKind::TestReference,
    }
}

fn disagreeing() -> IndependentCsoValidator {
    IndependentCsoValidator {
        kind: ValidatorKind::TestDisagrees,
    }
}

/// Test-only independent decoder, written separately from the production
/// reader straight from the public format and checked against the maxcso
/// fixture below. It is never available outside `cfg(test)`.
pub(super) fn reference_restore_sha256(cso: &Path) -> Result<String, PspShrinkError> {
    let bytes = fs::read(cso)?;
    let bad = |why: &str| PspShrinkError::InvalidCso(format!("reference decoder: {why}"));
    let word = |at: usize| -> Result<u32, PspShrinkError> {
        Ok(u32::from_le_bytes(
            bytes
                .get(at..at + 4)
                .ok_or_else(|| bad("short"))?
                .try_into()
                .unwrap(),
        ))
    };
    if bytes.get(0..4) != Some(b"CISO") {
        return Err(bad("magic"));
    }
    let total = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let block = word(16)? as usize;
    let align = u32::from(bytes[21]);
    let mut out = Vec::with_capacity(total);
    for i in 0..total / block {
        let this = word(24 + 4 * i)?;
        let next = word(28 + 4 * i)?;
        let start = ((this & 0x7fff_ffff) as usize) << align;
        let end = ((next & 0x7fff_ffff) as usize) << align;
        let payload = bytes.get(start..end).ok_or_else(|| bad("range"))?;
        if this & 0x8000_0000 != 0 {
            out.extend_from_slice(payload.get(..block).ok_or_else(|| bad("plain"))?);
        } else {
            let mut inflater = flate2::Decompress::new(false);
            let mut sector = vec![0u8; block];
            inflater
                .decompress(payload, &mut sector, flate2::FlushDecompress::Finish)
                .map_err(|_| bad("inflate"))?;
            if inflater.total_out() as usize != block {
                return Err(bad("short block"));
            }
            out.extend_from_slice(&sector);
        }
    }
    Ok(sha_hex(&out))
}

/// A standard CSO v1 writer for boundary tests (any `align`), written the
/// way PPSSPP/PCSX2/maxcso read the format.
fn standard_cso(iso: &[u8], align: u8) -> Vec<u8> {
    let blocks = iso.len() / CSO_BLOCK_SIZE;
    let unit = 1usize << align;
    let mut body = Vec::new();
    let mut position = 24 + 4 * (blocks + 1);
    let pad = |body: &mut Vec<u8>, position: &mut usize| {
        while !(*position).is_multiple_of(unit) {
            body.push(0);
            *position += 1;
        }
    };
    pad(&mut body, &mut position);
    let mut index = Vec::with_capacity(blocks + 1);
    for sector in iso.chunks(CSO_BLOCK_SIZE) {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(sector).unwrap();
        let compressed = encoder.finish().unwrap();
        let (payload, flag) = if compressed.len() < sector.len() {
            (compressed, 0)
        } else {
            (sector.to_vec(), CSO_PLAIN_FLAG)
        };
        index.push((position >> align) as u32 | flag);
        position += payload.len();
        body.extend_from_slice(&payload);
        pad(&mut body, &mut position);
    }
    index.push((position >> align) as u32);
    let mut out = b"CISO".to_vec();
    out.extend_from_slice(&24u32.to_le_bytes());
    out.extend_from_slice(&(iso.len() as u64).to_le_bytes());
    out.extend_from_slice(&(CSO_BLOCK_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&[1, align, 0, 0]);
    for entry in index {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

fn set_index(cso: &mut [u8], i: usize, value: u32) {
    cso[24 + 4 * i..28 + 4 * i].copy_from_slice(&value.to_le_bytes());
}

fn get_index(cso: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(cso[24 + 4 * i..28 + 4 * i].try_into().unwrap())
}

/// Decodes `cso` bytes with the production standard reader.
fn decode(cso: &[u8], size: u64) -> Result<Vec<u8>, PspShrinkError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.cso");
    fs::write(&path, cso).unwrap();
    let restored = dir.path().join("restored.iso");
    let result =
        restore_standard_cso(&path, &restored, size).map(|()| fs::read(&restored).unwrap());
    if result.is_err() {
        assert!(!restored.exists(), "a failed decode leaves no partial file");
    }
    result
}

/// The pre-correction EmuWiz writer, kept only to produce legacy test files.
fn write_legacy_emuwiz_cso(iso: &[u8]) -> Vec<u8> {
    let blocks = iso.len() / CSO_BLOCK_SIZE;
    let mut position = 24 + 4 * (blocks + 1);
    let mut index = vec![0u32; blocks + 1];
    let mut body = Vec::new();
    for (i, block) in iso.chunks(CSO_BLOCK_SIZE).enumerate() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(block).unwrap();
        let compressed = encoder.finish().unwrap();
        let (payload, flag) = if compressed.len() < block.len() {
            (compressed, LEGACY_COMPRESSED_FLAG)
        } else {
            (block.to_vec(), 0)
        };
        index[i] = (position / 4) as u32 | flag;
        body.extend_from_slice(&payload);
        position += payload.len();
        while !position.is_multiple_of(4) {
            body.push(0);
            position += 1;
        }
    }
    index[blocks] = (position / 4) as u32;
    let mut out = b"CISO".to_vec();
    out.extend_from_slice(&24u32.to_le_bytes());
    out.extend_from_slice(&(iso.len() as u64).to_le_bytes());
    out.extend_from_slice(&(CSO_BLOCK_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&[1, 0, 0, 0]);
    for entry in index {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

// ---- independent references -----------------------------------------------

#[test]
fn the_synthetic_iso_is_the_one_the_fixture_was_made_from() {
    assert_eq!(sha_hex(&synthetic_iso()), SYNTHETIC_ISO_SHA256);
}

#[test]
fn the_reader_and_the_test_reference_decode_the_maxcso_fixture_byte_for_byte() {
    // Independent writer -> EmuWiz reader, and -> the test reference.
    let dir = tempfile::tempdir().unwrap();
    let cso = dir.path().join("maxcso.cso");
    fs::write(&cso, MAXCSO_FIXTURE).unwrap();
    assert_eq!(
        classify_cso_dialect(&cso).unwrap(),
        CsoDialect::Standard { align: 0 }
    );
    assert_eq!(decode(MAXCSO_FIXTURE, ISO_LEN).unwrap(), synthetic_iso());
    assert_eq!(
        reference_restore_sha256(&cso).unwrap(),
        SYNTHETIC_ISO_SHA256
    );
}

#[test]
fn the_maxcso_version_is_read_from_stderr_as_maxcso_1_13_prints_it() {
    assert_eq!(
        maxcso_version_line(b"", b"maxcso v1.13.0\n").as_deref(),
        Some("maxcso v1.13.0")
    );
    assert_eq!(
        maxcso_version_line(b"maxcso v1.14.0\n", b"").as_deref(),
        Some("maxcso v1.14.0")
    );
    assert_eq!(maxcso_version_line(b"", b"").as_deref(), None);
    assert_eq!(
        maxcso_version_line(b"7-Zip 23.01\n", b"usage: x\n").as_deref(),
        None
    );
}

#[test]
fn maxcso_decodes_the_written_cso_when_it_is_installed() {
    // Real EmuWiz writer -> real maxcso. The Q1-A gate requires this test to
    // RUN (not skip) on the implementation machine; CI logs a visible skip.
    let validator = match IndependentCsoValidator::locate() {
        Ok(validator) => validator,
        Err(error) => {
            eprintln!("SKIPPED: {error}");
            return;
        }
    };
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let output = scratch.output();
    let result =
        convert_psp_iso_to_cso(&source, &output, &validator, &scratch.placement()).unwrap();
    assert!(result.checks.independent_validator.starts_with("maxcso v"));
    assert_eq!(
        result.checks.independent_restored_sha256,
        SYNTHETIC_ISO_SHA256
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

// ---- conversion: success and the four separate results ---------------------

#[test]
fn a_successful_conversion_reports_four_separate_results_and_cleans_its_stage() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let output = scratch.output();
    let result =
        convert_psp_iso_to_cso(&source, &output, &reference(), &scratch.placement()).unwrap();
    let checks = &result.checks;
    assert!(checks.format_valid);
    assert_eq!(checks.emuwiz_restored_sha256, SYNTHETIC_ISO_SHA256);
    assert_eq!(checks.independent_restored_sha256, SYNTHETIC_ISO_SHA256);
    assert_eq!(
        checks.independent_validator,
        "test reference decoder (unit tests only)"
    );
    assert_eq!(
        checks.emulator_compatibility,
        EmulatorCompatibility::NotTested
    );
    assert!(result.provenance.tool_version.starts_with("emuwiz-cso/2"));
    assert_eq!(fs::read(&source).unwrap(), bytes, "source unchanged");
    assert_eq!(
        reference_restore_sha256(&output).unwrap(),
        SYNTHETIC_ISO_SHA256
    );
    assert!(
        scratch.entries("staging").is_empty(),
        "stage removed after publication"
    );
    assert!(scratch.entries("failed").is_empty());
    // Nothing besides the source and the output was created in game/output folders.
    assert_eq!(fs::read_dir(scratch.root.join("games")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(scratch.root.join("out")).unwrap().count(), 1);
}

#[test]
fn written_cso_uses_standard_flags_raw_deflate_and_align_2() {
    let scratch = Scratch::new();
    let (source, iso) = scratch.source();
    let output = scratch.output();
    convert_psp_iso_to_cso(&source, &output, &reference(), &scratch.placement()).unwrap();
    let cso = fs::read(&output).unwrap();
    assert_eq!(&cso[20..24], &[1, CSO_ALIGN_SHIFT, 0, 0]);
    let blocks = iso.len() / CSO_BLOCK_SIZE;
    let data_start = (24 + 4 * (blocks + 1)) as u64;
    assert_eq!(
        u64::from(get_index(&cso, 0) & CSO_OFFSET_MASK) << CSO_ALIGN_SHIFT,
        data_start
    );
    let (mut plain, mut compressed) = (0, 0);
    for i in 0..blocks {
        let entry = get_index(&cso, i);
        let start = (u64::from(entry & CSO_OFFSET_MASK) << CSO_ALIGN_SHIFT) as usize;
        let expected = &iso[i * CSO_BLOCK_SIZE..][..CSO_BLOCK_SIZE];
        if entry & CSO_PLAIN_FLAG != 0 {
            plain += 1;
            assert_eq!(&cso[start..start + CSO_BLOCK_SIZE], expected);
        } else {
            compressed += 1;
            assert_ne!(cso[start], 0x78, "no zlib wrapper");
            let mut expanded = Vec::new();
            DeflateDecoder::new(&cso[start..])
                .read_to_end(&mut expanded)
                .unwrap();
            assert_eq!(expanded, expected);
        }
    }
    assert_eq!((plain, compressed), (blocks / 3, blocks - blocks / 3));
}

// ---- refusals before anything is written ----------------------------------

#[test]
fn conversion_is_refused_when_the_independent_validator_is_missing() {
    for path in ["/nonexistent/maxcso", "maxcso"] {
        let error = IndependentCsoValidator::maxcso_at(Path::new(path)).unwrap_err();
        assert!(
            matches!(error, PspShrinkError::DependencyMissing(_)),
            "{path}"
        );
        assert!(error.to_string().contains("Nothing was written"));
    }
}

#[test]
fn a_diagnostics_location_inside_a_game_source_is_refused_before_writing() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let output = scratch.output();
    // The whole scratch root is configured as a source folder.
    let mut placement = scratch.placement();
    placement.source_folders = vec![scratch.root.clone()];
    let error = convert_psp_iso_to_cso(&source, &output, &reference(), &placement).unwrap_err();
    assert!(
        matches!(error, PspShrinkError::DiagnosticsUnavailable(_)),
        "{error}"
    );
    assert!(
        !scratch.diagnostics().exists(),
        "never created inside a source"
    );
    assert!(!output.exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn symlinked_or_shared_diagnostics_paths_are_refused_before_writing() {
    let scratch = Scratch::new();
    let (source, _) = scratch.source();
    let elsewhere = scratch.root.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_dir(scratch.root.join(".emuwiz-diagnostics")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, scratch.root.join(".emuwiz-diagnostics")).unwrap();
    let error = convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing written through the link"
    );

    fs::remove_file(scratch.root.join(".emuwiz-diagnostics")).unwrap();
    fs::create_dir(scratch.root.join(".emuwiz-diagnostics")).unwrap();
    fs::set_permissions(
        scratch.root.join(".emuwiz-diagnostics"),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    let error = convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("writable by other users"),
        "{error}"
    );
    assert!(!scratch.output().exists());
}

// ---- diagnostics location preference order (owner decision 2026-11-24) ----

fn not_root() -> bool {
    // Permission tests mean nothing to root, which bypasses mode bits.
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root {
        eprintln!("SKIPPED: running as root; permission checks cannot be exercised");
    }
    !root
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

#[test]
fn a_missing_configured_folder_is_never_created_and_the_data_directory_is_used() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let data = scratch.root.join("data");
    private_dir(&data);
    let mut placement = scratch.placement();
    placement.configured_dirs = vec![scratch.root.join("not-there")];
    placement.data_dir = Some(data.clone());
    let root = DiagnosticsRoot::resolve(&scratch.root.join("out"), &placement).unwrap();
    assert_eq!(root.source(), DiagnosticsSource::DataDirectory);
    assert_eq!(root.path(), data.join("diagnostics").join("psp-cso"));
    assert!(
        root.skipped()[0].contains("never created"),
        "{:?}",
        root.skipped()
    );
    assert!(!scratch.root.join("not-there").exists());
    // The fallback location works end to end, and keeps failures there.
    let error =
        convert_psp_iso_to_cso(&source, &scratch.output(), &disagreeing(), &placement).unwrap_err();
    let PspShrinkError::ConversionFailed { evidence, .. } = &error else {
        panic!("{error}");
    };
    assert!(evidence.starts_with(data.join("diagnostics").join("psp-cso").join("failed")));
    assert!(evidence.join("output.cso").is_file());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    convert_psp_iso_to_cso(&source, &scratch.output(), &reference(), &placement).unwrap();
}

#[test]
fn a_system_disk_output_uses_the_data_directory_without_touching_the_unwritable_mount_root() {
    if !not_root() {
        return;
    }
    // Simulated system disk: the "mount root" is a folder the user cannot
    // write to, like `/`; the data directory is on the same filesystem.
    let scratch = Scratch::new();
    let mount = scratch.root.join("system-root");
    let home_out = mount.join("home-output");
    let data = scratch.root.join("data");
    fs::create_dir_all(&home_out).unwrap();
    private_dir(&data);
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o555)).unwrap();
    let placement = DiagnosticsPlacement {
        source_folders: vec![scratch.root.join("games")],
        configured_dirs: vec![],
        data_dir: Some(data.clone()),
        allow_mount_root: true,
    };
    let chosen = DiagnosticsRoot::resolve_with_mount_point(&home_out, &placement, &mount).unwrap();
    assert_eq!(chosen.source(), DiagnosticsSource::DataDirectory);
    assert!(!mount.join(DIAGNOSTICS_DIR_NAME).exists());
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_unwritable_mount_root_with_no_other_location_is_refused_with_how_to_configure() {
    if !not_root() {
        return;
    }
    let scratch = Scratch::new();
    let mount = scratch.root.join("system-root");
    let out = mount.join("out");
    fs::create_dir_all(&out).unwrap();
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o555)).unwrap();
    let placement = DiagnosticsPlacement {
        source_folders: vec![],
        configured_dirs: vec![],
        data_dir: None,
        allow_mount_root: true,
    };
    let error = DiagnosticsRoot::resolve_with_mount_point(&out, &placement, &mount).unwrap_err();
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o755)).unwrap();
    let message = error.to_string();
    assert!(matches!(error, PspShrinkError::DiagnosticsUnavailable(_)));
    assert!(message.contains("could not create"), "{message}");
    assert!(message.contains("cso-diagnostics-dirs.txt"), "{message}");
    assert!(message.contains("chmod 700"), "{message}");
    assert!(message.contains("Nothing was written"), "{message}");
    assert!(!mount.join(DIAGNOSTICS_DIR_NAME).exists());
}

#[test]
fn a_writable_mount_root_is_used_only_as_the_last_preference() {
    let scratch = Scratch::new();
    let mount = scratch.root.join("usb-root");
    let out = mount.join("out");
    fs::create_dir_all(&out).unwrap();
    let placement = DiagnosticsPlacement {
        source_folders: vec![mount.join("games")],
        configured_dirs: vec![],
        data_dir: None,
        allow_mount_root: true,
    };
    let chosen = DiagnosticsRoot::resolve_with_mount_point(&out, &placement, &mount).unwrap();
    assert_eq!(chosen.source(), DiagnosticsSource::MountRoot);
    assert_eq!(
        chosen.path(),
        mount.join(DIAGNOSTICS_DIR_NAME).join("psp-cso")
    );
    // A mount root owned by someone else would be refused; one that is a
    // game source folder itself is refused.
    let mut inside = placement.clone();
    inside.source_folders = vec![mount.clone()];
    let out2 = scratch.root.join("usb2").join("out");
    fs::create_dir_all(&out2).unwrap();
    assert!(
        DiagnosticsRoot::resolve_with_mount_point(&out2, &inside, &scratch.root.join("usb2"))
            .is_ok(),
        "a different disk root is unaffected by another disk's source folder"
    );
    let error = DiagnosticsRoot::resolve_with_mount_point(&out, &inside, &mount).unwrap_err();
    assert!(error.to_string().contains("game source folder"), "{error}");
}

#[test]
fn invalid_fallback_locations_are_each_skipped_and_then_refused() {
    let scratch = Scratch::new();
    let elsewhere = scratch.root.join("elsewhere");
    private_dir(&elsewhere);
    let link = scratch.root.join("diag-link");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let shared = scratch.root.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    let placement = DiagnosticsPlacement {
        source_folders: vec![scratch.root.join("games")],
        configured_dirs: vec![
            PathBuf::from("relative/diag"),
            link.clone(),
            shared.clone(),
            scratch.root.join("games").join("diag"),
        ],
        // The data directory inside a game source is refused too.
        data_dir: Some(scratch.root.join("games")),
        allow_mount_root: false,
    };
    let error = DiagnosticsRoot::resolve(&scratch.root.join("out"), &placement).unwrap_err();
    let message = error.to_string();
    for expected in [
        "not an absolute path",
        "symbolic link",
        "writable by other users",
        "game source folder",
        "cso-diagnostics-dirs.txt",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(!scratch.root.join("games").join("diag").exists());
    assert!(!scratch.root.join("games").join("diagnostics").exists());
}

#[test]
fn a_fallback_on_another_filesystem_is_refused() {
    let scratch = Scratch::new();
    let other = match tempfile::tempdir_in("/dev/shm") {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("SKIPPED: /dev/shm is not available");
            return;
        }
    };
    let other_root = fs::canonicalize(other.path()).unwrap();
    if fs::metadata(&other_root).unwrap().dev() == fs::metadata(&scratch.root).unwrap().dev() {
        eprintln!("SKIPPED: /dev/shm is on the same filesystem as the temporary directory");
        return;
    }
    private_dir(&other_root.join("diag"));
    let mut placement = scratch.placement();
    placement.configured_dirs = vec![other_root.join("diag")];
    placement.data_dir = Some(other_root.clone());
    let error = DiagnosticsRoot::resolve(&scratch.root.join("out"), &placement).unwrap_err();
    assert!(error.to_string().contains("another filesystem"), "{error}");
    assert!(!other_root.join("diag").join("psp-cso").exists());
    assert!(!other_root.join("diagnostics").exists());
}

#[test]
fn too_little_free_space_is_refused_before_writing() {
    let scratch = Scratch::new();
    let (source, _) = scratch.source();
    let needed = required_free_bytes(ISO_LEN);
    assert!(needed > 2 * ISO_LEN, "CSO + restored ISO + margin");
    FREE_SPACE_OVERRIDE.with(|cell| cell.set(Some(needed - 1)));
    let error = convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
    );
    FREE_SPACE_OVERRIDE.with(|cell| cell.set(None));
    assert!(matches!(
        error,
        Err(PspShrinkError::InsufficientSpace { .. })
    ));
    assert!(scratch.entries("staging").is_empty());
    assert!(!scratch.output().exists());
}

#[test]
fn an_existing_output_is_never_replaced() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let output = scratch.output();
    fs::write(&output, b"keep me").unwrap();
    assert!(matches!(
        convert_psp_iso_to_cso(&source, &output, &reference(), &scratch.placement()),
        Err(PspShrinkError::OutputExists(_))
    ));
    assert_eq!(fs::read(&output).unwrap(), b"keep me");
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn tool_failure_does_not_touch_source_or_create_output() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let output = scratch.output();
    let error = convert_internal(
        &source,
        &output,
        &reference(),
        &scratch.placement(),
        TestFailure::Tool,
    )
    .unwrap_err();
    assert!(error.to_string().contains("tool failed"));
    assert!(!output.exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

// ---- failures after work started: evidence is kept --------------------------

fn assert_retained(scratch: &Scratch, error: &PspShrinkError, check: &str) -> PathBuf {
    let PspShrinkError::ConversionFailed {
        reason,
        evidence,
        manifest_error,
        ..
    } = error
    else {
        panic!("expected ConversionFailed, got {error}");
    };
    assert!(reason.starts_with(check), "{reason}");
    assert!(manifest_error.is_none());
    assert!(evidence.starts_with(scratch.diagnostics().join("failed")));
    assert!(evidence.join("output.cso").is_file(), "failed CSO kept");
    assert!(evidence.join("intent.json").is_file());
    assert!(
        !evidence.join("restored.iso").exists(),
        "transient restore removed"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("diagnostic.json")).unwrap()).unwrap();
    assert_eq!(report["reason"], serde_json::Value::from(reason.as_str()));
    assert_eq!(report["intent"]["source_sha256"], SYNTHETIC_ISO_SHA256);
    assert_eq!(report["intent"]["writer_version"], TOOL_VERSION);
    assert!(report["failed_cso_sha256"].is_string());
    assert!(!scratch.output().exists(), "nothing published");
    evidence.clone()
}

#[test]
fn a_content_integrity_failure_publishes_nothing_and_keeps_the_evidence() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let error = convert_internal(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
        TestFailure::HashMismatch,
    )
    .unwrap_err();
    let evidence = assert_retained(&scratch, &error, "content integrity");
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let lines = scratch.manifest_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["evidence"], evidence.to_string_lossy().as_ref());
}

#[test]
fn an_independent_disagreement_publishes_nothing_even_though_emuwiz_reads_it() {
    let scratch = Scratch::new();
    let (source, _) = scratch.source();
    let error = convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &disagreeing(),
        &scratch.placement(),
    )
    .unwrap_err();
    assert_retained(&scratch, &error, "independent reversibility");
}

#[test]
fn repeated_failures_get_unique_entries_and_usage_is_reported() {
    let scratch = Scratch::new();
    let (source, _) = scratch.source();
    for _ in 0..2 {
        let error = convert_psp_iso_to_cso(
            &source,
            &scratch.output(),
            &disagreeing(),
            &scratch.placement(),
        )
        .unwrap_err();
        assert_retained(&scratch, &error, "independent reversibility");
    }
    let failed = scratch.entries("failed");
    assert_eq!(failed.len(), 2);
    assert_ne!(failed[0], failed[1]);
    assert_eq!(scratch.manifest_lines().len(), 2);
    let root = DiagnosticsRoot::resolve(&scratch.root.join("out"), &scratch.placement()).unwrap();
    let usage = diagnostics_usage(&root);
    assert_eq!(usage.failed_entries, 2);
    assert!(usage.failed_bytes > 2 * 1024);
    assert_eq!((usage.interrupted_entries, usage.missing_entries), (0, 0));
}

#[test]
fn an_unwritable_manifest_keeps_the_evidence_and_reports_its_location() {
    let scratch = Scratch::new();
    let (source, _) = scratch.source();
    // Establish the tree, then make the manifest path unusable.
    DiagnosticsRoot::resolve(&scratch.root.join("out"), &scratch.placement()).unwrap();
    fs::create_dir(scratch.diagnostics().join("manifest.jsonl")).unwrap();
    let error = convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &disagreeing(),
        &scratch.placement(),
    )
    .unwrap_err();
    let PspShrinkError::ConversionFailed {
        evidence,
        manifest_error,
        ..
    } = &error
    else {
        panic!("{error}");
    };
    assert!(manifest_error.is_some());
    assert!(evidence.join("output.cso").is_file());
    let message = error.to_string();
    assert!(
        message.contains(&evidence.display().to_string()),
        "{message}"
    );
    assert!(message.contains("could not be listed"), "{message}");
}

#[test]
fn an_interrupted_run_leaves_an_identifiable_stage_and_no_output() {
    let scratch = Scratch::new();
    let (source, bytes) = scratch.source();
    let _ = convert_internal(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
        TestFailure::Interrupt,
    );
    let staging = scratch.entries("staging");
    assert_eq!(staging.len(), 1);
    assert!(staging[0].join("output.cso").is_file());
    let intent: serde_json::Value =
        serde_json::from_slice(&fs::read(staging[0].join("intent.json")).unwrap()).unwrap();
    assert_eq!(
        intent["intended_output"],
        scratch.output().to_string_lossy().as_ref()
    );
    assert!(!scratch.output().exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let root = DiagnosticsRoot::resolve(&scratch.root.join("out"), &scratch.placement()).unwrap();
    assert_eq!(diagnostics_usage(&root).interrupted_entries, 1);
    // A later conversion does not remove the interrupted stage.
    convert_psp_iso_to_cso(
        &source,
        &scratch.output(),
        &reference(),
        &scratch.placement(),
    )
    .unwrap();
    assert_eq!(scratch.entries("staging"), staging);
}

// ---- standard reader boundaries ---------------------------------------------

#[test]
fn mixed_plain_and_compressed_blocks_decode_at_several_alignments() {
    let iso = synthetic_iso();
    for align in [0u8, 2, 4, 11] {
        let cso = standard_cso(&iso, align);
        assert_eq!(decode(&cso, ISO_LEN).unwrap(), iso, "align {align}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cso");
        fs::write(&path, &cso).unwrap();
        assert_eq!(
            classify_cso_dialect(&path).unwrap(),
            CsoDialect::Standard { align }
        );
    }
}

#[test]
fn a_decreasing_index_is_rejected() {
    let mut cso = standard_cso(&synthetic_iso(), 2);
    let (a, b) = (get_index(&cso, 3), get_index(&cso, 4));
    set_index(&mut cso, 3, b);
    set_index(&mut cso, 4, a);
    assert!(decode(&cso, ISO_LEN).is_err());
}

#[test]
fn an_offset_into_the_header_or_index_is_rejected() {
    let mut cso = standard_cso(&synthetic_iso(), 2);
    set_index(&mut cso, 0, 2); // byte 8: inside the header
    assert!(decode(&cso, ISO_LEN).is_err());
}

#[test]
fn an_offset_past_the_end_of_the_file_is_rejected() {
    let mut cso = standard_cso(&synthetic_iso(), 2);
    let len = cso.len() as u32;
    set_index(&mut cso, 96, (len >> 2) + 1000);
    assert!(decode(&cso, ISO_LEN).is_err());
}

#[test]
fn an_oversized_block_range_is_rejected_before_allocation() {
    let mut cso = standard_cso(&synthetic_iso(), 2);
    // Block 0 claims everything up to block 30: far more than one block.
    let block_30 = get_index(&cso, 30);
    set_index(&mut cso, 1, block_30);
    let error = decode(&cso, ISO_LEN).unwrap_err();
    assert!(error.to_string().contains("too large"), "{error}");
}

#[test]
fn a_truncated_index_table_or_data_is_rejected() {
    let cso = standard_cso(&synthetic_iso(), 2);
    for cut in [20, 24 + 4 * 10, cso.len() - 1] {
        assert!(decode(&cso[..cut], ISO_LEN).is_err(), "cut at {cut}");
    }
}

#[test]
fn a_compressed_block_that_expands_wrongly_is_rejected() {
    let iso = synthetic_iso();
    let mut cso = standard_cso(&iso, 2);
    // Block 0 is compressed (constant sector). Replace its payload with a
    // raw-deflate stream of only 1024 bytes, inside the same range. (Over-long
    // streams are cut off at 2049 bytes by the bounded decoder.)
    let start = ((get_index(&cso, 0) & CSO_OFFSET_MASK) << 2) as usize;
    let end = ((get_index(&cso, 1) & CSO_OFFSET_MASK) << 2) as usize;
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(&[0u8; 1024]).unwrap();
    let short = encoder.finish().unwrap();
    assert!(short.len() <= end - start);
    cso[start..start + short.len()].copy_from_slice(&short);
    assert!(decode(&cso, ISO_LEN).is_err());
}

#[test]
fn a_plain_block_with_the_wrong_length_is_rejected() {
    let mut cso = standard_cso(&synthetic_iso(), 2);
    // Block 2 is plain (incompressible). Mark block 0 plain instead: its
    // compressed payload is far shorter than 2048 bytes.
    let entry = get_index(&cso, 0);
    set_index(&mut cso, 0, entry | CSO_PLAIN_FLAG);
    assert!(decode(&cso, ISO_LEN).is_err());
}

#[test]
fn unsupported_headers_are_rejected() {
    let base = standard_cso(&synthetic_iso(), 2);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut magic = base.clone();
    magic[0] = b'Z';
    cases.push(("ZISO magic", magic));
    let mut version = base.clone();
    version[20] = 2;
    cases.push(("CSO v2", version));
    let mut align = base.clone();
    align[21] = 31;
    cases.push(("align 31", align));
    let mut block = base.clone();
    block[16..20].copy_from_slice(&4096u32.to_le_bytes());
    cases.push(("block size 4096", block));
    for (name, cso) in cases {
        assert!(decode(&cso, ISO_LEN).is_err(), "{name}");
    }
    assert!(
        decode(&base, ISO_LEN - 2048).is_err(),
        "declared size differs"
    );
}

#[test]
fn the_restore_never_overwrites_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let cso = dir.path().join("a.cso");
    fs::write(&cso, MAXCSO_FIXTURE).unwrap();
    let restored = dir.path().join("restored.iso");
    fs::write(&restored, b"keep").unwrap();
    assert!(matches!(
        restore_standard_cso(&cso, &restored, ISO_LEN),
        Err(PspShrinkError::OutputExists(_))
    ));
    assert_eq!(fs::read(&restored).unwrap(), b"keep");
}

// ---- legacy EmuWiz CSO ------------------------------------------------------

#[test]
fn a_legacy_emuwiz_cso_is_detected_and_refused_by_the_standard_reader() {
    let legacy = write_legacy_emuwiz_cso(&synthetic_iso());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.cso");
    fs::write(&path, &legacy).unwrap();
    assert_eq!(
        classify_cso_dialect(&path).unwrap(),
        CsoDialect::LegacyEmuWiz
    );
    assert!(decode(&legacy, ISO_LEN).is_err());
    assert!(
        reference_restore_sha256(&path).is_err(),
        "nor by the independent reference"
    );
}

#[test]
fn a_header_hint_alone_is_not_a_confirmed_legacy_file() {
    // Legacy-looking header and first index entry, damaged data.
    let mut legacy = write_legacy_emuwiz_cso(&synthetic_iso());
    let data_start = 24 + 4 * 97;
    legacy[data_start + 5] ^= 0xff; // breaks the first zlib stream / Adler-32
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("damaged.cso");
    fs::write(&path, &legacy).unwrap();
    assert_eq!(
        classify_cso_dialect(&path).unwrap(),
        CsoDialect::LegacyEmuWiz
    );
    assert!(confirm_legacy_emuwiz_cso(&path).is_err());
    let restored = dir.path().join("restored.iso");
    assert!(restore_legacy_emuwiz_cso(&path, &restored).is_err());
    assert!(
        !restored.exists(),
        "no partial ISO from an unconfirmed file"
    );
}

#[test]
fn a_legacy_emuwiz_cso_is_recovered_byte_for_byte_without_being_modified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.cso");
    let bytes = write_legacy_emuwiz_cso(&synthetic_iso());
    fs::write(&path, &bytes).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let confirmed = confirm_legacy_emuwiz_cso(&path).unwrap();
    assert_eq!((confirmed.blocks, confirmed.compressed_blocks), (96, 64));
    assert_eq!(confirmed.decoded_sha256, SYNTHETIC_ISO_SHA256);
    let restored = dir.path().join("restored.iso");
    assert_eq!(
        restore_legacy_emuwiz_cso(&path, &restored).unwrap(),
        confirmed
    );
    assert_eq!(sha_hex(&fs::read(&restored).unwrap()), SYNTHETIC_ISO_SHA256);
    assert_eq!(fs::read(&path).unwrap(), bytes, "legacy file untouched");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    assert!(matches!(
        restore_legacy_emuwiz_cso(&path, &restored),
        Err(PspShrinkError::OutputExists(_))
    ));
}

#[test]
fn legacy_recovery_refuses_a_standard_cso_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let cso = dir.path().join("maxcso.cso");
    fs::write(&cso, MAXCSO_FIXTURE).unwrap();
    let restored = dir.path().join("restored.iso");
    assert!(restore_legacy_emuwiz_cso(&cso, &restored).is_err());
    assert!(!restored.exists());
}
