use super::*;
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    dir: PathBuf,
}

fn fixture(autoexec: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("Game One");
    std::fs::create_dir_all(dir.join("DATA")).unwrap();
    std::fs::write(dir.join("GAME.EXE"), b"MZ").unwrap();
    std::fs::write(dir.join("DATA").join("RUN.BAT"), b"@echo off").unwrap();
    std::fs::write(
        dir.join("dosbox.conf"),
        format!("[dosbox]\nmemsize=16\n[autoexec]\n{autoexec}"),
    )
    .unwrap();
    Fixture { _root: root, dir }
}

fn review(autoexec: &str) -> (Fixture, DosBoxGameReview) {
    let f = fixture(autoexec);
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    (f, r)
}

#[test]
fn a_plain_mount_and_single_start_command_is_approvable() {
    let (f, r) = review("@echo off\nmount c .\nc:\ngame.exe\nexit\n");
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let start = r.start.as_ref().unwrap();
    assert_eq!(start.command_text, "game.exe");
    assert_eq!(start.program_file, f.dir.join("GAME.EXE"));
    assert!(r.approve().is_some());
}

#[test]
fn the_start_program_is_found_case_insensitively_without_an_extension() {
    let (f, r) = review("mount c .\nc:\nGame\n");
    assert_eq!(r.start.unwrap().program_file, f.dir.join("GAME.EXE"));
}

#[test]
fn cd_and_a_subfolder_batch_file_resolve_inside_the_mount() {
    let (f, r) = review("mount c .\nc:\ncd data\nrun.bat\n");
    assert_eq!(
        r.start.unwrap().program_file,
        f.dir.join("DATA").join("RUN.BAT")
    );
}

#[test]
fn loadfix_wraps_the_real_program() {
    let (_f, r) = review("mount c .\nc:\nloadfix game.exe\n");
    assert!(r.is_approvable());
}

#[test]
fn a_mount_of_the_host_root_is_unsafe_and_not_approvable() {
    let (_f, r) = review("mount c /\nc:\ngame.exe\n");
    assert!(matches!(
        r.problems[0],
        DosBoxReviewProblem::UnsafeMount { line: 4, .. }
    ));
    assert!(r.approve().is_none());
}

#[test]
fn home_parent_and_backslash_mounts_are_unsafe() {
    for line in [
        "mount c ~",
        "mount c ..",
        "mount c ../other",
        "mount c .\\DATA",
    ] {
        let (_f, r) = review(&format!("{line}\nc:\ngame.exe\n"));
        assert!(
            r.problems
                .iter()
                .any(|p| matches!(p, DosBoxReviewProblem::UnsafeMount { .. })),
            "{line}: {:?}",
            r.problems
        );
        assert!(r.approve().is_none(), "{line}");
    }
}

#[test]
fn a_symlink_that_escapes_the_game_folder_is_unsafe() {
    let f = fixture("mount c escape\nc:\ngame.exe\n");
    let outside = f._root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.dir.join("escape")).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(
        r.problems
            .iter()
            .any(|p| matches!(p, DosBoxReviewProblem::UnsafeMount { .. }))
    );
}

#[test]
fn host_device_flags_are_unsafe() {
    let (_f, r) = review("mount d . -t cdrom -usecd 0\nmount c .\nc:\ngame.exe\n");
    assert!(
        r.problems
            .iter()
            .any(|p| matches!(p, DosBoxReviewProblem::UnsafeMount { line: 4, .. }))
    );
}

#[test]
fn unreviewed_commands_fail_closed() {
    for line in [
        "config -set cycles=max",
        "boot a.img",
        "copy a b",
        "goto end",
    ] {
        let (_f, r) = review(&format!("mount c .\nc:\n{line}\ngame.exe\n"));
        assert!(
            r.problems
                .iter()
                .any(|p| matches!(p, DosBoxReviewProblem::UnsupportedLine { line: 6, .. })),
            "{line}: {:?}",
            r.problems
        );
        assert!(r.approve().is_none());
    }
}

