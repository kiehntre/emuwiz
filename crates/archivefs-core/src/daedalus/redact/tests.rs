use super::*;
use crate::daedalus::collect::{LaunchArtefacts, collect};
use crate::daedalus::evidence::{EvidenceArea, EvidenceSet, ExitOutcome, Fact, LogStream, Trust};
use crate::daedalus::retroarch;
use crate::daedalus::rules::diagnose;

fn redactor() -> Redactor {
    Redactor::new(Some("/home/alex"), Some("alex"))
}

#[test]
fn the_home_folder_and_user_name_are_hidden_in_paths() {
    let redactor = redactor();
    assert_eq!(
        redactor.text("Loading \"/home/alex/.config/retroarch/retroarch.cfg\"."),
        "Loading \"~/.config/retroarch/retroarch.cfg\"."
    );
    assert_eq!(redactor.text("/home/alex"), "~");
    assert_eq!(
        redactor.text("/mnt/games/alex/roms/a.gb"),
        "/mnt/games/[user]/roms/a.gb"
    );
    assert_eq!(
        redactor.text("/run/media/alex/USB/a.gb"),
        "/run/media/[user]/USB/a.gb"
    );
    // A longer name that merely starts the same way is someone else's.
    assert_eq!(redactor.text("/home/alexandra/x"), "/home/[user]/x");
    assert_eq!(redactor.text("/srv/alexandria/x"), "/srv/alexandria/x");
}

#[test]
fn other_people_s_home_folders_are_hidden_too() {
    let redactor = Redactor::new(None, None);
    assert_eq!(
        redactor.text("/home/sam/games/a.gb"),
        "/home/[user]/games/a.gb"
    );
    assert_eq!(
        redactor.text("\"/Users/sam/a.gb\""),
        "\"/Users/[user]/a.gb\""
    );
    assert_eq!(redactor.text("/opt/home/tools"), "/opt/home/tools");
    assert_eq!(redactor.text("/home/"), "/home/");
}

#[test]
fn values_of_secret_looking_keys_are_removed() {
    let redactor = redactor();
    for (input, expected) in [
        ("password=hunter2", "password=[redacted]"),
        (
            "netplay_password = \"hunter2 two\"",
            "netplay_password = \"[redacted]\"",
        ),
        ("cheevos_token: abc123DEF", "cheevos_token: [redacted]"),
        ("API_KEY=abc&user=bob", "API_KEY=[redacted]&user=bob"),
        (
            "\"access_token\":\"eyJhbGciOi\"",
            "\"access_token\":\"[redacted]\"",
        ),
        (
            "Set-Cookie: sid=1; Path=/",
            "Set-Cookie: [redacted]; Path=/",
        ),
        ("password=", "password="),
    ] {
        assert_eq!(redactor.text(input), expected, "{input}");
    }
    // Ordinary key/value text is left alone.
    for untouched in [
        "video_driver = \"vulkan\"",
        "[INFO] [Core]: Geometry: 376x464, Aspect: 0.000, FPS: 60.00, Sample rate: 0.00 Hz.",
        "Version: 1.21.0",
        "12:34:56 started",
    ] {
        assert_eq!(redactor.text(untouched), untouched);
    }
}

#[test]
fn credentials_in_urls_and_authorization_headers_are_removed() {
    let redactor = redactor();
    assert_eq!(
        redactor.text("GET https://bob:s3cret@example.org/api?x=1"),
        "GET https://[redacted]@example.org/api?x=1"
    );
    assert_eq!(
        redactor.text("https://example.org/a@b"),
        "https://example.org/a@b"
    );
    let header = redactor.text("Authorization: Bearer abc.def.ghi");
    assert!(!header.contains("abc.def.ghi"), "{header}");
    assert!(header.starts_with("Authorization: "), "{header}");
    assert_eq!(
        redactor.text("Proxy uses Basic dXNlcjpwYXNz now"),
        "Proxy uses Basic [redacted] now"
    );
}

#[test]
fn redaction_is_idempotent_and_never_panics_on_odd_text() {
    let redactor = redactor();
    for input in [
        "",
        "=",
        ":",
        "=====",
        "token=",
        "token",
        "pässwörd=ünïcode token=\u{1F600}x",
        "://",
        "a://@",
        "Bearer ",
        "/home//x",
        "password=\"unterminated",
        "\u{feff}/home/alex/é/token=ü",
    ] {
        let once = redactor.text(input);
        assert_eq!(redactor.text(&once), once, "{input:?}");
    }
}

#[test]
fn a_very_short_user_name_or_a_root_home_is_not_used_as_a_pattern() {
    let redactor = Redactor::new(Some("/"), Some("al"));
    assert_eq!(redactor.text("/usr/lib/al/x also"), "/usr/lib/al/x also");
    let generic = Redactor::new(Some("/home/retroarch"), Some("retroarch"));
    assert_eq!(
        generic.text("RetroArch at /home/retroarch/bin/retroarch"),
        "RetroArch at ~/bin/retroarch"
    );
}

