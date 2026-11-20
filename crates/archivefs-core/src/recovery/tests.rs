//! End-to-end and adversarial tests for the recovery engine, on synthetic
//! disc sets only.

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::diagnose::{CD32_FIRMWARE, HostFacts, RecoveryContext, RetroArchHost, diagnose};
use super::disc_zip::{DiscSetLimits, DiscSetRefusal, inspect_disc_zip};
use super::model::*;
use super::prepared_media::test_hooks::{HOOK, Point};
use super::prepared_media::*;
use crate::identity_source::hashing::Crc32;
use crate::safe_read::TrustedRoots;

const CUE_NAME: &str = "Synthetic Game (1994)(Test)[!].cue";
const ISO_NAME: &str = "Synthetic Game (1994)(Test)[!].iso";
const WAV_NAME: &str = "Synthetic Game (1994)(Test)(Track 2 of 2)[!].wav";

fn crc(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

fn iso_bytes(system: &str, volume: &str) -> Vec<u8> {
    let mut image = vec![0_u8; 2048 * 24];
    let descriptor = &mut image[16 * 2048..17 * 2048];
    descriptor[0] = 1;
    descriptor[1..6].copy_from_slice(b"CD001");
    descriptor[6] = 1;
    descriptor[8..40].fill(b' ');
    descriptor[8..8 + system.len()].copy_from_slice(system.as_bytes());
    descriptor[40..72].fill(b' ');
    descriptor[40..40 + volume.len()].copy_from_slice(volume.as_bytes());
    // Not all zeros, so a deflated copy is a real decode.
    for (index, byte) in image[18 * 2048..].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    image
}

fn wav_bytes() -> Vec<u8> {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + 2352 * 4).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&[
        16, 0, 0, 0, 1, 0, 2, 0, 0x44, 0xac, 0, 0, 0x10, 0xb1, 2, 0, 4, 0, 16, 0,
    ]);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(2352_u32 * 4).to_le_bytes());
    wav.extend((0..2352 * 4).map(|index| (index % 97) as u8));
    wav
}

fn cue_text() -> Vec<u8> {
    format!(
        "FILE \"{ISO_NAME}\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\nFILE \"{WAV_NAME}\" WAVE\r\n  TRACK 02 AUDIO\r\n    INDEX 01 00:00:00\r\n"
    )
    .into_bytes()
}

fn standard_members() -> Vec<(String, Vec<u8>)> {
    vec![
        (ISO_NAME.to_string(), iso_bytes("CDTV", "SYNTHETIC")),
        (WAV_NAME.to_string(), wav_bytes()),
        (CUE_NAME.to_string(), cue_text()),
    ]
}

/// A well-formed deflated ZIP written by the `zip` crate.
fn write_zip(path: &Path, members: &[(String, Vec<u8>)]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

/// One entry of a hand-built stored ZIP, so a test can lie in any field.
struct RawEntry {
    name: Vec<u8>,
    data: Vec<u8>,
    crc: Option<u32>,
    declared_size: Option<u32>,
    external_attributes: u32,
    version_made_by: u16,
    flags: u16,
}

impl RawEntry {
    fn new(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            crc: None,
            declared_size: None,
            external_attributes: 0,
            version_made_by: 20,
            flags: 0,
        }
    }
}

fn write_raw_zip(path: &Path, entries: &[RawEntry]) {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let offset = out.len() as u32;
        let crc = entry.crc.unwrap_or_else(|| crc(&entry.data));
        let size = entry.declared_size.unwrap_or(entry.data.len() as u32);
        let stored = entry.data.len() as u32;
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&20_u16.to_le_bytes());
        out.extend_from_slice(&entry.flags.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&stored.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&entry.name);
        out.extend_from_slice(&entry.data);

        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&entry.version_made_by.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&entry.flags.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&[0; 4]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&stored.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&entry.external_attributes.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(&entry.name);
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    std::fs::write(path, out).unwrap();
}

fn raw_standard() -> Vec<RawEntry> {
    standard_members()
        .iter()
        .map(|(name, bytes)| RawEntry::new(name, bytes))
        .collect()
}

struct Lab {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: PreparedMediaStore,
    host: HostFacts,
    cancel: AtomicBool,
    trusted: TrustedRoots,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("games/amiga-cd32")).unwrap();
        let system = root.join("retroarch-system");
        std::fs::create_dir_all(&system).unwrap();
        for firmware in CD32_FIRMWARE {
            std::fs::write(system.join(firmware.file_name), b"synthetic, not firmware").unwrap();
        }
        Self {
            store: PreparedMediaStore::at(root.join("data/prepared-media")),
            host: HostFacts {
                retroarch: vec![RetroArchHost {
                    label: "Native RetroArch".into(),
                    system_directory: Some(system),
                    core_stems: vec!["puae".into(), "mgba".into()],
                }],
                retroarch_discovered: true,
            },
            cancel: AtomicBool::new(false),
            trusted: TrustedRoots::none(),
            root,
            _dir: dir,
        }
    }

    fn zip_path(&self, name: &str) -> PathBuf {
        self.root.join("games/amiga-cd32").join(name)
    }

    fn standard_zip(&self) -> PathBuf {
        let path = self.zip_path("Synthetic Game (1994)(Test)[!].zip");
        write_zip(&path, &standard_members());
        path
    }

    fn subject(&self, path: &Path) -> RecoverySubject {
        RecoverySubject::new(path, Some("AmigaCD32")).with_platform_source(Some("folder_alias"))
    }

    fn diagnose(&self, path: &Path) -> RecoveryReport {
        diagnose(
            &self.subject(path),
            &RecoveryContext {
                store: &self.store,
                trusted: &self.trusted,
                limits: DiscSetLimits::default(),
                host: &self.host,
                cancel: &self.cancel,
            },
        )
    }

    fn prepare(&self, path: &Path) -> Result<PrepareOutcome, PrepareError> {
        self.prepare_with(path, &DiscSetLimits::default(), &|_| {})
    }

    fn prepare_with(
        &self,
        path: &Path,
        limits: &DiscSetLimits,
        progress: &dyn Fn(PrepareProgress<'_>),
    ) -> Result<PrepareOutcome, PrepareError> {
        self.store.prepare(
            path,
            "AmigaCD32",
            &self.trusted,
            limits,
            &self.cancel,
            progress,
        )
    }

    /// Every path under the store, sorted, for "nothing was left behind".
    fn store_files(&self) -> Vec<PathBuf> {
        fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(&path, found);
                } else {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(self.store.root(), &mut found);
        found.retain(|path| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("lock" | "journal.jsonl")
            )
        });
        found.sort();
        found
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.store.root().join("v1/journal.jsonl")).unwrap_or_default()
    }
}

/// Size, times and bytes of a file: what "unchanged" means for an original.
fn snapshot(path: &Path) -> (u64, i64, i64, i64, u64, Vec<u8>) {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    (
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime_nsec(),
        metadata.ino(),
        std::fs::read(path).unwrap(),
    )
}

