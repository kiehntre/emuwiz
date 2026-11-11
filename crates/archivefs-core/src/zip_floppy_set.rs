//! The disk set of a zipped Amiga floppy game: every IPF image in one ZIP,
//! in boot order, for launch-time staging (`zip_cartridge_stage`).
//!
//! The archive itself is validated exactly as a zipped cartridge is
//! (`zip_cartridge_select::validated_file_entries`: encryption, unsafe or
//! duplicate names, symbolic links, inconsistent headers, member count).
//! On top of that a disk set is refused, never trimmed, when:
//! - any file in it is not an `.ipf` image (a BIOS pack, a readme, a
//!   KryoFlux stream: nothing is guessed or skipped);
//! - it holds more than [`MAX_FLOPPY_DISKS`] disks, a disk declares more than
//!   [`MAX_FLOPPY_DISK_BYTES`], the set more than [`MAX_FLOPPY_SET_BYTES`],
//!   or a disk's declared decompression ratio exceeds the cartridge limit;
//! - its disks do not all name the same game. One ZIP is one game, and the
//!   member names must prove it: every name is `<title> (<disk label>)` for
//!   one shared `<title>` (at most one disk may be the bare `<title>`).
//!
//! Order: `(Disk N)` / `(Disk N of M)` titles (the shared catalogue token
//! reader, `media_set::dat_disc_order::dat_disc_token`) order by number when
//! every disk carries one with distinct numbers. Otherwise the disk whose
//! label marks it as the one to boot (`Disk 1`, `Disk A`, `Boot`, `Program`,
//! ...) comes first, installers last, the rest in natural label order. Every
//! disk stays reachable through RetroArch's disk control either way.

use std::fs::File;

use zip::ZipArchive;

use crate::game_identity::IdentityStatus;
use crate::media_set::dat_disc_order::dat_disc_token;
use crate::zip_cartridge_select::{
    MAX_ZIP_CARTRIDGE_RATIO, ValidatedEntry, ZipCartridgeMember, validated_file_entries,
};

/// Most disks one zipped game may have (the largest No-Intro Amiga set
/// holds 19).
pub const MAX_FLOPPY_DISKS: usize = 32;
/// Largest single IPF image accepted (real IPF images stay under 2 MiB).
pub const MAX_FLOPPY_DISK_BYTES: u64 = 8 * 1024 * 1024;
/// Largest total a staged disk set may write.
pub const MAX_FLOPPY_SET_BYTES: u64 = 96 * 1024 * 1024;

fn refuse(status: IdentityStatus, message: impl Into<String>) -> (IdentityStatus, String) {
    (status, message.into())
}

fn stem_of(name: &str) -> Option<&str> {
    let base = name.rsplit('/').next()?;
    let (stem, extension) = base.rsplit_once('.')?;
    extension.eq_ignore_ascii_case("ipf").then_some(stem)
}

/// `title (label)` → `(title, label)`, splitting at the last group.
fn split_last_group(stem: &str) -> Option<(&str, &str)> {
    let inner = stem.strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    Some((&inner[..open], &inner[open + 2..]))
}

/// The shared title, when every stem is `title` or `title (label)` and at
/// most one is the bare title.
fn shared_title<'a>(stems: &[&'a str]) -> Option<&'a str> {
    let first = stems.first()?;
    let mut options = vec![*first];
    if let Some((title, _)) = split_last_group(first) {
        options.push(title);
    }
    options.into_iter().find(|title| {
        let mut bare = 0usize;
        stems.iter().all(|stem| {
            if stem == title {
                bare += 1;
                true
            } else {
                split_last_group(stem).is_some_and(|(t, _)| t == *title)
            }
        }) && bare <= 1
    })
}

