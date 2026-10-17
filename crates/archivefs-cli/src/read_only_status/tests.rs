use super::*;

#[test]
fn local_lifecycle_state_never_claims_an_update_check_or_machine_wide_absence() {
    assert_eq!(
        emulator_state(LifecycleState::InstalledCurrent),
        "InstalledLocalVersionKnownUpdateNotEvaluated"
    );
    assert_eq!(
        emulator_state(LifecycleState::Missing),
        "NoCandidateInInspectedSources"
    );
}

#[test]
fn arguments_allow_only_read_only_forms() {
    for command in ["emulators", "activity", "history"] {
        let sub = if command == "emulators" {
            "status"
        } else {
            "list"
        };
        for args in [
            vec![],
            vec![sub],
            vec!["--json"],
            vec![sub, "--json"],
            vec!["--json", sub],
        ] {
            assert!(
                options(
                    command,
                    &args.into_iter().map(String::from).collect::<Vec<_>>()
                )
                .is_ok()
            );
        }
        assert_eq!(options(command, &["--help".into()]).unwrap(), (false, true));
        for args in [
            vec!["apply"],
            vec!["launch"],
            vec!["--json", "--json"],
            vec![sub, sub],
            vec!["--help", "--json"],
            vec!["--root", "/tmp"],
        ] {
            assert!(
                options(
                    command,
                    &args.into_iter().map(String::from).collect::<Vec<_>>()
                )
                .is_err()
            );
        }
    }
    for command in ["emulators", "activity", "history"] {
        assert!(HELP.contains(command));
    }
    assert!(HELP.contains("read") || HELP.contains("Read"));
}

#[test]
fn unavailable_locations_remain_visible_without_cwd_fallback() {
    let unavailable = || Err("synthetic root resolution failure".into());
    let paths = HistoryPaths {
        rename: unavailable(),
        views: unavailable(),
        shared: unavailable(),
        backups: unavailable(),
        database: unavailable(),
    };
    for command in ["activity", "history"] {
        let report: serde_json::Value =
            serde_json::from_str(&history_output(command, &paths, true).unwrap()).unwrap();
        assert!(report["records"].as_array().unwrap().is_empty());
        assert_eq!(report["problems"].as_array().unwrap().len(), 4);
        assert!(
            report["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["state"] == "unavailable" && s["path"].is_null())
        );
        let human = history_output(command, &paths, false).unwrap();
        assert!(human.contains("unavailable"));
        assert!(human.contains("Warning:"));
    }
}

#[test]
fn unreadable_directory_is_not_reported_as_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("not-a-directory");
    std::fs::write(&path, b"preserve").unwrap();
    let mut sources = Vec::new();
    let mut problems = Vec::new();
    assert!(directory("test", &Ok(path.clone()), &mut sources, &mut problems).is_none());
    assert_eq!(sources[0].state, "unavailable");
    assert_eq!(problems.len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), b"preserve");
}
