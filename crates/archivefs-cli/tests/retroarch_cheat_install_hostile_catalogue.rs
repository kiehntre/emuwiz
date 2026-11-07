//! Target path: crates/archivefs-cli/tests/retroarch_cheat_install_hostile_catalogue.rs
//!
//! NOT EXECUTED: needs a build (disk below the cargo-iso floor during Night Watch P3).
//! The same scenarios WERE executed against the baseline CLI binary
//! (09bb453) with the shell repro in regressions/D7_repro_install.sh;
//! recorded output in regressions/D7_repro_install.out.
//!
//! Asserts:
//!  * D7-01: installing from a JSON manifest must not write the manifest's raw
//!    JSON bytes as `<game>.cht`; the written file must be a RetroArch .cht
//!    document that mentions only that game's cheats (or the entry must be
//!    refused). Fails at baseline: file bytes == manifest bytes.
//!  * D7-02: the path written must equal the previewed `proposed_destination_path`;
//!    names with leading/trailing whitespace or control characters must not
//!    produce a different (unreviewed) file name. Fails at baseline:
//!    ` Beta Run (USA).cht` and `Alpha Quest (USA)\n.cht` are written while the
//!    preview showed the trimmed names.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in [
            "home",
            "config/emuwiz",
            "data",
            "cache",
            "state",
            "tmp",
            "lib/snes",
            "mnt",
        ] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(
            root.join("config/emuwiz/config.toml"),
            format!(
                "mount_root = {:?}\nratarmount_bin = \"ratarmount\"\n\n[[source]]\npath = {:?}\nenabled = true\n",
                root.join("mnt"),
                root.join("lib/snes")
            ),
        )
        .unwrap();
        Env { _dir: dir, root }
    }
    fn cli(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"));
        let r = &self.root;
        c.env("HOME", r.join("home"))
            .env("XDG_CONFIG_HOME", r.join("config"))
            .env("XDG_DATA_HOME", r.join("data"))
            .env("XDG_CACHE_HOME", r.join("cache"))
            .env("XDG_STATE_HOME", r.join("state"))
            .env("TMPDIR", r.join("tmp"))
            .current_dir(r);
        c
    }
}

