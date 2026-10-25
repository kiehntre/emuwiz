//! Read-only per-platform firmware information, with an explicit import of a
//! user-selected RetroBIOS metadata file.
//!
//! The only file this reads is the one metadata JSON the user picks; local
//! BIOS evidence comes from the page's existing bounded folder inspection.
//! Nothing here downloads, imports or links firmware, or edits emulator
//! settings.

use archivefs_core::bios_projection::{BiosContentClass, BiosMasterInventory};
use archivefs_core::emulator_inventory::EmulatorInventory;
use archivefs_core::retrobios_provider::{
    RETROBIOS_MAX_SNAPSHOT_BYTES, RetroBiosMetadataSnapshot, RetroBiosSignatureState,
    parse_snapshot,
};
use archivefs_core::retrobios_viewer::{
    EmulatorFirmwareUse, EmulatorInstallation, FIRMWARE_PLATFORMS, FirmwareRequiredness,
    FirmwareRow, FirmwareRowSource, FirmwareRowState, PlatformFirmwareView, firmware_platform,
    platform_firmware_view,
};
use eframe::egui;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Existing pages the viewer can send the user to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FirmwareNavigation {
    SetupDoctor,
    EmulatorSetup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

struct ImportedCatalogue {
    path: PathBuf,
    stamp: FileStamp,
    snapshot: RetroBiosMetadataSnapshot,
    /// The file changed or disappeared after it was imported.
    stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViewKey {
    platform: &'static str,
    inventory: u64,
    catalogue: u64,
    installed: u64,
}

pub(crate) struct FirmwareInformationViewer {
    platform: &'static str,
    catalogue: Option<ImportedCatalogue>,
    catalogue_generation: u64,
    import_error: Option<String>,
    installed: Option<EmulatorInventory>,
    installed_generation: u64,
    cache: Option<(ViewKey, PlatformFirmwareView)>,
    navigation: Option<FirmwareNavigation>,
}

impl Default for FirmwareInformationViewer {
    fn default() -> Self {
        Self {
            platform: FIRMWARE_PLATFORMS[0].key,
            catalogue: None,
            catalogue_generation: 0,
            import_error: None,
            installed: None,
            installed_generation: 0,
            cache: None,
            navigation: None,
        }
    }
}

fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// Reads and validates one user-selected metadata file through the existing
/// Core parser. BIOS packs and other archives are refused before parsing.
fn read_catalogue(path: &Path) -> Result<ImportedCatalogue, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("The metadata file could not be checked: {error}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular RetroBIOS metadata file, not a folder or link.".into());
    }
    if metadata.len() > RETROBIOS_MAX_SNAPSHOT_BYTES as u64 {
        return Err("This file is larger than a RetroBIOS metadata file can be. BIOS packs and firmware archives are not imported.".into());
    }
    let stamp = file_stamp(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(RETROBIOS_MAX_SNAPSHOT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("The metadata file could not be read: {error}"))?;
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"7z\xBC\xAF\x27\x1C")
        || bytes.starts_with(b"Rar!")
        || bytes.starts_with(&[0x1f, 0x8b])
    {
        return Err("This looks like an archive, such as a BIOS pack. EmuWiz imports only RetroBIOS metadata JSON and never imports firmware.".into());
    }
    let imported_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| format!("unix:{}", elapsed.as_secs()))
        .unwrap_or_else(|_| "unknown".into());
    let snapshot = parse_snapshot(&bytes, imported_at)
        .map_err(|error| format!("Not imported: {error}. Nothing was changed."))?;
    Ok(ImportedCatalogue {
        path: path.to_path_buf(),
        stamp,
        snapshot,
        stale: false,
    })
}

impl FirmwareInformationViewer {
    /// Mirrors the emulator inventory the user already scanned. Never scans.
    pub(crate) fn set_installed(&mut self, installed: Option<&EmulatorInventory>) {
        if self.installed.as_ref() != installed {
            self.installed = installed.cloned();
            self.installed_generation += 1;
        }
    }

    pub(crate) fn take_navigation(&mut self) -> Option<FirmwareNavigation> {
        self.navigation.take()
    }

