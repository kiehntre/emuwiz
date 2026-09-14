# Universal Library Publisher Profiles — Phase 1 and Phase 2C status

Post-0.9.0 feature work. Phase 1 planning remains preview-only. Phase 2C adds an explicit, typed-confirmation GUI Apply/Rollback path for the proven hardlink/symlink transaction foundation; no frontend configuration file (`es_systems.xml`, RomM API calls, etc.) is ever written. See §9 for the planning-side structural proof.

## 1. Inventory of existing organisation/publishing code (before writing anything new)

| Area | What already exists | Where |
|---|---|---|
| 1G1R / Playing Library election | Full deterministic election engine (region/language/revision/parent-clone tiers, explainable, no opaque score) producing `PlayingLibraryPlan` / `ElectedGame` / `LinkedLibraryOperation` | `crates/archivefs-core/src/playing_library/{mod,model}.rs` |
| RomM projection | `build_romm_projection` — projects a `PlayingLibraryPlan` into `<root>/roms/<slug>/`, gated on `DatPlatformIdentity` Strong confidence, apply-capable | `crates/archivefs-core/src/playing_library/romm_projection.rs` |
| ES-DE/RetroDECK projection | `build_retrodeck_projection` — the same pattern for an ES-DE-compatible tree, including `EsDeVisibility`/Flatpak same-path-bind contracts and `es_de_publish` gamelist wiring | `crates/archivefs-core/src/playing_library/retrodeck_projection.rs` |
| RomM platform mapping | `production_romm_slug`/`production_romm_status`, a reviewed, tiered (override → live cache → vetted static table) canonical-platform → slug resolver with an explicit `Mapped/Unmapped/Ambiguous/Unsupported` vocabulary | `crates/archivefs-core/src/platform_evidence_fusion/romm_platform_mapping.rs` |
| ES-DE platform mapping | `es_de_system_for_platform`/`ES_DE_SYSTEM_MAP`, a reviewed canonical-platform → ES-DE system-folder table with an equivalence fallback | `crates/archivefs-core/src/launch/es_de_export.rs` |
| Rename/organisation | `OrganisationMode` (RenameInPlace/MoveRealFile/OrganiseSymlinkOnly/BuildLinkedLibrary), a durable-journal rename/link apply+rollback engine | `crates/archivefs-core/src/dat/rename_apply/`, `crates/archivefs-core/src/dat/rom_organisation/` |
| Canonical platform registry | `crate::platform::PLATFORMS`, the single canonical-platform-id source of truth | `crates/archivefs-core/src/platform/mod.rs` |
| DAT authority / identity | `DatPlatformIdentity` (`Unknown`/`Resolved{confidence}`/`Ambiguous`), `DatPlatformConfidence` (`Weak`/`Corroborated`/`Strong`) | `crates/archivefs-core/src/dat/identity.rs` |
| Media-set handling | A dedicated `media_set` module (CUE/GDI/M3U companion resolution) already feeds the 1G1R election's companion-file model | `crates/archivefs-core/src/media_set/` |
| GUI surface | `playing_library_page.rs` (2,252 lines) already renders RomM/RetroDECK projection previews and drives their apply engines as *modes* of the Library Organisation page, not a separate sidebar destination | `crates/archivefs-gui/src/playing_library_page.rs` |

**Conclusion**: the real gap was never "no publishing logic exists" — it was that RomM and ES-DE/RetroDECK each got their *own* projection module, with no shared, strongly-typed conflict/safety/explanation vocabulary and no seam a third or fourth frontend could reuse without copying an entire projection module. Publisher Profiles Phase 1 is a **generalization** of the existing `romm_projection`/`retrodeck_projection` pattern, not a replacement for either — both existing, apply-capable modules are untouched.

## 2. Architecture

New module: `crates/archivefs-core/src/publisher_profile/`

```
publisher_profile/
├── mod.rs                    – module doc, re-exports, Phase 2 execution-boundary note
├── model.rs                  – every generic type (below)
├── romm.rs                   – RomM PublisherProfile + platform-mapping wrapper
├── es_de.rs                  – ES-DE PublisherProfile + platform-mapping wrapper
├── planner.rs                – the one frontend-agnostic planning engine
├── destination_inspection.rs – read-only existing-destination inspection
└── tests.rs                  – synthetic test matrix
```

