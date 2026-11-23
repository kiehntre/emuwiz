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
        // The tests do not depend on what the temporary folder is really on.
        assume_ext4();
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

// ---------------------------------------------------------------------------
// Renames and moves of a prepared ZIP
// ---------------------------------------------------------------------------

mod moves_tests {
    use super::*;
    use crate::launch::remembered_launch_system as remembered;
    use crate::recovery::moves;

    struct Moved {
        lab: Lab,
        zip: PathBuf,
        summary: PreparedSetSummary,
        remembered_file: PathBuf,
    }

    /// A prepared ZIP with a remembered launch choice.
    fn prepared_and_remembered() -> Moved {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let summary = lab.prepare(&zip).unwrap().summary;
        let remembered_file = lab.root.join("config/remembered_launch_systems.json");
        remembered::remember_launch_system_at(
            &remembered_file,
            &remembered::RememberedLaunchSystem::for_current_file(&zip, "AmigaCD32").unwrap(),
        )
        .unwrap();
        Moved {
            lab,
            zip,
            summary,
            remembered_file,
        }
    }

    fn remembered_for(moved: &Moved, path: &Path) -> Option<&'static str> {
        remembered::remembered_launch_system_at(&moved.remembered_file, path)
    }

    /// What EmuWiz's own rename does: capture, rename, carry.
    fn emuwiz_rename(moved: &Moved, from: &Path, to: &Path) -> usize {
        let pending = moves::before_in(&moved.lab.store, [(from, to)]);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(from, to).unwrap();
        moves::after_in(
            &moved.lab.store,
            Some(&moved.remembered_file),
            pending,
            |_, _| true,
        )
    }

    fn rediscover(moved: &Moved, path: &Path) -> Result<RelinkOutcome, PrepareError> {
        moves::rediscover(
            &moved.lab.store,
            Some(&moved.remembered_file),
            path,
            &moved.lab.trusted,
            &DiscSetLimits::default(),
            &moved.lab.cancel,
            &|_| {},
        )
    }

    #[test]
    fn an_emuwiz_rename_move_and_undo_keep_the_copy_and_the_remembered_choice() {
        let moved = prepared_and_remembered();
        let track_inode = std::fs::metadata(&moved.summary.launch_path).unwrap().ino();
        let renamed = moved.lab.zip_path("A Better Name (1994).zip");
        assert_eq!(emuwiz_rename(&moved, &moved.zip, &renamed), 1);
        assert_eq!(
            moved.lab.store.prepared_for_source(&renamed),
            Some(moved.summary.clone())
        );
        assert!(moved.lab.store.prepared_for_source(&moved.zip).is_none());
        assert_eq!(remembered_for(&moved, &renamed), Some("AmigaCD32"));
        assert_eq!(remembered_for(&moved, &moved.zip), None);

        // A move to another folder on the same disk.
        let elsewhere = moved
            .lab
            .root
            .join("games/sorted/cd32/A Better Name (1994).zip");
        assert_eq!(emuwiz_rename(&moved, &renamed, &elsewhere), 1);
        assert_eq!(
            moved.lab.store.prepared_for_source(&elsewhere),
            Some(moved.summary.clone())
        );
        assert_eq!(remembered_for(&moved, &elsewhere), Some("AmigaCD32"));
        // What a launch checks after the move: the copy for this file, and
        // its cue sheet through the launch gate.
        let after_move = moved.lab.store.prepared_for_source(&elsewhere).unwrap();
        assert!(matches!(
            moved.lab.store.check_launch_path(&after_move.launch_path),
            LaunchPathCheck::Verified(_)
        ));

        // Undo, twice: back where it started.
        assert_eq!(emuwiz_rename(&moved, &elsewhere, &renamed), 1);
        assert_eq!(emuwiz_rename(&moved, &renamed, &moved.zip), 1);
        assert_eq!(
            moved.lab.store.prepared_for_source(&moved.zip),
            Some(moved.summary.clone())
        );
        assert_eq!(remembered_for(&moved, &moved.zip), Some("AmigaCD32"));
        // Nothing was extracted again and exactly one link record exists.
        assert_eq!(
            std::fs::metadata(&moved.summary.launch_path).unwrap().ino(),
            track_inode
        );
        assert_eq!(
            std::fs::read_dir(moved.lab.store.root().join("v1/sources"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            moved
                .lab
                .journal()
                .matches("source_link_followed_rename")
                .count(),
            4
        );
    }

    #[test]
    fn a_rename_that_did_not_happen_or_a_stale_link_is_not_followed() {
        let moved = prepared_and_remembered();
        let renamed = moved.lab.zip_path("renamed.zip");
        // The transaction reports that this entry was not applied.
        let pending =
            moves::before_in(&moved.lab.store, [(moved.zip.as_path(), renamed.as_path())]);
        assert!(!pending.is_empty());
        assert_eq!(
            moves::after_in(
                &moved.lab.store,
                Some(&moved.remembered_file),
                pending,
                |_, _| false
            ),
            0
        );
        assert!(moved.lab.store.prepared_for_source(&moved.zip).is_some());

        // The ZIP changed after preparation: nothing to capture, nothing carried.
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(&moved.zip)
            .unwrap();
        file.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3_000_000),
        )
        .unwrap();
        drop(file);
        assert_eq!(emuwiz_rename(&moved, &moved.zip, &renamed), 0);
        assert!(moved.lab.store.prepared_for_source(&renamed).is_none());
        assert_eq!(remembered_for(&moved, &renamed), None);
    }

    #[test]
    fn a_different_file_with_the_same_name_size_and_times_is_never_accepted() {
        let moved = prepared_and_remembered();
        let metadata = std::fs::metadata(&moved.zip).unwrap();
        let (size, modified, accessed) = (
            metadata.len(),
            metadata.modified().unwrap(),
            metadata.accessed().unwrap(),
        );
        let renamed = moved.lab.zip_path("renamed.zip");
        let pending =
            moves::before_in(&moved.lab.store, [(moved.zip.as_path(), renamed.as_path())]);
        // Instead of the rename: another file of the same size and times
        // appears at the new name, and the original disappears.
        std::fs::write(&renamed, vec![0x5a_u8; size as usize]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&renamed)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(accessed)
                    .set_modified(modified),
            )
            .unwrap();
        std::fs::remove_file(&moved.zip).unwrap();
        assert_eq!(
            moves::after_in(
                &moved.lab.store,
                Some(&moved.remembered_file),
                pending,
                |_, _| true
            ),
            0
        );
        assert!(moved.lab.store.prepared_for_source(&renamed).is_none());
        assert_eq!(remembered_for(&moved, &renamed), None);
        // The content check refuses it too.
        assert!(rediscover(&moved, &renamed).is_err());
        assert!(moved.lab.store.prepared_for_source(&renamed).is_none());

        // The same trick at the original path: same name, size and times.
        let second = prepared_and_remembered();
        let metadata = std::fs::metadata(&second.zip).unwrap();
        let (modified, accessed) = (metadata.modified().unwrap(), metadata.accessed().unwrap());
        let mut members = standard_members();
        members[0].1 = iso_bytes("CDTV", "IMPOSTOR!");
        write_zip(&second.zip, &members);
        std::fs::File::options()
            .write(true)
            .open(&second.zip)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(accessed)
                    .set_modified(modified),
            )
            .unwrap();
        assert!(second.lab.store.prepared_for_source(&second.zip).is_none());
    }

    #[test]
    fn a_zip_renamed_outside_emuwiz_finds_its_copy_by_content_without_extracting() {
        let moved = prepared_and_remembered();
        let track_inode = std::fs::metadata(&moved.summary.launch_path).unwrap().ino();
        let renamed = moved
            .lab
            .root
            .join("games/amiga-cd32/Renamed In A File Manager.zip");
        std::fs::rename(&moved.zip, &renamed).unwrap();
        assert!(moved.lab.store.prepared_for_source(&renamed).is_none());
        let report = moved.lab.diagnose(&renamed);
        assert!(report.relinkable);
        assert_eq!(report.verdict, RecoveryVerdict::FixableByPreparation);

        assert_eq!(
            rediscover(&moved, &renamed).unwrap(),
            RelinkOutcome::Linked {
                summary: Box::new(moved.summary.clone()),
                moved_from: Some(moved.zip.clone()),
            }
        );
        assert_eq!(
            moved.lab.store.prepared_for_source(&renamed),
            Some(moved.summary.clone())
        );
        assert_eq!(remembered_for(&moved, &renamed), Some("AmigaCD32"));
        assert_eq!(
            std::fs::metadata(&moved.summary.launch_path).unwrap().ino(),
            track_inode,
            "nothing was extracted again"
        );
        // The stale record of the old path is gone.
        assert_eq!(
            std::fs::read_dir(moved.lab.store.root().join("v1/sources"))
                .unwrap()
                .count(),
            1
        );
        let report = moved.lab.diagnose(&renamed);
        assert!(!report.relinkable);
        assert_eq!(report.verdict, RecoveryVerdict::PlayableFromPreparedCopy);
        assert!(moved.lab.journal().contains("source_linked_by_content"));
    }

    #[test]
    fn a_copy_of_the_zip_is_linked_but_inherits_no_remembered_choice() {
        let moved = prepared_and_remembered();
        let duplicate = moved.lab.zip_path("duplicate.zip");
        std::fs::copy(&moved.zip, &duplicate).unwrap();
        assert_eq!(
            rediscover(&moved, &duplicate).unwrap(),
            RelinkOutcome::Linked {
                summary: Box::new(moved.summary.clone()),
                moved_from: None,
            }
        );
        assert!(moved.lab.store.prepared_for_source(&duplicate).is_some());
        assert!(moved.lab.store.prepared_for_source(&moved.zip).is_some());
        assert_eq!(
            remembered_for(&moved, &duplicate),
            None,
            "ambiguous: both exist"
        );
        assert_eq!(remembered_for(&moved, &moved.zip), Some("AmigaCD32"));
    }

    #[test]
    fn a_modified_or_unknown_zip_is_not_linked() {
        let moved = prepared_and_remembered();
        std::fs::remove_file(&moved.zip).unwrap();
        // Another disc entirely.
        let other = moved.lab.zip_path("other.zip");
        let mut members = standard_members();
        members[0].1 = iso_bytes("CDTV", "OTHERDISC");
        write_zip(&other, &members);
        assert_eq!(
            rediscover(&moved, &other).unwrap(),
            RelinkOutcome::NoMatchingCopy
        );
        assert!(moved.lab.store.prepared_for_source(&other).is_none());
        assert_eq!(remembered_for(&moved, &other), None);
        // Not a ZIP at all.
        let broken = moved.lab.zip_path("broken.zip");
        std::fs::write(&broken, b"not a zip").unwrap();
        assert!(rediscover(&moved, &broken).is_err());
        // A store that does not exist yet is not created by looking.
        let empty = Lab::new();
        let zip = empty.standard_zip();
        assert_eq!(
            empty
                .store
                .relink(
                    &zip,
                    &empty.trusted,
                    &DiscSetLimits::default(),
                    &empty.cancel,
                    &|_| {}
                )
                .unwrap(),
            RelinkOutcome::NoMatchingCopy
        );
        assert!(!empty.store.root().exists());
    }

    #[test]
    fn a_rename_during_another_preparation_is_recovered_by_content_afterwards() {
        let moved = std::sync::Arc::new(prepared_and_remembered());
        let other = moved.lab.zip_path("other.zip");
        let mut members = standard_members();
        members[0].1 = iso_bytes("CDTV", "OTHERDISC");
        write_zip(&other, &members);
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        let worker = {
            let moved = moved.clone();
            std::thread::spawn(move || {
                with_hook(
                    move |reached| {
                        if reached == Point::AfterIntent {
                            reached_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        }
                    },
                    || moved.lab.prepare(&other),
                )
            })
        };
        reached_rx.recv().unwrap();
        let renamed = moved.lab.zip_path("renamed.zip");
        // The store is busy: the rename itself is never held up or failed.
        assert_eq!(emuwiz_rename(&moved, &moved.zip, &renamed), 0);
        resume_tx.send(()).unwrap();
        assert!(worker.join().unwrap().is_ok());
        assert!(moved.lab.store.prepared_for_source(&renamed).is_none());
        assert!(matches!(
            rediscover(&moved, &renamed).unwrap(),
            RelinkOutcome::Linked {
                moved_from: Some(_),
                ..
            }
        ));
        assert_eq!(remembered_for(&moved, &renamed), Some("AmigaCD32"));
    }
}

// ---------------------------------------------------------------------------
// Bulk preparation
// ---------------------------------------------------------------------------

mod bulk_tests {
    use super::*;
    use crate::recovery::bulk::*;
    use crate::recovery::prepared_media::FREE_SPACE_OVERRIDE;

    fn candidate(path: &Path) -> BulkCandidate {
        BulkCandidate {
            path: path.to_path_buf(),
            platform_id: "AmigaCD32".into(),
            title: path.file_stem().unwrap().to_string_lossy().into_owned(),
        }
    }

