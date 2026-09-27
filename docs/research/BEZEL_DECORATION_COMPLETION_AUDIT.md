# EmuWiz Bezel / Decorations Completion Audit

**Audit basis:** `33ecab378c54cfdfa4dd0303e1008b114b920878`
**Scope:** existing bezel/decorations discovery, resolution, preview, RetroArch application, emulator seams, ownership, and GUI-v2 presentation.
**Method:** source-level audit of the isolated research worktree; no assets were downloaded and no production code was changed.

## Executive assessment

EmuWiz has a credible local-first bezel foundation, but it is not yet a complete decorations product. The current implementation can safely discover bounded local image assets, read optional sidecar mappings, resolve a candidate, render a preview, and apply one narrowly-scoped RetroArch overlay through the shared transaction machinery. It does not yet provide a complete target model, explicit selection workflow, removal action, or proven writers for standalone emulators.

The smallest safe completion path is:

1. make target identity and display geometry explicit;
2. make ambiguous matches review-only rather than silently selected;
3. add explicit per-game/per-platform selection state and a reversible “None” choice;
4. finish the RetroArch remove/undo presentation around the existing writer;
5. add one emulator adapter at a time only after its active config scope and ownership are proven.

The current code does not justify a general “bezel support” claim across all supported emulators.

## CURRENT ARCHITECTURE

| Area | Current evidence | Status |
|---|---|---|
| Local discovery | `bezel_decorations.rs:217-265` walks explicitly configured roots with bounded depth/assets, sorted directory entries, supported image extensions, and malformed-image warnings. | **COMPLETE for bounded local image discovery** |
| Image safety | `inspect_local_bezel` enforces 32 MiB source, 8192-pixel dimensions, and 64 MiB decoded allocation limits (`bezel_decorations.rs:290-323`). | **COMPLETE for current image formats** |
| Sources/provenance | `DecorationSource` distinguishes local pack, user override, and provider; `DecorationProvenance` retains provider/reference/retrieval time (`bezel_decorations.rs:27-39`). | **PARTIAL** — no license field and provider assets are not resolved by the local matcher |
| Targeting | Assets can name emulator/core and optional sidecar fields include game, DAT, platform/system, emulator, core, scope, and viewport (`bezel_decorations.rs:142-178`). | **PARTIAL** |
| Resolution | `resolve_decoration` filters target/readiness, sorts deterministically, reports same-rank conflicts, and returns candidates (`bezel_decorations.rs:425-480`). | **PARTIAL** — it still selects the first ambiguous candidate |
| Preview | GUI-v2 renders the asset and an optional viewport rectangle; thumbnails are loaded through the existing local thumbnail cache (`gui_v2/bezel.rs:358-373`, `:460-493`). | **COMPLETE for asset preview; not an emulator-composited preview** |
| Configuration persistence | Explicit local roots are stored atomically in `bezel_catalogue.json` (`bezel_decorations.rs:186-215`). | **PARTIAL** — selected game/platform policy is not persisted |
| Apply planning | `bezel_apply.rs:172-389` creates a bounded, fail-closed plan and seals it with a digest. | **COMPLETE as planning primitive** |
| Apply execution | `patch_manager/retroarch_bezel.rs:51-190` prepares a RetroArch-specific plan; `:195-352` stages and executes it through shared preview/transaction/history/rollback. | **PARTIAL product feature** — one adapter is real, but remove/action UX and broader target coverage are missing |
| GUI route | Game Details mounts a collapsed “Bezel & decorations” panel (`gui_v2/pages.rs:2223-2240`). | **PARTIAL** — no dedicated decoration library/selection route |
| Cache | Preview thumbnails use the existing cache; the catalogue itself is re-discovered from local roots. | **PARTIAL** — no provider/catalogue cache contract |

The module-level contract is deliberately conservative: `bezel_decorations.rs:1-4` says the resolver only resolves/previews and does not edit emulator configuration or source media. The later RetroArch adapter is the narrow exception and is separately gated.

## TARGETING MODEL

### Fields currently represented

Current sidecar metadata can express:

- `game_identity`;
- `dat_identity`;
- `platform` or `system`;
- `emulator` and `core`;
- `Game`, `System`, or `Default` scope;
- one rectangular viewport/cutout.