`planner::build_publisher_plan` never references RomM or ES-DE by name. It takes a `PublisherProfile` (what a frontend wants) plus a caller-already-resolved `PublisherPlatformMapping` (which `romm::resolve_romm_platform_mapping`/`es_de::resolve_es_de_platform_mapping` compute, each delegating straight to the existing production tables above) plus an existing `PlayingLibraryPlan`. Adding Pegasus, LaunchBox, ROMNight, or Steam later means adding one `PublisherFrontend` variant and one small profile-construction module — the planner itself does not change.

### Generic model (task section 2)

| Type | Role |
|---|---|
| `PublisherFrontend` | `RomM` \| `EsDe` (extensible) |
| `PublisherProfile` | frontend + path/naming/media/metadata rules + accepted extensions + declared unsupported features |
| `PublisherPathRule` / `PathSegment` | how a destination root + resolved platform folder become a parent directory |
| `PublisherNamingRule` | Phase 1: `PreserveSourceFileName` only |
| `PublisherMediaRule` | Phase 1: `PublishElectedFilesUnchanged` only |
| `PublisherMetadataRule` | Phase 1: `None` — no metadata file is ever read or required |
| `PublisherPlatformMapping` | `Mapped{folder}` \| `Unmapped` \| `Ambiguous` \| `Unsupported`, always built from the real per-frontend table |
| `PublisherActionKind` | `Hardlink` \| `Symlink` \| `Copy` \| `DirectoryCreate` \| `MetadataWrite` \| `PlaylistCreate` — never executed in Phase 1 |
| `PublisherActionSafety` | `SafeToAct` \| `ReviewRequired` \| `Blocked` \| `Unsupported` |
| `DestinationState` | `Unknown` \| `Missing` \| `AlreadyCorrect` \| `Conflicting` \| `Stale` |
| `PublisherConflict` | `DestinationExistsDifferentContent` \| `DestinationPlanCollision` \| `CaseFoldCollision` \| `MultipleReleasesSameName` \| `RegionCollision` |
| `PublisherWarning` | 8 variants covering platform/media/extension/representation/BIOS/existing-destination concerns |
| `PublisherPlanItem` / `PublisherCompanionItem` | one planned publication, launcher + companions, each with its own reason/safety/conflicts/warnings |
| `PublisherPlan` / `PublisherPlanSummary` | the full read-only result plus the dry-run count shape from task section 17 |

## 3. RomM profile

`romm::romm_profile()`: destination layout `<destination_root>/roms/<slug>/`, matching `build_romm_projection`'s own existing `romm_root = destination_root.join("roms").join(slug)` exactly. `romm::resolve_romm_platform_mapping` delegates to `production_romm_status`/`production_romm_slug` — the same tiered (override → live cache → vetted static table) resolution `build_romm_projection` already trusts. `accepted_extensions` is deliberately empty ("not reviewed yet", never "accepts everything") because RomM's own scanner is broad and this crate has not independently reviewed an exhaustive list. `unsupported_features`: `playlist_generation`, `bios_separation`, `metadata_write`, `config_write`.

Multi-disc releases, CHD, and archive formats are all handled by **passthrough**: Phase 1 publishes exactly whichever file the 1G1R election already resolved as the launcher (a CUE, GDI, M3U, CHD, or loose ROM), unchanged, plus its companions. No representation is swapped and no playlist is generated — see §5.

## 4. ES-DE profile

`es_de::es_de_profile()`: destination layout `<destination_root>/<system>/`, matching `retrodeck_projection`'s own existing ES-DE-compatible tree and ES-DE's real `%ROMPATH%/<system>/` convention. `es_de::resolve_es_de_platform_mapping` delegates to `es_de_system_for_platform` (the same table `retrodeck_projection`/`es_de_publish` already use), including its `equivalent_platform_ids` fallback. **No `es_systems.xml` or `gamelist.xml` is ever written** by this profile in Phase 1 — `unsupported_features` names `gamelist_xml_write` and `es_systems_xml_write` explicitly rather than silently doing nothing.

ES-DE's own table draws no Ambiguous/Unsupported distinction the way RomM's does — a platform is either in the vetted table (directly or via a recognized equivalence) or it is `Unmapped`. Phase 1 reports that honest default rather than inventing a finer status the underlying table doesn't itself carry.

## 5. Path rules, media handling, and representation selection