    /// `count` different discs.
    fn discs(lab: &Lab, count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| {
                let path = lab.zip_path(&format!("disc-{index}.zip"));
                let mut members = standard_members();
                members[0].1 = iso_bytes("CDTV", &format!("DISC{index}"));
                write_zip(&path, &members);
                path
            })
            .collect()
    }

    fn plan(lab: &Lab, paths: &[PathBuf]) -> BulkPlan {
        let candidates: Vec<_> = paths.iter().map(|path| candidate(path)).collect();
        plan_bulk(
            &lab.store,
            &candidates,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap()
    }

    fn run(lab: &Lab) -> Result<Queue, BulkError> {
        run_queue(
            &lab.store,
            None,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        )
    }

    fn states(queue: &Queue) -> Vec<&'static str> {
        queue
            .items
            .iter()
            .map(|item| match item.state {
                QueueState::Pending => "pending",
                QueueState::Prepared => "prepared",
                QueueState::Reused => "reused",
                QueueState::Failed(_) => "failed",
            })
            .collect()
    }

    #[test]
    fn the_review_sorts_games_and_counts_each_disc_once_and_writes_nothing() {
        let lab = Lab::new();
        let mut paths = discs(&lab, 2);
        let duplicate = lab.zip_path("duplicate-of-0.zip");
        std::fs::copy(&paths[0], &duplicate).unwrap();
        let broken = lab.zip_path("broken.zip");
        std::fs::write(&broken, b"not a zip").unwrap();
        let wrong = lab.root.join("games/amiga-cd32/game.chd");
        std::fs::write(&wrong, b"x").unwrap();
        paths.extend([duplicate, broken, wrong, paths[1].clone()]);
        let plan = plan(&lab, &paths);
        let kinds: Vec<&str> = plan
            .items
            .iter()
            .map(|item| match item.plan {
                BulkItemPlan::Extract { .. } => "extract",
                BulkItemPlan::Reuse { .. } => "reuse",
                BulkItemPlan::AlreadyPrepared => "ready",
                BulkItemPlan::Refused(_) => "refused",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "extract", "extract", "reuse", "refused", "refused", "refused"
            ]
        );
        let one = match plan.items[0].plan {
            BulkItemPlan::Extract { bytes, .. } => bytes,
            _ => unreachable!(),
        };
        assert_eq!(plan.extract_bytes, 2 * one, "the duplicate adds nothing");
        assert!(plan.required_free_bytes > plan.extract_bytes + FREE_SPACE_RESERVE_BYTES);
        assert!(!lab.store.root().exists(), "a review creates nothing");

        let too_many: Vec<_> = (0..=MAX_BULK_ITEMS)
            .map(|index| candidate(&lab.zip_path(&format!("{index}.zip"))))
            .collect();
        assert!(
            plan_bulk(
                &lab.store,
                &too_many,
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel
            )
            .is_err()
        );
    }

    #[test]
    fn a_batch_prepares_only_the_chosen_games_and_reuses_existing_copies() {
        let lab = Lab::new();
        let mut paths = discs(&lab, 3);
        let duplicate = lab.zip_path("duplicate-of-0.zip");
        std::fs::copy(&paths[0], &duplicate).unwrap();
        paths.push(duplicate.clone());
        let before: Vec<_> = paths.iter().map(|path| snapshot(path)).collect();
        let reviewed = plan(&lab, &paths);
        // Disc 2 is left unticked.
        let skipped = paths[2].clone();
        let queue = start_queue(&lab.store, &reviewed, &|path| path != skipped).unwrap();
        assert_eq!(states(&queue), ["pending", "pending", "pending"]);
        let queue = run(&lab).unwrap();
        assert_eq!(states(&queue), ["prepared", "prepared", "reused"]);
        assert_eq!(queue.stopped, None);
        assert_eq!(lab.store.list().len(), 2);
        assert!(lab.store.prepared_for_source(&paths[0]).is_some());
        assert!(lab.store.prepared_for_source(&duplicate).is_some());
        assert!(
            lab.store.prepared_for_source(&skipped).is_none(),
            "not chosen, not prepared"
        );
        for (path, facts) in paths.iter().zip(before) {
            assert_eq!(snapshot(path), facts, "{}", path.display());
        }
        // A new review sees them as done; a batch with nothing to do is refused.
        let again = plan(&lab, &paths);
        assert_eq!(again.actionable().count(), 1);
        assert_eq!(
            again.count(|item| matches!(item, BulkItemPlan::AlreadyPrepared)),
            3
        );
        assert_eq!(
            start_queue(&lab.store, &again, &|path| path != skipped),
            Err(BulkError::NothingChosen)
        );
    }

    #[test]
    fn a_game_that_fails_is_reported_and_the_batch_goes_on() {
        let lab = Lab::new();
        let paths = discs(&lab, 3);
        let reviewed = plan(&lab, &paths);
        start_queue(&lab.store, &reviewed, &|_| true).unwrap();
        // The second ZIP is damaged after the review.
        let mut bytes = std::fs::read(&paths[1]).unwrap();
        let middle = bytes.len() / 3;
        bytes[middle] ^= 0x55;
        std::fs::write(&paths[1], bytes).unwrap();
        let queue = run(&lab).unwrap();
        assert_eq!(states(&queue), ["prepared", "failed", "prepared"]);
        assert!(matches!(&queue.items[1].state, QueueState::Failed(why) if !why.is_empty()));
        assert_eq!(lab.store.list().len(), 2);
        assert!(lab.store.interrupted().is_empty());
    }

    #[test]
    fn cancelling_keeps_finished_games_and_the_rest_can_be_continued() {
        let lab = Lab::new();
        let paths = discs(&lab, 3);
        start_queue(&lab.store, &plan(&lab, &paths), &|_| true).unwrap();
        let queue = run_queue(
            &lab.store,
            None,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|step| {
                // During the second game.
                if matches!(step, BulkProgress::Step { index: 1, .. }) {
                    lab.cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert_eq!(states(&queue), ["prepared", "pending", "pending"]);
        assert_eq!(queue.stopped.as_deref(), Some("Cancelled."));
        assert!(
            lab.store.interrupted().is_empty(),
            "the cancelled game left nothing"
        );
        assert_eq!(lab.store.list().len(), 1);
        lab.cancel.store(false, Ordering::Relaxed);
        // A new batch is refused while this one is unfinished.
        assert_eq!(
            start_queue(&lab.store, &plan(&lab, &paths), &|_| true),
            Err(BulkError::QueueInProgress(2))
        );
        lab.cancel.store(false, Ordering::Relaxed);
        let queue = run(&lab).unwrap();
        assert_eq!(states(&queue), ["prepared", "prepared", "prepared"]);
        assert_eq!(load_queue(&lab.store).unwrap(), queue);
        clear_queue(&lab.store).unwrap();
        assert!(load_queue(&lab.store).is_none());
        assert_eq!(
            lab.store.list().len(),
            3,
            "clearing the list keeps the copies"
        );
    }

    #[test]
    fn a_killed_batch_resumes_after_a_restart_including_a_half_extracted_game() {
        let lab = Lab::new();
        let paths = discs(&lab, 3);
        start_queue(&lab.store, &plan(&lab, &paths), &|_| true).unwrap();
        // Killed in the middle of extracting the second game.
        let first_set = std::cell::Cell::new(true);
        let crashed = with_hook(
            move |reached| {
                if reached == Point::AfterMember(1) {
                    if first_set.get() {
                        first_set.set(false);
                    } else {
                        panic!("simulated kill while extracting");
                    }
                }
            },
            || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&lab))),
        );
        assert!(crashed.is_err());
        let saved = load_queue(&lab.store).unwrap();
        assert_eq!(states(&saved), ["prepared", "pending", "pending"]);
        assert_eq!(
            lab.store.interrupted().len(),
            1,
            "partial files are kept apart"
        );
        assert_eq!(lab.store.list().len(), 1);

        // "Restart": a new run cleans the partial extraction and finishes.
        let queue = run(&lab).unwrap();
        assert_eq!(states(&queue), ["prepared", "prepared", "prepared"]);
        assert!(lab.store.interrupted().is_empty());
        assert_eq!(lab.store.list().len(), 3);
        for path in &paths {
            let summary = lab.store.prepared_for_source(path).unwrap();
            assert!(lab.store.verify_full(&summary.set_id, &lab.cancel).is_ok());
        }
        // Killed between two games instead.
        let lab = Lab::new();
        let paths = discs(&lab, 2);
        start_queue(&lab.store, &plan(&lab, &paths), &|_| true).unwrap();
        AFTER_ITEM.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|index| {
                if index == 0 {
                    panic!("simulated kill between games");
                }
            }));
        });
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&lab)));
        AFTER_ITEM.with(|hook| *hook.borrow_mut() = None);
        assert!(crashed.is_err());
        assert_eq!(
            states(&load_queue(&lab.store).unwrap()),
            ["prepared", "pending"]
        );
        assert_eq!(states(&run(&lab).unwrap()), ["prepared", "prepared"]);
    }

    #[test]
    fn too_little_free_space_stops_before_anything_is_extracted() {
        let lab = Lab::new();
        let paths = discs(&lab, 3);
        // The review itself reports it.
        FREE_SPACE_OVERRIDE.with(|free| free.set(Some(10 * 1024 * 1024)));
        let reviewed = plan(&lab, &paths);
        assert_eq!(reviewed.available_bytes, Some(10 * 1024 * 1024));
        assert!(!reviewed.enough_space_for(&|_| true));
        let refused = start_queue(&lab.store, &reviewed, &|_| true);
        assert!(
            matches!(refused, Err(BulkError::NotEnoughSpace { .. })),
            "{refused:?}"
        );
        assert!(load_queue(&lab.store).is_none());
        assert!(lab.store.list().is_empty());
        assert!(lab.store_files().is_empty());

        // Space runs out after the batch was accepted: it stops, keeps what
        // is done, and leaves the rest to do.
        FREE_SPACE_OVERRIDE.with(|free| free.set(Some(u64::MAX / 2)));
        start_queue(&lab.store, &plan(&lab, &paths), &|_| true).unwrap();
        AFTER_ITEM.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|_| {
                FREE_SPACE_OVERRIDE.with(|free| free.set(Some(1024)));
            }));
        });
        let queue = run(&lab);
        AFTER_ITEM.with(|hook| *hook.borrow_mut() = None);
        let queue = queue.unwrap();
        assert_eq!(states(&queue), ["prepared", "pending", "pending"]);
        assert!(
            queue.stopped.as_deref().unwrap().contains("free space"),
            "{:?}",
            queue.stopped
        );
        assert!(lab.store.interrupted().is_empty());
        FREE_SPACE_OVERRIDE.with(|free| free.set(None));
        assert_eq!(
            states(&run(&lab).unwrap()),
            ["prepared", "prepared", "prepared"]
        );
    }

    #[test]
    fn only_one_batch_runs_at_a_time_and_there_is_no_batch_without_a_start() {
        let lab = std::sync::Arc::new(Lab::new());
        assert_eq!(run(&lab), Err(BulkError::NoQueue));
        let paths = discs(&lab, 2);
        start_queue(&lab.store, &plan(&lab, &paths), &|_| true).unwrap();
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        let worker = {
            let lab = lab.clone();
            std::thread::spawn(move || {
                with_hook(
                    move |reached| {
                        if reached == Point::AfterIntent {
                            let _ = reached_tx.send(());
                            let _ = resume_rx.recv();
                        }
                    },
                    || run(&lab),
                )
            })
        };
        reached_rx.recv().unwrap();
        assert_eq!(run(&lab), Err(BulkError::Busy));
        assert_eq!(clear_queue(&lab.store), Err(BulkError::Busy));
        resume_tx.send(()).unwrap();
        drop(resume_tx);
        assert_eq!(
            states(&worker.join().unwrap().unwrap()),
            ["prepared", "prepared"]
        );
    }
}

// ---------------------------------------------------------------------------
// Test support for "which disk": READ THIS BEFORE TRUSTING THESE TESTS
// ---------------------------------------------------------------------------
//
// A test cannot format or mount a disk, so the tests below cannot hold a real
// ext4 filesystem with a UUID of its own. `disk_of` WORKS BACKWARDS: it reads
// the identity of the filesystem the test folder is on (or the one a test
// pretends it is on) and builds a UUID that has that identity. That is the
// very thing the application must never do, and it is done here only to
// reach the code behind the check.
//
// These tests therefore show that the refusals work (no UUID, wrong UUID,
// missing or replaced disk, unsupported filesystem, ambiguous identity,
// links, mount changes) and that nothing is written when one applies. They
// do NOT show that a real disk's real UUID is accepted: the link between an
// ext4 UUID and the kernel's identity is checked separately, against fixed
// values computed outside this code (`disk_identity_tests`) and with a real
// ext4 filesystem in the acceptance run (docs/GAME_RECOVERY_ENGINE.md).

/// A UUID with the given identity, built backwards (see above).
fn uuid_with_identity(id: u64) -> String {
    let hex: String = id
        .to_le_bytes()
        .iter()
        .chain([0_u8; 8].iter())
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Makes this test's folders count as ext4 with one connected disk per
/// identity, unless the test has said otherwise. Each test runs on its own
/// thread, so this never reaches another test.
fn assume_ext4() {
    use crate::recovery::disk_identity::{EXT4_OVERRIDE, SAME_IDENTITY_OVERRIDE};
    EXT4_OVERRIDE.with(|hook| {
        let mut hook = hook.borrow_mut();
        if hook.is_none() {
            *hook = Some(Box::new(|_| Some(true)));
        }
    });
    SAME_IDENTITY_OVERRIDE.with(|hook| {
        let mut hook = hook.borrow_mut();
        if hook.is_none() {
            *hook = Some(Box::new(|_| Some(Ok(1))));
        }
    });
}

/// The UUID of "the disk this folder is on", built backwards from the
/// filesystem the folder (or the nearest folder above it that exists) is on
/// now, real or pretended. Also applies [`assume_ext4`].
fn disk_of(path: &Path) -> String {
    assume_ext4();
    let mut probe = path;
    while std::fs::symlink_metadata(probe).is_err() {
        match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
            _ => return uuid_with_identity(0x0bad_0bad_0bad_0bad),
        }
    }
    uuid_with_identity(
        crate::recovery::prepared_media::filesystem_id(probe).unwrap_or(0x0bad_0bad_0bad_0bad),
    )
}

// ---------------------------------------------------------------------------
// The storage folder
// ---------------------------------------------------------------------------

mod storage_tests {
    use super::*;
    use crate::recovery::storage::*;

    #[test]
    fn a_folder_is_accepted_only_when_it_is_safe_to_keep_copies_in() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let library = root.join("games");
        std::fs::create_dir_all(library.join("amiga-cd32")).unwrap();
        let libraries = vec![library.clone()];
        let problems =
            |folder: &Path| check_folder(folder, &libraries, Some(&disk_of(folder))).problems;