The current match context only carries verified identity, DAT identity, platform, and game title (`bezel_decorations.rs:158-164`). It does **not** carry release hash, region, revision, orientation, aspect ratio, display rotation, content type, or disc/arcade identity.

### Required precedence

The eventual resolver should use a typed match result, not a filename score:

1. explicit user mapping for this game and emulator/core;
2. exact game identity plus exact release/revision evidence;
3. exact verified game identity;
4. exact DAT/canonical identity;
5. game identity with compatible region/revision/orientation/aspect;
6. platform/system fallback with compatible geometry;
7. generic default fallback;
8. filename/title similarity only as a visible, low-confidence suggestion.

At every level, an incompatible region, revision, orientation, or aspect must exclude the candidate rather than merely lower its score. An exact match at the same level must not be silently replaced by another exact match.

### Current deviation and risk

`match_asset` checks game identity, DAT identity, platform, then normalised filename equality (`bezel_decorations.rs:372-423`). Filename equality is promoted to `DecorationScope::Game`, although its evidence strength is only 1. `resolve_decoration` then orders source rank, scope rank, and evidence strength (`:425-480`). Thus:

- title similarity can become the selected game asset;
- there is no region/revision/geometry rejection;
- same-strength candidates are exposed as conflicts but the first deterministic ID still wins;
- `UserOverride` has the highest source rank, which is reasonable only when it is an explicit user mapping, not merely a source classification.

This is safe enough for a preview catalogue when the candidate and conflict list are visible, but not sufficient for unattended automatic application.

## ASSET TYPES

### Proven current support

The implementation validates PNG, JPG/JPEG, and WebP image files. It can retain a rectangular viewport/cutout and show that rectangle in the GUI. It does not distinguish asset kinds beyond the general decoration scope.

### Not currently modelled

These need separate typed metadata before automatic selection can be reliable:

- bezel/overlay image versus background/frame;
- viewport mask versus artwork-only frame;
- shader-associated layout or shader preset;
- portrait/vertical arcade orientation;
- target aspect ratio and integer-scaling assumptions;
- display rotation;
- multi-layer assets and ordering;
- emulator/core capability requirements.

Do not infer these from filenames. A first extension should add `DecorationAssetKind`, orientation, aspect constraints, and a typed geometry contract while retaining unknown values as review-only.

## RETROARCH

### What is proven

RetroArch environment discovery exposes an `Overlays` path purpose mapped to `overlay_directory` (`emulator_environment/retroarch.rs:69-100`). GUI-v2 accepts a scope only when there is one present, non-lossy profile and, for per-game routing, a uniquely identifiable core (`gui_v2/native_workflows.rs:203-233`).

The writer:

- requires a local regular source asset, bounded size, approved roots, verified identity, core, config path, and overlay root (`bezel_apply.rs:172-286`);
- keeps the overlay destination under the selected config root;
- rejects unsafe names, symlink destinations, path traversal, and unsafe parents;
- creates a copied image and an EmuWiz descriptor;
- writes a core/game override rather than editing `retroarch.cfg` (`patch_manager/retroarch_bezel.rs:75-180`);
- preserves unrelated lines in the existing override and detects an existing conflicting overlay;
- stages all three outputs and uses the shared transaction executor (`:246-328`);
- supports exact journal-backed rollback, including removal of transaction-created files (`:330-352` and the module tests).

Therefore the current RetroArch path is **SUPPORTED for a narrow per-game apply workflow**, subject to the exact discovered profile/core and explicit review. It is not proof that every RetroArch installation or overlay mode has identical precedence.

### Remaining RetroArch gaps

- no explicit platform fallback file/application policy;
- no “Use game-specific / platform default / None” persisted choice;
- no dedicated Remove action that only removes EmuWiz-owned references/files;
- no emulator-composited preview showing actual viewport/aspect interaction;
- no typed ownership marker beyond path conventions and byte preconditions;
- the GUI passes `replacement_approved: !plan.conflicts.is_empty()` after the general plan checkbox (`gui_v2/bezel.rs:404-444`). This is reviewable, but replacement approval should become a separate, explicit decision for a user-owned overlay conflict;
- no dedicated rollback/undo button in the decoration panel, even though the shared history can roll back the transaction.

### RetroArch classification