#[test]
fn no_start_command_is_not_guessed_from_the_files() {
    let (_f, r) = review("mount c .\nc:\n");
    assert_eq!(r.problems, vec![DosBoxReviewProblem::NoStartCommand]);
    assert!(r.start.is_none());
}

#[test]
fn two_start_commands_are_ambiguous() {
    let (_f, r) = review("mount c .\nc:\ngame.exe\ncd data\nrun.bat\n");
    assert!(matches!(
        r.problems.as_slice(),
        [DosBoxReviewProblem::AmbiguousStart { lines }] if lines == &vec![6, 8]
    ));
    assert!(r.approve().is_none());
}

#[test]
fn a_start_program_that_is_not_in_the_folder_is_reported() {
    let (_f, r) = review("mount c .\nc:\nmissing.exe\n");
    assert!(matches!(
        r.problems[0],
        DosBoxReviewProblem::ProgramUnavailable { line: 6, .. }
    ));
}

#[test]
fn a_start_before_any_mount_is_reported() {
    let (_f, r) = review("c:\ngame.exe\n");
    assert!(!r.is_approvable());
}

#[test]
fn a_non_dos_extension_cannot_be_the_start_program() {
    let f = fixture("mount c .\nc:\nnotes.txt\n");
    std::fs::write(f.dir.join("notes.txt"), b"x").unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(!r.is_approvable());
}

#[test]
fn a_symlinked_start_program_is_refused() {
    let f = fixture("mount c .\nc:\nlink.exe\n");
    std::os::unix::fs::symlink(f.dir.join("GAME.EXE"), f.dir.join("link.exe")).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(!r.is_approvable());
}

#[test]
fn approval_matches_an_unchanged_folder_and_rejects_each_kind_of_change() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let approval = review_dosbox_game_directory(&f.dir)
        .unwrap()
        .approve()
        .unwrap();
    assert_eq!(
        approval.matches(&review_dosbox_game_directory(&f.dir).unwrap()),
        Ok(())
    );

    std::fs::write(f.dir.join("GAME.EXE"), b"MZ-bigger-now").unwrap();
    assert_eq!(
        approval.matches(&review_dosbox_game_directory(&f.dir).unwrap()),
        Err(ApprovalMismatch::StartFile)
    );

    std::fs::write(
        f.dir.join("dosbox.conf"),
        "[dosbox]\nmemsize=32\n[autoexec]\nmount c .\nc:\ngame.exe\n",
    )
    .unwrap();
    assert_eq!(
        approval.matches(&review_dosbox_game_directory(&f.dir).unwrap()),
        Err(ApprovalMismatch::Config)
    );
}

#[test]
fn a_different_start_argument_is_a_different_command() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let approval = review_dosbox_game_directory(&f.dir)
        .unwrap()
        .approve()
        .unwrap();
    // Restore the original start program identity but change the command.
    let text = std::fs::read_to_string(f.dir.join("dosbox.conf")).unwrap();
    std::fs::write(
        f.dir.join("dosbox.conf"),
        text.replace("game.exe", "game.exe -x"),
    )
    .unwrap();
    let fresh = review_dosbox_game_directory(&f.dir).unwrap();
    assert_ne!(approval.matches(&fresh), Ok(()));
}

#[test]
fn a_symlinked_game_folder_or_missing_config_cannot_be_reviewed() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let link = f._root.path().join("link");
    std::os::unix::fs::symlink(&f.dir, &link).unwrap();
    assert!(review_dosbox_game_directory(&link).is_err());
    std::fs::remove_file(f.dir.join("dosbox.conf")).unwrap();
    assert!(review_dosbox_game_directory(&f.dir).is_err());
}

