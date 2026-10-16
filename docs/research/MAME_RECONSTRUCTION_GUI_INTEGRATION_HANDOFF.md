# MAME merged reconstruction: GUI integration handoff

Status (2026-10-16): the backend and an isolated GUI-v2 adapter are complete
and tested. The adapter is **not yet reachable** from the Organisation page.
The page's call site is in `crates/archivefs-gui/src/gui_v2/organisation.rs`,
which the active Blue GUI integration (`feature/emuwiz-blue-gui-20261016`)
also edits. That integration was mid-merge when this work was done, so
`organisation.rs`, `backend.rs`, `tests.rs` and `tests/mame_health.rs` were
deliberately left unchanged. The only shared-file edit is one `mod` line in
`gui_v2/mod.rs`.

Until the steps below are done, the Organisation page still runs the older
inline flow from main. That flow previews, publishes and undoes on the GUI
thread and publishes through `apply_staged_reconstruction_output`.

## What is ready

- `gui_v2/mame_reconstruction.rs`: `MameReconstructionPanel` with
  `show(&mut self, ui)`. It covers the whole journey:
  1. Choose the machine (folder, verified MAME 0.174 DAT, set).
  2. Inspect dependencies (parent and clones; BIOS, ROM-source and device sets
     stored separately; members that cannot be resolved; CHDs; samples; the
     runnability note).
  3. Read-only preview.
  4. Typed confirmation (`PUBLISH MAME 1 OUTPUTS`) and publish.
  5. Results history and Undo (`UNDO MAME <parent>`).

  The evidence refresh, preview, publish, Undo and history load all run on a
  background thread, and the page repaints every 100 ms while a job runs. No
  Cancel control is offered because none of the backend steps can be
  cancelled. Only one job runs at a time; every action is disabled while a
  job runs.
- Publishing goes through `review_reconstruction_publication(plan, None)` and
  `ReviewedReconstructionPublication::apply`. Both are source- and
  target-bound. The publish step rebuilds the plan and refuses with a stale
  plan if it differs from the reviewed one. It never replaces an existing set:
  replacement needs target evidence, which this GUI does not offer.
- Success requires all three of the following; anything else is
  `PublishOutcome::Failed` with its transaction kept:
  - the transaction is `Applied`;
  - every entry is `Applied`;
  - the published copy re-verifies.

  An apply error after a journal was reserved is also `Failed`, not "nothing
  changed".
- Undo re-reads the transaction from its journal on the worker thread. It
  runs only when `undo_actionable` holds, which means an `Applied`,
  `Applying` or `RollingBack` entry exists. A result counts as "rolled back"
  only when the rollback result, the transaction state and every entry
  receipt agree. These are the 470010cb rules, ported unchanged.
- A clone-free parent is reported as "No merged parent/clone reconstruction
  is needed for this game". It is not reported as an identity fault.

## Integration steps (in organisation.rs, after Blue lands)

1. In `OrganisationState`, replace the fields `mame_root`, `mame_dat`,
   `mame_plan`, `mame_message`, `mame_evidence_set`, `mame_confirmation`,
   `mame_publish_pending`, `mame_undo_pending`, `mame_history` and
   `mame_history_loaded` with:
   `pub(super) mame_reconstruction: super::mame_reconstruction::MameReconstructionPanel`
   (its `Default` uses the production executor).
2. In the `OrganisationView::MameNormalizer` arm, call
   `self.organisation.mame_reconstruction.show(ui)` instead of
   `show_mame_normalizer(ui, &mut self.organisation)`. Remove the
   `ensure_mame_history_loaded` call, because the panel loads its own history
   on a background thread on first show.