        assert!(!problems(Path::new("relative/folder")).is_empty());
        assert!(!problems(&root.join("a/../b")).is_empty());
        let file = root.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(problems(&file)[0].contains("file"));
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(problems(&link)[0].contains("link"));
        assert!(
            problems(&library.join("amiga-cd32"))
                .iter()
                .any(|problem| problem.contains("inside your game folder"))
        );
        assert!(
            problems(&root)
                .iter()
                .any(|problem| problem.contains("is inside this folder"))
        );
        let shared = root.join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::os::unix::fs::PermissionsExt::from_mode(0o777))
            .unwrap();
        assert!(
            problems(&shared)
                .iter()
                .any(|problem| problem.contains("Other users"))
        );
        assert!(
            problems(&root.join("missing/parent/folder"))
                .iter()
                .any(|problem| problem.contains("does not exist"))
        );

        let good = check_folder(&real, &libraries, Some(&disk_of(&real)));
        assert!(good.usable(), "{:?}", good.problems);
        assert!(good.available_bytes.is_some());
        assert!(
            std::fs::read_dir(&real).unwrap().next().is_none(),
            "the probe is removed"
        );
        // A folder that does not exist yet is examined without creating it.
        let new = root.join("new-folder");
        assert!(check_folder(&new, &libraries, Some(&disk_of(&new))).usable());
        assert!(!new.exists());
    }

    #[test]
    fn the_choice_is_saved_only_after_the_check_and_an_unusable_setting_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let setting = root.join("config/prepared_media_folder.txt");
        assert_eq!(configured_folder_at(&setting), Ok(None));
        let library = root.join("games");
        std::fs::create_dir_all(&library).unwrap();

        // Refused: nothing is saved and nothing is created.
        assert!(
            set_folder_at(
                &setting,
                Some(&library.join("copies")),
                std::slice::from_ref(&library),
                Some(&disk_of(&library)),
            )
            .is_err()
        );
        assert!(!setting.exists());
        assert!(!library.join("copies").exists());

        let chosen = root.join("big-disk/emuwiz-prepared");
        std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
        let check = set_folder_at(
            &setting,
            Some(&chosen),
            std::slice::from_ref(&library),
            Some(&disk_of(&chosen)),
        )
        .unwrap()
        .unwrap();
        assert!(check.usable());
        assert_eq!(configured_folder_at(&setting), Ok(Some(chosen.clone())));
        assert_eq!(
            std::fs::metadata(&chosen).unwrap().mode() & 0o777,
            0o700,
            "a folder EmuWiz creates is private"
        );
        // A store there works end to end.
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let store = PreparedMediaStore::at(&chosen);
        let summary = store
            .prepare(
                &zip,
                "AmigaCD32",
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel,
                &|_| {},
            )
            .unwrap()
            .summary;
        assert!(summary.launch_path.starts_with(&chosen));

        // A damaged setting is never silently replaced by the default.
        std::fs::write(&setting, "relative/path\n").unwrap();
        assert!(configured_folder_at(&setting).is_err());
        std::fs::write(&setting, "/a\n/b\n").unwrap();
        assert!(configured_folder_at(&setting).is_err());
        assert_eq!(set_folder_at(&setting, None, &[], None), Ok(None));
        assert_eq!(configured_folder_at(&setting), Ok(None));
        assert!(chosen.exists(), "clearing the choice deletes nothing");
    }
}

// ---------------------------------------------------------------------------
// Regressions from the second independent review (2026-11-20, f6476af4)
// ---------------------------------------------------------------------------

mod second_review_tests {
    use super::*;
    use crate::launch::remembered_launch_system as remembered;
    use crate::recovery::bulk::*;
    use crate::recovery::moves;
    use crate::recovery::storage::*;

    /// A disk that is not there right now is not a file that is gone.
    #[test]
    fn an_unavailable_folder_retires_no_link_and_moves_no_remembered_choice() {
        let lab = Lab::new();
        let removable = lab.root.join("removable/amiga-cd32");
        std::fs::create_dir_all(&removable).unwrap();
        let original = removable.join("Game.zip");
        write_zip(&original, &standard_members());
        let summary = lab.prepare(&original).unwrap().summary;
        let remembered_file = lab.root.join("config/remembered_launch_systems.json");
        remembered::remember_launch_system_at(
            &remembered_file,
            &remembered::RememberedLaunchSystem::for_current_file(&original, "AmigaCD32").unwrap(),
        )
        .unwrap();
        // A duplicate elsewhere with the same size and time (`cp -p`).
        let duplicate = lab.zip_path("Game.zip");
        std::fs::copy(&original, &duplicate).unwrap();
        let metadata = std::fs::metadata(&original).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&duplicate)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(metadata.accessed().unwrap())
                    .set_modified(metadata.modified().unwrap()),
            )
            .unwrap();
        // The "disk" goes away: its whole folder is unavailable.
        let parked = lab.root.join("unmounted");
        std::fs::rename(lab.root.join("removable"), &parked).unwrap();

        let outcome = moves::rediscover(
            &lab.store,
            Some(&remembered_file),
            &duplicate,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        )
        .unwrap();
        assert_eq!(
            outcome,
            RelinkOutcome::Linked {
                summary: Box::new(summary.clone()),
                moved_from: None,
            },
            "nothing proves the original was moved"
        );
        assert_eq!(
            remembered::remembered_launch_system_at(&remembered_file, &duplicate),
            None
        );
        // The "disk" comes back: the original has its copy and its choice.
        std::fs::rename(&parked, lab.root.join("removable")).unwrap();
        assert_eq!(lab.store.prepared_for_source(&original), Some(summary));
        assert_eq!(
            remembered::remembered_launch_system_at(&remembered_file, &original),
            Some("AmigaCD32")
        );
    }

    #[test]
    fn a_second_name_for_the_same_file_does_not_carry_the_link_while_the_first_exists() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let summary = lab.prepare(&zip).unwrap().summary;
        let second = lab.zip_path("hard-link.zip");
        let pending = moves::before_in(&lab.store, [(zip.as_path(), second.as_path())]);
        // Not a rename: a hard link, so the old name is still there.
        std::fs::hard_link(&zip, &second).unwrap();
        assert_eq!(moves::after_in(&lab.store, None, pending, |_, _| true), 0);
        assert!(lab.store.prepared_for_source(&second).is_none());
        // (A hard link changes the file's change time, so the first name
        // needs the content check too; it passes, being the same bytes.)
        assert!(matches!(
            lab.store
                .relink(
                    &zip,
                    &lab.trusted,
                    &DiscSetLimits::default(),
                    &lab.cancel,
                    &|_| {}
                )
                .unwrap(),
            RelinkOutcome::Linked { .. }
        ));
        assert_eq!(lab.store.prepared_for_source(&zip), Some(summary));
    }

    #[test]
    fn the_launch_gate_covers_every_store_given_to_it() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let old = PreparedMediaStore::at(lab.root.join("old-store"));
        let summary = old
            .prepare(
                &zip,
                "AmigaCD32",
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel,
                &|_| {},
            )
            .unwrap()
            .summary;
        let current = PreparedMediaStore::at(lab.root.join("new-store"));
        let track = summary.directory.join("media").join(ISO_NAME);
        // Only the current store: a track in the old one slips through.
        assert_eq!(
            launch_refusal_in(std::slice::from_ref(&current), &track),
            None
        );
        // Every known store: refused; the old store's cue sheet still starts.
        let known = [current, old];
        assert!(launch_refusal_in(&known, &track).is_some());
        assert_eq!(launch_refusal_in(&known, &summary.launch_path), None);
        assert_eq!(launch_refusal_in(&known, &zip), None);
    }

    #[test]
    fn a_saved_folder_is_rechecked_each_time_and_earlier_folders_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let setting = root.join("config/prepared_media_folder.txt");
        let first = root.join("first");
        let second = root.join("second");
        set_folder_at(&setting, Some(&first), &[], Some(&disk_of(&first))).unwrap();
        assert_eq!(still_safe(&first), Ok(()));
        assert!(
            previous_folders_at(&setting.with_file_name("prepared_media_previous_folders.txt"))
                .is_empty()
        );
        set_folder_at(&setting, Some(&second), &[], Some(&disk_of(&second))).unwrap();
        set_folder_at(&setting, None, &[], None).unwrap();
        assert_eq!(
            previous_folders_at(&setting.with_file_name("prepared_media_previous_folders.txt")),
            vec![second.clone(), first.clone()],
            "folders used earlier stay known, newest first"
        );
        assert!(first.is_dir() && second.is_dir(), "nothing is deleted");

        // Later made writable by others, or swapped for a link: refused at use.
        std::fs::set_permissions(&first, std::os::unix::fs::PermissionsExt::from_mode(0o777))
            .unwrap();
        assert!(still_safe(&first).is_err());
        std::fs::remove_dir(&second).unwrap();
        std::os::unix::fs::symlink(&first, &second).unwrap();
        assert!(still_safe(&second).is_err());
        assert_eq!(still_safe(&root.join("not-created-yet")), Ok(()));
        assert!(still_safe(Path::new("relative")).is_err());
    }

    #[test]
    fn a_reuse_whose_copy_is_not_in_the_store_counts_towards_the_space_needed() {
        let lab = Lab::new();
        let first = lab.zip_path("first.zip");
        write_zip(&first, &standard_members());
        let twin = lab.zip_path("twin.zip");
        std::fs::copy(&first, &twin).unwrap();
        let candidates: Vec<BulkCandidate> = [&first, &twin]
            .iter()
            .map(|path| BulkCandidate {
                path: path.to_path_buf(),
                platform_id: "AmigaCD32".into(),
                title: "disc".into(),
            })
            .collect();
        let plan = plan_bulk(
            &lab.store,
            &candidates,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        assert!(matches!(plan.items[0].plan, BulkItemPlan::Extract { .. }));
        assert!(matches!(
            plan.items[1].plan,
            BulkItemPlan::Reuse {
                in_store: false,
                ..
            }
        ));
        let both = plan.required_for(&|_| true);
        let only_twin = plan.required_for(&|path| path == twin);
        let only_first = plan.required_for(&|path| path == first);
        assert!(only_twin > 0, "the twin alone has to be extracted");
        assert_eq!(only_twin, only_first);
        assert_eq!(both, only_first, "together the disc is counted once");

        // Starting just the twin with too little space is refused, and
        // nothing is created.
        crate::recovery::prepared_media::FREE_SPACE_OVERRIDE.with(|free| free.set(Some(1024)));
        let refused = start_queue(&lab.store, &plan, &|path| path == twin);
        crate::recovery::prepared_media::FREE_SPACE_OVERRIDE.with(|free| free.set(None));
        assert!(
            matches!(refused, Err(BulkError::NotEnoughSpace { .. })),
            "{refused:?}"
        );
        assert!(!lab.store.root().exists());

        // A copy that was in the store at the review but is gone at the
        // start counts again.
        let summary = lab.prepare(&first).unwrap().summary;
        let reviewed = plan_bulk(
            &lab.store,
            &candidates[1..],
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        assert!(matches!(
            reviewed.items[0].plan,
            BulkItemPlan::Reuse { in_store: true, .. }
        ));
        assert_eq!(reviewed.required_for(&|_| true), 0);
        lab.store.remove(&summary.set_id).unwrap();
        assert!(
            reviewed.required_with(&|_| true, &|fingerprint| {
                lab.store.lookup(fingerprint).is_some()
            }) > 0
        );
    }

    #[test]
    fn a_queue_entry_the_engine_has_no_preparation_for_is_never_run() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let candidate = BulkCandidate {
            path: zip.clone(),
            platform_id: "AmigaCD32".into(),
            title: "disc".into(),
        };
        let plan = plan_bulk(
            &lab.store,
            std::slice::from_ref(&candidate),
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        start_queue(&lab.store, &plan, &|_| true).unwrap();
        // The queue file is edited by hand to name another system.
        let queue_file = lab.store.root().join("v1/queue.json");
        let text = std::fs::read_to_string(&queue_file).unwrap();
        std::fs::write(&queue_file, text.replace("AmigaCD32", "Xbox")).unwrap();
        let queue = run_queue(
            &lab.store,
            None,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        )
        .unwrap();
        assert!(matches!(queue.items[0].state, QueueState::Failed(_)));
        assert!(lab.store.list().is_empty());
    }

    #[test]
    fn a_record_without_a_filesystem_id_or_with_another_one_is_never_retired() {
        for tamper in ["strip", "other"] {
            let lab = Lab::new();
            let zip = lab.standard_zip();
            let summary = lab.prepare(&zip).unwrap().summary;
            let record = std::fs::read_dir(lab.store.root().join("v1/sources"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let mut link: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
            if tamper == "strip" {
                link.as_object_mut().unwrap().remove("filesystem_id");
            } else {
                link["filesystem_id"] = serde_json::json!(0x1234_5678_9abc_u64);
            }
            std::fs::write(&record, serde_json::to_string(&link).unwrap()).unwrap();
            assert_eq!(
                lab.store.prepared_for_source(&zip),
                Some(summary.clone()),
                "{tamper}"
            );

            // The file disappears from a folder that is still there; a twin
            // is then linked by content.
            let twin = lab.zip_path("twin.zip");
            std::fs::copy(&zip, &twin).unwrap();
            std::fs::remove_file(&zip).unwrap();
            let outcome = lab
                .store
                .relink(
                    &twin,
                    &lab.trusted,
                    &DiscSetLimits::default(),
                    &lab.cancel,
                    &|_| {},
                )
                .unwrap();
            assert_eq!(
                outcome,
                RelinkOutcome::Linked {
                    summary: Box::new(summary),
                    moved_from: None,
                },
                "{tamper}: nothing proves where the first file went"
            );
            assert!(record.exists(), "{tamper}: the record is kept");
        }
    }

    #[test]
    fn the_saved_folder_is_checked_when_used_and_a_missing_one_is_never_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(PreparedMediaStore::configured(None), Ok(None));
        let chosen = root.join("chosen");
        // The filesystem the folder is on, as recorded when it is chosen.
        let on_disk = filesystem_id(&root);
        assert!(on_disk.is_some(), "the test folder's filesystem has no id");
        std::fs::create_dir(&chosen).unwrap();
        std::fs::set_permissions(&chosen, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        assert_eq!(
            PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk),
            Ok(Some(PreparedMediaStore::at(&chosen)))
        );
        // Made writable by others afterwards.
        std::fs::set_permissions(&chosen, std::os::unix::fs::PermissionsExt::from_mode(0o777))
            .unwrap();
        assert!(PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk).is_err());
        // Gone (its disk is not connected): an error, and nothing is created.
        std::fs::remove_dir(&chosen).unwrap();
        let missing = PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk);
        assert!(missing.is_err_and(|detail| detail.contains("not there")));
        assert!(!chosen.exists());

        // A store handed out while the folder was there (a batch in
        // progress) does not recreate it either once it is gone.
        std::fs::create_dir(&chosen).unwrap();
        std::fs::set_permissions(&chosen, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk)
            .unwrap()
            .unwrap();
        std::fs::remove_dir(&chosen).unwrap();
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let result = store.prepare(
            &zip,
            "AmigaCD32",
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        );
        assert!(matches!(result, Err(PrepareError::Store(detail)) if detail.contains("not there")));
        assert!(!chosen.exists(), "the chosen folder was recreated");

        // The same through a batch: a queue saved in the chosen folder, the
        // folder then gone. The run does not happen and nothing is created.
        std::fs::create_dir(&chosen).unwrap();
        std::fs::set_permissions(&chosen, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk)
            .unwrap()
            .unwrap();
        let candidate = BulkCandidate {
            path: zip.clone(),
            platform_id: "AmigaCD32".into(),
            title: "disc".into(),
        };
        let plan = plan_bulk(
            &store,
            std::slice::from_ref(&candidate),
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        start_queue(&store, &plan, &|_| true).unwrap();
        // Handles taken while the folder was there keep the rule, whichever
        // way they were obtained.
        let resolved = store.resolved().unwrap();
        let made_ready = store.ready().unwrap();
        std::fs::remove_dir_all(&chosen).unwrap();
        assert!(resolved.ready().is_err());
        assert!(made_ready.ready().is_err());
        assert!(!chosen.exists(), "a ready handle recreated the folder");
        let run = run_queue(
            &store,
            None,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        );
        assert!(run.is_err(), "{run:?}");
        assert!(
            !chosen.exists(),
            "the chosen folder was recreated by a batch"
        );

        // The folder goes while a batch is running, between two games. The
        // next game is not prepared, the run stops with an error, and neither
        // the preparation nor the saved queue brings the folder back.
        std::fs::create_dir(&chosen).unwrap();
        std::fs::set_permissions(&chosen, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), on_disk)
            .unwrap()
            .unwrap();
        let candidates: Vec<BulkCandidate> = (0..2)
            .map(|index| {
                let path = lab.zip_path(&format!("mid-run-{index}.zip"));
                let mut members = standard_members();
                members[0].1 = iso_bytes("CDTV", &format!("MID{index}"));
                write_zip(&path, &members);
                BulkCandidate {
                    path,
                    platform_id: "AmigaCD32".into(),
                    title: format!("mid-run {index}"),
                }
            })
            .collect();
        let plan = plan_bulk(
            &store,
            &candidates,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        start_queue(&store, &plan, &|_| true).unwrap();
        let unplugged = chosen.clone();
        AFTER_ITEM.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |index| {
                if index == 0 {
                    std::fs::remove_dir_all(&unplugged).unwrap();
                }
            }));
        });
        let run = run_queue(
            &store,
            None,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        );
        AFTER_ITEM.with(|hook| *hook.borrow_mut() = None);
        assert!(
            matches!(&run, Err(BulkError::Store(detail)) if detail.contains("not there")),
            "{run:?}"
        );
        assert!(
            !chosen.exists(),
            "the chosen folder was recreated in the middle of a batch"
        );
    }

    #[test]
    fn the_launch_gate_does_not_open_for_folders_it_can_no_longer_trust() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let old_root = lab.root.join("old-store");
        let old = PreparedMediaStore::at(&old_root);
        let summary = old
            .prepare(
                &zip,
                "AmigaCD32",
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel,
                &|_| {},
            )
            .unwrap()
            .summary;
        let track = summary.directory.join("media").join(ISO_NAME);
        let previous = vec![old_root.clone()];
        let stores = vec![PreparedMediaStore::at(lab.root.join("current")), old];
        // Trusted earlier folder: a track is refused, the cue sheet is not.
        assert!(launch_refusal_with(None, None, &previous, &stores, &track).is_some());
        assert_eq!(
            launch_refusal_with(None, None, &previous, &stores, &summary.launch_path),
            None
        );
        // The earlier folder stops being trustworthy and so drops out of the
        // list of stores: files inside it are still refused, all of them.
        std::fs::set_permissions(
            &old_root,
            std::os::unix::fs::PermissionsExt::from_mode(0o777),
        )
        .unwrap();
        let without_old = &stores[..1];
        assert!(launch_refusal_with(None, None, &previous, without_old, &track).is_some());
        assert!(
            launch_refusal_with(None, None, &previous, without_old, &summary.launch_path).is_some()
        );
        assert_eq!(
            launch_refusal_with(None, None, &previous, without_old, &zip),
            None
        );
        std::fs::set_permissions(
            &old_root,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();

        // The same distrusted folder reached through a link in its path, and
        // through `..`: still refused.
        std::fs::set_permissions(
            &old_root,
            std::os::unix::fs::PermissionsExt::from_mode(0o777),
        )
        .unwrap();
        let alias = lab.root.join("alias-to-root");
        std::os::unix::fs::symlink(&lab.root, &alias).unwrap();
        let through_link = alias
            .join("old-store")
            .join(track.strip_prefix(&old_root).unwrap());
        assert!(launch_refusal_with(None, None, &previous, without_old, &through_link).is_some());
        // A path that leaves the folder's own spelling and comes back in.
        std::fs::create_dir_all(lab.root.join("elsewhere")).unwrap();
        let dotted = lab
            .root
            .join("elsewhere")
            .join("..")
            .join("old-store")
            .join(track.strip_prefix(&old_root).unwrap());
        assert!(!dotted.starts_with(&old_root));
        assert!(launch_refusal_with(None, None, &previous, without_old, &dotted).is_some());
        // And a previous folder that was saved under a path with a link in it.
        let saved_with_link = vec![alias.join("old-store")];
        assert!(launch_refusal_with(None, None, &saved_with_link, without_old, &track).is_some());
        std::fs::set_permissions(
            &old_root,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();

        // The saved choice cannot be used: files inside it are refused, even
        // with no store left to check them against.
        let refused = launch_refusal_with(
            Some("its disk is not connected".into()),
            Some(old_root.clone()),
            &[],
            &[],
            &summary.launch_path,
        );
        assert!(refused.is_some_and(|detail| detail.contains("can no longer be used")));
        assert_eq!(
            launch_refusal_with(Some("x".into()), Some(old_root), &[], &[], &zip),
            None
        );
    }

    #[test]
    fn the_unchecked_setting_reader_is_bounded_and_never_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let setting = dir.path().join("prepared_media_folder.txt");
        assert_eq!(configured_folder_unchecked_at(&setting), None);
        std::fs::write(&setting, "/chosen/folder\n").unwrap();
        assert_eq!(
            configured_folder_unchecked_at(&setting),
            Some(PathBuf::from("/chosen/folder"))
        );
        std::fs::write(&setting, "relative\n").unwrap();
        assert_eq!(configured_folder_unchecked_at(&setting), None);
        // Far larger than any setting: only the bounded start is read.
        let mut huge = b"/".to_vec();
        huge.resize(4 * 1024 * 1024, b'x');
        std::fs::write(&setting, huge).unwrap();
        let read = configured_folder_unchecked_at(&setting).expect("an absolute path");
        assert!(
            read.as_os_str().len() <= 8 * 1024,
            "{} bytes were read from the setting",
            read.as_os_str().len()
        );
        // A FIFO where the setting should be: answered at once, not waited on.
        std::fs::remove_file(&setting).unwrap();
        let name = std::ffi::CString::new(setting.to_str().unwrap()).unwrap();
        // SAFETY: `name` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(configured_folder_unchecked_at(&setting), None);
    }
}

