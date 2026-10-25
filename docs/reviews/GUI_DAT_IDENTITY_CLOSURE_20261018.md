# GUI DAT Identity wiring closure — 18 October 2026

Historical `feature/gui-dat-identity-wiring`, exact tip
`34f016c073fa0abe2d50a066e569e006ea16c519`, is functionally superseded. Its
preservation ref is
`refs/archive/emuwiz/2026-10-16/feature/gui-dat-identity-wiring`.
Comparison baseline: fresh `origin/main`
`7f3d6d13ccc0132154844a9c461fff5ce6bd23be`, including managed DAT completion
`403c2f419a29864639506a6f38655322126ef301` and Game Verification Guidance.
The historical tip is not an ancestor of main; this is a capability comparison,
not an ancestry claim or a reason to replay its seven commits.

## Capability comparison

| Historical responsibility | Current connection |
|---|---|
| Save and reload every source's identity, candidates and provenance | The shared database API and [database worker](../../crates/archivefs-gui/src/database_load.rs) reconstruct all per-item summaries. The original `0008_library_dat_identities.sql` is byte-identical; later additive migrations remain registered. |
| Select a library item and show its DAT result | [library_view](../../crates/archivefs-gui/src/library_view.rs) selects its persisted summaries; [selected_game_panel](../../crates/archivefs-gui/src/selected_game_panel.rs) passes them to the existing [DAT panel](../../crates/archivefs-gui/src/dat_identity_panel.rs). |
| Explain confidence, missing evidence, ambiguous candidates, conflicting sources and set dependencies | That panel retains every source separately, candidate names, hashes, freshness, catalogue revision, digest and set/dependency details. Its richer presentation replaces the seven old panel tests rather than retaining their names. |
| Native Blue identity and Game Details | [backend](../../crates/archivefs-gui/src/gui_v2/backend.rs) uses `current_library_dat_evidence`; [Game Details](../../crates/archivefs-gui/src/gui_v2/pages.rs) links View evidence/Review identity and existing verification. [Guidance](../../crates/archivefs-gui/src/gui_v2/game_verification_guidance.rs) distinguishes selecting, validating, auditing and rechecking. |
| DAT management and completed audit results reach the library | [native_workflows](../../crates/archivefs-gui/src/gui_v2/native_workflows.rs) hosts existing DAT controls and reports identity reloads; Blue reloads its library after successful work. The shared configured catalogue resolver includes trusted local and typed managed sources. |
| Detailed multi-source history remains reachable | Blue's explicit full technical interface opens the existing Library with selection context through [legacy](../../crates/archivefs-gui/src/gui_v2/legacy.rs). No second panel or identity model is added. |

The eighth absent historical test, `migration_0008_is_registered_and_last`, has
an obsolete “last migration” assertion. Current
`migrations_0008_through_0010_are_registered_in_order` and
`v08_schema_upgrades_additively_from_v07_and_keeps_prior_rows` retain registration
and upgrade/preservation coverage. An old test name or old GUI root location is
not a missing user-facing connection. DOS evidence/launch predecessors are
separate capabilities, not a reason to restore historical GUI wiring.

## Small current correction

The comparison found one actual inconsistency: Blue Review Identity read saved
rows with `source_available=true`, without the configured catalogue digest
checks already used by library badges. Opening View evidence could therefore
revive an exact match after its source was disabled, removed or changed.

The correction connects that loader to the existing configured catalogue map,
shared file-proof reconstruction, and `check_saved_identity`. Only Current
results enter its live knowledge projection. A completed lookup supersedes the
library snapshot's older exact flag; the page waits for that lookup before showing
a verdict. Independent file-derived identity remains valid. Saved rows and full per-source
technical history remain intact. No new matcher, confidence model, catalogue
policy or source-selection policy is introduced.

## Validation and limits

Synthetic regression and focused final results, exact candidate review and
promotion receipts are retained in the audit ledger under
`2026-10-18-gui-dat-identity-wiring`. Tests use disposable ROM bytes, DATs and
databases only. This task performs no production scan, production ROM write or
real emulator launch.

Blue Review Identity still presents one result through its existing `best`
selection; the full technical interface retains all per-source results. A new
cross-source conflict policy or a duplicate Selected Evidence panel is outside
this closure. Active Selected Evidence and Discovery/Import work is untouched.
Proofless legacy evidence requires a new explicit audit/recheck; filesystem
metadata can limit detection of changes until a content rehash. Verification
does not itself establish emulator readiness or permission to launch.

## Retirement handoff

The exact historical ref and SHA above are handed to the existing retirement
process with closure evidence and final promotion SHA. This task does not delete
that ref, alter the recovery bundle or retire another task's worktree/cache.
The retirement owner must perform its existing exact-SHA, preservation and
lifecycle checks; historical closure alone does not override those gates.