    fn import(&mut self, path: &Path) {
        match read_catalogue(path) {
            Ok(catalogue) => {
                self.catalogue = Some(catalogue);
                self.catalogue_generation += 1;
                self.import_error = None;
            }
            // A rejected file leaves any previously imported catalogue alone.
            Err(error) => self.import_error = Some(error),
        }
    }

    fn remove_catalogue(&mut self) {
        self.catalogue = None;
        self.catalogue_generation += 1;
        self.import_error = None;
    }

    fn select_platform(&mut self, key: &'static str) {
        if self.platform != key {
            self.platform = key;
            self.check_catalogue_freshness();
        }
    }

    /// One metadata check of the imported file, on platform switches only.
    fn check_catalogue_freshness(&mut self) {
        if let Some(catalogue) = &mut self.catalogue {
            let current = file_stamp(&catalogue.path).ok();
            catalogue.stale = current.as_ref() != Some(&catalogue.stamp);
        }
    }

    fn view(
        &mut self,
        inventory: Option<&BiosMasterInventory>,
        inventory_generation: u64,
    ) -> &PlatformFirmwareView {
        let key = ViewKey {
            platform: self.platform,
            inventory: inventory_generation,
            catalogue: self.catalogue_generation,
            installed: self.installed_generation,
        };
        if self.cache.as_ref().map(|(cached, _)| *cached) != Some(key) {
            let platform = firmware_platform(self.platform).unwrap_or(&FIRMWARE_PLATFORMS[0]);
            let view = platform_firmware_view(
                platform,
                inventory,
                self.catalogue.as_ref().map(|catalogue| &catalogue.snapshot),
                self.installed.as_ref(),
            );
            self.cache = Some((key, view));
        }
        &self.cache.as_ref().expect("cache filled above").1
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        inventory: Option<&BiosMasterInventory>,
        inventory_generation: u64,
    ) {
        egui::CollapsingHeader::new(
            egui::RichText::new("Firmware information by platform").strong(),
        )
        .default_open(true)
        .show(ui, |ui| self.show_body(ui, inventory, inventory_generation));
        ui.separator();
    }

    fn show_body(
        &mut self,
        ui: &mut egui::Ui,
        inventory: Option<&BiosMasterInventory>,
        inventory_generation: u64,
    ) {
        self.show_catalogue_controls(ui);
        let mut selected = self.platform;
        let label = firmware_platform(selected).map_or("", |platform| platform.label);
        egui::ComboBox::from_label("Platform")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for platform in FIRMWARE_PLATFORMS {
                    ui.selectable_value(&mut selected, platform.key, platform.label);
                }
            });
        self.select_platform(selected);
        let catalogue_loaded = self.catalogue.is_some();
        let view = self.view(inventory, inventory_generation).clone();
        ui.label(format!(
            "Mr Wiz · {}",
            mr_wiz_guidance(&view, catalogue_loaded)
        ));
        match &view.inventory_root {
            Some(root) => ui.small(format!(
                "Local files: from the last inspection of {}. Inspect again below after changing that folder.",
                root.display()
            )),
            None => ui.small("Local files: no BIOS folder inspected yet. Use Inspect BIOS Folder below."),
        };
        self.show_emulators(ui, &view.emulators);
        if view.rows.is_empty() {
            ui.label(if catalogue_loaded {
                "Neither EmuWiz's emulator table nor the imported metadata lists firmware for this platform."
            } else {
                "EmuWiz's emulator table has no firmware entry for this platform. Importing RetroBIOS metadata may add one."
            });
        }
        for (index, row) in view.rows.iter().enumerate() {
            ui.push_id(("firmware-row", self.platform, index), |ui| {
                show_row(ui, row)
            });
        }
    }

    fn show_catalogue_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if crate::widgets::action_button(
                ui,
                "Import RetroBIOS Metadata…",
                crate::widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("RetroBIOS metadata", &["json"])
                    .pick_file()
            {
                self.import(&path);
            }
            if self.catalogue.is_some() && ui.button("Stop using imported metadata").clicked() {
                self.remove_catalogue();
            }
        });
        match &self.catalogue {
            Some(catalogue) => {
                ui.label(catalogue_status(catalogue));
                if catalogue.stale {
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 130, 40),
                        "The metadata file changed or was removed after import. Import it again to use the current file.",
                    );
                }
            }
            None => {
                ui.small("No RetroBIOS metadata imported. Requirements below come from EmuWiz's emulator table, which lists filenames but no hashes.");
            }
        }
        ui.small("Metadata describes what emulators expect; it never proves your files by itself. EmuWiz does not supply or download BIOS files.");
        if let Some(error) = &self.import_error {
            ui.colored_label(egui::Color32::from_rgb(180, 70, 55), error);
        }
    }

    fn show_emulators(&mut self, ui: &mut egui::Ui, uses: &[EmulatorFirmwareUse]) {
        if uses.is_empty() {
            return;
        }
        ui.strong("Emulators");
        for use_ in uses {
            ui.label(emulator_summary(use_));
        }
        ui.small("This is firmware evidence only. Launch still runs its own readiness checks before starting a game.");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open Setup & Doctor").clicked() {
                self.navigation = Some(FirmwareNavigation::SetupDoctor);
            }
            if ui.button("Open Emulator setup").clicked() {
                self.navigation = Some(FirmwareNavigation::EmulatorSetup);
            }
        });
    }
}