fn crc32(b: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &x in b {
        crc ^= u32::from(x);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Minimal single-member stored ZIP (no compression), enough for library-scan.
fn write_stored_zip(path: &Path, member: &str, data: &[u8]) {
    let name = member.as_bytes();
    let crc = crc32(data);
    let mut z = Vec::new();
    z.extend(0x0403_4b50u32.to_le_bytes());
    z.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(name);
    z.extend(data);
    let cd = z.len() as u32;
    z.extend(0x0201_4b50u32.to_le_bytes());
    z.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend([0u8; 12]);
    z.extend(0u32.to_le_bytes());
    z.extend(name);
    let cd_len = z.len() as u32 - cd;
    z.extend(0x0605_4b50u32.to_le_bytes());
    z.extend([0, 0, 0, 0, 1, 0, 1, 0]);
    z.extend(cd_len.to_le_bytes());
    z.extend(cd.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    fs::write(path, z).unwrap();
}

fn library(env: &Env, titles: &[&str]) {
    for t in titles {
        let mut rom = vec![0u8; 0x8000];
        rom.extend(t.as_bytes());
        write_stored_zip(
            &env.root.join(format!("lib/snes/{t}.zip")),
            &format!("{t}.sfc"),
            &rom,
        );
    }
    let out = env.cli().arg("library-scan").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn manifest(env: &Env, names: &[&str]) -> PathBuf {
    let games: Vec<_> = names
        .iter()
        .map(|n| {
            serde_json::json!({
                "game_name": n,
                "platform": "Nintendo - Super Nintendo Entertainment System",
                "cheats": [{"code": "7E0000:FF", "description": format!("only for {}", n.trim())}]
            })
        })
        .collect();
    let path = env.root.join("manifest.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"source_name": "t", "games": games})).unwrap(),
    )
    .unwrap();
    path
}

fn install_json(env: &Env, catalogue: &Path, dest: &Path) -> serde_json::Value {
    let out = env
        .cli()
        .args(["retroarch-cheat-install"])
        .arg(catalogue)
        .arg("--cheat-destination-root")
        .arg(dest)
        .args(["--yes", "--json"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).expect("install --json output")
}

#[test]
fn manifest_install_writes_a_per_game_cht_document_not_the_manifest() {
    let env = Env::new();
    library(&env, &["Alpha Quest (USA)", "Delta (USA)"]);
    let m = manifest(&env, &["Alpha Quest (USA)", "Delta (USA)"]);
    let dest = env.root.join("dest");
    let run = install_json(&env, &m, &dest);
    let manifest_bytes = fs::read(&m).unwrap();
    for entry in run["entries"].as_array().unwrap() {
        if entry["applied"] != true {
            continue;
        }
        let path = PathBuf::from(entry["destination_path"]["display"].as_str().unwrap());
        let written = fs::read(&path).unwrap();
        assert_ne!(
            written,
            manifest_bytes,
            "D7-01: raw manifest JSON written as {}",
            path.display()
        );
        let text = String::from_utf8(written).unwrap();
        assert!(
            text.lines().any(|line| line.starts_with("cheats = ")),
            "D7-01: not a .cht document: {text:.80}"
        );
        assert!(
            !text.trim_start().starts_with('{'),
            "D7-01: raw JSON written: {text:.80}"
        );
        let other = if path.ends_with("Delta (USA).cht") {
            "Alpha Quest"
        } else {
            "Delta"
        };
        assert!(
            !text.contains(other),
            "D7-01: another game's cheats leaked into {}",
            path.display()
        );
    }
}

#[test]
fn installed_path_equals_previewed_path_for_whitespace_and_control_names() {
    let env = Env::new();
    library(
        &env,
        &["Beta Run (USA)", "Alpha Quest (USA)", "Gamma (USA)"],
    );
    let m = manifest(
        &env,
        &[" Beta Run (USA)", "Alpha Quest (USA)\n", "Gamma (USA)\u{1}"],
    );
    let dest = env.root.join("dest");
    let preview = env
        .cli()
        .arg("retroarch-cheat-catalogue")
        .arg(&m)
        .arg("--cheat-destination-root")
        .arg(&dest)
        .arg("--json")
        .output()
        .unwrap();
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    let previewed: Vec<String> = preview["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| {
            e["staging_plan"]["proposed_destination_path"]["display"]
                .as_str()
                .map(str::to_string)
        })
        .collect();
    let run = install_json(&env, &m, &dest);
    for entry in run["entries"].as_array().unwrap() {
        if entry["applied"] != true {
            continue;
        }
        let written = entry["destination_path"]["display"].as_str().unwrap();
        assert!(
            previewed.iter().any(|p| p == written),
            "D7-02: wrote unreviewed path {written:?}; preview had {previewed:?}"
        );
        assert!(
            !written.chars().any(char::is_control),
            "D7-02: control character in written name {written:?}"
        );
    }
}

type CheatSpec<'a> = (&'a str, Option<&'a str>);
type GameSpec<'a> = (&'a str, &'a [CheatSpec<'a>]);

fn manifest_with_codes(env: &Env, games: &[GameSpec<'_>]) -> PathBuf {
    let games: Vec<_> = games
        .iter()
        .map(|(name, cheats)| {
            let cheats: Vec<_> = cheats
                .iter()
                .map(|(description, code)| match code {
                    Some(code) => serde_json::json!({"description": description, "code": code}),
                    None => serde_json::json!({"description": description}),
                })
                .collect();
            serde_json::json!({
                "game_name": name,
                "platform": "Nintendo - Super Nintendo Entertainment System",
                "cheats": cheats
            })
        })
        .collect();
    let path = env.root.join("manifest-codes.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"source_name": "t", "games": games})).unwrap(),
    )
    .unwrap();
    path
}

/// Owner decision 2026-11-06: only cheats with a code are installed; a
/// description-only game is reported as skipped (never installed, never a
/// misleading success) and a mixed game installs only its coded cheat.
#[test]
fn description_only_games_are_skipped_and_mixed_games_install_only_coded_cheats() {
    let env = Env::new();
    library(&env, &["Alpha Quest (USA)", "Beta Run (USA)"]);
    let m = manifest_with_codes(
        &env,
        &[
            (
                "Alpha Quest (USA)",
                &[("Coded one", Some("7E0000:FF")), ("Description only", None)],
            ),
            ("Beta Run (USA)", &[("Only a description", None)]),
        ],
    );
    let dest = env.root.join("dest");
    let run = install_json(&env, &m, &dest);
    let entries = run["entries"].as_array().unwrap();
    let alpha = entries
        .iter()
        .find(|e| {
            e["destination_path"]["display"]
                .as_str()
                .is_some_and(|p| p.ends_with("Alpha Quest (USA).cht"))
        })
        .expect("Alpha Quest is installed");
    assert_eq!(alpha["applied"], true);
    let text = fs::read_to_string(alpha["destination_path"]["display"].as_str().unwrap()).unwrap();
    assert!(
        text.starts_with("# EmuWiz") || text.trim_start().starts_with("cheats"),
        "{text:.80}"
    );
    assert!(text.contains("cheats = 1\n"), "{text}");
    assert!(text.contains("7E0000:FF"));
    assert!(!text.contains("Description only"));
    let beta = entries
        .iter()
        .find(|e| {
            e["source_path"]["display"].as_str().is_some()
                && e["reason_code"] == "no_installable_cheats"
        })
        .expect("the description-only game is reported with its reason");
    assert_eq!(beta["applied"], false);
    assert_eq!(beta["outcome"], "skipped_not_eligible");
    assert!(!dest.join("SNES").join("Beta Run (USA).cht").exists());
    assert!(
        fs::read_dir(dest.join("SNES")).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.')),
        "no temporary residue"
    );

    // The read-only preview says so in words too.
    let out = env
        .cli()
        .arg("retroarch-cheat-catalogue")
        .arg(&m)
        .arg("--cheat-destination-root")
        .arg(&dest)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Skipped: no usable cheat codes: 1 game(s)"),
        "{text}"
    );
}
