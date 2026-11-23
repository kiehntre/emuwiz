use super::*;
use std::ffi::OsStr;

fn argv(arguments: &[&str]) -> Vec<OsString> {
    arguments.iter().map(OsString::from).collect()
}

/// The arguments added to `before` to get `after`, and whether everything
/// else kept its order.
fn added(before: &[OsString], after: &[OsString]) -> (Vec<OsString>, bool) {
    let mut extra = Vec::new();
    let mut expected = before.iter().peekable();
    for argument in after {
        if expected.peek() == Some(&argument) {
            expected.next();
        } else {
            extra.push(argument.clone());
        }
    }
    (extra, expected.next().is_none())
}

#[test]
fn a_diagnostic_launch_adds_exactly_one_argument_and_it_is_verbose() {
    for before in [
        argv(&["-L", "/cores/a_libretro.so", "/games/a.gb"]),
        argv(&[
            "--config",
            "/p/retroarch.cfg",
            "-L",
            "/cores/a_libretro.so",
            "/games/a.gb",
        ]),
        argv(&[
            "run",
            "--filesystem=/games",
            "org.libretro.RetroArch",
            "-L",
            "/cores/a_libretro.so",
            "/games/a.gb",
        ]),
    ] {
        let mut after = before.clone();
        assert_eq!(
            prepare_launch_with(&mut after, None),
            OutputCapture::Diagnostic
        );
        let (extra, order_kept) = added(&before, &after);
        // The verified behaviour is for this one flag: nothing that names a
        // settings file, a settings key or a log file may ever join it.
        assert_eq!(extra, argv(&[VERBOSE_FLAG]), "{before:?}");
        assert!(order_kept);
        let core = after.iter().position(|argument| argument == "-L").unwrap();
        assert_eq!(after[core - 1], OsString::from(VERBOSE_FLAG));
        // The game stays the final argument.
        assert_eq!(after.last(), before.last());
    }
}

#[test]
fn the_flag_is_never_added_twice() {
    let mut once = argv(&["-L", "/cores/a_libretro.so", "/games/a.gb"]);
    assert!(request_verbose_logging(&mut once));
    let expected = once.clone();
    assert!(request_verbose_logging(&mut once));
    assert_eq!(once, expected);

    for present in ["--verbose", "-v"] {
        for before in [
            argv(&[present, "-L", "/cores/a_libretro.so", "/games/a.gb"]),
            // However far ahead of the core it is.
            argv(&[
                present,
                "--config",
                "/p/retroarch.cfg",
                "--fullscreen",
                "-L",
                "/cores/a_libretro.so",
                "/games/a.gb",
            ]),
            // Or after it.
            argv(&["-L", "/cores/a_libretro.so", present, "/games/a.gb"]),
        ] {
            let mut after = before.clone();
            assert!(request_verbose_logging(&mut after));
            assert_eq!(after, before);
        }
    }
}

#[test]
fn an_argv_that_cannot_be_located_is_left_exactly_as_it_was() {
    for before in [
        argv(&[]),
        argv(&["/games/a.gb"]),
        argv(&["--menu"]),
        argv(&["-L", "/cores/a.so", "-L", "/cores/b.so", "/games/a.gb"]),
    ] {
        let mut after = before.clone();
        assert_eq!(
            prepare_launch_with(&mut after, None),
            OutputCapture::StderrStart
        );
        assert_eq!(after, before);
    }
}

#[test]
fn the_quiet_switch_restores_the_previous_launch_exactly() {
    let before = argv(&["-L", "/cores/a_libretro.so", "/games/a.gb"]);
    let mut after = before.clone();
    assert_eq!(
        prepare_launch_with(&mut after, Some(OsStr::new("1"))),
        OutputCapture::StderrStart
    );
    assert_eq!(after, before);
    // Only the exact value "1" is a request for quiet.
    for other in ["0", "", "true", "yes"] {
        let mut after = before.clone();
        assert_eq!(
            prepare_launch_with(&mut after, Some(OsStr::new(other))),
            OutputCapture::Diagnostic,
            "{other:?}"
        );
    }
}
