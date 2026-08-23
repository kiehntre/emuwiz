use std::fs;
use std::path::PathBuf;

use super::*;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("archivefs-esde-env-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn mkdir(&self, relative: &str) {
        fs::create_dir_all(self.path(relative)).unwrap();
    }

    fn env(&self) -> DiscoveryEnvironment {
        DiscoveryEnvironment {
            home: Some(self.root.clone().into_os_string()),
            explicit_bundled_systems_files: Vec::new(),
            // No AppImage search roots by default - tests that care about
            // AppImage evidence opt in explicitly, so every other test
            // never depends on (or is broken by) this behavior.
            appimage_search_roots: Vec::new(),
            explicit_root: None,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const ONE_SYSTEM_XML: &str = r#"<?xml version="1.0"?>
<systemList>
  <system>
    <name>nes</name>
    <fullname>Nintendo Entertainment System</fullname>
    <path>%ROMPATH%/nes</path>
    <extension>.nes .NES .zip .ZIP</extension>
    <command>%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/nestopia_libretro.so %ROM%</command>
    <platform>nes</platform>
    <theme>nes</theme>
  </system>
</systemList>
"#;

fn profile_by_kind<'a>(report: &'a EsDeEnvironmentReport, kind: ProfileKind) -> &'a EsDeProfile {
    report
        .profiles
        .iter()
        .find(|profile| profile.profile_kind == kind)
        .unwrap()
}

#[test]
fn discovery_fails_only_when_home_is_unset() {
    let filesystem = HostReadOnlyFilesystem;
    let env = DiscoveryEnvironment {
        home: None,
        explicit_bundled_systems_files: Vec::new(),
        appimage_search_roots: Vec::new(),
        explicit_root: None,
    };
    assert_eq!(
        discover_es_de_environment(&filesystem, &env).unwrap_err(),
        DiscoveryError::NoHome
    );
}

#[test]
fn native_profile_uses_the_single_es_de_home_directory_not_a_config_data_split() {
    let fixture = Fixture::new("native");
    fixture.mkdir("ES-DE");

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.home_directory.probe, FsProbe::PresentDirectory);
    assert_eq!(
        native.home_directory.path.display,
        fixture.path("ES-DE").display().to_string()
    );
    // The first, uncorrected revision of this module invented a
    // `~/.config/ES-DE` directory - it must never exist anywhere in this
    // module any more.
    assert!(!fixture.path(".config/ES-DE").exists());
}

#[test]
fn no_flatpak_profile_kind_exists() {
    // ES-DE itself has no official standalone Flatpak (RetroDECK is a
    // separate, third-party product) - there must be no way to even ask
    // for a Flatpak profile any more.
    let fixture = Fixture::new("no-flatpak");
    fixture.mkdir("ES-DE");
    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    assert_eq!(report.profiles.len(), 1, "only Native, no Flatpak profile");
    assert_eq!(report.profiles[0].profile_kind, ProfileKind::Native);
}

#[test]
fn explicit_root_is_only_discovered_when_supplied() {
    let fixture = Fixture::new("explicit");
    fixture.mkdir("custom-home");
    let mut env = fixture.env();

    let report_without = discover_es_de_environment(&HostReadOnlyFilesystem, &env).unwrap();
    assert!(
        !report_without
            .profiles
            .iter()
            .any(|profile| profile.profile_kind == ProfileKind::Explicit)
    );

    env.explicit_root = Some(ExplicitRoot {
        home_directory: fixture.path("custom-home"),
    });
    let report_with = discover_es_de_environment(&HostReadOnlyFilesystem, &env).unwrap();
    let explicit = profile_by_kind(&report_with, ProfileKind::Explicit);
    assert_eq!(explicit.home_directory.probe, FsProbe::PresentDirectory);
}

#[test]
fn settings_file_location_is_probed_but_never_read() {
    let fixture = Fixture::new("settings-file");
    fixture.write("ES-DE/settings/es_settings.xml", "<config/>");

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.settings_file.probe, FsProbe::PresentFile);
    assert_eq!(
        native.settings_file.path.display,
        fixture
            .path("ES-DE/settings/es_settings.xml")
            .display()
            .to_string()
    );
}

