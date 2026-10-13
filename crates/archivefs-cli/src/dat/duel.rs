//! `emuwiz-cli dat duel <DAT-A> <DAT-B> [--json] [--allow-incompatible] [--limit N]`.
//!
//! Read-only comparison of two local DAT catalogues (see
//! `archivefs_core::dat::duel`). Nothing is imported, renamed, downloaded or
//! written; both files are parsed with the existing parsers and limits.

use std::path::Path;

use archivefs_core::dat::duel::{
    DEFAULT_MAX_ITEMS_PER_SECTION, DuelOptions, FileFingerprint, compare, fingerprint_file,
    render_text,
};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parsers::parse_dat_file;

const USAGE: &str = "usage: dat duel <DAT-A> <DAT-B> [--json] [--allow-incompatible] [--limit N]";
/// Entries printed per section in the text view (JSON uses `--limit`).
const TEXT_PER_SECTION: usize = 25;

fn load(label: &str, path: &Path) -> Result<(ParsedDat, FileFingerprint), String> {
    let outcome = parse_dat_file(path, DatLimits::default())
        .map_err(|e| format!("DAT {label} ({}): {e}", path.display()))?;
    let fingerprint =
        fingerprint_file(path).map_err(|e| format!("DAT {label} ({}): {e}", path.display()))?;
    Ok((outcome.dat, fingerprint))
}

fn execute(
    mut args: Vec<String>,
) -> Result<(archivefs_core::dat::duel::DuelReport, bool, usize), Box<dyn std::error::Error>> {
    let json = super::extract_flag(&mut args, "--json");
    let allow_incompatible = super::extract_flag(&mut args, "--allow-incompatible");
    let mut limit = DEFAULT_MAX_ITEMS_PER_SECTION;
    if let Some(pos) = args.iter().position(|a| a == "--limit") {
        let value = args
            .get(pos + 1)
            .ok_or("--limit requires a number")?
            .parse::<usize>()
            .map_err(|_| "--limit must be a non-negative integer")?;
        limit = value.min(DEFAULT_MAX_ITEMS_PER_SECTION);
        args.drain(pos..=pos + 1);
    }
    let path_a = super::take_first_path(&mut args, USAGE)?;
    let path_b = super::take_first_path(&mut args, USAGE)?;
    super::reject_extra(&args, "dat duel")?;

    let (dat_a, fp_a) = load("A", &path_a)?;
    let (dat_b, fp_b) = load("B", &path_b)?;
    let report = compare(
        &dat_a,
        &dat_b,
        [Some(fp_a), Some(fp_b)],
        DuelOptions {
            allow_incompatible,
            max_items_per_section: limit,
        },
    )?;

    Ok((report, json, limit))
}

pub(super) fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let (report, json, limit) = execute(args)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_text(&report, TEXT_PER_SECTION.min(limit)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dat_file(dir: &tempfile::TempDir, name: &str, header: &str, games: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\"?>\n<datafile><header><name>{header}</name><version>1</version></header>{games}</datafile>"
            ),
        )
        .unwrap();
        path
    }

    const S1: &str = "1111111111111111111111111111111111111111";
    const S2: &str = "2222222222222222222222222222222222222222";

    fn args(a: &Path, b: &Path, extra: &[&str]) -> Vec<String> {
        let mut v = vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    }

    #[test]
    fn end_to_end_reports_rename_conflict_and_leaves_sources_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let a = dat_file(
            &dir,
            "a.dat",
            "Test System",
            &format!(
                r#"<game name="Alpha"><rom name="a.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>
                   <game name="Beta"><rom name="b.bin" size="4" crc="bbbbbbbb" sha1="{S2}"/></game>
                   <game name="OnlyA"><rom name="o.bin" size="2" crc="cccccccc"/></game>"#
            ),
        );
        let b = dat_file(
            &dir,
            "b.dat",
            "Test System",
            &format!(
                r#"<game name="Alpha Renamed"><rom name="a.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>
                   <game name="Beta"><rom name="b.bin" size="9" crc="dddddddd" sha1="{S1}"/></game>"#
            ),
        );
        let before = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let (report, json, _) = execute(args(&a, &b, &["--json"])).unwrap();
        assert!(json);
        assert!(report.read_only);
        // A/Alpha is a clean verified rename. B/Beta shares A/Alpha's SHA-1
        // but contradicts its CRC32 and size, so it is a conflict (not a
        // candidate match), and it also conflicts with A/Beta by name.
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(report.summary.renamed_verified, 1);
        assert_eq!(report.same_name_conflicts.len(), 1);
        assert_eq!(report.cross_name_conflicts.len(), 1);
        assert_eq!(report.summary.ambiguous_groups, 0);
        assert_eq!(report.summary.only_in_a_no_counterpart, 1);
        assert!(report.sources[0].file.is_some());
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(rendered.contains("\"schema_version\":1"));
        assert_eq!(
            before,
            (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap())
        );
        // also nothing else appeared next to the sources
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn different_platform_catalogues_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = dat_file(&dir, "a.dat", "Nintendo - NES", "");
        let b = dat_file(&dir, "b.dat", "Sega - Mega Drive", "");
        let err = execute(args(&a, &b, &[])).unwrap_err().to_string();
        assert!(err.contains("incompatible"), "{err}");
        assert!(execute(args(&a, &b, &["--allow-incompatible"])).is_ok());
    }

    #[test]
    fn malformed_dat_is_an_error_naming_the_file_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let good = dat_file(&dir, "good.dat", "S", "");
        let bad = dir.path().join("bad.dat");
        std::fs::write(
            &bad,
            "<datafile><header><name>S</name></header><game name=\"x\"",
        )
        .unwrap();
        let err = execute(args(&good, &bad, &[])).unwrap_err().to_string();
        assert!(err.contains("DAT B") && err.contains("bad.dat"), "{err}");
        let missing = dir.path().join("nope.dat");
        let err = execute(args(&missing, &good, &[])).unwrap_err().to_string();
        assert!(err.contains("DAT A"), "{err}");
    }

    #[test]
    fn usage_errors_are_reported() {
        assert!(execute(vec![]).is_err());
        assert!(execute(vec!["only-one".into()]).is_err());
        assert!(execute(vec!["a".into(), "b".into(), "c".into()]).is_err());
        assert!(execute(vec!["a".into(), "b".into(), "--limit".into()]).is_err());
    }

    #[test]
    fn text_output_names_both_sources_and_the_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let a = dat_file(
            &dir,
            "a.dat",
            "S",
            &format!(r#"<game name="G"><rom name="r" size="1" sha1="{S1}"/></game>"#),
        );
        let b = dat_file(&dir, "b.dat", "S", "");
        let (report, _, _) = execute(args(&a, &b, &[])).unwrap();
        let text = render_text(&report, 5);
        assert!(text.contains("DAT Duel (read-only)") && text.contains("Namespace: Compatible"));
        assert!(text.contains("only in A"));
    }
}