/// A restart can give the same disk another device number. The link is to
/// the file, not to that number.
#[test]
fn a_prepared_copy_is_still_found_when_the_disk_gets_another_device_number() {
    let lab = Lab::new();
    let zip = lab.standard_zip();
    let summary = lab.prepare(&zip).unwrap().summary;
    // Rewrite the saved record as if it had been made when the disk had
    // another device number.
    let sources = lab.store.root().join("v1/sources");
    let record = std::fs::read_dir(&sources)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut link: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    let device = link["source"]["device"].as_u64().unwrap();
    link["source"]["device"] = serde_json::json!(device + 16);
    std::fs::write(&record, serde_json::to_string(&link).unwrap()).unwrap();

    assert_eq!(lab.store.prepared_for_source(&zip), Some(summary.clone()));
    assert_eq!(
        lab.diagnose(&zip).verdict,
        RecoveryVerdict::PlayableFromPreparedCopy
    );
    // An EmuWiz rename still carries it (the comparison there is between the
    // start and the end of the one operation).
    let renamed = lab.zip_path("renamed-after-restart.zip");
    let pending =
        crate::recovery::moves::before_in(&lab.store, [(zip.as_path(), renamed.as_path())]);
    std::fs::rename(&zip, &renamed).unwrap();
    assert_eq!(
        crate::recovery::moves::after_in(&lab.store, None, pending, |_, _| true),
        1
    );
    assert_eq!(lab.store.prepared_for_source(&renamed), Some(summary));
    // Anything else about the file changing still breaks the link.
    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(&renamed)
        .unwrap();
    file.set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(4_000_000),
    )
    .unwrap();
    drop(file);
    assert!(lab.store.prepared_for_source(&renamed).is_none());
}

/// The folder for prepared copies must stay on the filesystem it is expected
/// on. A test cannot mount or unmount a disk, so "the disk went away" is
/// either a record that names another filesystem than the one the folder is
/// really on (exactly what an unmounted disk's empty mount point looks like),
/// or, for a change in the middle of an operation, `FILESYSTEM_OVERRIDE`.
mod mount_identity_tests {
    use super::*;
    use crate::recovery::bulk::*;
    use crate::recovery::prepared_media::{
        FILESYSTEM_OVERRIDE, Filesystem, MOUNT_ROOT_OVERRIDE, filesystem_id, is_mount_root,
        launch_refusal_judged, mount_points_in,
    };
    use crate::recovery::storage::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Clears the pretended filesystem when the test ends, however it ends.
    struct Pretend;

    impl Drop for Pretend {
        fn drop(&mut self) {
            FILESYSTEM_OVERRIDE.with(|hook| *hook.borrow_mut() = None);
        }
    }

    fn real_filesystem(path: &Path) -> Filesystem {
        Filesystem {
            device: std::fs::metadata(path).unwrap().dev(),
            id: filesystem_id(path),
        }
    }

    fn another(real: Filesystem) -> Filesystem {
        Filesystem {
            device: real.device.wrapping_add(7),
            id: Some(real.id.unwrap() ^ 0x5a5a_5a5a),
        }
    }