Every item's destination file name is the source file's own name, verbatim (`PublisherNamingRule::PreserveSourceFileName`) — Phase 1 never renames anything, matching the explicit "avoid unnecessary renaming, never rename source files" instruction.

Representation selection (CUE/BIN vs CHD, GDI vs CHD, ADF vs IPF, TZX vs TAP) is **not re-decided** by this module. The existing 1G1R election has already picked exactly one launcher file per family (that is what `ElectedGame::launcher_operation` *is*); Publisher Profile planning only ever passes that already-elected representation straight through. This was a deliberate scope decision, not an oversight: re-deciding representation preference a second time, independently of the election that already made this decision, would risk disagreeing with it. If a future profile needs representation-specific behavior beyond passthrough (e.g. "RomM prefers CHD, reject raw CUE/BIN"), that is a Phase 2B extension to `PublisherMediaRule`, not a Phase 1 gap.

Multi-disc/multi-file releases: every companion the election resolved (BIN/audio tracks, other GDI tracks, other M3U discs) is projected alongside the launcher. A companion whose source path has no file name (a real, if rare, malformed-election edge case) is *never* silently dropped — it produces `PublisherWarning::IncompleteMediaSet` and the item is not silently marked safe.

## 6. Destination-collision semantics (task section 15)

`planner::detect_plan_collisions` runs two `BTreeMap`-keyed passes — never pairwise `O(N²)` comparison (task section 24):

1. **Exact collision** (`DestinationPlanCollision`): two elections propose the literal same destination path. Both contenders are `Blocked`.
2. **Case-fold collision** (`CaseFoldCollision`): two elections propose destinations that only clash after case-folding (`Game.rom` vs `GAME.rom`) but are not byte-identical. Reported *only* when the case-folded group actually contains more than one distinct exact path — a group that is really just the same exact collision repeated is reported once, as the sharper `DestinationPlanCollision`, never twice.

`DestinationExistsDifferentContent` is reported separately, by the read-only existing-destination inspection pass (§7), never confused with a plan-vs-plan collision.

`MultipleReleasesSameName`/`RegionCollision` are declared in the type but not yet independently populated in Phase 1 — the 1G1R election upstream of this planner already prevents two same-region/same-name releases from both being elected in the first place (that is its own job), so no synthetic case currently exercises these two variants. They are kept as named future extension points rather than removed, since a future non-1G1R input source might need them.

## 7. Existing-destination inspection (task section 16)