fn with_hook<T>(hook: impl Fn(Point) + 'static, run: impl FnOnce() -> T) -> T {
    HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    let result = run();
    HOOK.with(|slot| *slot.borrow_mut() = None);
    result
}

// ---------------------------------------------------------------------------
// The vertical slice
// ---------------------------------------------------------------------------

#[test]
fn a_zipped_cd32_disc_goes_from_diagnosis_to_a_verified_prepared_copy_and_back() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let before = snapshot(&zip);

    // Diagnosis: blocked, fixable, with evidence and a create-new action.
    let report = lab.diagnose(&zip);
    assert_eq!(report.verdict, RecoveryVerdict::FixableByPreparation);
    let finding = report.finding(FindingCode::DiscSetInsideZip).unwrap();
    assert_eq!(finding.blocking, Blocking::Launch);
    assert_eq!(finding.confidence, Confidence::High);
    assert!(finding.evidence.len() >= 3);
    assert_eq!(finding.actions.len(), 1);
    assert_eq!(finding.actions[0].kind, ActionKind::PrepareDiscSetFromZip);
    assert_eq!(finding.actions[0].effect, ActionEffect::CreatesNewFiles);
    assert_eq!(finding.actions[0].performer, ActionPerformer::Engine);
    let plan = report.plan.as_ref().unwrap();
    assert_eq!(plan.files.len(), 3);
    assert_eq!(plan.files[0].name, CUE_NAME);
    assert!(report.prepared.is_none());
    assert!(lab.store_files().is_empty(), "diagnosis writes nothing");
    assert!(!lab.store.root().exists(), "diagnosis creates no folder");
    // Folder-only system stays a hint.
    let system = report.finding(FindingCode::SystemFromFolderOnly).unwrap();
    assert_eq!(system.confidence, Confidence::Low);
    assert_eq!(system.blocking, Blocking::Nothing);
    // Firmware present but not a known release: said plainly, not blocking.
    assert_eq!(
        report
            .finding(FindingCode::FirmwarePresent)
            .unwrap()
            .confidence,
        Confidence::Medium
    );

    // Preparation.
    let stages = std::cell::RefCell::new(Vec::new());
    let outcome = lab
        .prepare_with(&zip, &DiscSetLimits::default(), &|progress| {
            stages.borrow_mut().push(format!("{progress:?}"));
        })
        .unwrap();
    assert!(!outcome.already_prepared);
    let summary = outcome.summary;
    assert_eq!(summary.set_id, plan.set_id);
    assert_eq!(summary.directory, plan.destination);
    assert_eq!(summary.file_count, 3);
    assert_eq!(summary.platform_id, "AmigaCD32");
    assert_eq!(
        summary.disc,
        Some(DiscFacts {
            system_identifier: "CDTV".into(),
            volume_identifier: "SYNTHETIC".into()
        })
    );
    assert_eq!(summary.launch_path.file_name().unwrap(), CUE_NAME);
    assert!(
        stages
            .borrow()
            .iter()
            .any(|stage| stage.contains("Extracting"))
    );
    assert!(
        stages
            .borrow()
            .iter()
            .any(|stage| stage.contains("Verifying"))
    );
    assert_eq!(stages.borrow().last().unwrap(), "Publishing");
    for (name, bytes) in standard_members() {
        let path = summary.directory.join("media").join(&name);
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o444, "{name} is read-only");
    }
    assert_eq!(snapshot(&zip), before, "the original ZIP is unchanged");
    assert!(lab.store.interrupted().is_empty());
    assert!(lab.journal().contains("prepared_copy_created"));

    // Diagnosis now: playable from the prepared copy, disc evidence added.
    let report = lab.diagnose(&zip);
    assert_eq!(report.verdict, RecoveryVerdict::PlayableFromPreparedCopy);
    assert_eq!(
        report.prepared.as_ref().unwrap().launch_path,
        summary.launch_path
    );
    assert!(report.plan.is_none());
    let ready = report.finding(FindingCode::PreparedCopyReady).unwrap();
    assert!(ready.actions.iter().any(|action| {
        action.kind == ActionKind::RemovePreparedCopy
            && action.effect == ActionEffect::ChangesExistingState
    }));
    assert_eq!(
        report
            .finding(FindingCode::DiscSetInsideZip)
            .unwrap()
            .blocking,
        Blocking::Nothing
    );
    assert_eq!(
        report
            .finding(FindingCode::SystemFromFolderOnly)
            .unwrap()
            .confidence,
        Confidence::Medium,
        "the disc's own CDTV identifier is evidence, but never proof of CD32"
    );

    // Launch gate and full verification.
    assert_eq!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Verified(summary.clone())
    );
    assert_eq!(
        lab.store.verify_full(&summary.set_id, &lab.cancel).unwrap(),
        summary
    );

    // Undo: only the prepared files go; the ZIP stays.
    let removal = lab.store.remove(&summary.set_id).unwrap();
    assert_eq!(removal.removed_files.len(), 3);
    assert!(removal.kept.is_empty());
    assert!(lab.store_files().is_empty());
    assert_eq!(snapshot(&zip), before);
    assert_eq!(
        lab.diagnose(&zip).verdict,
        RecoveryVerdict::FixableByPreparation
    );
    assert!(lab.journal().contains("prepared_copy_removed"));
}

#[test]
fn preparing_twice_writes_nothing_the_second_time() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let first = lab.prepare(&zip).unwrap().summary;
    let inode = std::fs::metadata(&first.launch_path).unwrap().ino();
    let files = lab.store_files();
    let second = lab.prepare(&zip).unwrap();
    assert!(second.already_prepared);
    assert_eq!(second.summary, first);
    assert_eq!(std::fs::metadata(&first.launch_path).unwrap().ino(), inode);
    assert_eq!(lab.store_files(), files);
}

#[test]
fn two_zips_holding_the_same_disc_share_one_prepared_copy() {
    let lab = Lab::new();
    let first = lab.standard_zip();
    // Same members, different order, different compression, different name.
    let second = lab.zip_path("A Renamed Duplicate.zip");
    let mut members = standard_members();
    members.reverse();
    write_raw_zip(
        &second,
        &members
            .iter()
            .map(|(name, bytes)| RawEntry::new(name, bytes))
            .collect::<Vec<_>>(),
    );
    let one = lab.prepare(&first).unwrap();
    assert!(!one.already_prepared);
    // Before it is linked, the second ZIP is offered the existing copy.
    assert!(lab.store.prepared_for_source(&second).is_none());
    let report = lab.diagnose(&second);
    assert_eq!(report.verdict, RecoveryVerdict::FixableByPreparation);
    assert_eq!(
        report
            .finding(FindingCode::DiscSetInsideZip)
            .unwrap()
            .actions[0]
            .title,
        "Use the existing prepared copy"
    );
    let two = lab.prepare(&second).unwrap();
    assert!(two.already_prepared);
    assert_eq!(one.summary, two.summary);
    assert_eq!(lab.store.list().len(), 1);
    assert_eq!(
        lab.diagnose(&second).verdict,
        RecoveryVerdict::PlayableFromPreparedCopy
    );
}

