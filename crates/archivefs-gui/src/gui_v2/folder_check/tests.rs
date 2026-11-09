use super::*;

fn finding(id: i64, tier: Tier, detected: &str, folder: &str) -> Finding {
    Finding {
        archive_id: id,
        title: format!("Game {id}"),
        path: PathBuf::from(folder).join(format!("Game {id}.zip")),
        folder: PathBuf::from(folder),
        assigned: "SNES".into(),
        assigned_source: Some("folder_alias".into()),
        tier,
        detected: detected.into(),
        lines: vec!["Catalogue X version 1: entry \"E\", SHA-1 ab.".into()],
        has_unh: true,
    }
}

#[test]
fn card_headline_is_plain_and_counts() {
    let findings = vec![
        finding(1, Tier::Confirmed, "NES", "/roms/snes"),
        finding(2, Tier::Confirmed, "NES", "/roms/snes"),
    ];
    let c = cards(&findings, &HashSet::new());
    assert_eq!(c.len(), 1);
    assert_eq!(
        c[0].headline(),
        format!(
            "Your folder is named snes, but 2 games inside match {} catalogue entries. Review them?",
            display_name_for("NES")
        )
    );
    let mixed = vec![
        finding(1, Tier::Confirmed, "NES", "/roms/snes"),
        finding(2, Tier::Probable, "NES", "/roms/snes"),
    ];
    assert!(
        cards(&mixed, &HashSet::new())[0]
            .headline()
            .contains("match or look like")
    );
}

#[test]
fn ambiguous_alone_raises_no_card() {
    let findings = vec![finding(1, Tier::Ambiguous, "NES", "/roms/snes")];
    assert!(cards(&findings, &HashSet::new()).is_empty());
}

#[test]
fn dismissal_is_per_evidence_and_changes_nothing_else() {
    let findings = vec![finding(1, Tier::Confirmed, "NES", "/roms/snes")];
    let mut state = State {
        findings: findings.clone(),
        ..Default::default()
    };
    let card = cards(&state.findings, &state.dismissed)[0].clone();
    state.dismiss(&card);
    assert!(cards(&state.findings, &state.dismissed).is_empty());
    // Findings (and so assignments, which this state cannot touch) are intact.
    assert_eq!(state.findings, findings);
    // New evidence for the same folder shows the card again.
    state
        .findings
        .push(finding(2, Tier::Probable, "NES", "/roms/snes"));
    assert_eq!(cards(&state.findings, &state.dismissed).len(), 1);
}

#[test]
fn review_list_groups_by_detected_system_then_tier() {
    let findings = vec![
        finding(1, Tier::Probable, "NES", "/a"),
        finding(2, Tier::Confirmed, "NES", "/a"),
        finding(3, Tier::Confirmed, "Game Boy", "/a"),
    ];
    let g = grouped(&findings);
    assert_eq!(g.len(), 2);
    let nes = &g["NES"];
    assert_eq!(
        nes.keys().copied().collect::<Vec<_>>(),
        vec![Tier::Confirmed, Tier::Probable]
    );
    assert_eq!(nes[&Tier::Confirmed][0].archive_id, 2);
}

#[test]
fn playable_note_explains_unh_for_nes_only() {
    assert!(playable_note("NES", true).contains(".unh"));
    assert!(playable_note("NES", true).contains("staged for the emulator"));
    assert!(!playable_note("NES", false).contains(".unh"));
    assert!(!playable_note("Game Boy", true).contains(".unh"));
    assert!(playable_note("Game Boy", true).contains("does not necessarily make the ROM playable"));
    assert!(playable_note("NES", false).contains("does not necessarily make the ROM playable"));
}

#[test]
fn choose_system_preview_shows_current_source_suggestion_evidence_and_effect() {
    let f = finding(1, Tier::Confirmed, "NES", "/roms/snes");
    let lines = system_change_preview(
        "SNES",
        "Taken from the folder name.",
        Some("folder_alias"),
        "NES",
        Some(&f),
    )
    .join("\n");
    assert!(lines.contains("source: folder_alias"));
    assert!(lines.contains("Taken from the folder name"));
    assert!(lines.contains(&format!(
        "Folder check suggestion: {} (Confirmed)",
        display_name_for("NES")
    )));
    assert!(lines.contains("Evidence: Catalogue X"));
    assert!(lines.contains("staged for the emulator"));
    assert!(lines.contains("remembered by the game's content"));
    assert!(lines.contains("No game file is changed"));
}

