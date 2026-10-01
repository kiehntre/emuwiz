# Safe per-launch cheat composition (Cheat Core Batch 5)

Status: planner and safety model only. **Nothing here is wired into a real
launch**, and no emulator is declared ready. Baseline: `origin/main` @
`a5e604e7`. Inputs: `docs/design/CHEAT_PER_LAUNCH_WORKSPACE_V1.md` and
`docs/research/RETROARCH_APPENDCONFIG_PERSISTENCE_TEST.md` (both on unmerged
branches at the time of writing).

## What exists on current main (audit)

- **Persistent installs only.** Every cheat apply path (`shared_preview` ->
  `shared_transaction`, plus per-emulator native writers) publishes permanent
  files and journals them. None is launch-scoped.
- **One launch-scoped cheat path:** ScummVM's `--config` trainer.
- **`resource_grants`** is declarative intent; it has no production caller
  except `retroarch_resource_projection`, which is itself not called from the
  launch flow. It writes only `system_directory` and `savefile_directory` into
  an *append* config and never sets `config_save_on_exit`.
- **`cheat_apply_support` means "can install persistently"**, not "can apply
  for one launch".
- **Selection is flattened today:** `ResolvedCheatPlan.selected_entries`
  records the *review outcome* (what survives conflict review). It is not a
  user's choice of what to enable for one launch, so it must not be used as one.
- **`spawn_watched_process` injects no environment**, so env-based profile
  redirection is not expressible yet.
- Source files: nothing in the launch path writes ROMs or source cheat files.

## Model

| Concept | Type | Notes |
|---|---|---|
| Available | `CheatCandidate` (+ `CheatVariant`s) | Never enables anything. |
| Chosen | `CheatLaunchSelection` | The only way in. Carries the explicit variant and a review acknowledgement. |
| Applicability | `patch_manager::CheatApplicabilityState` | The one applicability model (consolidated; the earlier stand-in is gone). `applicability_verdict()` maps it to Allowed / ReviewRequired / Blocked. |
| Capability | `CheatEmulatorCapability`, `CheatLaunchMode` | `PersistentInstallOnly` is never treated as launch-scoped. |
| Plan | `CheatLaunchPlan` | `NoCheatsSelected` / `Ready` / `Blocked`, with structured `CheatLaunchBlockReason`s. |
| Derivative | `CheatLaunchDerivative` (patch_manager), `PlannedDerivative` (launch) | Bytes + provenance back to the source. |
| Ownership | `LaunchResourceGrantSet` (+ new `CheatMaterial` role) | Saves pass through, scratch is generated. |
| Verification | `LaunchStateExpectation`, `capture_baseline`, `verify_expectation` | Targeted fingerprints only; no directory hashing. |

## patch_manager / launch boundary

`patch_manager::render_retroarch_selected_derivative` is pure: parsed entries in,
`.cht` bytes + entry provenance out (deterministic order, contiguous indexes,
built on the existing `ChtInstallEntry::from_entry` + `render_cht_file`, which
already refuse unselectable entries). It never touches disk. `launch`
(`cheat_launch_plan`) decides where the bytes live (launch-owned scratch), grants
them, and defines what must be verified. No `SharedTransactionPlan` or journal is
created for launch-scoped material.

## Rules enforced

- No cheat enters a plan without a selection; unselected candidates never appear
  in the derivative, whatever their applicability.
- Multiple implementations, or a reconciliation-reported conflict, block until
  `variant_id` is chosen; only that variant is composed. Two variants of one
  logical cheat are never emitted together (also enforced in the renderer).
- Blocked outright: wrong region/revision, different game, unsupported format or
  emulator, malformed. Only `Ready` / `ExactGameMatch` launch freely. Weak
  states (`StrongMatch`, `PossibleMatch`, `NeedsReview`, `MissingRequiredEvidence`,
  `ConflictingVariants`) need an explicit acknowledgement and are recorded
  as-is, never promoted. A title that merely looks similar is never enough.