#[test]
fn valid_systems_file_is_parsed_with_every_field() {
    let fixture = Fixture::new("valid-systems");
    fixture.write("ES-DE/custom_systems/es_systems.xml", ONE_SYSTEM_XML);

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems.len(), 1);
    let system = &native.systems[0];
    assert_eq!(system.name.as_deref(), Some("nes"));
    assert_eq!(
        system.fullname.as_deref(),
        Some("Nintendo Entertainment System")
    );
    assert_eq!(system.rom_path_raw.as_deref(), Some("%ROMPATH%/nes"));
    assert_eq!(
        system.rom_path_resolution,
        PathResolutionState::ContainsUnexpandedVariable
    );
    assert!(system.rom_path_resolved.is_none());
    assert_eq!(system.extensions, vec![".nes", ".NES", ".zip", ".ZIP"]);
    assert_eq!(
        system.command.as_deref(),
        Some("%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/nestopia_libretro.so %ROM%")
    );
    assert_eq!(system.platform_tags, vec!["nes"]);
    assert_eq!(system.theme.as_deref(), Some("nes"));

    assert_eq!(native.systems_files[0].role, SystemsFileRole::Custom);
    match &native.systems_files[0].read {
        SystemsFileReadOutcome::Parsed {
            systems_found,
            truncated,
        } => {
            assert_eq!(*systems_found, 1);
            assert!(!truncated);
        }
        other => panic!("expected Parsed, got {other:?}"),
    }
}

#[test]
fn multiple_systems_are_all_parsed_in_order() {
    let fixture = Fixture::new("multi-systems");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        r#"<?xml version="1.0"?>
<systemList>
  <system><name>nes</name><path>/roms/nes</path></system>
  <system><name>snes</name><path>/roms/snes</path></system>
  <system><name>genesis</name><path>/roms/genesis</path></system>
</systemList>
"#,
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    let names: Vec<_> = native
        .systems
        .iter()
        .map(|system| system.name.as_deref().unwrap())
        .collect();
    assert_eq!(names, vec!["nes", "snes", "genesis"]);
}

#[test]
fn absolute_and_tilde_rom_paths_resolve_correctly() {
    let fixture = Fixture::new("rom-path-resolution");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        r#"<?xml version="1.0"?>
<systemList>
  <system><name>abs</name><path>/roms/abs</path></system>
  <system><name>tilde</name><path>~/roms/tilde</path></system>
  <system><name>relative</name><path>roms/relative</path></system>
  <system><name>empty</name><path></path></system>
</systemList>
"#,
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    let by_name = |name: &str| {
        native
            .systems
            .iter()
            .find(|system| system.name.as_deref() == Some(name))
            .unwrap()
    };

    let abs = by_name("abs");
    assert_eq!(abs.rom_path_resolution, PathResolutionState::Resolved);
    assert_eq!(abs.rom_path_resolved.as_ref().unwrap().display, "/roms/abs");

    let tilde = by_name("tilde");
    assert_eq!(tilde.rom_path_resolution, PathResolutionState::Resolved);
    assert_eq!(
        tilde.rom_path_resolved.as_ref().unwrap().display,
        fixture.path("roms/tilde").display().to_string()
    );

    let relative = by_name("relative");
    assert_eq!(
        relative.rom_path_resolution,
        PathResolutionState::Unresolved
    );
    assert!(relative.rom_path_resolved.is_none());

    let empty = by_name("empty");
    assert_eq!(
        empty.rom_path_resolution,
        PathResolutionState::NotConfigured
    );
    assert!(empty.rom_path_raw.is_none());
}

#[test]
fn extensions_are_whitespace_split_and_preserved_verbatim() {
    let fixture = Fixture::new("extensions");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system><name>x</name><extension>.a .B  .ccc\t.D</extension></system></systemList>",
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems[0].extensions, vec![".a", ".B", ".ccc", ".D"]);
}

#[test]
fn launch_command_is_preserved_verbatim_never_tokenized_or_executed() {
    let fixture = Fixture::new("command");
    let command = "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/some_core.so \"%ROM%\"";
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        &format!(
            "<systemList><system><name>x</name><command>{command}</command></system></systemList>"
        ),
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems[0].command.as_deref(), Some(command));
}

#[test]
fn truncated_xml_yields_a_diagnostic_and_keeps_already_parsed_systems() {
    let fixture = Fixture::new("truncated-xml");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system><name>good</name></system><system><name>unterminated",
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems.len(), 1);
    assert_eq!(native.systems[0].name.as_deref(), Some("good"));
    assert!(
        native
            .diagnostics
            .iter()
            .any(|d| d.code == "systems_file_unclosed_element_at_eof")
    );
}

#[test]
fn malformed_xml_yields_a_diagnostic_and_keeps_already_parsed_systems() {
    let fixture = Fixture::new("malformed-xml");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system><name>good</name></system><system><name>bad</name></wrongtag></systemList>",
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems.len(), 1);
    assert_eq!(native.systems[0].name.as_deref(), Some("good"));
    assert!(
        native
            .diagnostics
            .iter()
            .any(|d| d.code == "systems_file_malformed_xml")
    );
}

#[test]
fn missing_custom_systems_file_is_handled_safely_with_no_systems_and_no_panic() {
    let fixture = Fixture::new("missing-config");
    fixture.mkdir("ES-DE");

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert!(native.systems.is_empty());
    assert_eq!(
        native.systems_files[0].read,
        SystemsFileReadOutcome::NotFound
    );
}

