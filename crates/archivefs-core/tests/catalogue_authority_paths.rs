//! Narrow structural alarm for catalogue authority paths (not a SQL parser).
//! It scans normalized source text, so comments/literals can false-positive and
//! dynamically assembled SQL can evade it; canonical database paths provide safety.

use std::fs;
use std::path::{Path, PathBuf};

fn production_sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.file_name().unwrap().to_string_lossy();
                if !name.contains("tests") {
                    let text = fs::read_to_string(&path).unwrap();
                    let text = match text.rfind("\n#[cfg(test)]\nmod tests {") {
                        Some(cut) => text[..cut].to_owned(),
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

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

/// Recognizes only the four SQLite write forms and the named target table.
/// This intentionally handles formatting/keyword variants without pretending
/// to parse arbitrary SQL.
fn target_writes(text: &str, table: &str) -> Vec<String> {
    let tokens: Vec<String> = normalize(text)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let table = table.to_ascii_uppercase();
    let mut writes = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let target = match token.as_str() {
            "UPDATE" => tokens.get(i + 1),
            "DELETE" if tokens.get(i + 1).is_some_and(|t| t == "FROM") => tokens.get(i + 2),
            "INSERT" | "REPLACE" => {
                let mut j = i + 1;
                if tokens.get(j).is_some_and(|t| t == "OR") {
                    j += 2; // INSERT OR {REPLACE|IGNORE}
                }
                if tokens.get(j).is_some_and(|t| t == "INTO") {
                    tokens.get(j + 1)
                } else {
                    None
                }
            }
            _ => None,
        };
        if target.is_some_and(|t| t == &table) {
            writes.push(token.clone());
        }
    }
    writes
}

fn missing_mutations(text: &str) -> usize {
    let tokens: Vec<String> = normalize(text)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let mut count = 0;
    for i in 0..tokens.len().saturating_sub(2) {
        if tokens[i] != "UPDATE" || tokens[i + 1] != "ARCHIVES" {
            continue;
        }
        let end = tokens[i + 2..]
            .iter()
            .position(|t| t == "WHERE")
            .map_or(tokens.len(), |n| i + 2 + n);
        count += usize::from((i + 2..end).any(|j| {
            tokens[j] == "LAST_VERIFIED_MISSING_AT"
                && tokens.get(j + 1).is_some_and(|rhs| rhs != "NULL")
        }));
    }
    count
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} not found"));
    let open = source[start..].find('{').unwrap() + start;
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed function {signature}")
}

#[test]
fn target_operation_guard_handles_case_spacing_and_all_write_forms() {
    assert_eq!(
        target_writes(
            "iNsErT   INTO   source_scan_bindings",
            "source_scan_bindings"
        ),
        ["INSERT"]
    );
    assert_eq!(
        target_writes(
            "insert\n OR IGNORE\n INTO source_scan_bindings",
            "source_scan_bindings"
        ),
        ["INSERT"]
    );
    assert_eq!(
        target_writes("UPDATE   source_scan_bindings", "source_scan_bindings"),
        ["UPDATE"]
    );
    assert_eq!(
        target_writes("delete\n FROM source_scan_bindings", "source_scan_bindings"),
        ["DELETE"]
    );
    assert_eq!(
        target_writes("REPLACE INTO source_scan_bindings", "source_scan_bindings"),
        ["REPLACE"]
    );
    assert!(target_writes("DELETE FROM other_table", "source_scan_bindings").is_empty());
}

#[test]
fn source_bindings_have_one_insert_and_one_update_writer() {
    let mut writes = Vec::new();
    for (path, text) in production_sources() {
        for op in target_writes(&text, "source_scan_bindings") {
            writes.push((path.clone(), text.clone(), op));
        }
    }
    assert_eq!(
        writes
            .iter()
            .map(|(_, _, op)| op.as_str())
            .collect::<Vec<_>>(),
        ["INSERT", "UPDATE"]
    );
    for (path, text, _) in writes {
        assert!(
            path.ends_with("database/catalogue_health.rs"),
            "write in {path:?}"
        );
        assert!(function_body(&text, "fn write_rebind(").contains("source_scan_bindings"));
    }
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/database/catalogue_health.rs"),
    )
    .unwrap();
    let writer = function_body(&text, "fn write_rebind(");
    assert!(writer.contains("next_source_generation("));
    assert!(
        function_body(&text, "pub(super) fn next_source_generation(").contains("checked_add(1)")
    );
    assert_eq!(
        text.matches("write_rebind(").count(),
        3,
        "writer definition, first capture and one reviewed path"
    );
    assert!(function_body(&text, "pub(super) fn bind_scan_source(").contains("write_rebind("));
    assert!(function_body(&text, "pub fn confirm_source_rebind(").contains("write_rebind("));
    assert!(
        function_body(&text, "pub fn rebind_source_after_review(")
            .contains("confirm_source_rebind(review)")
    );
}

#[test]
fn missing_has_one_guarded_mutation_path() {
    let mut writes = Vec::new();
    for (path, text) in production_sources() {
        let mutation_count = missing_mutations(&text);
        if mutation_count != 0 {
            writes.push((path, text, mutation_count));
        }
    }
    assert_eq!(writes.len(), 1);
    let (path, text, count) = &writes[0];
    assert_eq!(*count, 1, "only one SQL mutation may set Missing");
    assert!(path.ends_with("database.rs"));
    let body = normalize(function_body(text, "pub fn mark_unseen_archives_missing("));
    assert!(body.contains("LAST_VERIFIED_MISSING_AT"));
    assert!(body.matches("ASSERT_MISSING_AUTHORITY(").count() >= 3);
    assert!(body.contains("LOCK_MISSING_AUTHORITY("));
    assert_eq!(missing_mutations(text), 1);
}

#[test]
fn missing_guard_detects_unauthorized_update_but_ignores_restoration() {
    assert_eq!(
        missing_mutations("update archives set last_verified_missing_at = ?2 where id=?1"),
        1
    );
    assert_eq!(
        missing_mutations("UPDATE  archives\n SET last_verified_missing_at=NULL WHERE id=1"),
        0
    );
}

#[test]
fn only_the_reviewed_rebind_api_is_public() {
    let mut public = Vec::new();
    for (_, text) in production_sources() {
        let normalized = normalize(&text);
        for chunk in normalized.split("PUB FN ").skip(1) {
            if let Some(name) = chunk.split('(').next() {
                if name.contains("REBIND") {
                    public.push(name.trim().to_owned());
                }
            }
        }
    }
    public.sort();
    assert_eq!(
        public,
        [
            "CONFIRM_SOURCE_REBIND",
            "REBIND_SOURCE_AFTER_REVIEW",
            "REBIND_SOURCE_AFTER_REVIEW_AT",
            "REBIND_SOURCE_AFTER_REVIEW_DEFAULT",
            "REVIEW_SOURCE_REBIND",
            "REVIEW_SOURCE_REBIND_AT",
            "REVIEW_SOURCE_REBIND_DEFAULT",
        ]
    );
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/database/catalogue_health.rs"),
    )
    .unwrap();
    let wrapper = normalize(function_body(&source, "pub fn rebind_source_after_review("));
    assert!(wrapper.contains("CONFIRM_SOURCE_REBIND(REVIEW)"));
}