#[test]
fn extra_files_in_the_zip_are_ignored_and_never_extracted() {
    let lab = Lab::new();
    let zip = lab.zip_path("with-notes.zip");
    let mut members = standard_members();
    members.push(("readme.txt".into(), b"notes".to_vec()));
    write_zip(&zip, &members);
    let summary = lab.prepare(&zip).unwrap().summary;
    assert_eq!(summary.file_count, 3);
    assert!(!summary.directory.join("media/readme.txt").exists());
}

// ---------------------------------------------------------------------------
// Diagnosis rules beyond the slice
// ---------------------------------------------------------------------------

#[test]
fn missing_firmware_or_core_is_the_persons_to_fix_and_never_a_download() {
    let mut lab = Lab::new();
    let zip = lab.standard_zip();
    let system = lab.host.retroarch[0].system_directory.clone().unwrap();
    std::fs::remove_file(system.join("kick40060.CD32.ext")).unwrap();
    let report = lab.diagnose(&zip);
    assert_eq!(report.verdict, RecoveryVerdict::FixableWithYourHelp);
    let firmware = report.finding(FindingCode::FirmwareMissing).unwrap();
    assert_eq!(firmware.blocking, Blocking::Launch);
    assert_eq!(firmware.actions[0].kind, ActionKind::AddOwnFirmware);
    assert_eq!(firmware.actions[0].performer, ActionPerformer::Person);
    assert!(firmware.actions[0].detail.contains("never downloads"));
    // Preparation is still offered: it does not depend on firmware.
    assert!(report.plan.is_some());

    lab.host.retroarch[0].core_stems = vec!["mgba".into()];
    let report = lab.diagnose(&zip);
    let route = report.finding(FindingCode::EmulatorRouteMissing).unwrap();
    assert_eq!(route.blocking, Blocking::Launch);
    assert_eq!(route.actions[0].performer, ActionPerformer::Person);
    assert!(report.finding(FindingCode::FirmwareMissing).is_none());
}

