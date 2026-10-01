//! Structural guard for the one-authority-path rule.
//!
//! There must be exactly one place that can write a source binding after first
//! capture, exactly one place that can write Missing evidence, and that place must
//! decide authority itself. These tests read the production sources so a future
//! second path (a new binding writer, a Missing write that skips the assertion, a
//! second public rebind) fails here instead of silently bypassing the model.

use std::fs;
use std::path::{Path, PathBuf};

fn production_sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                // Test modules may fabricate state; production code may not.
                if !name.contains("tests") {
                    let text = fs::read_to_string(&path).unwrap();
                    // Drop an in-file `#[cfg(test)] mod tests { ... }` tail (always last).
                    let text = match text.rfind(
                        "
#[cfg(test)]
mod tests {",
                    ) {
                        Some(cut) => text[..cut].to_string(),
                        None => text,
                    };
                    out.push((path, text));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} not found"));
    let rest = &source[start..];
    // The function ends at its first line that is exactly the closing brace at the
    // signature's own indentation.
    let indent: String = source[..start]
        .rsplit('\n')
        .next()
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default();
    let end = rest.find(&format!("\n{indent}}}\n")).expect("function end");
    &rest[..end]
}

#[test]
fn source_bindings_are_written_only_by_the_single_rebind_writer() {
    let mut writers = Vec::new();
    for (path, text) in production_sources() {
        for needle in ["INTO source_scan_bindings", "UPDATE source_scan_bindings"] {
            for (index, _) in text.match_indices(needle) {
                writers.push((path.clone(), needle, index, text.clone()));
            }
        }
    }
    assert_eq!(
        writers.len(),
        2,
        "exactly one INSERT and one UPDATE may exist"
    );
    for (path, needle, index, text) in writers {
        assert!(
            path.ends_with("database/catalogue_health.rs"),
            "{needle} in {path:?}"
        );
        let body = function_body(&text, "fn write_rebind(");
        let start = text.find("fn write_rebind(").unwrap();
        assert!(
            index > start && index < start + body.len(),
            "{needle} is outside write_rebind"
        );
    }
}

#[test]
fn every_binding_write_goes_through_write_rebind_and_allocates_a_checked_generation() {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/database/catalogue_health.rs"),
    )
    .unwrap();
    let writer = function_body(&text, "fn write_rebind(");
    assert!(
        writer.contains("next_source_generation("),
        "write_rebind must allocate"
    );
    let allocator = function_body(&text, "pub(super) fn next_source_generation(");
    assert!(
        allocator.contains("checked_add(1)"),
        "generation allocation must be checked"
    );
    assert!(
        allocator.contains("MAX(source_generation)"),
        "must look above all coverage"
    );
    // Every caller that changes a binding routes through the single writer.
    assert_eq!(
        text.matches("write_rebind(").count(),
        4,
        "definition + 3 call sites"
    );
    for caller in [
        "pub(super) fn bind_scan_source(",
        "pub fn rebind_source_after_review(",
        "pub fn confirm_source_rebind(",
    ] {
        assert!(
            function_body(&text, caller).contains("write_rebind("),
            "{caller}"
        );
    }
}

#[test]
fn missing_evidence_has_one_writer_and_it_decides_authority_itself() {
    let mut writers = 0;
    for (_, text) in production_sources() {
        writers += text.matches("SET last_verified_missing_at = ?2").count();
        writers += text.matches("SET last_verified_missing_at=?2").count();
    }
    assert_eq!(
        writers, 1,
        "exactly one statement may mark an archive missing"
    );
    let database =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/database.rs")).unwrap();
    let body = function_body(&database, "    pub fn mark_unseen_archives_missing(");
    assert!(
        body.contains("SET last_verified_missing_at = ?2"),
        "the one writer lives here"
    );
    // Before reading candidates, inside the write transaction, and before commit.
    assert!(
        body.matches("assert_missing_authority(").count() >= 3,
        "the authority assertion must run at the boundary, in the transaction, and before commit"
    );
    let first_assert = body.find("assert_missing_authority(").unwrap();
    let write = body.find("SET last_verified_missing_at = ?2").unwrap();
    assert!(
        first_assert < write,
        "authority is established before any write"
    );
}

#[test]
fn there_is_no_second_public_rebind_or_binding_entry_point() {
    let mut public = Vec::new();
    for (_, text) in production_sources() {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("pub fn ") && line.contains("rebind") {
                public.push(line.split('(').next().unwrap().trim().to_string());
            }
        }
    }
    public.sort();
    assert_eq!(
        public,
        [
            "pub fn confirm_source_rebind",
            "pub fn rebind_source_after_review",
            "pub fn rebind_source_after_review_at",
            "pub fn rebind_source_after_review_default",
            "pub fn review_source_rebind",
            "pub fn review_source_rebind_at",
            "pub fn review_source_rebind_default",
        ],
        "a new public binding path needs the same review as the existing ones"
    );
}