    /// From now on (while `active` is set) every folder at or below `below`
    /// reports `pretended`: the disk that was mounted there is gone and what
    /// is at that path now is on another filesystem.
    fn pretend_below(below: &Path, pretended: Filesystem, active: Rc<Cell<bool>>) -> Pretend {
        let below = below.to_path_buf();
        FILESYSTEM_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| {
                (active.get() && path.starts_with(&below)).then_some(pretended)
            }));
        });
        Pretend
    }

    fn everything_under(folder: &Path) -> Vec<PathBuf> {
        fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                found.push(path.clone());
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(&path, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(folder, &mut found);
        // Everything: folders, the journal and the lock included.
        found.sort();
        found
    }

    fn prepare_in(
        store: &PreparedMediaStore,
        lab: &Lab,
        zip: &Path,
    ) -> Result<PrepareOutcome, PrepareError> {
        store.prepare(
            zip,
            "AmigaCD32",
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
            &|_| {},
        )
    }

    fn disc(lab: &Lab, label: &str) -> PathBuf {
        let path = lab.zip_path(&format!("{label}.zip"));
        let mut members = standard_members();
        members[0].1 = iso_bytes("CDTV", label);
        write_zip(&path, &members);
        path
    }

    fn private_folder(path: &Path) {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
    }

    /// Choosing a folder records the disk the person named for it, and a
    /// filesystem without an identity is refused in the application itself.
    #[test]
    fn choosing_a_folder_records_the_named_disk_and_refuses_one_without_identity() {
        let lab = Lab::new();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let chosen = lab.root.join("chosen");
        set_folder_at(&setting, Some(&chosen), &[], Some(&disk_of(&chosen))).unwrap();
        let real = filesystem_id(&chosen);
        assert!(real.is_some());
        assert_eq!(recorded_filesystem_at(&setting, &chosen), Ok(real));
        assert_eq!(
            recorded_filesystem_at(&setting, &lab.root.join("never-chosen")),
            Ok(None)
        );

        // The record holds the UUID the person named, not something read
        // from the folder.
        let record = std::fs::read_to_string(
            setting.with_file_name("prepared_media_folder_filesystems.json"),
        )
        .unwrap();
        assert!(record.contains("\"format_version\": 2"), "{record}");
        assert!(record.contains(&disk_of(&chosen)), "{record}");

        // A filesystem that reports no id (FUSE, network) cannot be the disk
        // of any UUID: refused, nothing saved, nothing created.
        let nameless = lab.root.join("on-a-network-share");
        private_folder(&nameless);
        let named = disk_of(&nameless);
        let active = Rc::new(Cell::new(true));
        let _pretend = pretend_below(
            &nameless,
            Filesystem {
                device: 99,
                id: None,
            },
            active.clone(),
        );
        for folder in [nameless.clone(), nameless.join("inside")] {
            let check = check_folder(&folder, &[], Some(&named));
            assert!(
                !check.usable() && check.filesystem_uuid.is_none(),
                "{check:?}"
            );
            let refused = set_folder_at(&setting, Some(&folder), &[], Some(&named));
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|detail| detail.contains("is not on the disk with filesystem UUID")),
                "{refused:?}"
            );
            assert_eq!(recorded_filesystem_at(&setting, &folder), Ok(None));
        }
        assert!(everything_under(&nameless).is_empty());
        assert_eq!(configured_folder_at(&setting), Ok(Some(chosen.clone())));

        // A record that is not EmuWiz's is an error, never "no record".
        std::fs::write(
            setting.with_file_name("prepared_media_folder_filesystems.json"),
            b"{ not json",
        )
        .unwrap();
        assert!(recorded_filesystem_at(&setting, &chosen).is_err());
        // Choosing again writes a good record.
        set_folder_at(&setting, Some(&chosen), &[], Some(&disk_of(&chosen))).unwrap();
        assert_eq!(recorded_filesystem_at(&setting, &chosen), Ok(real));
    }

    /// An unmounted disk leaves its mount point behind as an empty folder on
    /// the disk underneath. The folder exists; nothing may be written to it.
    #[test]
    fn an_unavailable_mount_is_refused_and_nothing_is_created_in_its_place() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let mount_point = lab.root.join("mnt-external");
        private_folder(&mount_point);
        let underneath = filesystem_id(&mount_point).unwrap();
        // The id of the disk that was mounted here when the folder was
        // chosen. It is not what is at this path now.
        let disk = underneath ^ 0x1234_5678;

        let refused =
            PreparedMediaStore::configured_recorded(Some(mount_point.clone()), Some(disk));
        assert!(
            refused
                .as_ref()
                .is_err_and(|detail| detail.contains("not on the disk it was on")),
            "{refused:?}"
        );
        // No record at all is a refusal too, never "any disk will do".
        let unrecorded = PreparedMediaStore::configured_recorded(Some(mount_point.clone()), None);
        assert!(
            unrecorded
                .as_ref()
                .is_err_and(|detail| detail.contains("no record")),
            "{unrecorded:?}"
        );
        assert!(everything_under(&mount_point).is_empty());

        // A handle obtained while the disk was mounted (a page left open, a
        // batch about to start) refuses every write once it is gone.
        let active = Rc::new(Cell::new(true));
        let _pretend = pretend_below(
            &mount_point,
            Filesystem {
                device: 4242,
                id: Some(disk),
            },
            active.clone(),
        );
        let store = PreparedMediaStore::configured_recorded(Some(mount_point.clone()), Some(disk))
            .unwrap()
            .unwrap();
        active.set(false); // unmounted: the path is the empty folder underneath
        for attempt in [
            prepare_in(&store, &lab, &zip).map(|_| ()),
            store.ready().map(|_| ()).map_err(PrepareError::Store),
        ] {
            assert!(
                matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not on the disk it was on")),
                "{attempt:?}"
            );
        }
        assert!(store.resolved().is_none());
        let plan = plan_bulk(
            &store,
            &[BulkCandidate {
                path: zip.clone(),
                platform_id: "AmigaCD32".into(),
                title: "disc".into(),
            }],
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        let started = start_queue(&store, &plan, &|_| true);
        assert!(matches!(&started, Err(BulkError::Store(_))), "{started:?}");
        assert!(
            everything_under(&mount_point).is_empty(),
            "something was written to the folder under the missing disk: {:?}",
            everything_under(&mount_point)
        );
    }

    /// The folder is there and holds a store, but on another filesystem than
    /// the one it was chosen on (another disk mounted in its place, or copies
    /// an earlier build left on the disk underneath). Nothing is prepared,
    /// reused, linked or removed there, and nothing in it may be started.
    #[test]
    fn a_changed_filesystem_is_refused_for_writing_removing_and_launching() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let chosen = lab.root.join("chosen");
        private_folder(&chosen);
        let real = real_filesystem(&chosen);
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), real.id)
            .unwrap()
            .unwrap();
        let summary = prepare_in(&store, &lab, &zip).unwrap().summary;
        assert!(matches!(
            store.check_launch_path(&summary.launch_path),
            LaunchPathCheck::Verified(_)
        ));
        assert_eq!(store.prepared_for_source(&zip), Some(summary.clone()));
        let before = everything_under(&chosen);

        let active = Rc::new(Cell::new(true));
        let _pretend = pretend_below(&chosen, another(real), active.clone());
        // Looked up again, as every new operation does.
        let refused = PreparedMediaStore::configured_recorded(Some(chosen.clone()), real.id);
        assert!(refused.is_err(), "{refused:?}");
        // The handle from before the change.
        let other = disc(&lab, "OTHER");
        let attempt = prepare_in(&store, &lab, &other);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not on the disk it was on")),
            "{attempt:?}"
        );
        // Not even the copy that is already there is handed out or linked.
        let again = prepare_in(&store, &lab, &zip);
        assert!(matches!(&again, Err(PrepareError::Store(_))), "{again:?}");
        assert_eq!(store.prepared_for_source(&zip), None);
        assert!(store.resolved().is_none());
        assert_eq!(store.list(), Vec::new());
        assert_eq!(store.remove(&summary.set_id), Err(RemoveError::NotFound));
        assert_eq!(
            store.relink(
                &zip,
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel,
                &|_| {}
            ),
            Ok(RelinkOutcome::NoMatchingCopy)
        );
        assert!(matches!(
            store.check_launch_path(&summary.launch_path),
            LaunchPathCheck::Refused(detail) if detail.contains("not on the disk it was chosen on")
        ));
        assert_eq!(everything_under(&chosen), before, "the folder was changed");

        // A filesystem whose identity cannot be read is refused as well.
        drop(_pretend);
        let _pretend = pretend_below(
            &chosen,
            Filesystem {
                device: real.device,
                id: None,
            },
            active.clone(),
        );
        let unverifiable = prepare_in(&store, &lab, &other);
        assert!(
            matches!(&unverifiable, Err(PrepareError::Store(_))),
            "{unverifiable:?}"
        );
        assert_eq!(everything_under(&chosen), before);

        // The disk is back: everything works again, nothing was lost.
        active.set(false);
        assert_eq!(store.prepared_for_source(&zip), Some(summary));
        prepare_in(&store, &lab, &other).unwrap();
    }

    /// A store that was not chosen (the default folder, an explicit one) has
    /// no recorded filesystem, but a handle made ready is still tied to the
    /// folder and filesystem it started with.
    #[test]
    fn a_ready_store_is_tied_to_the_folder_and_filesystem_it_started_with() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let ready = lab.store.ready().unwrap();
        let root = ready.root().to_path_buf();
        let real = real_filesystem(&root);

        // Another filesystem at the same path.
        let active = Rc::new(Cell::new(true));
        let pretend = pretend_below(&root, another(real), active.clone());
        let attempt = prepare_in(&ready, &lab, &zip);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("no longer the folder on the disk")),
            "{attempt:?}"
        );
        assert!(ready.ready().is_err());
        assert!(ready.resolved().is_none());
        drop(pretend);

        // Another folder at the same path (really: no pretending). The
        // original is moved aside and an empty folder takes its place.
        let aside = lab.root.join("data/moved-aside");
        std::fs::rename(&root, &aside).unwrap();
        private_folder(&root);
        let attempt = prepare_in(&ready, &lab, &zip);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("no longer the folder on the disk")),
            "{attempt:?}"
        );
        assert!(
            everything_under(&root).is_empty(),
            "the replacement folder was written to"
        );
        // And when the folder is gone altogether it is not created again.
        std::fs::remove_dir(&root).unwrap();
        let attempt = prepare_in(&ready, &lab, &zip);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not there")),
            "{attempt:?}"
        );
        assert!(!root.exists());
    }

    /// The disk goes at each write boundary inside one preparation. The
    /// preparation stops with a store error and no copy is published.
    #[test]
    fn a_preparation_stops_at_the_next_write_boundary_when_the_disk_goes() {
        for boundary in ["after intent", "after first file", "before publish"] {
            let lab = Lab::new();
            let zip = lab.standard_zip();
            let ready = lab.store.ready().unwrap();
            let root = ready.root().to_path_buf();
            let real = real_filesystem(&root);
            let active = Rc::new(Cell::new(false));
            let _pretend = pretend_below(&root, another(real), active.clone());
            let trigger = active.clone();
            let attempt = with_hook(
                move |point| {
                    let now = match point {
                        Point::AfterIntent => boundary == "after intent",
                        Point::AfterMember(0) => boundary == "after first file",
                        Point::BeforePublish => boundary == "before publish",
                        _ => false,
                    };
                    if now {
                        trigger.set(true);
                    }
                },
                || prepare_in(&ready, &lab, &zip),
            );
            assert!(
                matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("no longer the folder on the disk")),
                "{boundary}: {attempt:?}"
            );
            active.set(false);
            assert!(
                everything_under(&root.join("v1/sets")).is_empty(),
                "{boundary}: a copy was published"
            );
            assert_eq!(lab.store.prepared_for_source(&zip), None, "{boundary}");
            // With the disk back the same game prepares normally.
            let _ = lab.store.clean_interrupted();
            prepare_in(&lab.store, &lab, &zip).unwrap();
        }
    }

    /// The path of the store's staging folder comes to lead to another disk
    /// between the check of the root and the first write. The new work folder
    /// is never used there.
    #[test]
    fn a_work_folder_on_another_disk_is_never_written_to() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let ready = lab.store.ready().unwrap();
        let root = ready.root().to_path_buf();
        let real = real_filesystem(&root);
        let elsewhere = another(real);
        let staging = root.join("v1/staging");
        let inside = staging.clone();
        FILESYSTEM_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| {
                (path != inside && path.starts_with(&inside)).then_some(elsewhere)
            }));
        });
        let _pretend = Pretend;
        // Refused before the first byte: the preparation never gets as far
        // as writing its intent record into that folder.
        let wrote = Rc::new(Cell::new(false));
        let seen = wrote.clone();
        let attempt = with_hook(move |_| seen.set(true), || prepare_in(&ready, &lab, &zip));
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not on the disk the prepared-media folder is on")),
            "{attempt:?}"
        );
        assert!(!wrote.get(), "something was written into the work folder");
        assert!(
            everything_under(&staging).is_empty(),
            "a work folder was left behind: {:?}",
            everything_under(&staging)
        );
        assert!(everything_under(&root.join("v1/sets")).is_empty());
    }

    /// A batch loses its destination between two games. The finished game
    /// stays finished, the next one is not prepared, the run stops with an
    /// error, nothing is written, and the batch continues once the disk is
    /// back.
    #[test]
    fn a_batch_interrupted_by_the_loss_of_its_destination_writes_nothing_more() {
        let lab = Lab::new();
        let chosen = lab.root.join("chosen");
        private_folder(&chosen);
        let real = real_filesystem(&chosen);
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), real.id)
            .unwrap()
            .unwrap();
        let candidates: Vec<BulkCandidate> = (0..3)
            .map(|index| BulkCandidate {
                path: disc(&lab, &format!("BATCH{index}")),
                platform_id: "AmigaCD32".into(),
                title: format!("batch {index}"),
            })
            .collect();
        let plan = plan_bulk(
            &store,
            &candidates,
            &lab.trusted,
            &DiscSetLimits::default(),
            &lab.cancel,
        )
        .unwrap();
        start_queue(&store, &plan, &|_| true).unwrap();

        let active = Rc::new(Cell::new(false));
        let _pretend = pretend_below(&chosen, another(real), active.clone());
        let unplug = active.clone();
        let at_loss: Rc<std::cell::RefCell<Vec<PathBuf>>> = Rc::default();
        let listing = at_loss.clone();
        let folder = chosen.clone();
        AFTER_ITEM.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |index| {
                if index == 0 {
                    *listing.borrow_mut() = everything_under(&folder);
                    unplug.set(true);
                }
            }));
        });
        let run = |store: &PreparedMediaStore| {
            run_queue(
                store,
                None,
                &lab.trusted,
                &DiscSetLimits::default(),
                &lab.cancel,
                &|_| {},
            )
        };
        let interrupted = run(&store);
        AFTER_ITEM.with(|hook| *hook.borrow_mut() = None);
        assert!(
            matches!(&interrupted, Err(BulkError::Store(detail)) if detail.contains("not on the disk it was on")),
            "{interrupted:?}"
        );
        assert_eq!(
            everything_under(&chosen),
            *at_loss.borrow(),
            "something was written after the disk went"
        );
        // While it is away the batch cannot be run or restarted.
        let while_away = run(&store);
        assert!(while_away.is_err(), "{while_away:?}");
        assert_eq!(everything_under(&chosen), *at_loss.borrow());

        // Back again: the first game is as it was, the other two are done.
        active.set(false);
        let saved = load_queue(&store.resolved().unwrap()).unwrap();
        assert_eq!(saved.items[0].state, QueueState::Prepared);
        assert_eq!(saved.items[1].state, QueueState::Pending);
        assert_eq!(saved.items[2].state, QueueState::Pending);
        let finished = run(&store).unwrap();
        assert!(
            finished
                .items
                .iter()
                .all(|item| item.state == QueueState::Prepared),
            "{finished:?}"
        );
    }

    /// Clears the pretended mount points when the test ends.
    struct PretendMount;

    impl Drop for PretendMount {
        fn drop(&mut self) {
            MOUNT_ROOT_OVERRIDE.with(|hook| *hook.borrow_mut() = None);
        }
    }

    fn pretend_mount_root(folder: &Path) -> PretendMount {
        let folder = folder.to_path_buf();
        MOUNT_ROOT_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| (path == folder).then_some(true)));
        });
        PretendMount
    }

    /// A folder that is itself a mount point stays behind, empty, when its
    /// disk is unmounted. It is never accepted as the place for prepared
    /// copies: not when chosen, not when used, not by a store given directly.
    #[test]
    fn a_mount_root_is_never_a_place_for_prepared_copies() {
        // Real mount points of this machine, and the mount table's format.
        assert_eq!(is_mount_root(Path::new("/")), Ok(true));
        assert_eq!(is_mount_root(Path::new("/proc")), Ok(true));
        let lab = Lab::new();
        assert_eq!(is_mount_root(&lab.root), Ok(false));
        let table = "36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw\n\
                     40 36 0:31 / /mnt/My\\040Disk\\134x rw shared:5 - tmpfs tmpfs rw\n";
        assert_eq!(
            mount_points_in(table),
            vec![PathBuf::from("/mnt2"), PathBuf::from("/mnt/My Disk\\x")]
        );
        // A folder of our own, as if a disk were mounted on it.
        let zip = lab.standard_zip();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let mount_point = lab.root.join("mnt-disk");
        private_folder(&mount_point);
        let recorded = filesystem_id(&mount_point);
        let pretend = pretend_mount_root(&mount_point);
        let chosen = set_folder_at(
            &setting,
            Some(&mount_point),
            &[],
            Some(&disk_of(&mount_point)),
        );
        assert!(
            chosen
                .as_ref()
                .is_err_and(|detail| detail.contains("top folder of a disk")),
            "{chosen:?}"
        );
        assert_eq!(
            configured_folder_at(&setting),
            Ok(None),
            "nothing was saved"
        );
        // A setting that names it anyway (written by hand, or by a build
        // before this check) is refused when used.
        let used = PreparedMediaStore::configured_recorded(Some(mount_point.clone()), recorded);
        assert!(
            used.as_ref()
                .is_err_and(|detail| detail.contains("top folder of a disk")),
            "{used:?}"
        );
        // And a store pointed straight at it prepares nothing.
        let direct = PreparedMediaStore::at(&mount_point);
        let attempt = prepare_in(&direct, &lab, &zip);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("top folder of a disk")),
            "{attempt:?}"
        );
        assert!(direct.ready().is_err());
        assert!(
            everything_under(&mount_point).is_empty(),
            "something was created in the mount point"
        );
        // A folder inside the disk is fine.
        let inside = mount_point.join("prepared");
        set_folder_at(&setting, Some(&inside), &[], Some(&disk_of(&inside))).unwrap();
        assert_eq!(configured_folder_at(&setting), Ok(Some(inside.clone())));
        drop(pretend);
        // Not a mount point after all: the same folder is accepted.
        set_folder_at(
            &setting,
            Some(&mount_point),
            &[],
            Some(&disk_of(&mount_point)),
        )
        .unwrap();
    }

    /// Only ext4 is supported: there the identity the kernel reports comes
    /// from the UUID alone. Any other filesystem is refused whatever UUID is
    /// given, when a folder is checked, chosen or named, and nothing is
    /// created or written on it.
    #[test]
    fn a_filesystem_that_is_not_ext4_is_refused_and_not_written_to() {
        use crate::recovery::disk_identity::EXT4_OVERRIDE;
        let lab = Lab::new();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let disk = lab.root.join("on-xfs");
        private_folder(&disk);
        let existing = disk.join("prepared");
        private_folder(&existing);
        let named = disk_of(&existing);
        let below = disk.clone();
        EXT4_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| Some(!path.starts_with(&below))));
        });
        for folder in [existing.clone(), disk.join("new-folder")] {
            let check = check_folder(&folder, &[], Some(&named));
            assert!(
                check
                    .problems
                    .iter()
                    .any(|problem| problem.contains("is not on an ext4 filesystem")),
                "{check:?}"
            );
            assert!(check.filesystem_uuid.is_none());
            let refused = set_folder_at(&setting, Some(&folder), &[], Some(&named));
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|detail| detail.contains("is not on an ext4 filesystem")),
                "{refused:?}"
            );
        }
        let by_name = PreparedMediaStore::named(&existing, Some(&named), "--store");
        assert!(
            by_name
                .as_ref()
                .is_err_and(|detail| detail.contains("is not on an ext4 filesystem")),
            "{by_name:?}"
        );
        assert_eq!(everything_under(&disk), vec![existing.clone()]);
        assert_eq!(configured_folder_at(&setting), Ok(None));
        assert!(
            !setting
                .with_file_name("prepared_media_folder_filesystems.json")
                .exists()
        );
    }

    /// The whole life of a removable disk in one sequence: mounted,
    /// unmounted, another disk in its place, and the first disk back.
    #[test]
    fn mounted_unmounted_changed_and_remounted() {
        let lab = Lab::new();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let chosen = lab.root.join("mnt-disk/prepared");
        std::fs::create_dir(lab.root.join("mnt-disk")).unwrap();
        let underneath = real_filesystem(&lab.root);
        let disk = another(underneath);
        let other_disk = Filesystem {
            device: disk.device + 1,
            id: Some(disk.id.unwrap() ^ 0xffff),
        };
        // 0 = unmounted (what is really there), 1 = the disk, 2 = another.
        let phase = Rc::new(Cell::new(1_u8));
        let now = phase.clone();
        // The disk is mounted on `mnt-disk`; the folder is inside it.
        let below = lab.root.join("mnt-disk");
        FILESYSTEM_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| {
                if !path.starts_with(&below) {
                    return None;
                }
                match now.get() {
                    1 => Some(disk),
                    2 => Some(other_disk),
                    _ => None,
                }
            }));
        });
        let _pretend = Pretend;
        let open = || {
            PreparedMediaStore::configured_recorded(
                Some(chosen.clone()),
                recorded_filesystem_at(&setting, &chosen).unwrap(),
            )
        };

        // Mounted: chosen (which records the disk), saved, used.
        set_folder_at(&setting, Some(&chosen), &[], Some(&disk_of(&chosen))).unwrap();
        assert_eq!(recorded_filesystem_at(&setting, &chosen), Ok(disk.id));
        let first = disc(&lab, "FIRST");
        let second = disc(&lab, "SECOND");
        let store = open().unwrap().unwrap();
        let summary = prepare_in(&store, &lab, &first).unwrap().summary;
        let on_the_disk = everything_under(&chosen);

        // Unmounted, then another disk: refused both ways, for a fresh
        // lookup and for the handle from before; nothing is written.
        for (state, name) in [(0_u8, "unmounted"), (2, "another disk")] {
            phase.set(state);
            let fresh = open();
            assert!(
                fresh
                    .as_ref()
                    .is_err_and(|detail| detail.contains("not on the disk it was on")),
                "{name}: {fresh:?}"
            );
            let attempt = prepare_in(&store, &lab, &second);
            assert!(
                matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not on the disk it was on")),
                "{name}: {attempt:?}"
            );
            assert_eq!(store.prepared_for_source(&first), None, "{name}");
            assert_eq!(everything_under(&chosen), on_the_disk, "{name}");
        }

        // Remounted: the same disk again. The copy is served and new ones
        // can be prepared, through the old handle and a fresh lookup alike.
        phase.set(1);
        assert_eq!(store.prepared_for_source(&first), Some(summary));
        let fresh = open().unwrap().unwrap();
        prepare_in(&fresh, &lab, &second).unwrap();

        // The saved record decides, not the folder: with the record naming
        // another disk the very same folder is refused.
        std::fs::write(
            setting.with_file_name("prepared_media_folder_filesystems.json"),
            format!(
                "{{\"format_version\":2,\"folders\":{{{:?}:{:?}}}}}",
                chosen.to_str().unwrap(),
                uuid_with_identity(other_disk.id.unwrap())
            ),
        )
        .unwrap();
        let refused = PreparedMediaStore::configured_recorded(
            Some(chosen.clone()),
            recorded_filesystem_at(&setting, &chosen).unwrap(),
        );
        assert!(
            refused
                .as_ref()
                .is_err_and(|detail| detail.contains("not on the disk it was on")),
            "{refused:?}"
        );
    }

    /// An earlier folder whose disk record is missing, unreadable or names
    /// another disk is not used, and the launch check refuses every file in
    /// it rather than letting it through unchecked.
    #[test]
    fn an_earlier_folder_without_a_usable_disk_record_is_refused_at_launch() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let earlier = lab.root.join("earlier");
        set_folder_at(&setting, Some(&earlier), &[], Some(&disk_of(&earlier))).unwrap();
        let store = PreparedMediaStore::configured_recorded(
            Some(earlier.clone()),
            recorded_filesystem_at(&setting, &earlier).unwrap(),
        )
        .unwrap()
        .unwrap();
        let summary = prepare_in(&store, &lab, &zip).unwrap().summary;
        let track = summary.directory.join("media").join(ISO_NAME);
        assert_eq!(earlier_folder_distrust_at(&setting, &earlier), Ok(()));

        let record = setting.with_file_name("prepared_media_folder_filesystems.json");
        let good = std::fs::read(&record).unwrap();
        let other = filesystem_id(&earlier).unwrap() ^ 0x77;
        for (name, contents) in [
            ("unreadable", b"{ not json".to_vec()),
            (
                "from a later build",
                b"{\"format_version\":3,\"folders\":{}}".to_vec(),
            ),
            (
                "from the build that did not verify the disk",
                b"{\"format_version\":1,\"folders\":{}}".to_vec(),
            ),
            (
                "no entry",
                b"{\"format_version\":2,\"folders\":{}}".to_vec(),
            ),
            (
                "another disk",
                format!(
                    "{{\"format_version\":2,\"folders\":{{{:?}:{:?}}}}}",
                    earlier.to_str().unwrap(),
                    uuid_with_identity(other)
                )
                .into_bytes(),
            ),
        ] {
            std::fs::write(&record, contents).unwrap();
            let trust = earlier_folder_distrust_at(&setting, &earlier);
            assert!(trust.is_err(), "{name}: {trust:?}");
            // The folder is not among the stores any more; the gate still
            // refuses everything inside it, the cue sheet included.
            let previous = vec![(earlier.clone(), trust)];
            for inside in [&track, &summary.launch_path] {
                assert!(
                    launch_refusal_judged(None, None, &previous, &[], inside).is_some(),
                    "{name}: {} was let through",
                    inside.display()
                );
            }
            assert_eq!(
                launch_refusal_judged(None, None, &previous, &[], &zip),
                None,
                "{name}: a file outside the folder is not this gate's business"
            );
        }
        std::fs::write(&record, good).unwrap();
        assert_eq!(earlier_folder_distrust_at(&setting, &earlier), Ok(()));
        let previous = vec![(earlier.clone(), Ok(()))];
        let stores = vec![store];
        assert!(launch_refusal_judged(None, None, &previous, &stores, &track).is_some());
        assert_eq!(
            launch_refusal_judged(None, None, &previous, &stores, &summary.launch_path),
            None
        );
    }

    /// Each kind of refusal a named folder can meet, through both routes.
    fn named_is_refused(folder: &Path, uuid: Option<&str>, reason: &str) {
        use crate::recovery::prepared_media::PREPARED_MEDIA_DIR_ENV;
        let os = folder.to_path_buf().into_os_string();
        for attempt in [
            PreparedMediaStore::named(folder, uuid, "--store"),
            PreparedMediaStore::named(folder, uuid, PREPARED_MEDIA_DIR_ENV),
            PreparedMediaStore::for_profile_given(Some(os), uuid.map(std::ffi::OsString::from)),
        ] {
            assert!(
                attempt
                    .as_ref()
                    .is_err_and(|detail| detail.contains(reason)),
                "expected {reason:?}: {attempt:?}"
            );
        }
    }

    /// A folder named for one run (`--store`, `EMUWIZ_PREPARED_MEDIA_DIR`)
    /// must already be there and pass the checks a chosen folder passes. A
    /// missing one is never created: it may be a folder inside a disk that is
    /// not connected.
    #[test]
    fn a_named_folder_must_already_exist_and_is_never_created() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        // The empty mount point of a disk that is not connected, and the
        // folder inside that disk the copies are meant to go to.
        let mount_point = lab.root.join("mnt-absent");
        private_folder(&mount_point);
        let inside = mount_point.join("prepared");
        let some_disk = uuid_with_identity(0x1111_2222_3333_4444);
        assume_ext4();
        named_is_refused(&inside, Some(&some_disk), "is not there");
        // Even the UUID of the disk underneath does not get it created.
        named_is_refused(&inside, Some(&disk_of(&mount_point)), "is not there");
        assert!(
            everything_under(&mount_point).is_empty(),
            "the missing folder was created under the absent disk: {:?}",
            everything_under(&mount_point)
        );

        // Not a relative path, a file, a link, or a folder others can change.
        let file = lab.root.join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let real = lab.root.join("real-folder");
        private_folder(&real);
        let here = disk_of(&real);
        let link = lab.root.join("a-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let open = lab.root.join("open-folder");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o777))
            .unwrap();
        for unusable in [Path::new("prepared"), &file, &link, &open] {
            assert!(PreparedMediaStore::named(unusable, Some(&here), "--store").is_err());
        }
        // Not a mount point itself, even on the named disk.
        {
            let _mount = pretend_mount_root(&real);
            named_is_refused(&real, Some(&here), "top folder of a disk");
        }
        for folder in [&real, &open] {
            assert!(everything_under(folder).is_empty());
        }

        // An existing, plain, private folder on the named disk is accepted
        // and works as before, through either route.
        let store = PreparedMediaStore::named(&real, Some(&here), "--store").unwrap();
        prepare_in(&store, &lab, &zip).unwrap();
        assert_eq!(store.list().len(), 1);
        let again = PreparedMediaStore::for_profile_given(
            Some(real.clone().into_os_string()),
            Some(here.clone().into()),
        )
        .unwrap();
        assert_eq!(again.list().len(), 1);
    }

    /// A named folder being there proves nothing about which disk is. It is
    /// used only on the disk named with it: a folder of the same name on the
    /// disk underneath an absent one (with whatever is in it), another disk
    /// at the same path, no UUID, a UUID that is not one, or an identity two
    /// connected disks share are all refused, and nothing is written.
    #[test]
    fn a_named_folder_is_used_only_on_the_disk_named_with_it() {
        use crate::recovery::disk_identity::SAME_IDENTITY_OVERRIDE;
        use crate::recovery::prepared_media::PREPARED_MEDIA_FILESYSTEM_ENV;
        let lab = Lab::new();
        let zip = lab.standard_zip();
        assume_ext4();

        // The intended disk is absent. A folder of the same name really
        // exists on the disk underneath: plain, private, holding a store's
        // folders and a binding file left by an earlier build, which names
        // the very filesystem it is on.
        let mount_point = lab.root.join("mnt-absent");
        private_folder(&mount_point);
        let same_name = mount_point.join("prepared");
        private_folder(&same_name);
        for inner in ["v1", "v1/sets", "v1/staging", "v1/sources"] {
            private_folder(&same_name.join(inner));
        }
        let underneath = real_filesystem(&same_name);
        std::fs::write(
            same_name.join("emuwiz-storage-binding.json"),
            format!(
                "{{\"format_version\":1,\"filesystem_id\":\"{:016x}\"}}\n",
                underneath.id.unwrap()
            ),
        )
        .unwrap();
        let intended = uuid_with_identity(another(underneath).id.unwrap());
        let before = everything_under(&mount_point);
        named_is_refused(
            &same_name,
            Some(&intended),
            "is not on the disk with filesystem UUID",
        );
        named_is_refused(&same_name, None, "say which disk");
        for not_a_uuid in [
            "",
            "prepared",
            "0123",
            "01234567-89ab-cdef-0123-456789abcdeg",
            "01234567x89ab-cdef-0123-456789abcdef",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(
                PreparedMediaStore::named(&same_name, Some(not_a_uuid), "--store").is_err(),
                "{not_a_uuid:?}"
            );
        }
        // The second variable alone names no folder: refused, not ignored.
        let alone = PreparedMediaStore::for_profile_given(None, Some(intended.clone().into()));
        assert!(
            alone
                .as_ref()
                .is_err_and(|detail| detail.contains(PREPARED_MEDIA_FILESYSTEM_ENV)),
            "{alone:?}"
        );
        assert_eq!(
            everything_under(&mount_point),
            before,
            "something was written"
        );

        // The disk is there when the command starts, then goes; later
        // another disk is at the same path.
        let external = lab.root.join("mnt-external");
        private_folder(&external);
        let prepared = external.join("prepared");
        private_folder(&prepared);
        let disk = another(real_filesystem(&prepared));
        let active = Rc::new(Cell::new(true));
        let pretend = pretend_below(&external, disk, active.clone());
        let named = disk_of(&prepared);
        assert_eq!(named, uuid_with_identity(disk.id.unwrap()));
        let store = PreparedMediaStore::named(&prepared, Some(&named), "--store").unwrap();
        active.set(false); // the disk goes; the same-named folder is underneath
        let before = everything_under(&external);
        named_is_refused(
            &prepared,
            Some(&named),
            "is not on the disk with filesystem UUID",
        );
        // A store value made while the disk was there refuses at its next
        // write boundary too.
        let attempt = prepare_in(&store, &lab, &zip);
        assert!(
            matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains("not on the disk it was on")),
            "{attempt:?}"
        );
        assert!(store.ready().is_err());
        assert!(store.resolved().is_none());
        assert_eq!(everything_under(&external), before, "something was written");
        // The disk is back: usable again.
        active.set(true);
        let store = PreparedMediaStore::named(&prepared, Some(&named), "--store").unwrap();
        prepare_in(&store, &lab, &zip).unwrap();
        // Another disk at the same path, holding the same folder and files.
        drop(pretend);
        let _other = pretend_below(&external, another(disk), Rc::new(Cell::new(true)));
        named_is_refused(
            &prepared,
            Some(&named),
            "is not on the disk with filesystem UUID",
        );
        drop(_other);

        // Two connected disks report the identity (a copied disk), none
        // does, or the list of disks cannot be read: refused each time.
        let plain = lab.root.join("plain");
        private_folder(&plain);
        let here = disk_of(&plain);
        for (connected, reason) in [
            (Ok(2), "more than one connected disk"),
            (Ok(0), "is not among the mounted ext4 disks"),
            (
                Err("the list of mounted disks could not be read".to_string()),
                "could not be read",
            ),
        ] {
            SAME_IDENTITY_OVERRIDE.with(|hook| {
                let connected = connected.clone();
                *hook.borrow_mut() = Some(Box::new(move |_| Some(connected.clone())));
            });
            named_is_refused(&plain, Some(&here), reason);
        }
        assert!(everything_under(&plain).is_empty());
    }

    /// A named folder is tied to the filesystem it was on when the command
    /// started: if the folder is gone afterwards (as after a real unmount),
    /// it is not created again and nothing is written.
    #[test]
    fn a_named_folder_that_goes_away_is_not_created_again() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let mount_point = lab.root.join("mnt-named");
        private_folder(&mount_point);
        let named = mount_point.join("prepared");
        private_folder(&named);
        let here = disk_of(&named);
        let store = PreparedMediaStore::named(&named, Some(&here), "--store").unwrap();
        std::fs::remove_dir(&named).unwrap();
        assert!(store.ready().is_err());
        assert!(prepare_in(&store, &lab, &zip).is_err());
        assert!(PreparedMediaStore::named(&named, Some(&here), "--store").is_err());
        assert!(everything_under(&mount_point).is_empty());
    }

    /// The first time a folder is chosen (Blue's storage card and
    /// `storage --set` both end here), the disk is never taken from where
    /// the folder happens to be. With the intended disk absent, nothing is
    /// created underneath, nothing is probed, and nothing is saved, whether
    /// or not a folder of the same name exists there.
    #[test]
    fn choosing_a_folder_never_trusts_the_disk_that_happens_to_be_there() {
        let lab = Lab::new();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let record = setting.with_file_name("prepared_media_folder_filesystems.json");
        assume_ext4();
        let mount_point = lab.root.join("mnt-disk");
        private_folder(&mount_point);
        let folder = mount_point.join("prepared");
        let underneath = real_filesystem(&mount_point);
        let disk = another(underneath);
        let intended = uuid_with_identity(disk.id.unwrap());
        let nothing_saved = || {
            assert_eq!(configured_folder_at(&setting), Ok(None));
            assert!(!record.exists(), "a record was written");
        };
        let refused = |uuid: Option<&str>, reason: &str| {
            let check = check_folder(&folder, &[], uuid);
            assert!(
                !check.usable()
                    && check.filesystem_uuid.is_none()
                    && check
                        .problems
                        .iter()
                        .any(|problem| problem.contains(reason)),
                "expected {reason:?}: {check:?}"
            );
            let set = set_folder_at(&setting, Some(&folder), &[], uuid);
            assert!(
                set.as_ref().is_err_and(|detail| detail.contains(reason)),
                "expected {reason:?}: {set:?}"
            );
        };

        // The disk is absent and the folder does not exist underneath.
        refused(Some(&intended), "is not on the disk with filesystem UUID");
        refused(None, "Say which disk this folder is on");
        refused(Some("not-a-uuid"), "is not a filesystem UUID");
        assert!(
            everything_under(&mount_point).is_empty(),
            "created underneath"
        );
        nothing_saved();

        // The disk is absent and a folder of the same name exists
        // underneath, private and ready to be written to.
        private_folder(&folder);
        std::fs::write(folder.join("emuwiz-storage-binding.json"), b"{}").unwrap();
        let before = everything_under(&mount_point);
        refused(Some(&intended), "is not on the disk with filesystem UUID");
        refused(None, "Say which disk this folder is on");
        assert_eq!(everything_under(&mount_point), before, "written underneath");
        nothing_saved();
        std::fs::remove_file(folder.join("emuwiz-storage-binding.json")).unwrap();
        std::fs::remove_dir(&folder).unwrap();

        // Another disk is mounted at the path.
        {
            let _wrong = pretend_below(&mount_point, another(disk), Rc::new(Cell::new(true)));
            refused(Some(&intended), "is not on the disk with filesystem UUID");
        }
        // The identity is shared by two connected disks.
        {
            use crate::recovery::disk_identity::SAME_IDENTITY_OVERRIDE;
            let _there = pretend_below(&mount_point, disk, Rc::new(Cell::new(true)));
            SAME_IDENTITY_OVERRIDE
                .with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(Ok(2)))));
            refused(Some(&intended), "more than one connected disk");
            SAME_IDENTITY_OVERRIDE.with(|hook| *hook.borrow_mut() = None);
            assume_ext4();
        }
        assert!(
            everything_under(&mount_point).is_empty(),
            "created on the wrong disk"
        );
        nothing_saved();

        // A path that leads somewhere else through a link: the folder above
        // is resolved first and must itself be on the named disk.
        let elsewhere = lab.root.join("elsewhere");
        private_folder(&elsewhere);
        let through = lab.root.join("looks-like-the-disk");
        std::os::unix::fs::symlink(&elsewhere, &through).unwrap();
        let set = set_folder_at(
            &setting,
            Some(&through.join("prepared")),
            &[],
            Some(&intended),
        );
        assert!(set.is_err(), "{set:?}");
        let set = set_folder_at(&setting, Some(&through), &[], Some(&disk_of(&elsewhere)));
        assert!(
            set.as_ref().is_err_and(|detail| detail.contains("link")),
            "{set:?}"
        );
        assert!(everything_under(&elsewhere).is_empty());
        nothing_saved();

        // The mount changes between the checks and the creation: wherever
        // the change lands, either the choice is refused and nothing is left
        // underneath, or everything was verified on the disk.
        for lands_after in 0..8_usize {
            let seen = Rc::new(Cell::new(0_usize));
            let below = mount_point.clone();
            let count = seen.clone();
            FILESYSTEM_OVERRIDE.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |path| {
                    if !path.starts_with(&below) {
                        return None;
                    }
                    count.set(count.get() + 1);
                    (count.get() <= lands_after).then_some(disk)
                }));
            });
            let set = set_folder_at(&setting, Some(&folder), &[], Some(&intended));
            FILESYSTEM_OVERRIDE.with(|hook| *hook.borrow_mut() = None);
            match set {
                Err(_) => {
                    assert!(
                        everything_under(&mount_point).is_empty(),
                        "change after {lands_after} checks left {:?}",
                        everything_under(&mount_point)
                    );
                    nothing_saved();
                }
                Ok(_) => {
                    assert!(
                        seen.get() <= lands_after,
                        "accepted after the disk had gone"
                    );
                    std::fs::remove_dir(&folder).unwrap();
                    std::fs::remove_file(&setting).unwrap();
                    std::fs::remove_file(&record).unwrap();
                }
            }
        }

        // The disk is there: the folder is created on it, the named UUID is
        // saved, and the choice works while the disk stays.
        let active = Rc::new(Cell::new(true));
        let _there = pretend_below(&mount_point, disk, active.clone());
        let check = set_folder_at(&setting, Some(&folder), &[], Some(&intended.to_uppercase()))
            .unwrap()
            .unwrap();
        assert_eq!(check.filesystem_uuid.as_deref(), Some(intended.as_str()));
        assert!(folder.is_dir());
        assert_eq!(recorded_filesystem_at(&setting, &folder), Ok(disk.id));
        let recorded = recorded_filesystem_at(&setting, &folder).unwrap();
        assert!(
            PreparedMediaStore::configured_recorded(Some(folder.clone()), recorded)
                .unwrap()
                .is_some()
        );
        // And is refused, as before, once the disk is gone.
        active.set(false);
        assert!(PreparedMediaStore::configured_recorded(Some(folder.clone()), recorded).is_err());
    }

    /// Creating the chosen folder is itself guarded, apart from the checks
    /// made before and after it. The folder above is verified before
    /// anything is created in it: on the wrong disk, no folder is made and
    /// removed again (the folder above keeps its modification time).
    #[test]
    fn a_folder_is_not_created_even_briefly_under_the_wrong_disk() {
        let lab = Lab::new();
        assume_ext4();
        let mount_point = lab.root.join("mnt-disk");
        private_folder(&mount_point);
        let folder = mount_point.join("prepared");
        let intended = uuid_with_identity(another(real_filesystem(&mount_point)).id.unwrap());
        let modified = |path: &Path| {
            let facts = std::fs::metadata(path).unwrap();
            (facts.mtime(), facts.mtime_nsec())
        };
        let before = modified(&mount_point);
        // Longer than the clock step a folder's timestamps are taken with.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(create_on_verified_disk(&folder, &[], Some(&intended)).is_err());
        assert!(everything_under(&mount_point).is_empty());
        assert_eq!(
            modified(&mount_point),
            before,
            "a folder was created underneath and removed again"
        );
    }

    /// The folder just created is not taken to be on the disk because the
    /// folder above was: if the first look at it finds another filesystem,
    /// it is refused and removed. (Two checks look at the new folder, one
    /// through the handle it was created by and one by its path; this test
    /// cannot tell them apart, so it does not prove the first on its own.)
    #[test]
    fn a_created_folder_first_seen_on_another_disk_is_refused_and_removed() {
        let lab = Lab::new();
        assume_ext4();
        let mount_point = lab.root.join("mnt-disk");
        private_folder(&mount_point);
        let folder = mount_point.join("prepared");
        let disk = another(real_filesystem(&mount_point));
        let elsewhere = another(disk);
        let intended = uuid_with_identity(disk.id.unwrap());
        let looks = Rc::new(Cell::new(0_usize));
        let (count, new_folder) = (looks.clone(), folder.clone());
        FILESYSTEM_OVERRIDE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path| {
                if path != new_folder {
                    return Some(disk);
                }
                count.set(count.get() + 1);
                Some(if count.get() == 1 { elsewhere } else { disk })
            }));
        });
        let _clear = Pretend;
        let made = create_on_verified_disk(&folder, &[], Some(&intended));
        assert!(looks.get() >= 1, "the new folder was never looked at");
        assert!(
            made.is_err(),
            "accepted a folder first seen on another disk"
        );
        assert!(everything_under(&mount_point).is_empty());
    }

    /// A saved choice is held, each time it is used, to the same proof as
    /// when it was chosen: not only the recorded identity, but an ext4
    /// filesystem and no second connected disk reporting that identity (a
    /// copied disk mounted in its place).
    #[test]
    fn a_saved_choice_is_held_to_the_full_proof_each_time_it_is_used() {
        use crate::recovery::disk_identity::{EXT4_OVERRIDE, SAME_IDENTITY_OVERRIDE};
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let chosen = lab.root.join("chosen");
        private_folder(&chosen);
        let real = real_filesystem(&chosen);
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), real.id)
            .unwrap()
            .unwrap();
        let refused_with = |reason: &str| {
            let attempt = prepare_in(&store, &lab, &zip);
            assert!(
                matches!(&attempt, Err(PrepareError::Store(detail)) if detail.contains(reason)),
                "expected {reason:?}: {attempt:?}"
            );
            assert!(store.ready().is_err());
            assert!(everything_under(&chosen).is_empty());
        };

        SAME_IDENTITY_OVERRIDE.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(Ok(2)))));
        refused_with("more than one connected disk");
        SAME_IDENTITY_OVERRIDE.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(Ok(0)))));
        refused_with("not among the mounted ext4 disks");
        SAME_IDENTITY_OVERRIDE.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(Ok(1)))));

        EXT4_OVERRIDE.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(false))));
        refused_with("is not on an ext4 filesystem");
        EXT4_OVERRIDE.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(true))));

        // With the proof whole again the same store prepares.
        assert!(prepare_in(&store, &lab, &zip).is_ok());
    }

    /// A folder tied to a disk is never created, whichever way the store
    /// was made: an earlier folder that is gone (its disk is absent) is
    /// refused, and nothing appears where its disk would be mounted.
    #[test]
    fn a_folder_tied_to_a_disk_is_never_created() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let mount_point = lab.root.join("mnt-gone");
        private_folder(&mount_point);
        let folder = mount_point.join("prepared");
        let recorded = real_filesystem(&mount_point).id.unwrap();
        let store = PreparedMediaStore::earlier(folder, recorded);
        assert!(store.ready().is_err());
        assert!(prepare_in(&store, &lab, &zip).is_err());
        assert!(everything_under(&mount_point).is_empty());
    }

    /// Removing a prepared copy deletes its link records by path. If the
    /// disk goes while the copy is being removed, those deletions are not
    /// made: what is at the path now is not the store the removal began in.
    #[test]
    fn link_records_are_not_removed_by_path_once_the_disk_has_gone() {
        let lab = Lab::new();
        let zip = lab.standard_zip();
        let chosen = lab.root.join("chosen");
        private_folder(&chosen);
        let real = real_filesystem(&chosen);
        let store = PreparedMediaStore::configured_recorded(Some(chosen.clone()), real.id)
            .unwrap()
            .unwrap();
        let set_id = prepare_in(&store, &lab, &zip).unwrap().summary.set_id;
        let records = || {
            std::fs::read_dir(chosen.join("v1/sources"))
                .unwrap()
                .count()
        };
        let before = records();
        assert!(before >= 1);
        let gone = Rc::new(Cell::new(false));
        let _pretend = pretend_below(&chosen, another(real), gone.clone());
        let flag = gone.clone();
        let _ = with_hook(
            move |reached| {
                if reached == Point::AfterRemovalParked {
                    flag.set(true);
                }
            },
            || store.remove(&set_id),
        );
        assert!(
            gone.get(),
            "the removal never reached the point of the change"
        );
        assert_eq!(records(), before, "link records were removed by path");
    }

    /// A record written by the build that remembered whatever filesystem a
    /// folder was on (format 1) is not trusted: the folder is refused until
    /// it is chosen again with its disk named, and the launch check refuses
    /// the files inside it meanwhile.
    #[test]
    fn a_record_that_never_verified_the_disk_is_not_trusted() {
        let lab = Lab::new();
        let setting = lab.root.join("config/prepared_media_folder.txt");
        let record = setting.with_file_name("prepared_media_folder_filesystems.json");
        let folder = lab.root.join("chosen-earlier");
        private_folder(&folder);
        std::fs::create_dir_all(setting.parent().unwrap()).unwrap();
        std::fs::write(&setting, format!("{}\n", folder.display())).unwrap();
        std::fs::write(
            &record,
            format!(
                "{{\"format_version\":1,\"folders\":{{\"{}\":{}}}}}",
                folder.display(),
                filesystem_id(&folder).unwrap()
            ),
        )
        .unwrap();
        let old = recorded_filesystem_at(&setting, &folder);
        assert!(
            old.as_ref()
                .is_err_and(|detail| detail.contains("did not verify which disk")),
            "{old:?}"
        );
        assert!(earlier_folder_distrust_at(&setting, &folder).is_err());
        // A damaged UUID in a current record is an error too.
        std::fs::write(
            &record,
            format!(
                "{{\"format_version\":2,\"folders\":{{\"{}\":\"not-a-uuid\"}}}}",
                folder.display()
            ),
        )
        .unwrap();
        assert!(recorded_filesystem_at(&setting, &folder).is_err());
        // Chosen again with its disk named: a current record replaces it.
        set_folder_at(&setting, Some(&folder), &[], Some(&disk_of(&folder))).unwrap();
        assert_eq!(
            recorded_filesystem_at(&setting, &folder),
            Ok(filesystem_id(&folder))
        );
        assert_eq!(earlier_folder_distrust_at(&setting, &folder), Ok(()));
    }
}