#[test]
fn known_firmware_is_recognised_by_checksum_only() {
    // The engine never ships firmware; this checks the comparison itself.
    for firmware in CD32_FIRMWARE {
        assert_eq!(firmware.sha1.len(), 40);
        assert!(firmware.sha1.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(firmware.size_bytes, 524_288);
    }
}

#[test]
fn formats_with_no_validated_preparation_are_reported_as_unsupported_or_not_assessed() {
    let lab = Lab::new();
    let cases: &[(&str, &str, RecoveryVerdict, FindingCode)] = &[
        (
            "game.zip",
            "Xbox",
            RecoveryVerdict::Unsupported,
            FindingCode::FormatUnsupported,
        ),
        (
            "game.7z",
            "Xbox",
            RecoveryVerdict::Unsupported,
            FindingCode::FormatUnsupported,
        ),
        (
            "game.iso",
            "Xbox360",
            RecoveryVerdict::Unsupported,
            FindingCode::FormatUnsupported,
        ),
        (
            "game.pkg",
            "PS3",
            RecoveryVerdict::Unsupported,
            FindingCode::FormatUnsupported,
        ),
        (
            "game.chd",
            "AmigaCD32",
            RecoveryVerdict::Unsupported,
            FindingCode::FormatUnsupported,
        ),
        (
            "game.zip",
            "Commodore CDTV",
            RecoveryVerdict::NotAssessed,
            FindingCode::NotAssessed,
        ),
        (
            "game.sfc",
            "SNES",
            RecoveryVerdict::NotAssessed,
            FindingCode::NotAssessed,
        ),
    ];
    for (name, platform, verdict, code) in cases {
        let path = lab.root.join(name);
        std::fs::write(&path, b"x").unwrap();
        let report = diagnose(
            &RecoverySubject::new(&path, Some(platform)),
            &RecoveryContext {
                store: &lab.store,
                trusted: &lab.trusted,
                limits: DiscSetLimits::default(),
                host: &lab.host,
                cancel: &lab.cancel,
            },
        );
        assert_eq!(report.verdict, *verdict, "{name} / {platform}");
        assert!(report.finding(*code).is_some(), "{name} / {platform}");
        assert!(report.plan.is_none());
        assert!(
            report
                .actions()
                .all(|action| action.performer == ActionPerformer::Person),
            "the engine offers to do nothing itself for {name} / {platform}"
        );
    }
    assert!(!lab.store.root().exists());
}

#[test]
fn a_missing_or_linked_game_file_is_refused() {
    let lab = Lab::new();
    let report = lab.diagnose(&lab.zip_path("gone.zip"));
    assert_eq!(report.verdict, RecoveryVerdict::Refused);
    let zip = lab.standard_zip();
    let link = lab.zip_path("link.zip");
    std::os::unix::fs::symlink(&zip, &link).unwrap();
    assert_eq!(lab.diagnose(&link).verdict, RecoveryVerdict::Refused);
    assert!(matches!(
        lab.prepare(&link),
        Err(PrepareError::Refused(DiscSetRefusal::Source(_)))
    ));
    assert!(lab.store_files().is_empty());
}

// ---------------------------------------------------------------------------
// Adversarial archives
// ---------------------------------------------------------------------------

fn refusal(lab: &Lab, path: &Path) -> DiscSetRefusal {
    inspect_disc_zip(path, &lab.trusted, &DiscSetLimits::default(), &lab.cancel).unwrap_err()
}

#[test]
fn member_names_that_escape_the_folder_are_refused_before_anything_is_written() {
    let lab = Lab::new();
    for name in [
        "../escape.cue",
        "/abs/escape.cue",
        "sub/../../escape.cue",
        "..\\escape.cue",
        "C:escape.cue",
    ] {
        let zip = lab.zip_path("traversal.zip");
        let mut entries = raw_standard();
        entries.push(RawEntry::new(name, b"x"));
        write_raw_zip(&zip, &entries);
        let report = lab.diagnose(&zip);
        assert!(
            matches!(
                report.verdict,
                RecoveryVerdict::Refused | RecoveryVerdict::Unsupported
            ),
            "{name}: {:?}",
            report.verdict
        );
        assert!(report.plan.is_none(), "{name}");
        assert!(lab.prepare(&zip).is_err(), "{name}");
        assert!(lab.store_files().is_empty(), "{name}");
        assert!(!lab.root.join("games/escape.cue").exists());
        assert!(!lab.root.join("escape.cue").exists());
        std::fs::remove_file(&zip).unwrap();
    }
}

#[test]
fn a_cue_sheet_that_points_outside_or_at_missing_files_is_refused() {
    let lab = Lab::new();
    let zip = lab.zip_path("bad-cue.zip");

    let mut members = standard_members();
    members[2].1 =
        b"FILE \"../../../etc/passwd\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n".to_vec();
    write_zip(&zip, &members);
    assert!(matches!(refusal(&lab, &zip), DiscSetRefusal::Cue(_)));

    let mut members = standard_members();
    members.remove(1);
    write_zip(&zip, &members);
    assert_eq!(
        refusal(&lab, &zip),
        DiscSetRefusal::MissingTracks(vec![WAV_NAME.to_string()])
    );
    assert_eq!(lab.diagnose(&zip).verdict, RecoveryVerdict::Unsupported);

    let mut members = standard_members();
    members[0].0 = ISO_NAME.to_uppercase();
    write_zip(&zip, &members);
    assert!(matches!(
        refusal(&lab, &zip),
        DiscSetRefusal::CaseMismatch { .. }
    ));

    let mut members = standard_members();
    members[2].1 = vec![0xff, 0xfe, 0x00, 0x01];
    write_zip(&zip, &members);
    assert!(matches!(refusal(&lab, &zip), DiscSetRefusal::Cue(_)));

    let mut members = standard_members();
    members.push(("second.cue".into(), cue_text()));
    write_zip(&zip, &members);
    assert!(matches!(
        refusal(&lab, &zip),
        DiscSetRefusal::MultipleCueSheets(_)
    ));

    write_zip(&zip, &standard_members()[..2]);
    assert_eq!(refusal(&lab, &zip), DiscSetRefusal::NoCueSheet);

    let members: Vec<_> = standard_members()
        .into_iter()
        .map(|(name, bytes)| (format!("Game/{name}"), bytes))
        .collect();
    write_zip(&zip, &members);
    assert_eq!(refusal(&lab, &zip), DiscSetRefusal::NestedFolders);
    assert!(lab.store_files().is_empty());
}

#[test]
fn links_special_files_encryption_and_duplicates_inside_the_zip_are_refused() {
    let lab = Lab::new();
    let zip = lab.zip_path("special.zip");

    let mut entries = raw_standard();
    entries[0].version_made_by = 3 << 8;
    entries[0].external_attributes = 0o120777 << 16;
    write_raw_zip(&zip, &entries);
    assert!(matches!(
        refusal(&lab, &zip),
        DiscSetRefusal::UnsafeMember(_)
    ));

    let mut entries = raw_standard();
    entries[1].flags = 1;
    write_raw_zip(&zip, &entries);
    assert!(matches!(
        refusal(&lab, &zip),
        DiscSetRefusal::Archive(_) | DiscSetRefusal::Limit(_)
    ));

    let mut entries = raw_standard();
    entries.push(RawEntry::new(&ISO_NAME.to_lowercase(), b"other"));
    write_raw_zip(&zip, &entries);
    assert!(matches!(
        refusal(&lab, &zip),
        DiscSetRefusal::UnsafeMember(_)
    ));
    assert!(lab.prepare(&zip).is_err());
    assert!(lab.store_files().is_empty());
}

#[test]
fn a_file_that_only_claims_to_be_a_zip_is_refused() {
    let lab = Lab::new();
    let zip = lab.zip_path("not-a-zip.zip");
    std::fs::write(&zip, iso_bytes("CDTV", "X")).unwrap();
    assert!(matches!(refusal(&lab, &zip), DiscSetRefusal::Archive(_)));
    let report = lab.diagnose(&zip);
    assert_eq!(report.verdict, RecoveryVerdict::Refused);
    assert_eq!(
        report
            .finding(FindingCode::ArchiveRefused)
            .unwrap()
            .blocking,
        Blocking::Preparation
    );
    std::fs::write(&zip, b"").unwrap();
    assert!(matches!(refusal(&lab, &zip), DiscSetRefusal::Archive(_)));
}

#[test]
fn compression_bombs_and_oversized_sets_are_refused_by_fixed_limits() {
    let lab = Lab::new();
    let zip = lab.zip_path("bomb.zip");
    let mut members = standard_members();
    // 8 MiB of zeros deflates to a few KiB: far above 1000 to 1.
    members[0].1 = {
        let mut image = vec![0_u8; 8 * 1024 * 1024];
        image[..2048 * 24].copy_from_slice(&iso_bytes("CDTV", "BOMB"));
        image[18 * 2048..2048 * 24].fill(0);
        image
    };
    write_zip(&zip, &members);
    assert_eq!(
        refusal(&lab, &zip),
        DiscSetRefusal::Limit("compression ratio")
    );
    assert_eq!(lab.diagnose(&zip).verdict, RecoveryVerdict::Refused);
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::Refused(DiscSetRefusal::Limit(_)))
    ));

    let zip = lab.standard_zip();
    let small = DiscSetLimits {
        max_member_bytes: 1024,
        ..DiscSetLimits::default()
    };
    assert!(matches!(
        lab.prepare_with(&zip, &small, &|_| {}),
        Err(PrepareError::Refused(DiscSetRefusal::Limit(_)))
    ));
    let small = DiscSetLimits {
        max_total_bytes: 50_000,
        ..DiscSetLimits::default()
    };
    assert!(matches!(
        lab.prepare_with(&zip, &small, &|_| {}),
        Err(PrepareError::Refused(DiscSetRefusal::Limit(_)))
    ));
    let small = DiscSetLimits {
        max_members: 2,
        ..DiscSetLimits::default()
    };
    assert!(matches!(
        lab.prepare_with(&zip, &small, &|_| {}),
        Err(PrepareError::Refused(DiscSetRefusal::Limit(_)))
    ));
    assert!(lab.store_files().is_empty());
}

#[test]
fn a_zip_whose_index_lies_about_size_or_checksum_never_becomes_a_prepared_copy() {
    let lab = Lab::new();
    let zip = lab.zip_path("liar.zip");

    // The index promises a different checksum for the data track.
    let mut entries = raw_standard();
    entries[0].crc = Some(0xdead_beef);
    write_raw_zip(&zip, &entries);
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::Extraction(_))
    ));
    assert!(
        lab.store_files().is_empty(),
        "the failed attempt left nothing"
    );
    assert!(lab.store.list().is_empty());
    assert!(lab.journal().contains("preparation_failed"));

    // The index promises fewer bytes than the member holds.
    let mut entries = raw_standard();
    entries[0].declared_size = Some(2048);
    write_raw_zip(&zip, &entries);
    assert!(lab.prepare(&zip).is_err());
    assert!(lab.store_files().is_empty());

    // A flipped byte in the stored data.
    write_raw_zip(&zip, &raw_standard());
    let mut bytes = std::fs::read(&zip).unwrap();
    let position = bytes.len() / 3;
    bytes[position] ^= 0x55;
    std::fs::write(&zip, bytes).unwrap();
    assert!(lab.prepare(&zip).is_err());
    assert!(lab.store_files().is_empty());
}