3. Delete the inline implementations that the module now owns:
   - `show_mame_normalizer` and `current_mame_reconstruction_plan`;
   - `mame_staging_root` and `is_mame_reconstruction`;
   - `mame_publication_succeeded`, `mame_failure_reasons`,
     `mame_undo_actionable`, `mame_rollback_verified`, `mame_entry_states`
     and `mame_history_status`;
   - `load_mame_history`, `refresh_mame_evidence`, the two confirmation
     phrase functions, and
     `OrganisationState::{ensure_mame_history_loaded, publish_mame,
     record_mame_publication, undo_mame, record_mame_undo}`.

   Then drop the imports that become unused, including
   `apply_staged_reconstruction_output`.
4. Other callers:
   - `history_view.rs:1192` reads `self.organisation.mame_history`. Point it
     at `self.organisation.mame_reconstruction.history`.
   - `mod.rs:1564` assigns `self.organisation.mame_history`. Assign
     `...mame_reconstruction.history` instead.
   - `tests.rs:1895` and `tests/mame_health.rs:334,462,463,510,511,630` assert
     on `mame_publish_pending`, `mame_undo_pending` and `mame_plan`. Use
     `mame_reconstruction.publish_pending`, `.undo_pending` and `.preview`.
5. The tests in `organisation.rs` from
   `mame_ready_preview_exposes_publish_but_blocked_preview_does_not` through
   `mame_publish_confirmation_is_deterministic_and_not_a_yes_no_prompt`
   exercise the deleted inline handlers. Their safety coverage now lives in
   `gui_v2/mame_reconstruction/tests.rs` (see the list below). Delete them, or
   re-point their assertions at `run_publish`/`run_undo` and the panel. Do not
   delete any without checking that a replacement covers it, for example
   `persisted_mame_failure_journals_stay_truthful_after_a_restart`, which
   maps to `history_status` over `load_history`.
6. Remove the `#![cfg_attr(not(test), allow(dead_code))]` line at the top of
   `mame_reconstruction.rs`.
7. Under Blue, swap `theme::INFO/SUCCESS/WARNING/DANGER` for
   `theme::info/success/warning/danger(ui)` if Blue's helpers are the
   convention by then. The module already avoids the `→` and `✓` glyphs.

## Tests in the adapter (synthetic only)

- Merged output built from packed ZIP members, without the BIOS ROM.
- Donor archive unchanged.
- Missing members, wrong bytes, duplicate sources, and foreign or unreadable
  destinations.
- Source change after preview (stale), and a destination that appears after
  preview.
- Changed inputs make the preview stale in the panel.
- Interrupted Apply (journal left `Applying`): recovery is required, further
  publication is refused, and Undo reports success only when the copy is
  gone.
- A real inner `ApplyFailed` (read-only folder): reported as Failed, with no
  Undo offered.
- Undo of a changed output is never reported as success, including on
  repeat.
- Partial Undo and its retry.
- Repeated Undo is refused.
- A second preview after publishing shows "already published".
- Refusal mapping.
- Clone-free game.
- A blocked worker keeps frames rendering, and every action is disabled while
  it runs.
- A worker crash during Undo reports an unknown outcome.
- The full panel journey: render, confirm, publish, Undo.

## Remaining limitations (truthful scope)

- Only families with at least one clone are rebuilt. A clone-free parent is
  explained, not rebuilt.
- The output holds the parent's and clones' own ROMs only. It is not
  self-contained. BIOS, device, CHD and sample sets stay separate, are listed
  in the dependency step, and are not checked by the rebuild. A rebuilt set is
  never described as runnable.
- The destination is always `<MAME folder>/<parent>.zip`. In a packed
  split or non-merged folder, `<parent>.zip` usually already exists, so the
  preview shows "Something already exists at the destination and will not be
  replaced". Replacing an existing set
  (`ReconstructionPublicationAction::ReplaceExisting`) is not exposed in the
  GUI. Exposing it is an owner decision, because it rewrites a user archive.
  Publishing works when the parent ZIP is absent, for example an extracted
  folder, or a folder holding only non-merged clone ZIPs.
- An unreadable ZIP named like a family set refuses the whole preview.
- Only the pinned MAME 0.174 Arcade DAT is accepted, as on main.
- Progress is reported by phase, not as a percentage.