| Capability | Status |
|---|---|
| exact config/overlay destination discovery | **COMPLETE for one uniquely selected profile** |
| core/content-targeted override | **COMPLETE for selected core and identity path** |
| per-game apply | **SUPPORTED** |
| platform fallback | **PREVIEW-ONLY / missing policy** |
| safe merge | **PARTIAL** — narrow line-preserving merge, no managed block |
| enable/disable | **PARTIAL** — apply enables; no user-facing remove/disable projection |
| rollback | **COMPLETE in shared transaction path; GUI exposure incomplete** |
| viewport/aspect interaction | **PREVIEW-ONLY** |

## STANDALONE EMULATORS

No decoration-specific config writer was found for MAME, DuckStation, PCSX2, Dolphin, Flycast, PPSSPP, or other standalone profiles in the audited code. The generic asset model can carry an emulator target, but `build_bezel_apply_plan` rejects every emulator other than RetroArch (`bezel_apply.rs:237-245`). This means a non-RetroArch candidate is, at most, a local preview/inventory item.

| Emulator | Current status | Safe next step |
|---|---|---|
| RetroArch | **SUPPORTED** in the narrow path above | Finish explicit selection/remove and geometry policy |
| MAME | **PREVIEW-ONLY** | Research artwork/lay-file ownership and per-game scope; no writer yet |
| DuckStation | **PREVIEW-ONLY** | Prove per-game display/overlay config and active profile precedence |
| PCSX2 | **PREVIEW-ONLY** | Prove whether a decoration is emulator config or shader/display state |
| Dolphin | **PREVIEW-ONLY** | Prove per-title graphics/config scope; do not touch global settings |
| Flycast | **PREVIEW-ONLY** | Prove native overlay/decorations path and game binding |
| PPSSPP | **PREVIEW-ONLY** | Prove per-game display/config scope and aspect behavior |
| Other standalone emulators | **NOT SUPPORTED** | Keep candidates visible as unsupported rather than inventing paths |

The current resolver should not advertise “applies to emulator” merely because a sidecar names an emulator. Application capability must be derived from a registered, tested adapter.

## CONFLICT MODEL

Current conflicts are strings generated when adjacent candidates share scope and evidence strength. That is useful diagnostics, but it cannot express why a candidate is unsafe. The future typed model should include:

- `DuplicateExactAsset`;
- `MultipleExactGameAssets`;
- `GameOverridesPlatform`;
- `RegionMismatch`;
- `RevisionMismatch`;
- `OrientationMismatch`;
- `AspectMismatch`;
- `EmulatorNativeDecorationPresent`;
- `UserOwnedConfigConflict`;
- `UnsupportedTarget`;
- `MissingGeometry`.

Decision policy:

| Situation | Action |
|---|---|
| one exact compatible asset | auto-select for preview; apply only after ordinary confirmation |
| one exact asset plus compatible platform fallback | auto-select exact; show fallback |
| exact asset conflicts with another exact asset | ask user; no automatic apply |
| region/revision/orientation/aspect mismatch | refuse that candidate; retain explanation |
| existing user overlay differs | ask user with exact file/key; never silently replace |
| unsupported emulator/config scope | preview only |
| no candidate | show plain empty state and configuration action |

## PRECEDENCE

The product precedence should be based on evidence class first, then source/ownership, then specificity, then stable ID. A recommended total order is:

`explicit mapping > exact release identity > exact verified identity > exact DAT identity > compatible game metadata > platform/system > default > filename suggestion`.

Within one evidence class, explicit user choice may win, but a user choice must be represented as a durable mapping rather than inferred from `UserOverride`. Stable ID sorting is appropriate only as a deterministic tie-breaker after the resolver has marked the result ambiguous.

The current order (`source_rank → scope_rank → evidence strength → id`) is deterministic, but it is not the required evidence-first order. This is a **P1 correctness gap** for automatic matching and a **P0 safety gate** if unattended apply is ever introduced.

## APPLICATION SAFETY

The existing safety boundary is strong for the implemented RetroArch path:

- source files are copied, not moved or linked;
- source and destination fingerprints are checked between preview and apply;
- paths are bounded to caller-approved roots;
- symlink/non-directory parents are refused;
- generated files are staged and passed through shared preview/transaction logic;
- the source media and original artwork are never modified;
- partial failure attempts shared rollback.

The missing completion pieces are product-level:

- represent the exact active target and selected choice in the plan;
- validate the actual emulator/core content binding, not only a path-shaped filename;
- make user-owned replacement approval explicit;
- add a narrow remove plan that removes only EmuWiz-owned files/references;
- expose the transaction receipt and undo action in GUI-v2.