#[test]
fn tracks_that_are_not_what_the_cue_sheet_says_fail_verification() {
    let lab = Lab::new();
    let zip = lab.zip_path("wrong-kind.zip");

    let mut members = standard_members();
    members[1].1 = b"ID3 this is not a wave file at all".to_vec();
    write_zip(&zip, &members);
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::Verification(detail)) if detail.contains("not a WAV")
    ));
    assert!(lab.store_files().is_empty());

    let mut members = standard_members();
    members[0].1 = (0..2048 * 24).map(|index| (index % 253) as u8).collect();
    write_zip(&zip, &members);
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::Verification(detail)) if detail.contains("ISO 9660")
    ));
    assert!(lab.store_files().is_empty());
    assert!(lab.store.list().is_empty());
}

// ---------------------------------------------------------------------------
// Interruption, cancellation, concurrency, rollback
// ---------------------------------------------------------------------------

#[test]
fn a_killed_preparation_is_never_used_and_cleans_up_exactly() {
    for point in [
        Point::AfterIntent,
        Point::AfterMember(0),
        Point::AfterMember(2),
        Point::BeforeReceipt,
        Point::BeforePublish,
    ] {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let before = snapshot(&zip);
        let crashed = with_hook(
            move |reached| {
                if reached == point {
                    panic!("simulated kill at {point:?}");
                }
            },
            || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lab.prepare(&zip))),
        );
        assert!(crashed.is_err(), "{point:?}");

        // Nothing is committed, the leftovers are visible and unusable.
        assert!(lab.store.list().is_empty(), "{point:?}");
        let interrupted = lab.store.interrupted();
        assert_eq!(interrupted.len(), 1, "{point:?}");
        assert_eq!(interrupted[0].source_path.as_deref(), Some(zip.as_path()));
        let leftover_cue = interrupted[0].directory.join("media").join(CUE_NAME);
        assert!(matches!(
            lab.store.check_launch_path(&leftover_cue),
            LaunchPathCheck::Refused(_)
        ));
        let report = lab.diagnose(&zip);
        assert!(report.prepared.is_none());
        assert!(
            report
                .finding(FindingCode::PreparationInterrupted)
                .is_some()
        );
        assert_eq!(report.verdict, RecoveryVerdict::FixableByPreparation);

        // Clean-up removes exactly the leftovers; a fresh preparation works.
        let cleaned = lab.store.clean_interrupted().unwrap();
        assert_eq!(cleaned.len(), 1);
        assert!(
            cleaned[0].kept.is_empty(),
            "{point:?}: {:?}",
            cleaned[0].kept
        );
        assert!(lab.store_files().is_empty(), "{point:?}");
        assert!(lab.store.interrupted().is_empty());
        assert!(!lab.prepare(&zip).unwrap().already_prepared);
        assert_eq!(snapshot(&zip), before, "{point:?}");
    }
}

#[test]
fn cancelling_stops_the_preparation_and_leaves_nothing() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let result = lab.prepare_with(&zip, &DiscSetLimits::default(), &|progress| {
        if matches!(progress, PrepareProgress::Extracting { index: 1, .. }) {
            lab.cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(result.unwrap_err(), PrepareError::Cancelled);
    assert!(lab.store_files().is_empty());
    assert!(lab.store.interrupted().is_empty());
    lab.cancel.store(false, Ordering::Relaxed);
    assert!(lab.prepare(&zip).is_ok());
}

#[test]
fn a_zip_that_changes_during_preparation_is_refused() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let target = zip.clone();
    let result = with_hook(
        move |reached| {
            if reached == Point::AfterMember(2) {
                // Same bytes, new modification time, after the last file was
                // read out: still not the file that was inspected.
                let file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&target)
                    .unwrap();
                file.set_modified(
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
                )
                .unwrap();
            }
        },
        || lab.prepare(&zip),
    );
    assert_eq!(result.unwrap_err(), PrepareError::SourceChanged);
    assert!(lab.store_files().is_empty());
}

#[test]
fn concurrent_preparations_of_the_same_disc_produce_exactly_one_copy() {
    let lab = std::sync::Arc::new(Lab::new());
    let zip = lab.standard_zip();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let lab = lab.clone();
            let zip = zip.clone();
            std::thread::spawn(move || lab.prepare(&zip).unwrap())
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| !outcome.already_prepared)
            .count(),
        1
    );
    assert!(
        outcomes
            .windows(2)
            .all(|pair| pair[0].summary == pair[1].summary)
    );
    assert_eq!(lab.store.list().len(), 1);
    assert!(lab.store.interrupted().is_empty());
    assert_eq!(lab.journal().matches("prepared_copy_created").count(), 1);
}

#[test]
fn removal_and_clean_up_wait_their_turn_behind_a_running_preparation() {
    let lab = std::sync::Arc::new(Lab::new());
    let first = lab.standard_zip();
    let existing = lab.prepare(&first).unwrap().summary;
    let second = lab.zip_path("second.zip");
    let mut members = standard_members();
    members[0].1 = iso_bytes("CDTV", "SECOND");
    write_zip(&second, &members);

    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
    let worker = {
        let lab = lab.clone();
        std::thread::spawn(move || {
            with_hook(
                move |reached| {
                    if reached == Point::AfterIntent {
                        reached_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }
                },
                || lab.prepare(&second),
            )
        })
    };
    reached_rx.recv().unwrap();
    assert_eq!(lab.store.remove(&existing.set_id), Err(RemoveError::Busy));
    assert_eq!(lab.store.clean_interrupted(), Err(RemoveError::Busy));
    // The running preparation's work folder was not touched.
    assert_eq!(lab.store.interrupted().len(), 1);
    resume_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_ok());
    assert_eq!(lab.store.list().len(), 2);
    assert!(lab.store.remove(&existing.set_id).is_ok());
}

#[test]
fn a_prepared_copy_that_was_tampered_with_is_refused_never_overwritten_and_removed_safely() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    let track = summary.directory.join("media").join(ISO_NAME);
    std::fs::set_permissions(&track, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&track)
        .unwrap()
        .write_all(b"tampered")
        .unwrap();
    let stranger = summary
        .directory
        .join("media/save-written-by-something-else.dat");
    std::fs::write(&stranger, b"not ours").unwrap();

    assert!(matches!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Refused(_)
    ));
    let report = lab.diagnose(&zip);
    assert_eq!(report.verdict, RecoveryVerdict::FixableWithYourHelp);
    assert!(report.prepared.is_none());
    assert_eq!(
        report
            .finding(FindingCode::PreparedCopyDamaged)
            .unwrap()
            .blocking,
        Blocking::Launch
    );
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::ExistingCopyDamaged(_))
    ));
    assert!(lab.store.verify_full(&summary.set_id, &lab.cancel).is_err());

    // Removal deletes what is provably ours and keeps the rest, reported.
    let removal = lab.store.remove(&summary.set_id).unwrap();
    assert_eq!(removal.removed_files.len(), 2);
    assert!(removal.kept.iter().any(|path| path.ends_with(ISO_NAME)));
    let remaining = lab.store_files();
    assert_eq!(remaining.len(), 2, "{remaining:?}");
    assert!(remaining.iter().any(|path| path.ends_with(ISO_NAME)));
    assert!(
        remaining
            .iter()
            .any(|path| path.ends_with("save-written-by-something-else.dat"))
    );
    // Out of the committed sets: nothing can start from it, and a fresh
    // preparation is possible again.
    assert!(lab.store.list().is_empty());
    assert!(!lab.prepare(&zip).unwrap().already_prepared);
}