#[test]
fn locate_finds_the_folder_with_the_config_for_a_file_inside_it() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    assert_eq!(
        locate_dosbox_game_directory(&f.dir.join("GAME.EXE"), None),
        Some(f.dir.clone())
    );
    assert_eq!(
        locate_dosbox_game_directory(&f.dir.join("DATA").join("RUN.BAT"), None),
        Some(f.dir.clone())
    );
    // Never climbs above the stop folder.
    assert_eq!(
        locate_dosbox_game_directory(
            &f.dir.join("DATA").join("RUN.BAT"),
            Some(&f.dir.join("DATA"))
        ),
        None
    );
}

#[test]
fn reviewing_never_runs_or_changes_anything() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let before = std::fs::read(f.dir.join("dosbox.conf")).unwrap();
    let _ = review_dosbox_game_directory(&f.dir).unwrap();
    assert_eq!(std::fs::read(f.dir.join("dosbox.conf")).unwrap(), before);
}

fn with_option(option_section: &str) -> DosBoxGameReview {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    std::fs::write(
        f.dir.join("dosbox.conf"),
        format!("[dosbox]\nmemsize=16\n{option_section}\n[autoexec]\nmount c .\nc:\ngame.exe\n"),
    )
    .unwrap();
    review_dosbox_game_directory(&f.dir).unwrap()
}

#[test]
fn host_reaching_options_outside_autoexec_are_refused() {
    for option in [
        "[serial]\nserial1=directserial realport:ttyS0",
        "[parallel]\nparallel1=file dev:/tmp/x",
        "[dosbox]\ncaptures=/home/user",
        "[capture]\ncapture_dir=../../elsewhere",
    ] {
        let r = with_option(option);
        assert!(
            r.problems
                .iter()
                .any(|p| matches!(p, DosBoxReviewProblem::UnsafeOption { .. })),
            "{option}"
        );
        assert!(r.approve().is_none());
    }
}

#[test]
fn ordinary_options_and_disabled_ports_are_accepted() {
    let r =
        with_option("[cpu]\ncycles=max\n[serial]\nserial1=dummy\n[parallel]\nparallel1=disabled");
    assert!(r.is_approvable(), "{:?}", r.problems);
}

// --- independent-review regressions ---------------------------------------

#[test]
fn a_symlink_inside_the_game_folder_makes_it_unreviewable() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let outside = f._root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"s").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), f.dir.join("link.txt")).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(
        r.problems.iter().any(
            |p| matches!(p, DosBoxReviewProblem::UnsafeTree { path, .. } if path == "link.txt")
        )
    );
    assert!(r.approve().is_none());
}

#[test]
fn a_symlinked_subfolder_or_nested_link_is_refused() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let outside = f._root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.dir.join("DATA").join("odir")).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(!r.is_approvable());
}

#[test]
fn hard_links_and_pipes_inside_the_folder_are_refused() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let elsewhere = f._root.path().join("shared.txt");
    std::fs::write(&elsewhere, b"x").unwrap();
    std::fs::hard_link(&elsewhere, f.dir.join("hard.txt")).unwrap();
    assert!(
        !review_dosbox_game_directory(&f.dir)
            .unwrap()
            .is_approvable()
    );
    std::fs::remove_file(f.dir.join("hard.txt")).unwrap();
    let fifo = std::ffi::CString::new(f.dir.join("pipe.txt").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(
        !review_dosbox_game_directory(&f.dir)
            .unwrap()
            .is_approvable()
    );
}

#[test]
fn percent_expansion_cannot_hide_a_mount_path() {
    // A folder literally named %X% passes a path check, but DOSBox expands
    // %X% first. Any % in an autoexec line is refused.
    let f = fixture("set X=/etc\nmount c %X%\nc:\ngame.exe\n");
    std::fs::create_dir(f.dir.join("%X%")).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(r.problems.iter().any(
        |p| matches!(p, DosBoxReviewProblem::UnsupportedLine { command, .. } if command == "%")
    ));
    assert!(r.approve().is_none());
}