No broad “apply to all emulators” operation is safe to add without those adapter proofs.

## USER-OWNED CONFIG

Current ownership is mixed:

- `bezel_catalogue.json` is EmuWiz-owned and atomically persisted;
- local source roots and source images remain user-owned and read-only;
- RetroArch’s `retroarch.cfg` is intentionally not edited;
- the per-core/per-game override is read and merged, but its whole file is rewritten by the transaction;
- overlay image/descriptor destinations are selected under the discovered RetroArch config scope, but there is no explicit ownership manifest or managed-block marker.

The safest completion is an EmuWiz-owned sidecar/manifest for each applied decoration containing target identity, destination paths, source digest, and generated-file digests. Removal can then refuse if bytes or unrelated keys changed. Avoid wholesale replacement of user configuration and avoid claiming ownership based only on a filename.

## ROLLBACK

Core rollback is **COMPLETE for successful RetroArch transactions** through the shared history/backup journal. Tests in `patch_manager/retroarch_bezel.rs` cover exact restoration, newly-created overlay removal, stale source/destination refusal, and unsafe symlink destinations.

Product rollback is **PARTIAL** because GUI-v2 does not show a dedicated Undo/Remove action. A completion pass should:

1. list the last decoration operation for the selected game;
2. provide `Undo` through the existing journal, with external-modification refusal;
3. provide `Remove EmuWiz decoration`, which is a typed plan, not a file delete shortcut;
4. preserve user edits and refuse when the managed destination no longer matches its receipt.

## GUI CONTRACT

The current Game Details collapsing panel is a good attachment point and is already native GUI-v2 (`pages.rs:2237-2240`). It currently shows local roots, resolved asset, evidence, provenance, viewport, preview, conflicts, and an apply plan (`gui_v2/bezel.rs:278-457`).

The eventual panel should be:

```text
Decorations

Selected: Game-specific bezel
Source: Local / user mapping / provider reference
Applies to: This game · RetroArch · core
Match: Verified game identity / platform fallback / suggestion

[image preview with viewport/aspect notes]

Choice:  Use game-specific | Use platform default | None
        Preview | Apply | Remove | Undo

Warning: This emulator already has a custom overlay configured.
```

Plain-language empty state:

> No compatible decoration is linked to this game yet. Add a local decoration folder or choose a platform default.

Advanced details should expand to show identity evidence, hashes, paths, config keys, geometry, provenance, license, and transaction receipt. Unsupported standalone targets should say “Preview only — this emulator’s decoration config is not yet managed safely.”

## CONTROLLER FUTURE

Do not add Console Mode in this audit. The later controller contract should reuse the generic GUI action/focus abstraction:

- confirm: open/select;
- back: close preview or return to Game Details;
- left/right: cycle candidate/choice;
- shoulder buttons: move between game-specific/platform/none choices;
- menu: open evidence/config details;
- apply/remove: require a focused confirmation step.

The panel must never require a mouse-only file picker for existing configured assets; adding a root can remain an explicit setup action.

## LICENSING

The current model does not bundle or download bezel assets. `DecorationProvenance` records provider/reference/time, but it does not record a license or attribution requirement. That is adequate for local-first discovery, not for redistribution.

Before any provider/bundled pack work, add source URL/version, license, attribution, redistribution status, and user-import-only state. Do not treat a provider reference as permission to redistribute. Existing research also identifies overlays/bezels as a future layer and explicitly cautions against downloads or installation without licensing review (`docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md`, section N/O).

## P0 GAPS

P0 means a safety gate for any claim of automatic or broad apply, not necessarily a blocker for the current preview feature.

1. **No evidence-complete target model.** Region, revision/release, hash, orientation, and aspect constraints are absent. Automatic cross-release selection must remain disabled.
2. **Ambiguous exact matches can still be selected.** Conflicts are displayed, but `resolve_decoration` returns the first candidate. Apply must refuse unresolved exact conflicts.
3. **Only RetroArch has a proven writer.** No standalone emulator may be presented as supported until its config scope, ownership, active precedence, and rollback are tested.
4. **User-config replacement consent is too coarse.** A separate explicit approval is needed for an existing user overlay conflict; a general plan checkbox should not implicitly approve replacement.

## P1 GAPS

