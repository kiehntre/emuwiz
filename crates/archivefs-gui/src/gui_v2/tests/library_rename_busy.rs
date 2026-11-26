//! Every route into Rename while Organise & Export runs a job (Library
//! Rename selected / Rename all selected, the landing page's Rename verified
//! games, Quick Rename): the running preview or apply keeps its mode, scope
//! and state, the person is told why, and renaming works normally once the
//! job has finished.
use super::super::CanonicalOrganisationJobKind as Kind;
use super::*;
use crate::rom_organisation_page::scope::{OrganisationScope, ScopedGame};
use archivefs_core::dat::rom_organisation::OrganisationMode;

const BUSY: &str =
    "Organise & Export is busy. Rename the selection when the current job has finished.";

/// What Rename selected (`hidden_by_filter: 0`) and Rename all selected
/// (`hidden_by_filter > 0`) hand to the shared entry point.
fn selection(hidden_by_filter: usize) -> OrganisationScope {
    OrganisationScope::Games {
        games: vec![ScopedGame {
            archive_id: 7,
            path: "/roms/gbc/a.gbc".into(),
            known_missing: false,
        }],
        hidden_by_filter,
    }
}

#[test]
fn renaming_a_selection_waits_for_a_running_preview_or_apply() {
    for kind in [Kind::Preview, Kind::Apply] {
        for hidden_by_filter in [0, 3] {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.canonical_organisation
                .set_mode(OrganisationMode::MoveRealFile);
            let settings = app.canonical_organisation.input_fingerprint();
            let id = app
                .activity
                .queue("Organisation", Route::Section(Section::Build), false);
            let generation = app.canonical_organisation_generation;
            app.canonical_organisation_job = Some(super::super::CanonicalOrganisationJob {
                id,
                kind,
                generation,
                input_fingerprint: settings.clone(),
            });

            app.open_bulk_rename(selection(hidden_by_filter));

            let case = format!("{kind:?}, hidden {hidden_by_filter}");
            assert_eq!(
                app.canonical_organisation.input_fingerprint(),
                settings,
                "{case}: mode or scope changed"
            );
            assert!(!app.canonical_organisation.uses_scope(), "{case}");
            assert!(app.canonical_organisation.scope().is_none(), "{case}");
            let job = app.canonical_organisation_job.as_ref().expect("job kept");
            assert_eq!(
                (job.id, job.kind, job.generation),
                (id, kind, generation),
                "{case}"
            );
            assert_eq!(job.input_fingerprint, settings, "{case}");
            assert_eq!(
                app.library_selection.notice.as_deref(),
                Some(BUSY),
                "{case}"
            );

            // The job finishes through the real finisher; renaming then opens.
            let worker = app.canonical_organisation.clone();
            app.finish_canonical_organisation_job(id, kind, Box::new(worker), generation);
            assert!(app.canonical_organisation_job.is_none(), "{case}");
            app.open_bulk_rename(selection(hidden_by_filter));
            assert!(app.canonical_organisation.uses_scope(), "{case}");
            assert_eq!(
                app.canonical_organisation.scope(),
                Some(&selection(hidden_by_filter)),
                "{case}"
            );
        }
    }
}

/// An app in Move mode with a running job of `kind`, as the shell starts one.
fn busy_moving_app(context: &egui::Context, kind: Kind) -> (App, u64, u64, String) {
    let mut app = fixture(context);
    app.canonical_organisation
        .set_mode(OrganisationMode::MoveRealFile);
    let settings = app.canonical_organisation.input_fingerprint();
    let id = app
        .activity
        .queue("Organisation", Route::Section(Section::Build), false);
    let generation = app.canonical_organisation_generation;
    app.canonical_organisation_job = Some(super::super::CanonicalOrganisationJob {
        id,
        kind,
        generation,
        input_fingerprint: settings.clone(),
    });
    (app, id, generation, settings)
}

fn assert_job_untouched(app: &App, kind: Kind, id: u64, generation: u64, settings: &str) {
    assert_eq!(
        app.canonical_organisation.input_fingerprint(),
        settings,
        "{kind:?}"
    );
    assert!(
        !app.canonical_organisation.uses_scope(),
        "{kind:?}: switched to Rename"
    );
    let job = app.canonical_organisation_job.as_ref().expect("job kept");
    assert_eq!((job.id, job.kind, job.generation), (id, kind, generation));
    assert_eq!(job.input_fingerprint, settings);
}

fn finish(app: &mut App, kind: Kind, id: u64, generation: u64) {
    let worker = app.canonical_organisation.clone();
    app.finish_canonical_organisation_job(id, kind, Box::new(worker), generation);
    assert!(app.canonical_organisation_job.is_none());
}

#[test]
fn the_landing_rename_button_waits_for_a_running_preview_or_apply() {
    use crate::rom_organisation_page::ORGANISATION_BUSY_NOTICE;
    for kind in [Kind::Preview, Kind::Apply] {
        let context = egui::Context::default();
        let (mut app, id, generation, settings) = busy_moving_app(&context, kind);
        app.open_rename_verified_games();
        assert_job_untouched(&app, kind, id, generation, &settings);
        assert_eq!(
            app.organisation.busy_notice.as_deref(),
            Some(ORGANISATION_BUSY_NOTICE),
            "{kind:?}"
        );
        // The notice is on the job card the page shows.
        app.go(Route::Section(Section::Build));
        let shown = text(&frame(&context, &mut app, [1400.0, 1000.0]));
        assert!(
            shown
                .iter()
                .any(|line| line.contains("Organise & Export is busy")),
            "{kind:?}: {shown:?}"
        );

        finish(&mut app, kind, id, generation);
        app.open_rename_verified_games();
        assert!(app.canonical_organisation.uses_scope(), "{kind:?}");
        assert!(app.organisation.busy_notice.is_none(), "{kind:?}");
    }
}

#[test]
fn quick_rename_waits_for_a_running_preview_or_apply() {
    for kind in [Kind::Preview, Kind::Apply] {
        let context = egui::Context::default();
        let (mut app, id, generation, settings) = busy_moving_app(&context, kind);
        app.go(Route::QuickRename);
        let shown = text(&frame(&context, &mut app, [1400.0, 1000.0]));
        assert_job_untouched(&app, kind, id, generation, &settings);
        assert!(
            shown
                .iter()
                .any(|line| line.contains("Organise & Export is busy")),
            "{kind:?}: {shown:?}"
        );

        finish(&mut app, kind, id, generation);
        let _ = frame(&context, &mut app, [1400.0, 1000.0]);
        assert!(app.canonical_organisation.uses_scope(), "{kind:?}");
    }
}