#[test]
fn a_bit_flip_that_keeps_size_and_times_is_caught_by_full_verification() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    let track = summary.directory.join("media").join(ISO_NAME);
    let metadata = std::fs::metadata(&track).unwrap();
    let (accessed, modified) = (metadata.accessed().unwrap(), metadata.modified().unwrap());
    std::fs::set_permissions(&track, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let mut bytes = std::fs::read(&track).unwrap();
    bytes[40_000] ^= 1;
    std::fs::write(&track, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&track)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_accessed(accessed)
                .set_modified(modified),
        )
        .unwrap();
    // The change time moved, so even the quick check refuses it.
    assert!(matches!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Refused(_)
    ));
    assert!(lab.store.verify_full(&summary.set_id, &lab.cancel).is_err());
}

#[test]
fn only_a_committed_sets_cue_sheet_may_be_started() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    assert_eq!(
        lab.store.check_launch_path(&zip),
        LaunchPathCheck::NotPreparedMedia
    );
    assert!(matches!(
        lab.store
            .check_launch_path(&summary.directory.join("media").join(ISO_NAME)),
        LaunchPathCheck::Refused(_)
    ));
    assert!(matches!(
        lab.store
            .check_launch_path(&summary.directory.join("receipt.json")),
        LaunchPathCheck::Refused(_)
    ));
    // A folder that exists inside the store but is not a set.
    std::fs::create_dir_all(lab.store.root().join("v1/sets/not-a-set/media")).unwrap();
    assert!(matches!(
        lab.store
            .check_launch_path(&lab.store.root().join("v1/sets/not-a-set/media/x.cue")),
        LaunchPathCheck::Refused(_)
    ));
    assert!(matches!(
        lab.store
            .check_launch_path(&lab.store.root().join("v1/journal.jsonl")),
        LaunchPathCheck::Refused(_)
    ));
}

#[test]
fn a_store_whose_folders_were_replaced_by_links_is_refused() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let elsewhere = lab.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(lab.store.root().join("v1")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, lab.store.root().join("v1/staging")).unwrap();
    assert!(matches!(lab.prepare(&zip), Err(PrepareError::Store(_))));
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());

    let lab = Lab::new();
    let zip = lab.standard_zip();
    std::fs::create_dir_all(lab.store.root().join("v1")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, lab.store.root().join("v1/sets")).unwrap();
    assert!(matches!(lab.prepare(&zip), Err(PrepareError::Store(_))));
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
}

#[test]
fn removing_needs_a_usable_receipt_and_a_real_id() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    assert_eq!(lab.store.remove("../../etc"), Err(RemoveError::NotFound));
    assert_eq!(
        lab.store.remove(&"0".repeat(32)),
        Err(RemoveError::NotFound)
    );
    std::fs::remove_file(summary.directory.join("receipt.json")).unwrap();
    assert!(matches!(
        lab.store.remove(&summary.set_id),
        Err(RemoveError::NoUsableReceipt(_))
    ));
    // Nothing was deleted.
    assert!(summary.launch_path.exists());
}

#[test]
fn a_launch_finds_the_prepared_copy_only_for_the_unchanged_zip() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    assert!(lab.store.prepared_for_source(&zip).is_none());
    let summary = lab.prepare(&zip).unwrap().summary;
    assert_eq!(lab.store.prepared_for_source(&zip), Some(summary.clone()));
    // Another path is not this ZIP, whatever it holds.
    let copy = lab.zip_path("copy.zip");
    std::fs::copy(&zip, &copy).unwrap();
    assert!(lab.store.prepared_for_source(&copy).is_none());
    // The ZIP changed after preparation: the link no longer applies.
    let file = std::fs::OpenOptions::new().append(true).open(&zip).unwrap();
    file.set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000),
    )
    .unwrap();
    drop(file);
    assert!(lab.store.prepared_for_source(&zip).is_none());
    assert_eq!(
        lab.diagnose(&zip).verdict,
        RecoveryVerdict::FixableByPreparation
    );
    // Preparing again re-links it without extracting.
    assert!(lab.prepare(&zip).unwrap().already_prepared);
    assert_eq!(lab.store.prepared_for_source(&zip), Some(summary.clone()));
    // Removing the set removes its links.
    lab.store.remove(&summary.set_id).unwrap();
    assert!(lab.store.prepared_for_source(&zip).is_none());
    assert!(lab.store_files().is_empty());
}

// ---------------------------------------------------------------------------
// Performance census on a disposable copy of a real library (opt-in)
// ---------------------------------------------------------------------------