fn catalogue_status(catalogue: &ImportedCatalogue) -> String {
    let provenance = &catalogue.snapshot.provenance;
    let signature = match provenance.signature_state {
        RetroBiosSignatureState::Verified => "signature reported verified",
        RetroBiosSignatureState::Present => "signature present, not checked",
        RetroBiosSignatureState::Absent => "no signature",
        RetroBiosSignatureState::Unknown => "signature unknown",
    };
    format!(
        "RetroBIOS metadata imported for this session: {} · ref {} · snapshot {} · {} records · {signature}{}",
        catalogue.path.display(),
        provenance.upstream_ref,
        provenance.snapshot_timestamp,
        catalogue.snapshot.items.len(),
        if provenance.mirror_used {
            " · fetched through a mirror"
        } else {
            ""
        }
    )
}

fn installation_label(installation: &EmulatorInstallation) -> String {
    match installation {
        EmulatorInstallation::Installed { versions } => {
            format!("installed ({})", versions.join(", "))
        }
        EmulatorInstallation::NotFound => "not found by the emulator inventory".into(),
        EmulatorInstallation::NotScanned => "installation not checked yet".into(),
        EmulatorInstallation::NotTracked => "installation not tracked by EmuWiz".into(),
    }
}

fn emulator_summary(use_: &EmulatorFirmwareUse) -> String {
    let mut text = format!(
        "{} — {}",
        use_.emulator,
        installation_label(&use_.installation)
    );
    if !use_.verified.is_empty() {
        text.push_str(&format!(". Can use verified: {}", use_.verified.join(", ")));
    }
    if !use_.blocking.is_empty() {
        text.push_str(&format!(
            ". Required but missing or mismatched: {}",
            use_.blocking.join(", ")
        ));
    }
    if use_.verified.is_empty() && use_.blocking.is_empty() {
        text.push_str(". No verified firmware yet");
    }
    text
}

pub(crate) fn state_label(state: FirmwareRowState) -> &'static str {
    match state {
        FirmwareRowState::Verified => "Verified",
        FirmwareRowState::FilenameOnly => "Filename match · unverified",
        FirmwareRowState::Incompatible => "Incompatible · hash or size differs",
        FirmwareRowState::MultipleCandidates => "Several candidates",
        FirmwareRowState::Missing => "Missing",
        FirmwareRowState::NotRequired => "Not required",
        FirmwareRowState::ExternalInstall => "Installed through the emulator",
        FirmwareRowState::NotInspected => "Not inspected",
        FirmwareRowState::Unknown => "Unknown",
    }
}

fn requiredness_label(requiredness: FirmwareRequiredness) -> &'static str {
    match requiredness {
        FirmwareRequiredness::Required => "Required",
        FirmwareRequiredness::Optional => "Optional",
        FirmwareRequiredness::HleFallback => "Optional · built-in fallback exists",
        FirmwareRequiredness::NotRequired => "Not required",
        FirmwareRequiredness::NotStated => "Requiredness not stated",
    }
}

