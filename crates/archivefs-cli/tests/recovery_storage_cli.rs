//! `recovery storage --set` end to end, in a subprocess with disposable
//! roots. No real application folder is used.
//!
//! The accepting test names the disk by the UUID that `findmnt` reports for
//! the filesystem the temporary folder is on: a real UUID, read by a tool
//! that is not EmuWiz. It runs only where that filesystem is ext4; elsewhere
//! it says so and is skipped, because EmuWiz refuses every other filesystem
//! and there is nothing to accept. CI points TMPDIR at an ext4 image and sets
//! EMUWIZ_R2_REQUIRE_QUALIFIED=1, with which a skip is a failure.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Some other disk's UUID: of no filesystem on this machine.
const ANOTHER_DISK: &str = "6f1d2c3b-4a59-4e68-9778-a6b5c4d3e2f1";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        for folder in ["bin", "data", "config", "disk"] {
            let path = fixture.root.path().join(folder);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
                .unwrap();
        }
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        fs::canonicalize(self.root.path()).unwrap().join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", self.path("bin"))
            .env("EMUWIZ_DATA_HOME", self.path("data"))
            .env("EMUWIZ_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("TMPDIR", self.path("tmp"))
            .current_dir(self.root.path())
            .args(args)
            .output()
            .unwrap()
    }

    /// Everything in the fixture apart from the four empty folders it starts
    /// with: a refused command must leave all of it alone.
    fn written(&self) -> Vec<PathBuf> {
        fn walk(path: &Path, found: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(path).into_iter().flatten().flatten() {
                found.push(entry.path());
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(&entry.path(), found);
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.path(""), &mut found);
        found.retain(|path| {
            !["bin", "data", "config", "disk"]
                .iter()
                .any(|start| *path == self.path(start))
        });
        found
    }

    /// The UUID of the ext4 filesystem the fixture is on, or `None` when it
    /// is on another kind of filesystem or `findmnt` cannot say.
    fn real_ext4_uuid(&self) -> Option<String> {
        let answer = Command::new("findmnt")
            .args(["-n", "-o", "FSTYPE,UUID", "-T"])
            .arg(self.path("disk"))
            .output()
            .ok()?;
        let text = String::from_utf8(answer.stdout).ok()?;
        let mut fields = text.split_whitespace();
        (fields.next()? == "ext4")
            .then(|| fields.next().map(str::to_string))
            .flatten()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_folder_is_not_chosen_without_its_disk_or_with_a_wrong_one() {
    let fixture = Fixture::new();
    let folder = fixture.path("disk").join("prepared");
    let folder = folder.to_str().unwrap();
    let refusals: [(&[&str], &[&str]); 4] = [
        // No disk named.
        (
            &["recovery", "storage", "--set", folder],
            &["Say which disk"],
        ),
        // A disk named without a folder.
        (
            &["recovery", "storage", "--filesystem", ANOTHER_DISK],
            &["--filesystem"],
        ),
        // Not a UUID.
        (
            &[
                "recovery",
                "storage",
                "--set",
                folder,
                "--filesystem",
                "my-disk",
            ],
            &["UUID"],
        ),
        // Another disk's UUID: refused whatever this folder is on (on ext4
        // because it is another disk, elsewhere because it is not ext4).
        (
            &[
                "recovery",
                "storage",
                "--set",
                folder,
                "--filesystem",
                ANOTHER_DISK,
            ],
            &[
                "is not on the disk with filesystem UUID",
                "is not on an ext4 filesystem",
            ],
        ),
    ];
    for (command, reasons) in refusals {
        let output = fixture.run(command);
        let said = text(&output);
        assert!(!output.status.success(), "{command:?} was accepted: {said}");
        assert!(
            reasons.iter().any(|reason| said.contains(reason)),
            "{command:?}: {said}"
        );
        assert_eq!(
            fixture.written(),
            Vec::<PathBuf>::new(),
            "{command:?} wrote something"
        );
    }
}

#[test]
fn a_folder_is_chosen_on_the_disk_named_by_its_real_uuid_and_reset_again() {
    let fixture = Fixture::new();
    let Some(uuid) = fixture.real_ext4_uuid() else {
        assert!(
            std::env::var_os("EMUWIZ_R2_REQUIRE_QUALIFIED").is_none_or(|value| value != "1"),
            "findmnt names no ext4 filesystem for the temporary folder, and this run requires one"
        );
        eprintln!(
            "SKIPPED: the temporary folder is not on an ext4 filesystem that findmnt can name, so there is no disk EmuWiz would accept here"
        );
        return;
    };
    let folder = fixture.path("disk").join("prepared");
    let setting = fixture.path("config").join("prepared_media_folder.txt");
    let record = fixture
        .path("config")
        .join("prepared_media_folder_filesystems.json");

    let chosen = fixture.run(&[
        "recovery",
        "storage",
        "--set",
        folder.to_str().unwrap(),
        "--filesystem",
        &uuid.to_uppercase(),
    ]);
    assert!(chosen.status.success(), "{}", text(&chosen));
    assert!(folder.is_dir(), "the folder was not created");
    assert_eq!(
        fs::read_to_string(&setting).unwrap().trim(),
        folder.to_str().unwrap()
    );
    // The record names the disk by the UUID that was given, in its usual form.
    assert!(
        fs::read_to_string(&record).unwrap().contains(&uuid),
        "{uuid}"
    );

    let shown = fixture.run(&["recovery", "storage"]);
    assert!(shown.status.success(), "{}", text(&shown));
    assert!(
        text(&shown).contains(&format!("{} (your choice)", folder.display())),
        "{}",
        text(&shown)
    );

    let reset = fixture.run(&["recovery", "storage", "--reset"]);
    assert!(reset.status.success(), "{}", text(&reset));
    assert!(!setting.exists(), "the setting is still there");
    assert!(text(&fixture.run(&["recovery", "storage"])).contains("(default)"));
}