/// Not a correctness test: prints how long the engine's per-game entry points
/// take over every row of a library database copy. Run with
/// `EMUWIZ_RECOVERY_CENSUS_DB=<copy of library.sqlite3>` and `--ignored
/// --nocapture`. The database is opened read-only; game files are only read
/// when `EMUWIZ_RECOVERY_CENSUS_DIAGNOSE=1`, and then only by `diagnose`.
#[test]
#[ignore = "needs a library database copy; see the doc comment"]
fn census_on_a_library_copy() {
    use std::os::unix::ffi::OsStringExt;
    let Some(database) = std::env::var_os("EMUWIZ_RECOVERY_CENSUS_DB") else {
        return;
    };
    let connection = rusqlite::Connection::open_with_flags(
        &database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare(
            "SELECT a.absolute_path_cached, p.platform, p.source FROM archives a \
             LEFT JOIN platform_assignments p ON p.archive_id = a.id AND p.is_current = 1",
        )
        .unwrap();
    let rows: Vec<(PathBuf, Option<String>, Option<String>)> = statement
        .query_map([], |row| {
            Ok((
                PathBuf::from(std::ffi::OsString::from_vec(row.get::<_, Vec<u8>>(0)?)),
                row.get(1)?,
                row.get(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    println!("census: {} library rows", rows.len());

    // 1. The check every caller runs first (and the only recovery code a
    //    game page runs for a game the engine has nothing for).
    let started = std::time::Instant::now();
    let mut candidates = Vec::new();
    let mut covered = 0_usize;
    for (path, platform, source) in &rows {
        if super::diagnose::may_apply(path, platform.as_deref()) {
            covered += 1;
        }
        if super::diagnose::preparation_may_apply(path, platform.as_deref()) {
            candidates.push((path.clone(), platform.clone(), source.clone()));
        }
    }
    let elapsed = started.elapsed();
    println!(
        "census: pure applicability check over all rows: {:?} total, {:.0} ns per game; {covered} rows have a capability rule, {} are preparation candidates",
        elapsed,
        elapsed.as_nanos() as f64 / rows.len().max(1) as f64,
        candidates.len()
    );

    // 2. The readiness seam's lookup for candidates (no ZIP is read).
    let store_root = std::env::var_os("EMUWIZ_RECOVERY_CENSUS_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("emuwiz-recovery-census-empty-store"));
    let store = PreparedMediaStore::at(store_root);
    let started = std::time::Instant::now();
    let mut linked = 0_usize;
    for (path, _, _) in &candidates {
        if store.prepared_for_source(path).is_some() {
            linked += 1;
        }
    }
    let elapsed = started.elapsed();
    println!(
        "census: prepared-copy lookup for {} candidates: {:?} total, {:.1} us each; {linked} have a verified prepared copy",
        candidates.len(),
        elapsed,
        elapsed.as_micros() as f64 / candidates.len().max(1) as f64
    );

    // 3. Full diagnosis of the candidates (worker-thread work; reads each
    //    ZIP's index and cue sheet).
    if std::env::var_os("EMUWIZ_RECOVERY_CENSUS_DIAGNOSE").is_none() {
        return;
    }
    let host = HostFacts::default();
    let cancel = AtomicBool::new(false);
    let trusted = TrustedRoots::none();
    let mut verdicts = std::collections::BTreeMap::<String, usize>::new();
    let mut slowest = std::time::Duration::ZERO;
    let started = std::time::Instant::now();
    for (path, platform, source) in &candidates {
        let one = std::time::Instant::now();
        let report = diagnose(
            &RecoverySubject::new(path, platform.as_deref())
                .with_platform_source(source.as_deref()),
            &RecoveryContext {
                store: &store,
                trusted: &trusted,
                limits: DiscSetLimits::default(),
                host: &host,
                cancel: &cancel,
            },
        );
        slowest = slowest.max(one.elapsed());
        let disc = report
            .findings
            .iter()
            .find(|finding| {
                matches!(
                    finding.code,
                    FindingCode::DiscSetInsideZip
                        | FindingCode::DiscSetIncomplete
                        | FindingCode::ArchiveRefused
                        | FindingCode::SourceUnavailable
                )
            })
            .map(|finding| format!("{:?}", finding.code))
            .unwrap_or_else(|| "none".into());
        *verdicts.entry(disc).or_default() += 1;
    }
    let elapsed = started.elapsed();
    println!(
        "census: diagnosis of {} candidates: {:?} total, {:.1} ms mean, {:?} slowest; outcomes {verdicts:?}",
        candidates.len(),
        elapsed,
        elapsed.as_millis() as f64 / candidates.len().max(1) as f64,
        slowest
    );
}

// ---------------------------------------------------------------------------
// Regressions from the independent review (2026-11-20)
// ---------------------------------------------------------------------------

/// Four bytes that make `prefix || bytes` have the CRC-32 `target`.
fn crc_forging_suffix(prefix: &[u8], target: u32) -> [u8; 4] {
    let mut table = [0_u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
        }
        *entry = value;
    }
    // The register after the prefix, and the register the target needs.
    let mut state = 0xffff_ffff_u32;
    for byte in prefix {
        state = table[((state ^ u32::from(*byte)) & 0xff) as usize] ^ (state >> 8);
    }
    let mut wanted = target ^ 0xffff_ffff;
    let mut indices = [0_u8; 4];
    for slot in indices.iter_mut().rev() {
        let index = table
            .iter()
            .position(|entry| entry >> 24 == wanted >> 24)
            .unwrap();
        *slot = index as u8;
        wanted = (wanted ^ table[index]) << 8;
    }
    let mut suffix = [0_u8; 4];
    for (position, index) in indices.iter().enumerate() {
        suffix[position] = (state as u8) ^ index;
        state = table[*index as usize] ^ (state >> 8);
    }
    suffix
}

#[test]
fn a_zip_with_the_same_names_sizes_and_checksums_but_other_bytes_does_not_get_the_existing_copy() {
    let lab = Lab::new();
    let first = lab.standard_zip();
    let summary = lab.prepare(&first).unwrap().summary;

    // A different data track engineered to the same size and CRC-32.
    let original = iso_bytes("CDTV", "SYNTHETIC");
    let mut forged = original.clone();
    let length = forged.len();
    forged[20 * 2048] ^= 0xff;
    let suffix = crc_forging_suffix(&forged[..length - 4], crc(&original));
    forged[length - 4..].copy_from_slice(&suffix);
    assert_ne!(forged, original);
    assert_eq!(crc(&forged), crc(&original));
    let mut members = standard_members();
    members[0].1 = forged;
    let second = lab.zip_path("collision.zip");
    write_zip(&second, &members);

    let inspection = |path: &Path| {
        inspect_disc_zip(path, &lab.trusted, &DiscSetLimits::default(), &lab.cancel).unwrap()
    };
    assert_eq!(
        inspection(&first).fingerprint,
        inspection(&second).fingerprint
    );
    assert!(matches!(
        lab.prepare(&second),
        Err(PrepareError::Verification(detail)) if detail.contains("different contents")
    ));
    assert!(lab.store.prepared_for_source(&second).is_none());
    // The existing copy is untouched and still serves its own ZIP.
    assert_eq!(lab.store.prepared_for_source(&first), Some(summary.clone()));
    assert!(lab.store.verify_full(&summary.set_id, &lab.cancel).is_ok());
}

#[test]
fn bytes_that_differ_on_disk_from_what_was_decoded_fail_the_read_back() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let store_root = lab.store.root().to_path_buf();
    let result = with_hook(
        move |reached| {
            // After the last file was written and synced, and before the
            // read-back: damage one written file in place, keeping its size.
            if reached == Point::AfterMember(2) {
                let staging = std::fs::read_dir(store_root.join("v1/staging"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let track = staging.join("media").join(ISO_NAME);
                let mut bytes = std::fs::read(&track).unwrap();
                bytes[30_000] ^= 0x40;
                std::fs::write(&track, bytes).unwrap();
            }
        },
        || lab.prepare(&zip),
    );
    assert!(matches!(
        result,
        Err(PrepareError::Verification(detail)) if detail.contains("read back from disk")
    ));
    assert!(lab.store_files().is_empty());
    assert!(lab.store.list().is_empty());
}

#[test]
fn full_verification_compares_contents_even_when_the_quick_check_passes() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    // Damage a file, then forge the receipt's recorded inode and times so the
    // metadata-only quick check is satisfied.
    let track = summary.directory.join("media").join(ISO_NAME);
    std::fs::set_permissions(&track, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let mut bytes = std::fs::read(&track).unwrap();
    bytes[30_000] ^= 0x40;
    std::fs::write(&track, bytes).unwrap();
    let metadata = std::fs::symlink_metadata(&track).unwrap();
    let receipt_path = summary.directory.join("receipt.json");
    let mut receipt: Receipt =
        serde_json::from_str(&std::fs::read_to_string(&receipt_path).unwrap()).unwrap();
    let entry = receipt
        .files
        .iter_mut()
        .find(|file| file.name == ISO_NAME)
        .unwrap();
    entry.modified_ns =
        i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
    entry.changed_ns =
        i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
    std::fs::write(&receipt_path, serde_json::to_string(&receipt).unwrap()).unwrap();
    assert!(matches!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Verified(_)
    ));
    let error = lab
        .store
        .verify_full(&summary.set_id, &lab.cancel)
        .unwrap_err();
    assert!(error.contains("recorded checksum"), "{error}");
}

#[test]
fn removal_and_clean_up_never_delete_through_a_link() {
    // A set whose media folder was swapped for a link to a folder holding
    // files with the same names and sizes.
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    let outside = lab.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    for (name, bytes) in standard_members() {
        std::fs::write(outside.join(name), bytes).unwrap();
    }
    let media = summary.directory.join("media");
    let moved = lab.root.join("real-media");
    std::fs::rename(&media, &moved).unwrap();
    std::os::unix::fs::symlink(&outside, &media).unwrap();
    assert!(matches!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Refused(_) | LaunchPathCheck::NotPreparedMedia
    ));
    let removal = lab.store.remove(&summary.set_id).unwrap();
    assert!(removal.removed_files.is_empty());
    assert!(!removal.kept.is_empty());
    for (name, bytes) in standard_members() {
        assert_eq!(std::fs::read(outside.join(&name)).unwrap(), bytes, "{name}");
    }

    // A store whose staging or sources folder is a link is not used at all.
    for folder in ["staging", "sources"] {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let summary = lab.prepare(&zip).unwrap().summary;
        let elsewhere = lab.root.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("victim/media")).unwrap();
        std::fs::write(elsewhere.join("victim/intent.json"), b"{}").unwrap();
        std::fs::write(elsewhere.join("victim/media/keep.bin"), b"keep").unwrap();
        let real = lab.store.root().join("v1").join(folder);
        std::fs::rename(&real, lab.root.join("real-folder")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &real).unwrap();
        assert_eq!(
            lab.store.remove(&summary.set_id),
            Err(RemoveError::NotFound)
        );
        assert!(lab.store.clean_interrupted().unwrap().is_empty());
        assert!(lab.store.prepared_for_source(&zip).is_none());
        assert!(lab.store.list().is_empty());
        assert!(elsewhere.join("victim/intent.json").exists(), "{folder}");
        assert!(elsewhere.join("victim/media/keep.bin").exists(), "{folder}");
        assert!(
            matches!(lab.prepare(&zip), Err(PrepareError::Store(_))),
            "{folder}"
        );
        assert!(
            summary.launch_path.exists(),
            "{folder}: the set was not touched"
        );
    }

    // A linked `v1` is refused before anything is created through it.
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let elsewhere = lab.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(lab.store.root()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, lab.store.root().join("v1")).unwrap();
    assert!(matches!(lab.prepare(&zip), Err(PrepareError::Store(_))));
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
}

#[test]
fn a_removal_that_was_cut_short_is_finished_by_clean_up() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let before = snapshot(&zip);
    let summary = lab.prepare(&zip).unwrap().summary;
    let set_id = summary.set_id.clone();
    let crashed = with_hook(
        |reached| {
            if reached == Point::AfterRemovalParked {
                panic!("simulated kill during removal");
            }
        },
        || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lab.store.remove(&set_id))),
    );
    assert!(crashed.is_err());
    // Out of sight for launches at once; the files are still on disk.
    assert!(lab.store.list().is_empty());
    assert!(lab.store.prepared_for_source(&zip).is_none());
    assert!(matches!(
        lab.store.check_launch_path(&summary.launch_path),
        LaunchPathCheck::Refused(_) | LaunchPathCheck::NotPreparedMedia
    ));
    assert_eq!(lab.store.interrupted().len(), 1);
    assert!(lab.store_files().len() >= 3);

    let cleaned = lab.store.clean_interrupted().unwrap();
    assert_eq!(cleaned.len(), 1);
    assert_eq!(cleaned[0].removed_files.len(), 3);
    assert!(cleaned[0].kept.is_empty(), "{:?}", cleaned[0].kept);
    assert!(lab.store.interrupted().is_empty());
    // Only the stale index record may remain; preparing again replaces it.
    assert!(
        lab.store_files()
            .iter()
            .all(|path| path.parent().unwrap().ends_with("sources"))
    );
    assert!(!lab.prepare(&zip).unwrap().already_prepared);
    assert_eq!(snapshot(&zip), before);
}

#[test]
fn the_launch_gate_judges_where_a_file_really_is() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    let media = summary.directory.join("media");
    // The same files reached through a link and through `..`.
    let alias = lab.root.join("alias");
    std::os::unix::fs::symlink(&media, &alias).unwrap();
    assert!(matches!(
        lab.store.check_launch_path(&alias.join(ISO_NAME)),
        LaunchPathCheck::Refused(_)
    ));
    assert!(matches!(
        lab.store.check_launch_path(&alias.join(CUE_NAME)),
        LaunchPathCheck::Verified(_)
    ));
    let dotted = media.join("..").join("media").join(WAV_NAME);
    assert!(matches!(
        lab.store.check_launch_path(&dotted),
        LaunchPathCheck::Refused(_)
    ));
    let store_alias = lab.root.join("store-alias");
    std::os::unix::fs::symlink(lab.store.root(), &store_alias).unwrap();
    assert!(matches!(
        lab.store
            .check_launch_path(&store_alias.join("v1/journal.jsonl")),
        LaunchPathCheck::Refused(_)
    ));
}

#[test]
fn a_disc_whose_own_label_names_another_system_is_not_prepared_as_cd32() {
    let lab = Lab::new();
    let zip = lab.zip_path("playstation-disc-in-the-cd32-folder.zip");
    let mut members = standard_members();
    members[0].1 = iso_bytes("PLAYSTATION", "SLUS_000.00");
    write_zip(&zip, &members);
    assert!(matches!(
        lab.prepare(&zip),
        Err(PrepareError::Verification(detail)) if detail.contains("PLAYSTATION")
    ));
    assert!(lab.store_files().is_empty());
    assert!(lab.store.list().is_empty());
}