/// 0 = the disk to boot, 1 = other disks, 2 = installers and extras.
fn boot_rank(label: Option<&str>) -> u8 {
    let Some(label) = label else {
        return 0;
    };
    let lower = label.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.iter().any(|w| {
        matches!(
            *w,
            "install" | "installer" | "hd" | "hints" | "save" | "editors"
        )
    }) {
        return 2;
    }
    let first_disk = words.windows(2).any(|pair| {
        matches!(pair[0], "disk" | "disque" | "disquette" | "diskette")
            && matches!(pair[1], "1" | "01" | "a")
    });
    if first_disk
        || words.iter().any(|w| {
            matches!(
                *w,
                "boot" | "start" | "startup" | "program" | "master" | "system"
            )
        })
    {
        return 0;
    }
    1
}

/// Natural order: digit runs compare numerically.
fn natural_key(value: &str) -> Vec<(u8, u64, String)> {
    let mut key = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                digits.push(d);
                chars.next();
            }
            key.push((0, digits.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            let mut text = String::new();
            while let Some(&t) = chars.peek().filter(|t| !t.is_ascii_digit()) {
                text.push(t.to_ascii_lowercase());
                chars.next();
            }
            key.push((1, 0, text));
        }
    }
    key
}

/// Orders validated stems; `None` when they do not name one game.
fn boot_order(stems: &[&str]) -> Option<Vec<usize>> {
    let mut order: Vec<usize> = (0..stems.len()).collect();
    if stems.len() == 1 {
        return Some(order);
    }
    let tokens: Vec<_> = stems.iter().map(|stem| dat_disc_token(stem)).collect();
    if let Some(first) = tokens.first().and_then(Option::as_ref)
        && tokens.iter().all(|token| {
            token
                .as_ref()
                .is_some_and(|t| t.base_title == first.base_title && t.unit == first.unit)
        })
    {
        let mut numbers: Vec<u16> = tokens.iter().flatten().map(|t| t.number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        if numbers.len() != stems.len() {
            return None;
        }
        order.sort_by_key(|&i| tokens[i].as_ref().map_or(0, |t| t.number));
        return Some(order);
    }
    let title = shared_title(stems)?;
    let label = |i: usize| -> Option<&str> {
        (stems[i] != title).then(|| split_last_group(stems[i]).map_or("", |(_, l)| l))
    };
    order.sort_by(|&a, &b| {
        (boot_rank(label(a)), natural_key(label(a).unwrap_or("")))
            .cmp(&(boot_rank(label(b)), natural_key(label(b).unwrap_or(""))))
    });
    Some(order)
}

/// Validates the whole ZIP and returns its IPF disks in boot order. Reads
/// only the central directory; nothing is decompressed.
pub fn select_amiga_ipf_disk_set(
    archive: &mut ZipArchive<File>,
    members_seen: &mut usize,
) -> Result<Vec<ZipCartridgeMember>, (IdentityStatus, String)> {
    let entries: Vec<ValidatedEntry> = validated_file_entries(archive, members_seen)?;
    if entries.is_empty() {
        return Err(refuse(
            IdentityStatus::Missing,
            "ZIP contains no Amiga IPF disk image",
        ));
    }
    if entries.len() > MAX_FLOPPY_DISKS {
        return Err(refuse(
            IdentityStatus::ResourceLimitReached,
            format!("ZIP holds more than {MAX_FLOPPY_DISKS} disk images"),
        ));
    }
    let mut stems = Vec::with_capacity(entries.len());
    let mut total = 0u64;
    for entry in &entries {
        let name = std::str::from_utf8(&entry.name).unwrap_or_default();
        let Some(stem) = stem_of(name) else {
            return Err(refuse(
                IdentityStatus::Unsupported,
                "ZIP holds a file that is not an Amiga IPF disk image; nothing was selected",
            ));
        };
        if entry.declared == 0 || entry.declared > MAX_FLOPPY_DISK_BYTES {
            return Err(refuse(
                IdentityStatus::ResourceLimitReached,
                format!(
                    "a disk image declares {} bytes; IPF disks must be 1 to {MAX_FLOPPY_DISK_BYTES} bytes",
                    entry.declared
                ),
            ));
        }
        if entry.declared
            > entry
                .compressed
                .saturating_mul(MAX_ZIP_CARTRIDGE_RATIO)
                .max(4096)
        {
            return Err(refuse(
                IdentityStatus::ResourceLimitReached,
                "a disk image's decompression ratio exceeds the safety limit",
            ));
        }
        total = total.saturating_add(entry.declared);
        stems.push(stem);
    }
    if total > MAX_FLOPPY_SET_BYTES {
        return Err(refuse(
            IdentityStatus::ResourceLimitReached,
            format!(
                "the disk set declares {total} bytes; at most {MAX_FLOPPY_SET_BYTES} are staged"
            ),
        ));
    }
    let Some(order) = boot_order(&stems) else {
        return Err(refuse(
            IdentityStatus::Ambiguous,
            "the disks in this ZIP do not all name the same game (or repeat a disk number); none was selected",
        ));
    };
    Ok(order
        .into_iter()
        .map(|i| ZipCartridgeMember {
            index: entries[i].index,
            name: entries[i].name.clone(),
            declared: entries[i].declared,
            format: "ipf",
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordered(names: &[&str]) -> Option<Vec<String>> {
        boot_order(names).map(|o| o.into_iter().map(|i| names[i].to_string()).collect())
    }

    #[test]
    fn numbered_disks_order_by_number_not_text() {
        let got = ordered(&[
            "Game (Europe) (Disk 10)",
            "Game (Europe) (Disk 2)",
            "Game (Europe) (Disk 1)",
        ]);
        assert_eq!(
            got.unwrap(),
            [
                "Game (Europe) (Disk 1)",
                "Game (Europe) (Disk 2)",
                "Game (Europe) (Disk 10)"
            ]
        );
        let got = ordered(&["W (Disk 02)", "W (Disk 01)"]).unwrap();
        assert_eq!(got, ["W (Disk 01)", "W (Disk 02)"]);
    }

    #[test]
    fn labelled_sets_boot_from_the_program_or_first_disk_and_install_last() {
        assert_eq!(
            ordered(&["G (Data Disk)", "G (Install Disk)", "G (Program Disk)"]).unwrap(),
            ["G (Program Disk)", "G (Data Disk)", "G (Install Disk)"]
        );
        assert_eq!(
            ordered(&["A (Disk B)", "A (Intro Disk)", "A (Disk A)"]).unwrap(),
            ["A (Disk A)", "A (Disk B)", "A (Intro Disk)"]
        );
        assert_eq!(
            ordered(&["Oil (v3) (Data Disk)", "Oil (v3)"]).unwrap(),
            ["Oil (v3)", "Oil (v3) (Data Disk)"]
        );
        assert_eq!(
            ordered(&["AB (Disk 1)", "AB (Disk 2)", "AB (HD Installer and Hints)"]).unwrap()[0],
            "AB (Disk 1)"
        );
    }

    #[test]
    fn disks_of_different_games_or_repeated_numbers_are_refused() {
        assert!(ordered(&["Alpha (Disk 1)", "Beta (Disk 2)"]).is_none());
        assert!(ordered(&["Alpha (Program Disk)", "Beta (Data Disk)"]).is_none());
        assert!(ordered(&["Alpha", "Alpha (Europe)", "Alpha"]).is_none());
        assert!(ordered(&["X (Disk 1)", "X (Disk 1 of 2)"]).is_none());
    }

    fn zip_with(dir: &std::path::Path, members: &[(&str, usize)]) -> std::path::PathBuf {
        use std::io::Write;
        let path = dir.join("set.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, size) in members {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&vec![0x5a; *size]).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn select(path: &std::path::Path) -> Result<Vec<String>, String> {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        select_amiga_ipf_disk_set(&mut archive, &mut 0)
            .map(|set| {
                set.into_iter()
                    .map(|m| String::from_utf8(m.name).unwrap())
                    .collect()
            })
            .map_err(|(_, message)| message)
    }

    #[test]
    fn a_zip_of_one_game_s_ipf_disks_is_selected_in_boot_order() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_with(
            dir.path(),
            &[
                ("Op (Europe) (Disk 2).ipf", 6000),
                ("Op (Europe) (Disk 1).IPF", 6000),
            ],
        );
        assert_eq!(
            select(&zip).unwrap(),
            ["Op (Europe) (Disk 1).IPF", "Op (Europe) (Disk 2).ipf"]
        );
    }

    #[test]
    fn staging_writes_every_disk_and_a_per_game_playlist_then_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_with(
            dir.path(),
            &[
                ("Op (Europe) (Disk 2).ipf", 7000),
                ("Op (Europe) (Disk 1).ipf", 6000),
            ],
        );
        let before = std::fs::read(&zip).unwrap();
        let staged =
            crate::zip_cartridge_stage::stage_zip_cartridge_for_launch(&zip, "Amiga").unwrap();
        assert_eq!(
            staged.path().file_name().unwrap(),
            "Op (Europe) (Disk 1).ipf"
        );
        assert_eq!(std::fs::metadata(staged.path()).unwrap().len(), 6000);
        let playlist = staged
            .playlist()
            .expect("two disks get a playlist")
            .to_path_buf();
        assert_eq!(playlist.file_name().unwrap(), "Op (Europe) (Disk 1).m3u");
        assert_eq!(
            std::fs::read_to_string(&playlist).unwrap(),
            "Op (Europe) (Disk 1).ipf\nOp (Europe) (Disk 2).ipf\n"
        );
        let folder = playlist.parent().unwrap().to_path_buf();
        assert!(
            !folder.starts_with(dir.path()),
            "never staged beside the ZIP"
        );
        drop(staged);
        assert!(!folder.exists(), "dropping the stage removes it");
        assert_eq!(std::fs::read(&zip).unwrap(), before, "the ZIP is unchanged");

        let single = dir.path().join("one");
        std::fs::create_dir(&single).unwrap();
        let one = zip_with(&single, &[("Silk (Europe).ipf", 5000)]);
        let staged =
            crate::zip_cartridge_stage::stage_zip_cartridge_for_launch(&one, "Amiga").unwrap();
        assert!(staged.playlist().is_none());
        assert_eq!(staged.path().file_name().unwrap(), "Silk (Europe).ipf");
        assert_eq!(
            crate::zip_cartridge_stage::zip_cartridge_member_fits(&one, "Amiga"),
            Some(true)
        );
        let mixed_dir = dir.path().join("mixed");
        std::fs::create_dir(&mixed_dir).unwrap();
        let mixed = zip_with(
            &mixed_dir,
            &[("A (Disk 1).ipf", 10), ("B (Disk 2).ipf", 10)],
        );
        assert_eq!(
            crate::zip_cartridge_stage::zip_cartridge_member_fits(&mixed, "Amiga"),
            Some(false)
        );
        assert!(matches!(
            crate::zip_cartridge_stage::stage_zip_cartridge_for_launch(&mixed, "Amiga"),
            Err(crate::zip_cartridge_stage::StageZipError::Refused(_))
        ));
    }

    #[test]
    fn non_ipf_members_empty_sets_and_oversized_disks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mixed = zip_with(dir.path(), &[("G (Disk 1).ipf", 10), ("readme.txt", 10)]);
        assert!(select(&mixed).unwrap_err().contains("not an Amiga IPF"));
        let empty = zip_with(dir.path(), &[]);
        assert!(select(&empty).is_err());
        let big = zip_with(
            dir.path(),
            &[("G.ipf", (MAX_FLOPPY_DISK_BYTES + 1) as usize)],
        );
        assert!(select(&big).unwrap_err().contains("bytes"));
    }
}