#[test]
fn preview_without_finding_and_with_differing_choice() {
    let none = system_change_preview("SNES", "x.", None, "NES", None).join("\n");
    assert!(none.contains("No folder check result"));
    assert!(none.contains("not recorded"));
    let f = finding(1, Tier::Probable, "NES", "/r");
    let differ = system_change_preview("SNES", "x.", None, "Game Boy", Some(&f)).join("\n");
    assert!(differ.contains("differs from the folder check suggestion"));
}

#[test]
fn constants_carry_required_wording() {
    assert_eq!(
        READ_ONLY_NOTE,
        "Checking only reads. Nothing moves until you review the preview and approve it."
    );
    assert!(KNOWN_LIMITS.contains("renamed or moved file"));
    assert!(KNOWN_LIMITS.contains("remembered by the game's content"));
    assert!(KNOWN_LIMITS.contains("Undo"));
    assert!(KNOWN_LIMITS.contains("Probable"));
}

#[test]
fn state_runs_nothing_until_started() {
    let state = State::default();
    assert!(!state.busy());
    assert!(!state.ran);
    assert!(!state.has_more());
    assert!(state.findings.is_empty());
}

#[test]
fn unh_note_in_preview_needs_nes_and_a_real_unh_file() {
    let mut f = finding(1, Tier::Confirmed, "NES", "/r");
    let with = system_change_preview("SNES", "x.", None, "NES", Some(&f)).join("\n");
    assert!(with.contains(".unh"));
    f.has_unh = false;
    let without = system_change_preview("SNES", "x.", None, "NES", Some(&f)).join("\n");
    assert!(!without.contains(".unh"));
    assert!(
        !system_change_preview("SNES", "x.", None, "NES", None)
            .join("\n")
            .contains(".unh")
    );
}

#[test]
fn progress_text_separates_each_kind_and_shows_stopped_early() {
    let t = progress_text(40, 100, true, 1, 2, 3);
    assert!(t.starts_with("Stopped early - 40 of 100 checked."));
    assert!(t.contains("1 could not be read."));
    assert!(t.contains("2 too large to check."));
    assert!(t.contains("3 skipped"));
    let ok = progress_text(100, 100, false, 0, 0, 0);
    assert!(ok.starts_with("Checked 100 of 100"));
    assert!(!ok.contains("too large") && !ok.contains("skipped") && !ok.contains("read"));
}

#[test]
fn partial_catalogue_list_is_not_cached_and_stop_is_surfaced() {
    let mut state = State::default();
    let (sender, receiver) = mpsc::channel();
    state.receiver = Some(receiver);
    let cancelled = RunOutput {
        checked: 3,
        cancelled: true,
        ..Default::default()
    };
    sender
        .send(Message::Done(Ok((Arc::new(Vec::new()), false, cancelled))))
        .unwrap();
    state.poll();
    assert!(state.catalogues.is_none());
    assert!(state.stopped_early);
    assert_eq!(state.next, 3);

    let (sender, receiver) = mpsc::channel();
    state.receiver = Some(receiver);
    sender
        .send(Message::Done(Ok((
            Arc::new(Vec::new()),
            true,
            RunOutput::default(),
        ))))
        .unwrap();
    state.poll();
    assert!(state.catalogues.is_some());
    assert!(!state.stopped_early);

    // Complete list but the check itself was cancelled: not cached either.
    let mut state = State::default();
    let (sender, receiver) = mpsc::channel();
    state.receiver = Some(receiver);
    let cancelled = RunOutput {
        cancelled: true,
        ..Default::default()
    };
    sender
        .send(Message::Done(Ok((Arc::new(Vec::new()), true, cancelled))))
        .unwrap();
    state.poll();
    assert!(state.catalogues.is_none());
}

#[test]
fn limits_say_probable_results_never_move_on_their_own() {
    assert!(KNOWN_LIMITS.contains("never moves on its own"));
}