`destination_inspection::inspect_destination` is the *only* filesystem I/O anywhere in this feature, and it is exclusively a read: `std::fs::symlink_metadata` + `std::fs::read_link`, no write primitive anywhere in the module (see §9's structural test). Called only when a caller supplies `existing_destination_root: Some(..)`; with `None`, every item's `DestinationState` stays `Unknown` rather than a guessed `Missing`.

| Real state at destination | `DestinationState` |
|---|---|
| Nothing there | `Missing` |
| A symlink pointing at exactly this item's source | `AlreadyCorrect` |
| A symlink pointing somewhere else | `Conflicting` |
| A symlink whose target no longer exists | `Stale` |
| A regular file/directory already occupies it | `Conflicting` (Phase 1 never hashes content to guess otherwise) |

## 8. Relationship with 1G1R / Playing Library

Publisher Profile planning **consumes** an already-built `PlayingLibraryPlan` — it never re-scans, re-hashes, or re-elects. This is the same seam `romm_projection`/`retrodeck_projection` already use; Publisher Profiles is a generalization of that seam, reusable by future frontends, not a second election system (task section 11's explicit instruction). Region/revision/parent-clone selection remain entirely the existing 1G1R planner's job — a synthetic test (`fgh_elected_release_identity_flows_through_unchanged`) confirms an already-region/revision-qualified `dat_entry_name` passes through this planner completely unchanged.

## 9. Zero-side-effect proof (task section 25)

Two structural checks run as part of the crate's own test suite (`publisher_profile::tests::zero_side_effects`):

1. `planning_never_creates_a_destination` — plans against a destination root that is deliberately never created on disk, then asserts the root still does not exist after planning (including the existing-destination-inspection pass).
2. `no_source_module_references_a_filesystem_write_call` — parses each of `model.rs`/`planner.rs`/`romm.rs`/`es_de.rs`/`destination_inspection.rs`'s *production* source text (everything before its own `#[cfg(test)]` block) and asserts none of it contains `fs::write(`, `fs::create_dir`, `fs::remove_`, `fs::rename(`, `fs::hard_link(`, `fs::copy(`, or a real `std::os::unix::fs::symlink`/`windows::fs::symlink` call.

Both pass. `destination_inspection.rs`'s own `#[cfg(test)]` fixtures *do* create symlinks/files to exercise the read path — that is explicitly excluded from the scan (test fixture setup, not the module's own behavior).

## 10. Real collection audit (task section 21)

This machine's real library (`/mnt/ROM`, `/mnt/saturn-roms`, `/mnt/x68000-roms`, `/mnt/gba-roms`, plus TOSEC/No-Intro DAT packs under `/mnt/DATs`) was used to choose a representative real platform list; `examples/publisher_profile_real_platform_audit.rs` calls the real, unmodified `production_romm_status`/`es_de_system_for_platform` functions directly (no filesystem scan, no source/destination mutation) and both this crate's own `romm`/`es_de` wrapper functions, confirming they agree with the underlying production functions they delegate to:

| Canonical platform | RomM | ES-DE |
|---|---|---|
| PSX | Mapped → `ps` | Mapped → `psx` |
| PS2 | Mapped → `ps2` | Mapped → `ps2` |
| Amiga | Mapped → `amiga` | Mapped → `amiga` |
| AtariST | **Unmapped** | Mapped → `atarist` |
| ZX Spectrum | **Unmapped** | Mapped → `zxspectrum` |
| Dreamcast | Mapped → `dc` | Mapped → `dreamcast` |
| Saturn | Mapped → `saturn` | Mapped → `saturn` |
| Sharp X68000 | **Unmapped** | Mapped → `x68000` |
| Game Boy Advance | Mapped → `gba` | Mapped → `gba` |

6/9 RomM-mapped, 9/9 ES-DE-mapped in this sample. The three RomM gaps (Atari ST, ZX Spectrum, Sharp X68000) are genuine, pre-existing absences in RomM's own reviewed static table (documented in that module's own doc comment as deliberately conservative rather than guessed) — Phase 1 surfaces them honestly as `Unmapped` rather than inventing a slug.

**Limitation, stated plainly**: this is a platform-mapping-level audit, not a full hash-verified `PublisherPlan` run against the real multi-file library. Producing real `ElectedGame` input requires this repo's existing DAT-matching/verification pipeline wired through a CLI entry point that does not currently exist (`archivefs-cli` has no `playing-library`/`romm-scan` subcommand today) — building and safely running that against a real, multi-terabyte library is a substantial separate integration task, out of proportion to Phase 1's own scope. The synthetic test matrix (§12) and the 100k-item benchmark (§11) exercise the actual planning code at realistic scale instead.

## 11. Performance (task section 24)

`examples/publisher_profile_100k_benchmark.rs`, release build, 100,000 synthetic single-file `ElectedGame` items, one `Amiga`→`amiga` platform mapping:

| Phase | Time |
|---|---|
| Synthetic `PlayingLibraryPlan` construction (test setup, not planner work) | 83.8 ms |
| Profile platform-mapping resolution | 1.76 µs |
| **Destination planning + `O(N log N)` collision detection** | **715 ms** |
| Summary computation | 1.4 ms |
| Peak RSS | 166.7 MB |

Collision detection uses two `BTreeMap`s keyed by exact and case-folded destination strings (§6) — never a pairwise `O(N²)` compare, confirmed by the sub-second wall time at 100k items.

## 12. Synthetic test matrix (task section 22)

A representative, feasible subset (24 tests total across `publisher_profile::tests` and `destination_inspection::tests`):