#[test]
fn start_program_contents_are_bound_to_the_approval() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    let approval = review_dosbox_game_directory(&f.dir)
        .unwrap()
        .approve()
        .unwrap();
    // Same length, restored mtime: only the hash can catch it.
    let path = f.dir.join("GAME.EXE");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, b"XX").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(before)
        .unwrap();
    let fresh = review_dosbox_game_directory(&f.dir).unwrap();
    assert_eq!(approval.matches(&fresh), Err(ApprovalMismatch::StartFile));
}

#[test]
fn unknown_and_host_reaching_options_are_refused_by_the_allowlist() {
    for option in [
        "[midi]\nmididevice=alsa",
        "[midi]\nmidiconfig=128:0",
        "[ipx]\nipx=true",
        "[gus]\nultradir=C:\\ULTRASND",
        "[sdl]\nmapperfile=../x.map",
        "[madeup]\nthing=1",
    ] {
        let r = with_option(option);
        assert!(
            r.problems
                .iter()
                .any(|p| matches!(p, DosBoxReviewProblem::UnsafeOption { .. })),
            "{option}"
        );
    }
}

#[test]
fn unicode_whitespace_does_not_hide_or_split_a_line() {
    // DOSBox does not treat NBSP as whitespace, so it must not be trimmed:
    // the line is read as one unknown command, which fails closed.
    let (_f, r) = review("mount c .\nc:\n\u{a0}mount d /\ngame.exe\n");
    assert!(!r.is_approvable());
}

// --- second independent-review regressions -------------------------------

#[test]
fn a_cue_sheet_cannot_be_imgmounted_to_reach_host_files() {
    for flags in ["", " -t iso", " -t cdrom"] {
        let f = fixture(&format!(
            "mount c .\nimgmount d g.cue{flags}\nc:\ngame.exe\n"
        ));
        std::fs::write(f.dir.join("g.cue"), b"FILE \"/etc/hostname\" BINARY\n").unwrap();
        let r = review_dosbox_game_directory(&f.dir).unwrap();
        assert!(!r.problems.is_empty(), "{flags:?}");
        assert!(r.approve().is_none(), "{flags:?}");
    }
}

#[test]
fn a_floppy_image_inside_the_folder_is_accepted() {
    let f = fixture("mount c .\nimgmount a disk.img -t floppy\nc:\ngame.exe\n");
    std::fs::write(f.dir.join("disk.img"), vec![0u8; 512]).unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(r.is_approvable(), "{:?}", r.problems);
}

#[test]
fn an_unterminated_section_header_cannot_hide_lines() {
    let f = fixture("mount c .\n");
    std::fs::write(
        f.dir.join("dosbox.conf"),
        "[dosbox]\nmemsize=16\n[autoexec]\nmount c .\n[sdl\nmount d /etc\n[autoexec]\nc:\ngame.exe\n",
    )
    .unwrap();
    // Either discovery or the review itself must refuse it.
    if let Ok(r) = review_dosbox_game_directory(&f.dir) {
        assert!(!r.is_approvable());
    }
}

#[test]
fn redirection_and_pipes_are_refused() {
    for line in ["echo hi | mount d /etc", "echo x > file.txt", "type a < b"] {
        let (_f, r) = review(&format!("mount c .\nc:\n{line}\ngame.exe\n"));
        assert!(!r.is_approvable(), "{line}");
    }
}

#[test]
fn names_that_differ_only_by_case_are_refused() {
    let f = fixture("mount c .\nc:\ngame.exe\n");
    std::fs::write(f.dir.join("game.exe"), b"MZ-other").unwrap();
    let r = review_dosbox_game_directory(&f.dir).unwrap();
    assert!(
        r.problems
            .iter()
            .any(|p| matches!(p, DosBoxReviewProblem::UnsafeTree { .. }))
    );
}