#[test]
fn missing_custom_systems_file_alone_never_implies_es_de_has_no_systems() {
    // `custom_systems/es_systems.xml` complements the bundled default
    // list; it is not the whole story. Absence of the custom file must
    // never be read as "ES-DE has no systems configured" - the
    // `systems_may_be_incomplete` flag is the explicit, structural way
    // this adapter says so.
    let fixture = Fixture::new("no-custom-no-bundled");
    fixture.mkdir("ES-DE");

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert!(native.systems.is_empty());
    assert!(
        native.systems_may_be_incomplete,
        "with no bundled-role file read, the (empty) systems list must be flagged incomplete"
    );
}

#[test]
fn explicit_bundled_systems_file_is_tagged_and_clears_the_incomplete_flag() {
    let fixture = Fixture::new("bundled-explicit");
    fixture.write("bundled/es_systems.xml", ONE_SYSTEM_XML);
    let mut env = fixture.env();
    env.explicit_bundled_systems_files = vec![fixture.path("bundled/es_systems.xml")];

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &env).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    // custom_systems/es_systems.xml (NotFound, role Custom) + the one
    // explicit bundled-role file.
    assert_eq!(native.systems_files.len(), 2);
    assert_eq!(native.systems_files[0].role, SystemsFileRole::Custom);
    assert_eq!(native.systems_files[1].role, SystemsFileRole::Bundled);
    assert_eq!(native.systems.len(), 1);
    assert!(!native.systems_may_be_incomplete);
}

#[test]
fn gamelist_and_media_locations_are_discovered_per_system() {
    let fixture = Fixture::new("gamelist-media");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system><name>nes</name></system></systemList>",
    );
    fixture.write("ES-DE/gamelists/nes/gamelist.xml", "<gameList/>");
    fixture.mkdir("ES-DE/downloaded_media/nes");

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.gamelists_directory.probe, FsProbe::PresentDirectory);
    assert_eq!(native.media_root_directory.probe, FsProbe::PresentDirectory);
    assert_eq!(native.system_data.len(), 1);
    assert_eq!(native.system_data[0].system_name, "nes");
    assert_eq!(
        native.system_data[0].gamelist_file.probe,
        FsProbe::PresentFile
    );
    assert_eq!(
        native.system_data[0].media_directory.probe,
        FsProbe::PresentDirectory
    );
}

#[test]
fn a_system_with_no_discovered_gamelist_or_media_is_reported_missing_not_an_error() {
    let fixture = Fixture::new("gamelist-missing");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system><name>snes</name></system></systemList>",
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.system_data[0].gamelist_file.probe, FsProbe::Missing);
    assert_eq!(
        native.system_data[0].media_directory.probe,
        FsProbe::Missing
    );
}

#[test]
fn appimage_evidence_is_discovered_from_the_fixed_bounded_search_roots() {
    let fixture = Fixture::new("appimage-evidence");
    fixture.mkdir("ES-DE");
    fixture.write("Applications/ES-DE-x64.AppImage", "not a real appimage");
    fixture.write("Applications/unrelated-app.AppImage", "not ES-DE");
    let mut env = fixture.env();
    env.appimage_search_roots = vec![fixture.path("Applications")];

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &env).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.appimage_candidates.len(), 1);
    assert!(
        native.appimage_candidates[0]
            .path
            .display
            .contains("ES-DE-x64.AppImage")
    );
}

#[test]
fn appimage_evidence_is_never_extracted_or_executed() {
    // The candidate file's content is deliberately garbage - if this
    // module ever tried to mount/extract/execute it, this test would
    // fail loudly instead of passing.
    let fixture = Fixture::new("appimage-not-executed");
    fixture.mkdir("ES-DE");
    fixture.write(
        "Applications/ES-DE.AppImage",
        "definitely not a valid AppImage",
    );
    let mut env = fixture.env();
    env.appimage_search_roots = vec![fixture.path("Applications")];

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &env).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.appimage_candidates.len(), 1);
    assert_eq!(native.appimage_candidates[0].probe, FsProbe::PresentFile);
}

