//! Exercises the shipped launcher before any GUI/config/database bootstrap.
#![cfg(target_os = "linux")]
archivefs_core::install_test_environment!();

use archivefs_core::manual_document::{ManualDocument, ManualLimits, pdf_render::HELPER_ARGUMENT};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn fixture(path: &Path, width: u32, height: u32) -> ManualDocument {
    let paint = "1 0 0 rg 0 0 10 10 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << >> /Contents 5 0 R >>"
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 30] /Resources << >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{paint}\nendstream", paint.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    fs::write(path, bytes).unwrap();
    ManualDocument::open(path, &ManualLimits::default()).unwrap()
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_emuwiz"));
    // Even a regression in helper dispatch cannot access real user settings.
    command
        .env_clear()
        .env("HOME", root)
        .env("EMUWIZ_CONFIG_HOME", root.join("config"))
        .env("EMUWIZ_DATA_HOME", root.join("data"))
        .arg(HELPER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn run(
    root: &Path,
    document: &ManualDocument,
    page: usize,
    dimension: u32,
) -> std::process::Output {
    let mut child = command(root).spawn().unwrap();
    let request = serde_json::to_vec(
        &serde_json::json!({"id":document.id(),"page":page,"dimension":dimension}),
    )
    .unwrap();
    child.stdin.take().unwrap().write_all(&request).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn native_helper_renders_requested_pages_without_bootstrap_or_source_writes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.pdf");
    let doc = fixture(&path, 20, 30);
    let before = fs::read(&path).unwrap();
    assert_eq!(doc.pdf_page_index().unwrap().pages.len(), 2);
    for page in 0..2 {
        let result = run(root.path(), &doc, page, 128);
        assert!(result.status.success());
        assert_eq!(&result.stdout[..8], b"EMUPDF01");
        assert_eq!(
            u32::from_le_bytes(result.stdout[8..12].try_into().unwrap()),
            40
        );
        assert_eq!(
            u32::from_le_bytes(result.stdout[12..16].try_into().unwrap()),
            60
        );
        assert_eq!(result.stdout.len(), 16 + 40 * 60 * 4);
        let red = result.stdout[16..]
            .chunks_exact(4)
            .any(|pixel| pixel == [255, 0, 0, 255]);
        assert_eq!(red, page == 0);
    }
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        1,
        "the helper must not initialize settings, databases or caches"
    );
}

#[test]
fn native_helper_bounds_huge_dimensions_and_refuses_stale_or_corrupt_sources() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.pdf");
    let doc = fixture(&path, 500_000, 500_000);
    let result = run(root.path(), &doc, 0, 64);
    assert!(result.status.success());
    assert_eq!(result.stdout.len(), 16 + 64 * 64 * 4);
    assert!(!run(root.path(), &doc, 2, 64).status.success());
    fs::write(path, b"%PDF-1.7\ncorrupt").unwrap();
    assert!(!run(root.path(), &doc, 0, 64).status.success());
}

#[test]
fn native_helper_installs_hard_memory_cpu_and_core_dump_limits_before_reading_input() {
    let root = tempfile::tempdir().unwrap();
    let mut child = command(root.path()).spawn().unwrap();
    let input = child.stdin.take().unwrap(); // Keep stdin open: the helper waits for its bounded request.
    let deadline = Instant::now() + Duration::from_secs(5);
    let success = loop {
        if let Ok(limits) = fs::read_to_string(format!("/proc/{}/limits", child.id())) {
            let has = |name: &str, soft: &str, hard: &str| {
                limits.lines().any(|line| {
                    line.strip_prefix(name).is_some_and(|values| {
                        let mut values = values.split_whitespace();
                        values.next() == Some(soft) && values.next() == Some(hard)
                    })
                })
            };
            if has("Max address space", "536870912", "536870912")
                && has("Max cpu time", "10", "10")
                && has("Max core file size", "0", "0")
            {
                break true;
            }
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(input);
    if !success {
        let _ = child.kill();
    }
    let status = child.wait().unwrap();
    assert!(success, "helper did not install the expected hard limits");
    assert!(!status.success(), "empty request must fail");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