| Letter | Covered by |
|---|---|
| A. simple single-file ROM | `a_simple_single_file_rom_is_safe_to_act` |
| B. CHD optical game | `b_chd_optical_game_representation_is_passed_through_unchanged` |
| C. multi-disc complete set | `c_multi_disc_complete_set_publishes_launcher_and_every_companion` |
| D. incomplete media set | `d_incomplete_media_set_is_review_required_with_a_warning` |
| E. ambiguous media set | covered by D's same code path (a companion the planner cannot safely project) |
| F/G/H. 1G1R election, region, revision variants | `fgh_elected_release_identity_flows_through_unchanged` (inherited from the upstream 1G1R planner's own, separately-tested election guarantees) |
| I. same destination collision | `i_same_destination_collision_blocks_both_contenders` |
| J. case-fold collision | `j_case_fold_collision_is_reported_distinctly_from_an_exact_collision` |
| K. unsupported platform | `k_unsupported_platform_is_marked_unsupported_never_guessed` |
| L. unknown/unmapped platform | `l_unmapped_platform_is_review_required_never_guessed` |
| M. existing correct destination | `m_existing_correct_destination_is_already_present` |
| N. existing conflicting destination | `n_existing_conflicting_destination_is_blocked_not_overwritten` |
| O. BIOS-dependent system | `o_bios_requirements_are_honestly_empty_in_phase_1` (documented limitation, not fabricated coverage) |
| P. RomM mapping | `p_romm_mapping_reuses_the_existing_production_table` |
| Q. ES-DE mapping | `q_es_de_mapping_reuses_the_existing_reviewed_table` |

Plus two determinism tests (§13) and the two zero-side-effect tests (§9).

## 13. Determinism (task section 23)

`compute_plan_hash` hashes every item's `(dat_entry_name, source_path, planned_destination, companion count, safety, destination_state, conflict count)` as a sorted line list (never insertion order) with a dependency-free FNV-1a accumulator — not cryptographic, only a stable fingerprint. `determinism_same_input_produces_identical_hash_regardless_of_insertion_order` builds the identical two-game plan forwards and backwards and asserts equal hashes; `determinism_changing_a_source_path_changes_the_hash` asserts a real input change is detected.

## 14. GUI (task section 19)

`crates/archivefs-gui/src/publisher_profile_page.rs`: a target-profile picker (plain-language "Create a RomM-ready library" / "Create an ES-DE-ready library" per task section 20), a destination-root and canonical-platform-id input, a "Preview plan" button, the exact `Ready`/`Already present`/`Review required`/`Blocked`/`Unsupported` filter set, an "Advanced" toggle revealing slug/canonical-platform/action/destination-path detail, execution review, explicit HARDLINK/SYMLINK explanations, typed `PUBLISH N ITEMS` confirmation, result reporting, and typed `ROLL BACK N ITEMS` confirmation. COPY is not offered; execution delegates to the core publisher adapter and shared journaled transaction engine.

The page is reachable as **Library → Publisher / Frontend Library** in the normal Advanced View sidebar. The page routes as its own `MainView::PublisherProfiles`, has selected-state highlighting, uses the shared page-scroll policy, and returns to **Library Organisation** through its explicit handoff when no 1G1R plan is available. It receives only the already-built `PlayingLibraryPlan` retained by the existing Library Organisation page; it never creates a second source-election flow. The profile cards read **Create a RomM-ready library** and **Create an ES-DE-ready library**, with plain-language descriptions. Phase 2C adds explicit execution review, mode selection, typed Apply confirmation, result, and rollback to this same page.

The destination root is an explicitly entered preview root. When a preview is requested, the planner passes that same root to its optional read-only inspection path, classifying existing destinations as missing, already correct, conflicting, stale, or unknown. It never creates the root. The summary and filters are rendered from the single plan result, and selecting **Details** shows title, canonical platform, target, source/destination paths, planned future action, mapping, media-set companion count, warnings, conflicts, and safety.

## 15. GUI integration and bounded real sample findings

The bounded platform sample uses the representative systems available in the collection audit: PS1, PS2, Amiga, Atari ST, ZX Spectrum, Dreamcast, Saturn, Sharp X68000, and GBA. It is a mapping-level sample, not a fabricated full-library audit and not a hash-verified whole-library run.

| Result | RomM | ES-DE |
|---|---:|---:|
| Mapped | 6/9 | 9/9 |
| Unmapped | Atari ST, ZX Spectrum, Sharp X68000 | 0 |
| Review required / blocked | No additional cases in the mapping sample | No additional cases in the mapping sample |
| Collision cases | 0 in the bounded mapping sample | 0 in the bounded mapping sample |
| Existing destination cases | Not supplied for the real sample | Not supplied for the real sample |

RomM and ES-DE therefore differ for the three RomM mapping gaps; ES-DE has reviewed folders for all nine representative systems. Synthetic planner and destination-inspection tests cover mapped/unmapped, exact/case-fold collisions, already-correct destinations, stale destinations, and conflicting existing content.

GUI smoke verification was performed at the state/render-test level: the page
route, profile labels, preview and execution review, explicit mode selection,
typed confirmation/result/rollback surfaces, filters, detail selection, and
empty/degraded handoff are covered by focused tests. The built application was
also launched on the existing `DISPLAY=:0` under a bounded timeout without a
crash. A full Xvfb mouse click-through was unavailable because the requested
display was already occupied, so click automation remains
`BLOCKED-BY-ENVIRONMENT`. `cargo check --workspace` is run with a temporary
target directory because this checkout's tracked build target is mounted
read-only.

## 16. Phase 2A/2B/2C execution boundary

Phase 2A/2B now has an explicit, core-only transaction foundation. It converts
only `SafeToAct` items back into `LinkedLibraryOperation`s and reuses
`playing_library::apply_adapter::build_playing_library_transaction`, the shared
`RenameTransaction` model, journal, preflight, executor, rollback, and
reconciliation. Phase 2B adds explicitly planned destination directories and
explicit caller-selected `CreateSymlink` operations. There is no automatic
hardlink-to-symlink fallback.

The compatibility `build_publisher_transaction` entry point remains hardlink
only and requires existing destination directories. The policy entry point
accepts `HARDLINK` or `SYMLINK`; hardlinks fail closed when same-filesystem
evidence is unavailable, with a typed message directing the caller to choose
SYMLINK explicitly. Symlink targets use the shared transaction convention of
absolute source paths; relative links were not guessed because no portability
contract exists in the shared engine.

Directory creation is represented before apply as `PreExisting` or
`NotCreated`, and after apply as `CreatedByTransaction`. Only directories
recorded in the shared transaction's `created_directories` ownership list are
eligible for deepest-first rollback, and only when empty. The destination root
identity, source identities, exact destinations, case-fold collisions, source
presence, and destination state are rechecked before an executable transaction
is produced or applied. Existing correct links are excluded; wrong links,
broken links, ordinary files, stale plans, and root changes fail closed.

Phase 2C wires the explicit core apply and rollback helpers to this existing
page. Apply is disabled until a fresh transaction can be rebuilt for the
current plan, root, selected mode, and exact typed `PUBLISH N ITEMS`
confirmation. Any stale plan, changed source/destination, collision, or
unavailable hardlink fails closed and requests a fresh review. Results retain
the shared transaction identifier and state; rollback requires the exact typed
`ROLL BACK N ITEMS` phrase and removes only confirmed transaction-created
destination links and empty directories. Copy, reflink, BIOS projection,
metadata/configuration, playlists, and GUI cancellation remain future work.

## 17. Why Phase 1 still defaults to Symlink

The generic planner still proposes `Symlink` because profile planning does not
choose an execution mode. Phase 2B exposes hardlink and symlink as a separate
explicit policy at the transaction boundary; it never silently changes the
user's selected mode.

## Validation

- `cargo fmt --check` — clean.
- `CARGO_BUILD_JOBS=4 cargo check --workspace` — clean using a temporary target directory because the checkout's `target/debug` is read-only.
- `cargo test -p archivefs-core publisher_profile` — 24/24 passing.
- `cargo test -p archivefs-gui publisher_profile_page` — 6/6 passing (×3 binary targets).
- `cargo test -p archivefs-core playing_library` / `rom_organisation` / `media_set` / `romm` / `es_de` — all passing, zero regression.
- `cargo test --workspace` — 9,066 passed, 9 failed (all in `database`/`diagnostics`/`disk_format`, schema-migration-count assertions expecting 12 migrations but finding 16). **Confirmed pre-existing**: reproduced identically with this task's changes fully `git stash`ed on the untouched `v0.9.0` checkout — unrelated to Publisher Profiles.
- `git diff --check` — clean.

## Is Publisher Profiles Phase 1 complete?

Yes, for its stated Phase 1 product scope: a generic, reusable, read-only planning model; first-class RomM and ES-DE profiles built on the existing reviewed mapping tables; typed conflict/warning/safety semantics; deterministic `O(N log N)` planning verified at 100k items; a bounded real mapping audit; a structurally-proven zero-side-effect guarantee; and a reachable, preview-only GUI destination integrated with Library Organisation's existing Playing Library handoff. A full hash-verified real-library publisher audit and execution remain outside this phase.
