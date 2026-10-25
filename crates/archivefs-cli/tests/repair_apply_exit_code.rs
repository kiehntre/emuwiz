//! A repair apply that leaves an operation unresolved (its directory fsync failed
//! after the rename happened) must not exit 0 or say "complete".  Process-local
//! fault injection (LD_PRELOAD) against the real CLI; synthetic files only.
use std::path::Path;
use std::process::Command;

const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"; // sha1("test")

fn no_processes(home: &Path) -> std::path::PathBuf {
    let dir = home.join("no-processes");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn injector() -> &'static Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/repair_apply_sync_fault.c"),
            )
            .arg("-o")
            .arg(dir.path().join("fault.so"))
            .arg("-ldl")
            .output()
            .expect("the Linux fault regression requires a C compiler");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    })
    .path()
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn apply(fault: bool, json: bool) -> Run {
    let base = tempfile::tempdir().unwrap();
    let roms = base.path().join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    std::fs::write(roms.join("wrongname.bin"), b"test").unwrap();
    let dat = base.path().join("test.dat");
    std::fs::write(
        &dat,
        format!(
            "<datafile><header><name>Single</name></header><game name=\"Game\"><rom name=\"super.bin\" size=\"4\" sha1=\"{SHA1_TEST}\"/></game></datafile>"
        ),
    )
    .unwrap();
    let plan = base.path().join("plan.json");
    let home = base.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cli = env!("CARGO_BIN_EXE_emuwiz-cli");
    let scan = Command::new(cli)
        .args(["repair", "scan", "--root"])
        .arg(&roms)
        .arg("--dat")
        .arg(&dat)
        .arg("--plan-out")
        .arg(&plan)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        // An empty private process table: parallel tests' CLIs under other
        // homes must not look like older, unregistered writers.
        .env(
            archivefs_core::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
            no_processes(&home),
        )
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let generation = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&plan).unwrap())
        .unwrap()["generation"]
        .to_string();
    let mut command = Command::new(cli);
    command
        .args(["repair", "apply", "--plan"])
        .arg(&plan)
        .arg("--root")
        .arg(&roms)
        .arg("--dat")
        .arg(&dat)
        .args(["--generation", &generation, "--journal-dir"])
        .arg(base.path().join("journal"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        // An empty private process table: parallel tests' CLIs under other
        // homes must not look like older, unregistered writers.
        .env(
            archivefs_core::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
            no_processes(&home),
        );
    if json {
        command.arg("--json");
    }
    if fault {
        command
            .env("LD_PRELOAD", injector().join("fault.so"))
            .env("R3_SYNC_FAULT", &roms);
    }
    let out = command.output().unwrap();
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn control_without_a_fault_is_a_complete_success() {
    let text = apply(false, false);
    assert_eq!(text.code, Some(0), "{}\n{}", text.stdout, text.stderr);
    assert!(
        text.stdout.contains("Repair apply complete:"),
        "{}",
        text.stdout
    );
    let json = apply(false, true);
    assert_eq!(json.code, Some(0), "{}\n{}", json.stdout, json.stderr);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(value["status"], "complete");
    assert_eq!(value["complete"], true);
}

#[test]
fn text_apply_with_an_unresolved_operation_exits_nonzero_and_is_not_complete() {
    let run = apply(true, false);
    assert!(
        run.stderr.contains("FIRED"),
        "the fault must actually fire: {}",
        run.stderr
    );
    assert_eq!(run.code, Some(1), "{}\n{}", run.stdout, run.stderr);
    assert!(
        !run.stdout.contains("Repair apply complete:"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("INCOMPLETE"), "{}", run.stdout);
    assert!(run.stdout.contains("Unresolved"), "{}", run.stdout);
    assert!(
        run.stdout.contains("transaction "),
        "evidence is preserved: {}",
        run.stdout
    );
    assert!(run.stderr.contains("did not complete"), "{}", run.stderr);
}

#[test]
fn json_apply_with_an_unresolved_operation_exits_nonzero_and_says_so() {
    let run = apply(true, true);
    assert!(run.stderr.contains("FIRED"), "{}", run.stderr);
    assert_eq!(run.code, Some(1), "{}\n{}", run.stdout, run.stderr);
    let value: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(value["status"], "incomplete_recovery_required");
    assert_eq!(value["complete"], false);
    assert_eq!(value["rename"]["unresolved"], 1);
    assert_eq!(value["rename"]["applied"], 0);
}
