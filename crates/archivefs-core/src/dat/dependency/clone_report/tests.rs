use super::*;

fn game(name: &str, clone_of: Option<&str>) -> DatGameEntry {
    DatGameEntry {
        name: name.to_string(),
        clone_of: clone_of.map(str::to_string),
        ..DatGameEntry::default()
    }
}

fn status_for<'a>(
    reports: &'a [CloneRelationshipReport],
    name: &str,
) -> &'a CloneRelationshipStatus {
    &reports
        .iter()
        .find(|report| report.game_name == name)
        .expect("game present in report")
        .status
}

#[test]
fn a_valid_parent_clone_relationship_survives_parsing() {
    let games = vec![game("Parent", None), game("Clone (USA)", Some("Parent"))];
    let reports = report_clone_relationships(&games);

    assert_eq!(
        *status_for(&reports, "Parent"),
        CloneRelationshipStatus::NoRelationshipDeclared
    );
    match status_for(&reports, "Clone (USA)") {
        CloneRelationshipStatus::Resolved {
            parent_name,
            root_name,
            ..
        } => {
            assert_eq!(parent_name, "Parent");
            assert_eq!(root_name, "Parent");
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn a_multi_hop_chain_resolves_to_the_ultimate_root() {
    let games = vec![
        game("Grandparent", None),
        game("Parent", Some("Grandparent")),
        game("Clone", Some("Parent")),
    ];
    let reports = report_clone_relationships(&games);

    match status_for(&reports, "Clone") {
        CloneRelationshipStatus::Resolved {
            parent_name,
            root_name,
            ..
        } => {
            assert_eq!(parent_name, "Parent");
            assert_eq!(root_name, "Grandparent");
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn a_missing_parent_fails_closed_as_explicitly_unresolved() {
    let games = vec![game("Clone", Some("No Such Parent"))];
    let reports = report_clone_relationships(&games);

    match status_for(&reports, "Clone") {
        CloneRelationshipStatus::MissingParent { declared_reference } => {
            assert_eq!(declared_reference, "No Such Parent");
        }
        other => panic!("expected MissingParent, got {other:?}"),
    }
}

#[test]
fn a_clone_cycle_is_detected_and_never_silently_merged() {
    let games = vec![game("A", Some("B")), game("B", Some("A"))];
    let reports = report_clone_relationships(&games);

    match status_for(&reports, "A") {
        CloneRelationshipStatus::Cycle { declared_reference } => {
            assert_eq!(declared_reference, "B");
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
    match status_for(&reports, "B") {
        CloneRelationshipStatus::Cycle { declared_reference } => {
            assert_eq!(declared_reference, "A");
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}

#[test]
fn a_duplicated_parent_name_is_a_conflicting_reference_not_a_guess() {
    let games = vec![
        game("Parent", None),
        game("Parent", None),
        game("Clone", Some("Parent")),
    ];
    let reports = report_clone_relationships(&games);

    match status_for(&reports, "Clone") {
        CloneRelationshipStatus::ConflictingReference { declared_reference } => {
            assert_eq!(declared_reference, "Parent");
        }
        other => panic!("expected ConflictingReference, got {other:?}"),
    }
}

#[test]
fn an_empty_clone_of_declaration_is_malformed_not_absent() {
    let games = vec![game("Clone", Some(""))];
    let reports = report_clone_relationships(&games);

    assert_eq!(
        *status_for(&reports, "Clone"),
        CloneRelationshipStatus::MalformedDeclaration
    );
}