fn failure_report(home: &std::path::Path) -> crate::daedalus::rules::DiagnosisReport {
    let config = home.join(".config/retroarch/retroarch.cfg");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "video_driver = \"vulkan\"\n").unwrap();
    let log = format!(
        "[INFO] [Config]: Loading config: \"{}\".\n\
         [INFO] [Core]: Loading dynamic libretro core from: \"{}/cores/x_libretro.so\"\n\
         [ERROR] Failed to create Vulkan instance.\n\
         [ERROR] [Video]: Cannot open video driver.. Exiting..\n",
        config.display(),
        home.display()
    );
    let mut set = collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        game: Some("Example".into()),
        exit: Some(ExitOutcome::Exited { code: 1 }),
        stderr: Some(log.into_bytes()),
        settings: vec![(config, crate::daedalus::evidence::ConfigLayer::Main)],
        game_file: Some(home.join("games/example.gb")),
        environment: vec![
            ("DISPLAY".into(), Some(":0".into())),
            ("CHEEVOS_TOKEN".into(), Some("abc123".into())),
        ],
        ..LaunchArtefacts::default()
    });
    set.observe(
        EvidenceArea::LaunchEnvironment,
        Trust::Measured,
        "launch environment",
        Fact::EnvironmentVariable {
            name: "SESSION_COOKIE".into(),
            value: Some("opaque-value-without-a-pattern".into()),
        },
    );
    diagnose(&set)
}

#[test]
fn the_details_the_rules_describe_are_removed_from_every_text_field_of_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home/alexandra-q");
    let report = failure_report(&home);
    let raw = serde_json::to_string(&report).unwrap();
    assert!(raw.contains("alexandra-q") && raw.contains("abc123"));

    let redactor = Redactor::new(home.to_str(), Some("alexandra-q"));
    let redacted = redactor.report(&report);
    let text = serde_json::to_string(&redacted).unwrap();
    for private in ["alexandra-q", "abc123", "opaque-value-without-a-pattern"] {
        assert!(!text.contains(private), "{private} survived redaction");
    }
    assert!(text.contains("~/.config/retroarch/retroarch.cfg"));
    // The diagnosis itself is unchanged: same verdict, rules and evidence ids.
    assert_eq!(redacted.verdict, report.verdict);
    assert_eq!(redacted.causes.len(), report.causes.len());
    for (clean, original) in redacted.causes.iter().zip(&report.causes) {
        assert_eq!(clean.rule, original.rule);
        assert_eq!(clean.confidence, original.confidence);
        assert_eq!(clean.supporting, original.supporting);
    }
    assert_eq!(
        redacted.evidence.items().len(),
        report.evidence.items().len()
    );
    // The original still holds the real path, which freshness checks need.
    assert!(
        report
            .evidence
            .items()
            .iter()
            .filter_map(|item| item.fingerprint.as_ref())
            .any(|fingerprint| fingerprint.path.contains("alexandra-q"))
    );
    // Redacting twice changes nothing more.
    assert_eq!(redactor.report(&redacted), redacted);
}

#[test]
fn log_events_keep_their_meaning_after_redaction() {
    let mut set = EvidenceSet::default();
    retroarch::record_log(
        &mut set,
        b"[INFO] [Core]: Loading dynamic libretro core from: \"/home/alex/cores/a_libretro.so\"\n",
        LogStream::Stderr,
    );
    let redacted = redactor().report(&diagnose(&set));
    let event = redacted
        .evidence
        .items()
        .iter()
        .find_map(|item| match &item.fact {
            Fact::LogEvent(event) => Some(event.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(event.value.as_deref(), Some("~/cores/a_libretro.so"));
    assert!(event.excerpt.contains("\"~/cores/a_libretro.so\""));
    assert_eq!(event.signature, "retroarch.core.load_started");
}

#[test]
fn a_displayed_tail_is_the_end_only_redacted_visible_and_bounded() {
    let redactor = Redactor::new(Some("/home/alexandra-q"), Some("alexandra-q"));
    let mut output = String::new();
    for index in 0..200 {
        output.push_str(&format!("[INFO] line {index}\n"));
    }
    output.push_str(
        "[INFO] [Config]: Loading config: \"/home/alexandra-q/.config/retroarch/retroarch.cfg\".\n\
         [INFO] netplay password=hunter2\n\
         \u{1b}[31m[ERROR] Failed to create Vulkan instance.\u{202e}\n",
    );
    let shown = output_tail_for_display(output.as_bytes(), &redactor, 10);
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines.len(), 11, "{shown}");
    assert_eq!(lines[0], "(earlier output is not shown)");
    assert_eq!(lines[1], "[INFO] line 193");
    assert!(
        lines[10].ends_with("Failed to create Vulkan instance.\u{fffd}"),
        "{shown}"
    );
    for absent in ["alexandra-q", "/home/", "hunter2", "\u{1b}", "\u{202e}"] {
        assert!(!shown.contains(absent), "{absent:?} in {shown}");
    }
    assert!(
        shown.contains("~/.config/retroarch/retroarch.cfg"),
        "{shown}"
    );

    // Short output is shown whole, with no notice.
    let whole = output_tail_for_display(b"one\ntwo\n", &redactor, 10);
    assert_eq!(whole, "one\ntwo\n");
    assert_eq!(output_tail_for_display(b"", &redactor, 10), "");

    // A very large capture: only its end is looked at, and a line that
    // straddles the start of that window is dropped, not shown in part.
    let mut large = vec![b'x'; 300 * 1024];
    large.extend_from_slice(b"\nlast line\n");
    let shown = output_tail_for_display(&large, &redactor, 10);
    assert_eq!(shown, "(earlier output is not shown)\nlast line\n");

    // Bytes that are not text do not break it.
    let shown = output_tail_for_display(&[0xff, 0xfe, b'\n', b'o', b'k', b'\n'], &redactor, 10);
    assert!(shown.ends_with("ok\n"), "{shown:?}");
}