fn state_colour(state: FirmwareRowState) -> egui::Color32 {
    match state {
        FirmwareRowState::Verified | FirmwareRowState::NotRequired => {
            egui::Color32::from_rgb(70, 150, 85)
        }
        FirmwareRowState::Incompatible | FirmwareRowState::Missing => {
            egui::Color32::from_rgb(180, 70, 55)
        }
        _ => egui::Color32::from_rgb(180, 130, 40),
    }
}

fn show_row(ui: &mut egui::Ui, row: &FirmwareRow) {
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(&row.name);
            ui.colored_label(state_colour(row.state), state_label(row.state));
            ui.label(requiredness_label(row.requiredness));
        });
        for line in row_details(row) {
            ui.small(line);
        }
        ui.label(&row.explanation);
    });
}

/// Every detail line is backed by a field the source actually supplied.
pub(crate) fn row_details(row: &FirmwareRow) -> Vec<String> {
    let mut lines = vec![match &row.source {
        FirmwareRowSource::BuiltIn => format!("{} · source: EmuWiz emulator table", row.emulator),
        FirmwareRowSource::RetroBios { source_path } => {
            format!(
                "{} · source: RetroBIOS metadata ({source_path})",
                row.emulator
            )
        }
    }];
    if row.content_class == BiosContentClass::WritableState {
        lines.push("Writable emulator state: keep it local to the emulator.".into());
    }
    if !row.accepted_filenames.is_empty() {
        lines.push(format!(
            "Accepted filenames: {}",
            row.accepted_filenames.join(", ")
        ));
    }
    let identity: Vec<String> = [
        row.region.as_ref().map(|value| format!("region {value}")),
        row.revision
            .as_ref()
            .map(|value| format!("revision {value}")),
        row.version.as_ref().map(|value| format!("version {value}")),
        row.expected_size.map(|size| format!("{size} bytes")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !identity.is_empty() {
        lines.push(format!("Expected: {}", identity.join(" · ")));
    }
    let hashes = &row.expected_hashes;
    for (name, value) in [
        ("SHA-256", &hashes.sha256),
        ("SHA-1", &hashes.sha1),
        ("MD5", &hashes.md5),
        ("CRC32", &hashes.crc32),
    ] {
        if let Some(value) = value {
            lines.push(format!("Expected {name}: {value}"));
        }
    }
    for candidate in &row.candidates {
        lines.push(format!(
            "Local: {} · {} bytes · {}",
            candidate.relative_path.display(),
            candidate.size_bytes,
            candidate
                .sha256
                .as_deref()
                .map(|digest| format!("SHA-256 {digest}"))
                .unwrap_or_else(|| "not hashed".into())
        ));
    }
    lines
}

pub(crate) fn mr_wiz_guidance(view: &PlatformFirmwareView, catalogue_loaded: bool) -> String {
    let platform = view.platform.label;
    if view.rows.is_empty() {
        return format!("I have no firmware requirements to show for {platform} yet.");
    }
    if view
        .rows
        .iter()
        .all(|row| row.state == FirmwareRowState::NotRequired)
    {
        return format!("{platform} needs no conventional BIOS with the emulators listed here.");
    }
    if view.inventory_root.is_none() {
        return format!(
            "Inspect your BIOS folder below and I'll compare it with what {platform} emulators expect."
        );
    }
    let blocking: Vec<&str> = view
        .emulators
        .iter()
        .filter(|use_| !use_.blocking.is_empty())
        .map(|use_| use_.emulator)
        .collect();
    if !blocking.is_empty() {
        return format!(
            "{} is missing required firmware or has a file that doesn't match. Add a BIOS dumped from your own console, then inspect the folder again. EmuWiz doesn't supply BIOS files.",
            blocking.join(", ")
        );
    }
    let verified: Vec<&str> = view
        .emulators
        .iter()
        .filter(|use_| !use_.verified.is_empty())
        .map(|use_| use_.emulator)
        .collect();
    if !verified.is_empty() {
        return format!(
            "I verified firmware for {}. Launch still makes its own checks before a game starts.",
            verified.join(", ")
        );
    }
    if view
        .rows
        .iter()
        .any(|row| row.state == FirmwareRowState::FilenameOnly)
    {
        return if catalogue_loaded {
            "Some files have the right names, but nothing proves their contents yet. A matching name isn't a verified BIOS.".into()
        } else {
            "Some files have the right names, but EmuWiz's table has no hashes to check them. Import RetroBIOS metadata to compare hashes.".into()
        };
    }
    if view
        .rows
        .iter()
        .any(|row| row.state == FirmwareRowState::MultipleCandidates)
    {
        return "Several matching files were found. Keep the one your emulator should use; EmuWiz won't pick for you.".into();
    }
    format!("I couldn't confirm {platform} firmware from the evidence available.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::bios_projection::{BiosEvidence, BiosEvidenceSource, BiosMatchStatus};
    use serde_json::json;
    use tempfile::tempdir;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn metadata(items: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "provider": "retrobios",
            "upstream": {
                "url": "https://github.com/Abdess/retrobios",
                "ref": "v2026.09.04",
                "snapshot_digest": DIGEST,
                "timestamp": "2026-09-20T00:00:00Z",
                "signature_state": "present",
                "mirror_used": false
            },
            "items": items
        }))
        .unwrap()
    }

    fn ps1_item() -> serde_json::Value {
        json!([{
            "filename": "scph5501.bin",
            "emulator": "duckstation",
            "platform": "RetroArch",
            "system": "sony-playstation",
            "required": true,
            "region": "US",
            "revision": "5501",
            "size": 4,
            "hashes": {"sha256": DIGEST},
            "verification_mode": "exact_hash",
            "source_path": "emulators/duckstation.yml#scph5501.bin"
        }])
    }

    fn inventory(digest: Option<&str>) -> BiosMasterInventory {
        BiosMasterInventory {
            root: "/bios".into(),
            entries: vec![BiosEvidence {
                relative_path: "scph5501.bin".into(),
                filename: "scph5501.bin".into(),
                size_bytes: 4,
                sha256: digest.map(ToOwned::to_owned),
                source: BiosEvidenceSource::Hash,
                match_status: BiosMatchStatus::Unknown,
                platform: None,
            }],
            warnings: vec![],
        }
    }

    #[test]
    fn import_accepts_metadata_and_records_provenance() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("retrobios.json");
        std::fs::write(&path, metadata(ps1_item())).unwrap();
        let mut viewer = FirmwareInformationViewer::default();
        viewer.import(&path);
        assert!(viewer.import_error.is_none());
        let status = catalogue_status(viewer.catalogue.as_ref().unwrap());
        assert!(status.contains("ref v2026.09.04"));
        assert!(status.contains("1 records"));
        assert!(status.contains("signature present, not checked"));
    }

    #[test]
    fn import_rejects_malformed_incompatible_and_archive_files() {
        let directory = tempdir().unwrap();
        let mut viewer = FirmwareInformationViewer::default();
        let good = directory.path().join("good.json");
        std::fs::write(&good, metadata(ps1_item())).unwrap();
        viewer.import(&good);
        for (name, bytes) in [
            ("broken.json", b"{not json".to_vec()),
            ("pack.zip", b"PK\x03\x04rest".to_vec()),
            (
                "other.json",
                serde_json::to_vec(&json!({"provider": "other", "schema_version": 1})).unwrap(),
            ),
            (
                "unpinned.json",
                String::from_utf8(metadata(ps1_item()))
                    .unwrap()
                    .replace("v2026.09.04", "main")
                    .into_bytes(),
            ),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            viewer.import(&path);
            assert!(viewer.import_error.is_some(), "{name} must be rejected");
            // A rejected file never replaces the catalogue already in use.
            assert_eq!(viewer.catalogue.as_ref().unwrap().path, good);
        }
        let error = {
            viewer.import(&directory.path().join("pack.zip"));
            viewer.import_error.clone().unwrap()
        };
        assert!(error.contains("never imports firmware"));
    }

    #[test]
    fn without_metadata_filename_information_is_kept() {
        let mut viewer = FirmwareInformationViewer::default();
        let inventory = inventory(Some(DIGEST));
        let view = viewer.view(Some(&inventory), 1).clone();
        let row = &view.rows[0];
        assert_eq!(row.state, FirmwareRowState::FilenameOnly);
        assert_eq!(state_label(row.state), "Filename match · unverified");
        let details = row_details(row).join("\n");
        assert!(details.contains("Accepted filenames: scph1000.bin"));
        assert!(details.contains("Local: scph5501.bin"));
        assert!(!details.contains("region"));
        assert!(!details.contains("Expected SHA-256"));
        assert!(mr_wiz_guidance(&view, false).contains("Import RetroBIOS metadata"));
    }

    #[test]
    fn imported_metadata_verifies_only_matching_hashes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("retrobios.json");
        std::fs::write(&path, metadata(ps1_item())).unwrap();
        let mut viewer = FirmwareInformationViewer::default();
        viewer.import(&path);
        let verified = viewer.view(Some(&inventory(Some(DIGEST))), 1).clone();
        let row = verified.rows.last().unwrap();
        assert_eq!(row.state, FirmwareRowState::Verified);
        let details = row_details(row).join("\n");
        assert!(details.contains("region US · revision 5501"));
        assert!(details.contains(&format!("Expected SHA-256: {DIGEST}")));
        assert!(mr_wiz_guidance(&verified, true).contains("verified firmware for DuckStation"));
        let unhashed = viewer.view(Some(&inventory(None)), 2).clone();
        assert_eq!(
            unhashed.rows.last().unwrap().state,
            FirmwareRowState::FilenameOnly
        );
    }

    #[test]
    fn platform_switch_and_new_evidence_never_show_stale_rows() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("retrobios.json");
        std::fs::write(&path, metadata(ps1_item())).unwrap();
        let mut viewer = FirmwareInformationViewer::default();
        viewer.import(&path);
        let inventory = inventory(Some(DIGEST));
        assert_eq!(viewer.view(Some(&inventory), 1).platform.key, "ps1");
        viewer.select_platform("psp");
        let psp = viewer.view(Some(&inventory), 1).clone();
        assert_eq!(psp.platform.key, "psp");
        assert!(psp.rows.iter().all(|row| row.emulator == "PPSSPP"));
        viewer.select_platform("ps1");
        // A newer folder inspection replaces the cached rows.
        assert_eq!(
            viewer.view(None, 2).rows[0].state,
            FirmwareRowState::NotInspected
        );
        // The imported file changing on disk is flagged on the next switch.
        std::fs::write(&path, b"{}{}").unwrap();
        viewer.select_platform("ps2");
        assert!(viewer.catalogue.as_ref().unwrap().stale);
        viewer.remove_catalogue();
        assert!(viewer.view(Some(&inventory), 2).provenance.is_none());
    }

    #[test]
    fn navigation_requests_are_taken_once_and_installation_is_mirrored() {
        let mut viewer = FirmwareInformationViewer {
            navigation: Some(FirmwareNavigation::SetupDoctor),
            ..Default::default()
        };
        assert_eq!(
            viewer.take_navigation(),
            Some(FirmwareNavigation::SetupDoctor)
        );
        assert_eq!(viewer.take_navigation(), None);
        let inventory = EmulatorInventory::default();
        viewer.set_installed(Some(&inventory));
        let generation = viewer.installed_generation;
        viewer.set_installed(Some(&inventory));
        assert_eq!(viewer.installed_generation, generation);
        let view = viewer.view(Some(&self::inventory(Some(DIGEST))), 1).clone();
        assert_eq!(
            emulator_summary(&view.emulators[0]),
            "DuckStation — not found by the emulator inventory. No verified firmware yet"
        );
    }

    #[test]
    fn guidance_never_offers_downloads() {
        let mut viewer = FirmwareInformationViewer::default();
        for platform in FIRMWARE_PLATFORMS {
            viewer.select_platform(platform.key);
            for (generation, inventory) in [(1, None), (2, Some(inventory(None)))] {
                let view = viewer.view(inventory.as_ref(), generation).clone();
                let text = mr_wiz_guidance(&view, false).to_lowercase();
                assert!(
                    !text.contains("http") && !text.contains("download"),
                    "{text}"
                );
            }
        }
    }
}