#[test]
fn es_de_names_and_platform_tags_never_become_canonical_platform_identity() {
    // Every string field is set to a real, canonical-looking EmuWiz
    // platform name on purpose. This module must still only ever expose
    // them as opaque downstream strings - there is no platform-resolution
    // step anywhere in this module for them to silently feed.
    let fixture = Fixture::new("not-platform-authority");
    fixture.write(
        "ES-DE/custom_systems/es_systems.xml",
        "<systemList><system>\
           <name>Nintendo Entertainment System</name>\
           <fullname>Nintendo Entertainment System</fullname>\
           <platform>Nintendo Entertainment System</platform>\
           <theme>Nintendo Entertainment System</theme>\
         </system></systemList>",
    );

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    let system = &native.systems[0];
    assert_eq!(
        system.name.as_deref(),
        Some("Nintendo Entertainment System")
    );
    assert_eq!(system.platform_tags, vec!["Nintendo Entertainment System"]);
    assert_eq!(
        system.theme.as_deref(),
        Some("Nintendo Entertainment System")
    );

    // Structural guarantee: nothing in the whole report ever grows any
    // *other* "platform"-named field for this module to have silently
    // promoted a value into. `platform_tags` is the one reviewed,
    // intentionally-named exception - it explicitly documents (see
    // `EsDeSystemFinding`) that it holds ES-DE's own raw, unresolved tag
    // strings, never a canonical EmuWiz platform.
    fn assert_no_platform_keys(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, nested) in map {
                    assert!(
                        key == "platform_tags" || !key.to_ascii_lowercase().contains("platform"),
                        "no field in this module's output may ever be named/reference \
                         \"platform\" other than the reviewed `platform_tags`: found key {key:?}"
                    );
                    assert_no_platform_keys(nested);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(assert_no_platform_keys),
            _ => {}
        }
    }
    assert_no_platform_keys(&serde_json::to_value(&report).unwrap());
}

#[test]
fn discovery_makes_no_filesystem_writes() {
    let fixture = Fixture::new("no-writes");
    fixture.write("ES-DE/custom_systems/es_systems.xml", ONE_SYSTEM_XML);
    let before: Vec<_> = walk(&fixture.root);

    let filesystem = HostReadOnlyFilesystem;
    let _ = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();

    let after: Vec<_> = walk(&fixture.root);
    assert_eq!(
        before, after,
        "discovery must never create or modify a file"
    );
}

#[test]
fn explicit_bundled_systems_files_never_scans_the_containing_directory() {
    // A second, unrelated `.xml` file sits right next to the one
    // explicitly supplied - discovery must never notice it, because this
    // adapter never lists a directory to find `es_systems.xml`-shaped
    // files; it only ever probes the exact paths it was told about.
    let fixture = Fixture::new("no-recursive-scan");
    fixture.write("portable/es_systems.xml", ONE_SYSTEM_XML);
    fixture.write(
        "portable/unrelated_other_file.xml",
        "<systemList><system><name>should-never-be-read</name></system></systemList>",
    );
    let mut env = fixture.env();
    env.explicit_bundled_systems_files = vec![fixture.path("portable/es_systems.xml")];

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &env).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert_eq!(native.systems.len(), 1);
    assert_eq!(native.systems[0].name.as_deref(), Some("nes"));
}

#[test]
fn appimage_search_roots_are_each_listed_non_recursively_only() {
    // A subdirectory under a search root must never be descended into -
    // an AppImage placed inside it must never be discovered.
    let fixture = Fixture::new("appimage-no-recursion");
    fixture.mkdir("ES-DE");
    fixture.write("Applications/nested/ES-DE.AppImage", "not a real appimage");
    let mut env = fixture.env();
    env.appimage_search_roots = vec![fixture.path("Applications")];

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &env).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert!(native.appimage_candidates.is_empty());
}

#[test]
fn oversized_systems_file_is_reported_and_not_partially_trusted() {
    let fixture = Fixture::new("oversized");
    let oversized = "x".repeat(MAX_SYSTEMS_XML_BYTES + 1);
    fixture.write("ES-DE/custom_systems/es_systems.xml", &oversized);

    let filesystem = HostReadOnlyFilesystem;
    let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
    let native = profile_by_kind(&report, ProfileKind::Native);
    assert!(native.systems.is_empty());
    assert_eq!(
        native.systems_files[0].read,
        SystemsFileReadOutcome::TooLarge {
            limit_bytes: MAX_SYSTEMS_XML_BYTES as u64
        }
    );
}

#[test]
fn systems_file_symlink_is_not_followed() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("systems-symlink");
        fixture.write("real_systems.xml", ONE_SYSTEM_XML);
        fixture.mkdir("ES-DE/custom_systems");
        symlink(
            fixture.path("real_systems.xml"),
            fixture.path("ES-DE/custom_systems/es_systems.xml"),
        )
        .unwrap();

        let filesystem = HostReadOnlyFilesystem;
        let report = discover_es_de_environment(&filesystem, &fixture.env()).unwrap();
        let native = profile_by_kind(&report, ProfileKind::Native);
        assert!(native.systems.is_empty());
        assert_eq!(native.systems_files[0].probe, FsProbe::Symlink);
        assert!(
            native
                .diagnostics
                .iter()
                .any(|d| d.code == "systems_file_symlink_not_followed")
        );
    }
}

fn walk(root: &std::path::Path) -> Vec<(PathBuf, bool)> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path.clone());
            }
            entries.push((path, is_dir));
        }
    }
    entries.sort();
    entries
}