// ---------------------------------------------------------------------------
// From a filesystem UUID to the identity the kernel reports
// ---------------------------------------------------------------------------

/// Unlike the tests above, nothing here is built backwards: the expected
/// identities are fixed values worked out separately from this code, from
/// the kernel's rule for ext4 (`ext4_statfs`: the UUID's 16 bytes as two
/// little-endian 64-bit halves, XORed).
mod disk_identity_tests {
    use super::*;
    use crate::recovery::disk_identity::*;

    /// (UUID, identity) pairs computed outside this code. The third tells
    /// little-endian halves from big-endian ones; the last has no dashes.
    const VECTORS: [(&str, u64); 4] = [
        (
            "00112233-4455-6677-8899-aabbccddeeff",
            0x8888_8888_8888_8888,
        ),
        (
            "6f1d2c3b-4a59-4e68-9778-a6b5c4d3e2f1",
            0x99ac_8a8e_8e8a_65f8,
        ),
        (
            "ffffffff-ffff-ffff-0000-000000000001",
            0xfeff_ffff_ffff_ffff,
        ),
        ("0123456789abcdef0011223344556677", 0x98ab_fecd_5467_3201),
    ];

    #[test]
    fn known_uuids_give_their_known_identities() {
        for (uuid, identity) in VECTORS {
            let parsed = ExpectedFilesystem::parse(uuid).unwrap();
            assert_eq!(parsed.id(), identity, "{uuid}");
            // The same disk however it is written.
            let upper = ExpectedFilesystem::parse(&format!("  {}\n", uuid.to_uppercase())).unwrap();
            assert_eq!(upper, parsed);
            assert_eq!(parsed.uuid().len(), 36);
            assert_eq!(parsed.uuid().replace('-', ""), uuid.replace('-', ""));
            assert_eq!(ExpectedFilesystem::parse(parsed.uuid()).unwrap(), parsed);
        }
        // Not the big-endian reading, and not the halves swapped into
        // another value.
        assert_ne!(
            ExpectedFilesystem::parse(VECTORS[2].0).unwrap().id(),
            0xffff_ffff_ffff_fffe
        );
    }