- One blocked selection blocks the whole plan (no silent subset launch).
- Fail closed with a structured reason; the plan never falls back to persistent
  writes (`persistent_writes` and `global_config_mutations` are empty).

## RetroArch safety model (how the research shaped it)

| Verified finding | Design consequence |
|---|---|
| `--appendconfig` values are written into the base config, shortly after core load, and killing the process does not prevent it | No append config on the real profile. The plan hands RetroArch a **generated scratch base config** via `--config`; the real `retroarch.cfg` is a protected reference only. Safety never depends on a graceful or forced exit. |
| `config_save_on_exit = "false"` prevented the tested rewrite | Always emitted in the generated config, but treated as one layer, not the guarantee. |
| Core/game overrides can defeat that setting | `auto_overrides_enable = "false"` is emitted, and any *effective* override that sets a mandatory key blocks the plan (`OverrideDefeatsSetting`). |
| `--config` alone did not isolate writable state (core options, caches) | `--config` only is `ProfileIsolationInsufficient`. Ready needs `DisposableProfile`. Aux paths (`core_options_path`, `rgui_config_directory`, `content_history_path`, `playlist_directory`, `cache_directory`, `log_dir`) are pinned into scratch. |
| Explicit "Save Configuration" and auxiliary writes remain possible | They can only reach scratch; the real config is verified by SHA-256 and by a key probe on the mandatory keys. |
| Saves must not be forked | `savefile_directory` and `savestate_directory` are pinned to the real directories and granted `DirectPath`/`ReadWrite`/`Persistent`; a save path inside scratch is refused. |

Because current main cannot inject environment or redirect a Flatpak profile, a
production request will report `ProfileIsolationInsufficient` until an executor
supplies `DisposableProfile`. That is deliberate: readiness is not faked.

Generated `.cht`: `<launch_root>/retroarch/cheats/<core library_name>/<content
stem>.cht`, only selected entries, all enabled, ordered by logical id, source
`.cht` untouched, provenance in the header comment and in the plan.

## Protected-state expectations

Must remain unchanged: ROM/media (length+mtime), each source cheat file
(SHA-256), the real `retroarch.cfg` (SHA-256 plus key probe). May change: real
save and state directories (existence/not-emptied only). Ephemeral: generated
config, derivative, scratch root (`MustNotExistAfter`).

## Integration with Batches 1-4 and 6 (not merged here)

- **Batch 1 (parser hardening):** `ChtEntry::is_selectable` is the gate; the
  hardened parser feeds `CheatVariant.entry` unchanged.
- **Batch 2 (conflicts):** integrated. `CheatCandidate::requires_choice(group)`
  derives the conflict flag from the canonical reconciliation group's typed
  `CheatDuplicateKind`s; the rule "no choice, no composition" is unchanged.
- **Batch 3 (provenance):** project its record into `CheatSourceReference`.
- **Batch 4 (applicability):** integrated. The launch planner uses
  `patch_manager::CheatApplicabilityState` directly; `NotEvaluated` and
  `IdentityVerifiedOnly` no longer exist. A variant must carry the state from
  `assess_cheat_applicability`.
- **Capability registry:** `cheat_launch_capability` is a deliberately small
  stand-in for the design's canonical registry.
- **Batch 6 / GUI:** can read `CheatLaunchPlan.blocked` / `plan_blocks` to explain
  why a selected cheat cannot be launched. No GUI was changed here.

## Deferred

Executor (materialise, spawn, baseline capture, verify, cleanup, stale-tree
sweep); environment injection or Flatpak profile redirection; seeding the
scratch base config with the user's harmless settings (today it is minimal, so
input and video settings are defaults); detecting real override files;
BIOS composition (reuse `plan_retroarch_resource_grants`; pass its system
directory as `system_directory`); ScummVM and other adapters; a persistence
harness for any adapter besides the RetroArch case tested.