1. Persist explicit game/platform/none selections and expose them in Game Details.
2. Add typed asset kind, region/revision, orientation, aspect, rotation, and geometry metadata.
3. Add RetroArch remove/undo UI and an ownership manifest.
4. Add an emulator-composited preview or clearly label the existing image/viewport preview as an approximation.
5. Add license/attribution fields to provider provenance before any redistribution-capable source is added.
6. Add a dedicated decoration catalogue/management route for reviewing all candidates, not only the currently selected game.
7. Make provider-backed candidates either resolvable through an explicit cache or visibly inventory-only; current local matching ignores `Provider` sources (`bezel_decorations.rs:372-376`).

## QUICK WINS

- change filename-only matches to “suggestion” evidence that cannot be auto-applied;
- refuse selection when two exact candidates remain tied;
- add region/revision/orientation/aspect optional fields with unknown-safe behavior;
- split `Apply` and `Replace existing user overlay` confirmations;
- expose shared history’s rollback receipt as `Undo`;
- add `None` and `platform default` as explicit persisted choices;
- add a plain-language unsupported-emulator state;
- add the current source digest and target scope to the visible preview summary.

## IMPLEMENTATION SEAMS

1. **Core resolution:** extend `DecorationEvidence`, `BezelMatchContext`, sidecar metadata, and `DecorationResolution` with typed match dimensions and an ambiguity state. Keep `resolve_decoration` pure and deterministic.
2. **Core asset catalogue:** preserve bounded local discovery; add typed asset kind/license/geometry fields without widening filesystem crawling.
3. **RetroArch adapter:** retain `patch_manager/retroarch_bezel.rs`; add explicit managed receipt, remove plan, and separate replacement confirmation. Do not move logic into generic routing.
4. **GUI-v2 Game Details:** extend `gui_v2/bezel.rs` state with persisted selection and rollback/remove actions. Keep advanced evidence in a collapsible section.
5. **Adapter registry:** only add an emulator-specific decoration adapter after a complete config/path/parse/transaction test seam exists. Unsupported emulators remain visible as preview-only.
6. **Testing:** keep discovery/resolution tests in core, transactional tests beside the RetroArch adapter, and routing/presentation tests in GUI-v2.

## TEST PLAN

Required focused coverage for the completion work:

| Case | Expected result |
|---|---|
| exact game bezel | selected over compatible platform/default fallback |
| platform fallback | selected only when no more-specific compatible asset exists |
| region mismatch | excluded with an explanatory mismatch, never silently selected |
| duplicate exact match | marked ambiguous; apply disabled until user chooses |
| orientation mismatch | excluded for portrait/landscape conflict |
| existing user overlay | visible conflict; replacement requires separate approval |
| deterministic selection | same catalogue/context yields same candidates, reason, and tie result |
| preview only | no source/config mutation; preview digest remains observable |
| safe apply | exact target, staged files, reparsed config, transaction receipt |
| stale destination | apply refused after destination fingerprint changes |
| rollback | exact prior bytes restored or EmuWiz-created files safely removed |
| external modification | destructive rollback/remove refused |
| unsupported emulator | candidate remains preview-only and no config path is invented |
| no decoration | useful empty state with add/configure action |
| vertical arcade title | orientation/geometry-aware selection or explicit review state |

Existing evidence already covers several lower-level cases: bounded discovery and malformed images (`bezel_decorations.rs:674-720`), identity over filename inference (`:722-753`), duplicate visibility (`:755-805`), source/destination staleness and symlinks (`patch_manager/retroarch_bezel.rs:651-679`), and exact transactional undo (`:570-619`).

## IMPLEMENTATION ORDER

1. **Safety correction:** typed target dimensions and ambiguity refusal; add tests before expanding sources.
2. **Selection foundation:** persisted explicit game/platform/none mappings and a reviewable candidate list.
3. **RetroArch completion:** separate replacement approval, managed receipt, remove/undo UI, and active-scope diagnostics.
4. **Geometry/preview:** orientation/aspect-aware preview and explicit approximation language.
5. **Provider/licensing model:** only after local behavior is complete; retain local-first and user-import boundaries.
6. **Standalone adapters:** one at a time, ranked by documented per-game config capability; unsupported targets remain preview-only.

This order preserves the existing safe foundation and avoids turning a generic artwork catalogue into an unsafe emulator-config writer.