    #[test]
    fn anything_that_is_not_a_uuid_is_refused() {
        for text in [
            "",
            " ",
            "0",
            "00112233-4455-6677-8899-aabbccddeef",
            "00112233-4455-6677-8899-aabbccddeeff0",
            "00112233-4455-6677-8899-aabbccddeefg",
            "00112233_4455_6677_8899_aabbccddeeff",
            "0011-22334455-6677-8899-aabbccddeeff",
            "00112233-4455-6677-8899aabbccddeeff",
            "-00112233445566778899aabbccddeeff---",
            "{00112233-4455-6677-8899-aabbccddeeff}",
            "UUID=00112233-4455-6677-8899-aabbccddeeff",
            "/dev/sda1",
            "ｆfffffff-ffff-ffff-0000-00000000001",
        ] {
            assert!(ExpectedFilesystem::parse(text).is_err(), "{text:?}");
        }
        // The empty identity: the all-zero UUID, and halves that cancel.
        for empty in [
            "00000000-0000-0000-0000-000000000000",
            "01234567-89ab-cdef-0123-456789abcdef",
        ] {
            let refused = ExpectedFilesystem::parse(empty);
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|detail| detail.contains("does not identify")),
                "{refused:?}"
            );
        }
    }

    const TABLE: &str = "\
24 1 252:0 / / rw,relatime shared:1 - ext4 /dev/mapper/vg-root rw\n\
30 24 8:33 / /mnt/My\\040Disk rw shared:5 - ext4 /dev/sdc1 rw\n\
31 24 8:33 /sub /mnt/again rw shared:5 - ext4 /dev/sdc1 rw\n\
32 24 8:49 / /mnt/copy rw shared:6 - ext4 /dev/sdd1 rw\n\
33 24 0:40 / /mnt/tmp rw - tmpfs tmpfs rw\n\
34 24 8:65 / /mnt/xfs rw - xfs /dev/sde1 rw\n";

    fn identities(_device: &str, mount_point: &Path) -> Option<u64> {
        match mount_point.to_str()? {
            "/" => Some(VECTORS[0].1),
            "/mnt/My Disk" | "/mnt/again" | "/mnt/copy" => Some(VECTORS[1].1),
            // Never asked for: not ext4.
            "/mnt/tmp" | "/mnt/xfs" => Some(VECTORS[0].1),
            _ => None,
        }
    }

    /// A mount with another mount stacked directly on top of it is left out
    /// of the list altogether: it is neither counted nor asked (asking would
    /// reach the filesystem on top). Mounts elsewhere, and a mount below the
    /// covered one, stay.
    #[test]
    fn a_mount_covered_by_another_is_not_listed() {
        let stacked = "\
24 1 252:0 / / rw - ext4 /dev/mapper/vg-root rw\n\
77 24 8:33 /inner /mnt/stack rw - ext4 /dev/sdc1 rw\n\
90 24 8:33 / /mnt/disk rw - ext4 /dev/sdc1 rw\n\
91 90 8:49 / /mnt/disk/inside rw - ext4 /dev/sdd1 rw\n\
3406 77 0:178 / /mnt/stack rw - fuse.x x rw\n";
        let listed: Vec<(String, PathBuf, String)> = mount_lines(stacked);
        let points: Vec<&str> = listed
            .iter()
            .map(|(_, point, _)| point.to_str().unwrap())
            .collect();
        assert_eq!(points, ["/", "/mnt/disk", "/mnt/disk/inside", "/mnt/stack"]);
        assert_eq!(
            listed
                .iter()
                .filter(|(_, point, _)| point == Path::new("/mnt/stack"))
                .map(|(_, _, kind)| kind.as_str())
                .collect::<Vec<_>>(),
            ["fuse.x"]
        );
    }

    /// A mount is asked for its identity only if its path leads to that
    /// device now. Read-only, on this machine's own "/". (This shows the
    /// device comparison only; no covered mount is involved.)
    #[test]
    fn a_mount_is_asked_only_if_its_path_leads_to_its_device() {
        assert_eq!(device_name(libc::makedev(8, 33)), "8:33");
        assert_eq!(device_name(libc::makedev(252, 0)), "252:0");
        let root = Path::new("/");
        let device = std::fs::metadata(root).unwrap().dev();
        let real = crate::recovery::prepared_media::filesystem_id(root);
        assert_eq!(visible_identity(&device_name(device), root), real);
        // The table names another device for this path: not asked.
        let other = device_name(device.wrapping_add(1));
        assert_eq!(visible_identity(&other, root), None);
    }

    #[test]
    fn a_shared_identity_is_noticed_and_other_filesystems_do_not_count() {
        // One disk, mounted once.
        assert_eq!(
            devices_reporting(TABLE, VECTORS[0].1, &identities)
                .into_iter()
                .collect::<Vec<_>>(),
            ["252:0"]
        );
        // One disk mounted twice is one disk; a second device is another.
        assert_eq!(
            devices_reporting(TABLE, VECTORS[1].1, &identities)
                .into_iter()
                .collect::<Vec<_>>(),
            ["8:33", "8:49"]
        );
        assert!(devices_reporting(TABLE, VECTORS[2].1, &identities).is_empty());
    }

    #[test]
    fn only_consistently_identified_ext4_disks_are_offered() {
        let facts = |device: &str| match device {
            "252:0" => Some((VECTORS[0].0.to_string(), Some("system".to_string()))),
            // The device database names this UUID for both devices.
            "8:33" | "8:49" => Some((VECTORS[1].0.to_string(), None)),
            "8:65" => Some((VECTORS[2].0.to_string(), None)),
            _ => None,
        };
        let offered = mounted_ext4_disks_in(TABLE, &facts, &identities);
        assert_eq!(offered.len(), 1, "{offered:?}");
        assert_eq!(offered[0].uuid, VECTORS[0].0);
        assert_eq!(offered[0].label.as_deref(), Some("system"));
        assert_eq!(offered[0].mount_point, Path::new("/"));

        // Without the copy, the disk is offered once, at its shortest mount.
        let single: String =
            TABLE
                .lines()
                .filter(|l| !l.contains("8:49"))
                .fold(String::new(), |mut table, line| {
                    table.push_str(line);
                    table.push('\n');
                    table
                });
        let offered = mounted_ext4_disks_in(&single, &facts, &identities);
        assert_eq!(offered.len(), 2, "{offered:?}");
        assert_eq!(offered[1].mount_point, Path::new("/mnt/again"));
        // A device database that names another UUID than the mounted
        // filesystem reports: not offered.
        let stale = |device: &str| match device {
            "252:0" => Some((VECTORS[2].0.to_string(), None)),
            _ => None,
        };
        assert!(mounted_ext4_disks_in(&single, &stale, &identities).is_empty());

        assert_eq!(
            device_facts_in("S:disk/by-uuid/x\nE:ID_FS_LABEL=Games\nE:ID_FS_UUID=abc\n"),
            Some(("abc".to_string(), Some("Games".to_string())))
        );
        assert_eq!(device_facts_in("E:ID_FS_TYPE=ext4\n"), None);
    }

    /// Read-only, on whatever machine runs the tests: every mounted ext4
    /// filesystem whose UUID the system's device database knows must report
    /// exactly the identity this code derives from that UUID. Real UUIDs,
    /// real identities; none is printed. On a machine where none is visible
    /// (a container), this checks nothing and says so.
    #[test]
    fn this_machines_own_ext4_filesystems_agree_with_the_rule() {
        let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let mut checked = 0;
        for line in table.lines() {
            let Some((before, after)) = line.split_once(" - ") else {
                continue;
            };
            if !after.starts_with("ext4 ") {
                continue;
            }
            let fields: Vec<&str> = before.split(' ').collect();
            let (Some(device), Some(mount_point)) = (fields.get(2), fields.get(4)) else {
                continue;
            };
            if mount_point.contains('\\') {
                continue;
            }
            let Ok(record) = std::fs::read_to_string(format!("/run/udev/data/b{device}")) else {
                continue;
            };
            let Some((uuid, _)) = device_facts_in(&record) else {
                continue;
            };
            let Some(reported) =
                crate::recovery::prepared_media::filesystem_id(Path::new(mount_point))
            else {
                continue;
            };
            assert_eq!(
                ExpectedFilesystem::parse(&uuid).unwrap().id(),
                reported,
                "the ext4 filesystem mounted at {mount_point} does not report the identity of its UUID"
            );
            checked += 1;
        }
        println!("real ext4 filesystems checked against their UUIDs: {checked}");
    }
}
