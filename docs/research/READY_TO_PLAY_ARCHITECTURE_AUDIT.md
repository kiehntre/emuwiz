# Ready-to-Play Architecture Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot.** This document records research and design reasoning. It is not
> current capability documentation; see the [README](../../README.md),
> [launch support](../LAUNCH_SUPPORT.md), and [roadmap](../../ROADMAP.md) for present
> guidance. **Nothing in this document is implemented, and no production Rust file, GUI
> page, launch planner, Publisher Profile, Save Vault, or arcade election was modified.**

Status: **research only.** No Ready-to-Play UI was added, no launch planning was changed,
nothing was auto-fixed, and nothing was pushed. Every claim about EmuWiz's own code is a
`file:line` **CONCLUSION FROM SOURCE** verified by reading the repository at
`f951d54d4303e961e2672ab2d0cdf12137e6c39b` (`main`). Concurrent work landed two commits *during*
this research (`2625d0f` read-only emulator lifecycle inventory, `f951d54` read-only storage
health analysis); both are described in section 3.7 and were read but not modified. All cited
anchors were re-checked at `f951d54` after those commits landed.

**Tagging key**

| Tag | Meaning |
|---|---|
| **DOCUMENTED FACT** | Stated by an external, cited source (project documentation, upstream code). |
| **CONCLUSION FROM SOURCE** | Stated by a `file:line` in *this* repository. |
| **INFERENCE** | Reasoning drawn by this document; not directly asserted by a source. |
| **UNCERTAIN** | Explicitly flagged as unverified; not to be built on without a probe or test. |

## 1. Executive summary

The central finding is that **EmuWiz already contains almost every primitive a unified
Ready-to-Play model needs, and already has a unified aggregation surface — but it has no
single typed per-game readiness projection and no shared reason vocabulary across the
per-game and library-wide axes.**

Concretely (**CONCLUSION FROM SOURCE**):

- A per-game planner with a three-state verdict already exists:
  `LaunchReadiness::{Ready, ReadyWithWarnings, Blocked}` with a typed `LaunchBlockerKind`
  (**198 variants**) and `LaunchWarningKind`, built by the pure `build_launch_plan`
  (`crates/archivefs-core/src/launch/readiness.rs:53,67,572`;
  `launch/planning.rs:181,509`).
- Firmware/BIOS readiness already has a five-state vocabulary
  (`FirmwareReadiness::{Verified, PresentUnverified, Missing, Unknown, NotRequired}`,
  `readiness.rs:32`) fed by pure projections from each adapter's *own* existing BIOS state
  enum (`readiness.rs:617,631,721,743`), which are left untouched.
- Media topology, arcade set completeness, dependency closure, DAT authority (including a
  `bios_missing` count), emulator installation form, emulator-version/DAT compatibility,
  identity evidence, mod-package base matching, and cheat/patch candidate classification
  all already exist as typed evidence with explicit "what this does not prove" boundaries.
- A **unified attention model already aggregates across the library**:
  `AttentionSeverity::{Blocking, ActionNeeded, Warning, Info}` + `AttentionCategory`
  (13 categories including `Emulator`, `Launch`, `Identity`, `Dat`) + bounded
  `AttentionSnapshot` (`crates/archivefs-core/src/attention.rs:36,60,228,296`), and the GUI
  already turns a not-ready `LaunchPlan` into a `Blocking` attention item, routing missing
  BIOS to Emulator Setup and everything else to Launch Readiness
  (`crates/archivefs-gui/src/needs_attention.rs:280,310,319`).

**What is genuinely missing** is therefore small and specific:

1. **A per-game top-level projection** answering "can this game be launched right now, on
   this machine?" as one typed value. Today that answer exists only implicitly in a
   `LaunchPlan`'s candidate list, in Doctor findings, and in attention items that are
   *per-problem*, never *per-game*.
2. **A shared, small reason vocabulary** used by both the per-game projection and the
   library-wide surfaces, so a user sees the same words in both places ("Missing required
   PS2 BIOS") instead of three spellings.
3. **An explicit `Unknown` / "evidence not gathered" state at the per-game level.** The
   library-wide surface already gets this right (`Gathered<T>`, `CoverageStatus`,
   `coverage_notes`), and the GUI already refuses to infer failure from an unscanned lane
   (`needs_attention.rs:280-296`), but the per-game verdict has no equivalent: a plan built
   with an unscanned lane can read as "Blocked" rather than "Unknown".
4. **A fixability classification**, so a blocker is presented as user-fixable, guidable,
   safely repairable, unsupported, or informational. Doctor already expresses this in prose
   and via `KnownRecovery`/`repair`, but readiness has no equivalent field.

**Recommended shape:** add **one small projection type plus one reason enum** in core
(§4.1, §5.1), derived from existing evidence only, and *reuse* `AttentionSeverity`,
`DoctorSeverity`, `LaunchReadiness`, `FirmwareReadiness`, `MediaSetState`, `SetState`,
`DependencyState` and `ArcadeDatVersionCompatibility` unchanged. Do **not** build a second
readiness system, a second media grouping engine, a second emulator scanner, a second BIOS
checker, or a new installer/updater (sections 3.3, 26).

**Answers at a glance** (full answers in section 25):

| Question | Answer |
|---|---|
| Enough primitives already? | **Yes** — the primitive layer is complete; the missing piece is a projection plus a shared reason vocabulary. |
| Smallest new core model? | `ReadyToPlayState` (6 states) + a closed `ReadinessReason` enum + a `Fixability` field. |
| What should BLOCK launch? | Identity unresolved/conflicting; content not resolved; media proven incomplete for a required medium; arcade set or dependency proven missing; a command plan that cannot be built. |
| What should WARN only? | Unverified-but-present BIOS; emulator version unknown; controller requirement unknown; optional firmware missing; multiple eligible profiles; mods not installed; any uncertainty that cannot be proven harmful. |
| Require perfect identity? | **No** — require *launch-sufficient* identity (today: a resolved canonical identity from the launch bridge's identity-conferring facts). Never treat filename-only evidence as confirmed. |
| Arcade projection? | Project `SetState` + `DependencyState` + arcade working status + `ArcadeDatVersionCompatibility` into the same reason families; add no arcade logic. |
| Save-state separate? | **Yes** — a separate sub-status that must never reduce game readiness. |
| First implementation slice? | R0 + R1: the pure projection and its reason vocabulary, read-only, no new probes, no UI. |
| Explicitly NOT solve? | Emulator installation/updating, BIOS acquisition, save-state compatibility truth, controller mappings, mod curation, scoring. |
| Remains authoritative? | `launch::planning`/`readiness`, `media_set`, `dat::set`/`dat::dependency`, `diagnostics`, `attention`, `platform_evidence_fusion`, `verified_identity_cache`, `patch_manager`/`mod_package`. |

## 2. Scope: what Ready-to-Play means, and what it must not mean

### 2.1 Definition

**Ready-to-Play answers exactly one question, for one game, on this machine, now:**

> *Can EmuWiz build a launch plan for this game that it would be willing to execute, with
> every required component present and verified to the standard EmuWiz already applies
> elsewhere — and if not, precisely which missing or unproven thing prevents it?*

That definition has three deliberate consequences:

1. **It is a projection over evidence EmuWiz already trusts, not a new probe.** Nothing in
   Ready-to-Play may hash a file that the identity layer would not hash, scan an emulator
   directory that discovery would not scan, or read a BIOS that a verifier would not read
   (`crates/archivefs-core/src/launch/mod.rs:1-45`; `launch/planning.rs:1-20`).
2. **It is a statement about launchability, not about quality, completeness, or
   preservation.** A game can be perfectly playable while failing every archival standard,
   and EmuWiz's DAT/identity surfaces must keep saying so independently (section 7).
3. **It is never launch authority.** Planning is not permission; execution re-validates
   everything fresh. This is already explicit in the codebase
   (`crates/archivefs-core/src/verified_identity_cache.rs:1-14`: the cache "is **never**
   consulted for launch or cheat/mod authorization";
   `launch/process_spawn.rs` re-captures file identity before spawning;
   `docs/LAUNCH_SUPPORT.md` "Safety boundaries").

### 2.2 Ready-to-Play is explicitly NOT

| Not this | Why, with evidence |
|---|---|
| "The file exists" | File existence proves nothing about identity, format support, firmware, or a buildable command — the entire blocker vocabulary exists because of this. |
| "The DAT says the set is complete" | `SetState::Complete` is documented as **storage completeness only**: "it does not claim dependencies are resolved or the software runnable" (`crates/archivefs-core/src/dat/set.rs:269-273`), and `BIOS_RUNTIME_SELECTION_NOT_MODELLED` is a compile-time-true marker that no state from the dependency module "may be read as 'this runs'" (`dat/dependency/mod.rs:90-96`). |
| "The emulator is installed" | Installation form and executability are separate evidence (`diagnostics/profiles.rs:76-82`, `safe_executable` at `:84-98`); a binding can still fail, which is why `*BindingUnavailable` blockers exist. |
| "A BIOS file with the right name exists" | `FirmwareReadiness::PresentUnverified` exists precisely because "filename or mere presence alone is never verification" (`launch/readiness.rs:38-44`). |
| "identity is perfect, archivally" | Launch needs *sufficient* identity, not perfect identity (section 7). |
| A quality, rating, or "best games" signal | No scoring model exists or is proposed; the arcade audits explicitly reject subjective "best" lists as truth (`docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md`). |
| A licence to auto-fix | Exactly four Doctor repairs exist and every other finding is "reported and explained, never performed" (`diagnostics/mod.rs:657-661`). |
| A download/install/update service | Out of scope by instruction, and out of scope by prior in-tree research (`docs/research/EMUHAVEN_EMULATOR_MANAGER_AUDIT.md`, which re-confirms EmuWiz has no installer/updater beyond the one-shot managed-AppImage path). |

### 2.3 Boundaries of the model

- **In scope:** per-game launchability aggregation; the reason vocabulary; the Unknown /
  not-gathered state; fixability; explanations; a read-only view; caching/invalidation
  rules; provenance.
- **Adjacent but not owned:** identity resolution, media grouping, DAT auditing, arcade
  election, emulator discovery, BIOS verification, Doctor findings, attention aggregation,
  mods/cheats installation. Ready-to-Play **consumes** all of these and duplicates none.
- **Out of scope entirely (this task):** implementing anything; auto-repair; publisher
  profiles; Save Vault; arcade election changes; emulator installation; BIOS publishing.

### 2.4 The two axes that must not be conflated

| Axis | Question | Existing owner |
|---|---|---|
| **Per-game readiness** | "Can *this* game launch?" | `launch::planning` (plan/candidates/blockers) — has no single top-level value today |
| **Library-wide attention** | "What needs my attention across the whole library?" | `attention` + `diagnostics` + GUI Needs Attention — already unified and bounded |

**INFERENCE:** Ready-to-Play is the missing *first* axis expressed in the *second* axis's
vocabulary. It is not a third axis, and it must not create a parallel library-wide store.

## 3. Inventory of existing readiness primitives

Every row is **CONCLUSION FROM SOURCE** at the revision cited above. The "does not prove"
column is the part that matters: it is where a naive Ready-to-Play model would over-claim.

### 3.1 Identity and content

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `GameIdentityReport` + `IdentityStatus::{Verified, Candidate, Missing, Unsupported, Deferred, Invalid, Ambiguous, ResourceLimitReached}` + `IdentityKind` (`crates/archivefs-core/src/game_identity.rs:175,204,614`) | Per-file, per-kind evidence with explicit strength, read bounded | Nothing about launchability; `Verified` on a *format* or *title* kind is not launch identity | **Yes** — the reason-level evidence behind `IDENTITY_*` |
| `platform_evidence_fusion::identity_presentation::IdentityStatus::{Conflict, Ambiguous, VerifiedByDat, ContentAndDatAgree, ContentOnly, DatOnly, Unknown}` (`platform_evidence_fusion/identity_presentation.rs:33`) | A deterministic, documented priority order across content and DAT lanes | That the resolved identity is one the *planner* can use (different vocabulary) | **Yes** — keep as the library/identity surface; do not merge into the launch gate |
| `CanonicalIdentityStatus::{Resolved, Unknown, Conflict}` (`launch/planning.rs:45`) | The **launch gate**: the planner fails closed unless identity is resolved | Which evidence produced the resolution | **Yes** — the authoritative launch-identity input. Do not re-derive it |
| `evidence_bridge::is_identity_conferring` — an explicit 16-kind allowlist (PS1/PS2 serial, PSP disc ID, PS3 title ID, Saturn product number, Dreamcast/Sega CD product code, PCSX2 executable CRC, Dolphin game ID, loose-ROM SHA-256 + canonical variant, XBE/XEX title & media IDs, ScummVM game ID, 3DO disc ID, PC-FX disc hash) (`launch/evidence_bridge.rs:71-97`) | Exactly which identity facts may *confer* launch identity | Anything outside the list (PS4 title/content IDs are deliberately excluded as "PS4 Launch Phase 2") | **Yes, verbatim** — this is EmuWiz's existing definition of "sufficient for launch" |
| `IdentityConfidence::FilenameOnly` refusal + verified-identity persistence rules (`verified_identity_cache.rs:1-40`) | That filename-only evidence is never promoted; conflicting verified values persist neither | Anything about the file still existing (freshness is separate) | **Yes** — Ready-to-Play must inherit this refusal |
| `ArchiveSetIdentity` / `media_set` records (`platform_evidence_fusion/archive_set_identity.rs`; `media_set/model.rs`) | Grouping and set membership of multi-file/multi-disc releases | That any given medium is present and launchable | **Yes** — as media evidence, never as a launch gate by itself |
| `content_evidence` / `content_evidence_scope` / per-platform detectors (e.g. `chd_identity`, `disc_evidence_collector`, `gamecube_wii_boot_evidence`) | Content-level facts for specific platforms, bounded and read-only | Any emulator, firmware, or configuration fact | **Yes** — feeds `IdentityStatus::Verified` for the kinds above |
| `ExecutableSignatures`, `cartridge_header*`, `*_header_evidence`, `smd_normalization`, `n64_byte_order` | Format/header facts and lossless normalisation | Playability | **Information only** for Ready-to-Play |

### 3.2 Per-game launch planning and readiness (the closest existing model)

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `LaunchPlan` / `LaunchCandidate` / `LaunchPlanSummary{candidates,ready,ready_with_warnings,blocked}` (`launch/planning.rs:169,181,194`) | Every currently-knowable way to play one game, with per-candidate verdicts and an **optional** media topology projection | One top-level "can I play this?" answer; anything at all when identity is unresolved | **Yes** — the primary input to the new projection |
| `LaunchReadiness::{Ready, ReadyWithWarnings, Blocked}` (`launch/readiness.rs:53`) | A per-candidate verdict | Distinguishing "cannot work" from "must be fixed first" from "not yet checked" | **Yes** — map into the richer top-level state |
| `LaunchBlockerKind` (**198 typed variants**, `launch/readiness.rs:67`) + `LaunchBlocker{kind,detail}` (`:555`) | Precise, adapter-specific, structured reasons a candidate cannot launch | A stable *cross-adapter* vocabulary for UI grouping; any not-gathered distinction | **Yes as evidence; do not re-enumerate at the top level** (§5.2) |
| `LaunchWarningKind` (`launch/readiness.rs:572`) + `LaunchWarning` | Non-blocking conditions (optional firmware missing, firmware unverified, multiple eligible profiles, Cemu keys unverified, SameBoy header/custom-BOOT-ROM) | Any blocker | **Yes** — the existing precedent for "warning = uncertainty, not harm" |
| `build_launch_plan` — a documented **pure function**, no I/O (`launch/planning.rs:1-20,509`) | That the plan can be recomputed cheaply from already-gathered inputs | That those inputs were gathered | **Yes** — the projection must be equally pure |
| `LaunchTarget::{Standalone{..}, RetroArchCore{..}}` (`:134`) and `CandidatePreference::{Remembered,SoleEligible,Undetermined}` (`:157`) | Which target, and whether it was chosen by memory, by being the only option, or **not determined** | That the chosen target is a good one, or that a remembered preference still exists | **Yes** — `Undetermined` is exactly the "preferred emulator ambiguous" signal |
| `LaunchContentRef` / `LaunchContainerKind` / `LaunchContentKind` + `has_runnable_path()` (`:73,95,115,127`) | Which content was resolved, and whether a runnable path exists | Whether the emulator accepts that container (a separate `*ContentFormatUnsupported` blocker) | **Yes** |
| `MediaTopologyLaunchProjection{topology_state, start_media, media_sequence, swap_plan, action_safety, missing_media, conflicts, explanation}` (`launch/topology.rs:28`) | A launch-safe projection of an already-resolved media set, including a proven start medium and a typed missing-media list | Grouping truth (explicitly owned by `media_set`) | **Yes, verbatim** — media reasons read straight off this |
| `launch::execution` and the per-adapter `preflight_and_launch_*` functions (`launch/execution.rs`; `launch/process_spawn.rs`) | That execution re-validates everything fresh and never trusts cached readiness | Anything at planning time | **Yes, as the final gate's authority** — never replaced |
| `docs/LAUNCH_SUPPORT.md` ("Launch supported / Ready/planned / Blocked") | The user-facing launch vocabulary already published | A per-game unified label | **Yes** — new state names must not contradict it |

### 3.3 Emulator availability, version, and firmware/BIOS

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `RetroArchEnvironmentReport` + `RetroArchProfile` + `ProfileKind`/`ProfileScope` (`emulator_environment/retroarch.rs:684,704,135,147`) | Discovered RetroArch profiles/cores/executables and their resolution state | That a given game has a compatible core (planner-side) | **Yes** |
| `ExecutableState` / `ExecResolution` / `AppImageIdentificationConfidence` (`:593,604,563`) | Whether an executable resolved, and whether identification was exact or ambiguous | That it runs | **Yes** |
| Per-adapter readiness assessments: `ProfileAssessmentReport`, `XemuReadinessAssessment` (:1568), `XeniaReadinessAssessment` (:1789), `PpssppReadinessAssessment` (:1970), `Rpcs3ReadinessAssessment` (:2128) (`diagnostics/profiles.rs`) | Per-profile evidence: resolved executable, binding-problem text, and per-component system-file state (e.g. xemu `mcpx`/`flash_bios`/`eeprom`/`hdd`) | One library-wide per-game verdict | **Yes** — the emulator-side inputs |
| `LinuxEmulatorInstallationEvidence{emulator, installation_form, executable, profile, detail}` + `MANAGED_APPIMAGE_INSTALLATION_FORM` (`diagnostics/profiles.rs:70,76-82`) | How an emulator is installed (EmuWiz-managed AppImage, plain AppImage, Flatpak, PATH, config-only) | That the installation is complete or current | **Yes** — provenance for "installed" reasons |
| `safe_executable` (regular file, not a symlink, execute bit set) (`:84-98`) | Basic executability evidence | That the binary is the right emulator/version | **Yes** |
| `WritabilityAssessment::AppearsWritable` (`diagnostics/profiles.rs:1-30`) | Whether EmuWiz could *probably* write into a profile | A guaranteed write (portals can still refuse) | **Yes** — never upgraded to "writable" |
| `arcade_version_probe` — bounded `mame -version` probe (5 s timeout, 8 KiB cap: `PROBE_TIMEOUT`, `MAX_PROBE_OUTPUT`) (`diagnostics/arcade_version_probe.rs:51,56`) | An installed MAME version string; `None` (honest unknown) on timeout | FBNeo's version at all (deliberately un-probed) | **Yes** — the only permitted version probe; reuse it, add none |
| `ArcadeDatVersionCompatibility::{Matching, DatOlderThanEmulator, DatNewerThanEmulator, Unknown, NotApplicable}` (`diagnostics/arcade_dat_version.rs:226`) | Whether the audited arcade DAT came from the installed emulator build | Any non-arcade emulator version comparison | **Yes** — the version-mismatch reason family |
| `FirmwareReadiness::{Verified, PresentUnverified, Missing, Unknown, NotRequired}` (`launch/readiness.rs:32`) + projections from `DuckStationBiosState`, `Pcsx2BiosVerification`, `Rpcs3FirmwareStatus`, `XemuSystemFileState`, `FlycastSystemFileState`, `HatariTosHealth`, `PceCdFirmwareReadiness` (`:617,631,721,743`) | A uniform firmware state per target: verified by a real verifier, present-but-unverified, required-and-absent, unknown, or not required | That a *runtime-selected* BIOS is correct (see the arcade marker below) | **Yes, verbatim** — the BIOS reason family |
| `FirmwareIdentityRecord{system, provider, name, description, size_bytes, crc32, md5, sha1, dat_version}` (`dat/firmware_evidence.rs:84-104`) | Redump-published BIOS identity evidence extracted from a **user-supplied** DAT (PS1/PS2/Xbox), with DAT-version provenance | That a local file matches it (a separate hashing step); anything for systems without such a DAT | **Yes** — provenance for `BIOS_*`; only PCSX2 consumes it today |
| RetroArch `FirmwareRequirement{index, path, description, optional}` + `CoreInfoFinding` (`emulator_environment/retroarch.rs:372,390`) | Per-core declared firmware requirements, "optional unless marked required" | That the declared file is correct | **Yes** |
| `DatAuthorityDashboard` → `CompletenessCounts.bios_missing: Option<u64>` (`dat/authority.rs:71-85`) | A count of BIOS entries with no verified/pending representation, per platform/collection | Which *game* needs which BIOS; anything about runtime selection | **Yes** — library-level BIOS pressure, already surfaced as `Blocking` in Needs Attention (`needs_attention.rs:29-33`) |
| `BIOS_RUNTIME_SELECTION_NOT_MODELLED: bool = true` (`dat/dependency/mod.rs:96`) | An explicit in-code boundary: BIOS *storage* provision is resolved, **runtime BIOS selection is not modelled** | Anything about which BIOS a running machine would pick | **Must be honoured** — a `BIOS_SELECTION_UNMODELED` warning, never a readiness claim |

### 3.4 Media topology and arcade set/dependency evidence

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `MediaSet` / `MediaSetState::{CompleteSet, IncompleteSet, AmbiguousSet, ConflictingSet, UnverifiedSet, UnsupportedSet}` / `MediaSetConfidence` (`media_set/model.rs:39,284`) | Topology state of a release as a whole, with an explicit confidence axis | Launchability — the module states "a plan is data, never launch authority" (`media_set/mod.rs:1`) | **Yes** — media reason family |
| `ConflictKind` incl. `MissingMedium`, `MissingSide`, `UnknownCount`, `UnsupportedFormat`, `UnavailableRepresentation`, `UnresolvedRepresentation`, `CountConflict`, `OrdinalConflict` (`:80`) | Typed topology conflicts rather than prose | Which conflict actually blocks *this* game's launch | **Yes** — `MissingMedium`/`MissingSide` → blocking media reasons; the rest → review |
| `MediaAvailability::{Observed, Missing, Unverified}` (`:75`) | Per-member presence, keeping "unverified" distinct from "missing" | That "unverified" is safe to launch on | **Yes** — the crucial three-way distinction |
| `EvidenceKind` hierarchy `FuzzyTitle < Directory < Filename < Metadata < Embedded < TrustedDat < VerifiedNative` (`:55`) | A declared evidence hierarchy that "never upgrades a weak claim" | Emulator support | **Yes** — the identity-strength vocabulary for media reasons |
| `ExpectedCount` / `MediumRequirement{ordinal, side, role, medium, optional}` / `MediaRole` (`:163,168`) | Declared structure of a multi-medium release, including `optional` members | That optional members are optional *for launch* (a policy question) | **Yes** — `optional: true` must be a warning, never blocking |
| `SetState::{Complete, Incomplete, BadMetadata, NeedsReview}` + `SetResolution{members_required, members_verified, members_bad, members_optional, members_borrowed, disks_required, disks_verified, …}` (`dat/set.rs:269,290`) | Arcade/DAT **storage** completeness with `nodump`/`baddump` fail-closed rules and CHD header-identity disk verification | Dependencies, runnability, or runtime BIOS selection (documented at `dat/set.rs:269-273`) | **Yes** — arcade `SET_INCOMPLETE` / `CHD_REQUIRED` reasons |
| `DependencyState::{NotApplicable, NotEvaluated, Satisfied, Missing, Ambiguous, Cycle, Contradictory, Unsupported, EvidenceUnavailable}` + downgrade-only `apply_dependency_state` (`dat/dependency/mod.rs:236`) | Dependency closure — parent, ROM source, merged members, BIOS, devices, samples, CHD parents — fail-closed, never promoting a verdict | Runnability; `NotEvaluated` deliberately does not permit `Complete` | **Yes** — `DEPENDENCY_*` reasons |
| `ArcadeWorkingStatus::{Working, Imperfect, NotWorking, Unknown}` + `ArcadeCandidateEvidence{working_status, storage_complete, dependency_state, scan_complete}` + `eligible_storage()` (`playing_library/model.rs:48,70,89`) | Source-backed arcade working state, where "a negative result is trustworthy only when the source scan completed" | That an `Unknown` machine is playable (explicitly never eligible for a working-clone fallback) | **Yes** — arcade projection input |
| `PlayingLibraryPolicyMode::{Console1g1r, Arcade}` + elected parent/clone with explicit reasons (`playing_library/model.rs`, `matching.rs`) | Which parent/clone was elected, and why | That the elected set runs | **Yes** — `ARCADE_CLONE_ELECTED` / `ARCADE_PARENT_NOT_WORKING` reasons |
| `MameCommandPlan` blockers: `MameSetIncomplete`, `MameSetIdentityUnavailable/Ambiguous`, `MameDependencyBlocked`, `MameEmulatorUnavailable`, `MameSearchPathUnconfigured`, `MameLaunchArrangementUnsupported`, plus FBNeo equivalents (`launch/mame_command.rs:18,51`; `launch/readiness.rs:67`) | Arcade blockers already exist **inside** the shared blocker vocabulary | — | **Yes** — evidence that arcade needs no separate readiness system |
| `media_set::{filename_evidence, inspect, naming}` | Filename-derived media evidence at the declared lowest tier | Identity | **Yes**, at `EvidenceKind::Filename` tier only |

### 3.5 Mods, patches, cheats, and save data

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `mod_package` v1 manifest: `supported_game.identities`, `region`/`revision`, per-operation `required_source_sha256` and `expected_result_sha256`; only `create`/`replace`/declared `delete` eligible, and `patch` is parsed solely to refuse (`mod_package.rs:1-40`) | Base-match and output-verification semantics for local mod packages, fail-closed | That a mod is applied, or safe to apply | **Yes** — the model for `MOD_*` reasons |
| `CheatCandidateClassification::{CrossPlatform, Unsupported, Weak, Ambiguous, Strong, VerifiedExact}` + `is_installable()` (`patch_manager/cheat_candidates.rs:101-140`) | How strongly a candidate matches the selected archive, and whether it may ever be installed | That a mod changes game *launchability* | **Yes** — precedent for tiered, evidence-carrying compatibility |
| Revision/region mismatch tiers (`revision_mismatch` / `region_mismatch`, `patch_manager/cheat_catalogue.rs:1127,1433`; `cheat_candidates.rs:176`) | That a patch/cheat targets a different revision or region | — | **Yes** — the `PATCH_BASE_MISMATCH` reason |
| Cheat/mod install state and journals: `CheatInstalledState`, `CheatInstallOutcome`, `SharedApplyStatus`, `SharedApplyJournal`, rollback previews (`patch_manager/*`) | What is installed, with journaled apply/rollback | Game launchability | **Information only** — must never gate launch |
| Save data / save states | **Nothing.** No Save Vault or save-state code exists anywhere in `crates/` (re-confirmed by `docs/research/APOLLO_PS3_SAVE_VAULT_AUDIT.md` and the EmuHaven audit's fresh grep) | Any save-state compatibility fact | **None** — see section 14 |

### 3.6 Diagnostics, attention, aggregation, and caching

| Current primitive | What it proves | What it does **not** prove | Reuse for Ready-to-Play? |
|---|---|---|---|
| `Finding{id, category, subsystem, severity, title, explanation, why_it_matters, next_step, evidence, affected, recovery, repair, measurements}` (`diagnostics/mod.rs:369-420`) | One shared, namespaced, machine-readable diagnostic shape with typed `Measurement`s | Anything about one specific game's launch | **Yes** — the shape readiness reasons should mirror |
| `DoctorSeverity::{Healthy, Info, Warning, Error, Critical}` + `rank()` + **`is_blocking()`** (`:103-140`) | A severity scale with an explicit blocking predicate | A per-game verdict | **Yes, verbatim** — for the fixable-vs-blocking judgement |
| `KnownRecovery` ("informational metadata describing a repair the user can already perform elsewhere… carries no callable, no closure") + `Finding::repair: Option<DoctorRepairAction>` (`:336-368`) | A guide-vs-repair distinction, with repairs fieldless so no path or command can be smuggled in | That every repair is safe for every finding | **Yes** — the fixability model's precedent |
| `CoverageStatus` / `SubsystemCoverage` / `NotCheckedCheck` / `DeferredCheck` / `DEFERRED_CHECKS` (`:582-661`) | Explicit "what was not checked, and why" — the codebase's own not-gathered vocabulary | — | **Yes** — the model for `UNKNOWN` and coverage notes |
| `DoctorScan{overall_severity(), is_healthy(), count(), counts(), blocking_count(), checked_subsystems(), unavailable_subsystems(), by_category()}` + the **pure** `run_doctor_scan` (`diagnostics/runner.rs:248-398`) | Bounded, deterministic aggregation with a forced not-checked list | Per-game readiness | **Yes** — the aggregation pattern to copy |
| `AttentionSeverity::{Blocking, ActionNeeded, Warning, Info}`, `AttentionCategory` (13, incl. `Emulator`, `Launch`), `AttentionItem` (with `recommended_action`, `destination`, `recoverability`, `provenance`, `affected_count`), bounded `AttentionSnapshot` (`attention.rs:36,60,156,228,296`) | A library-wide, deduplicated, bounded, navigable attention model | A per-game verdict | **Yes** — the vocabulary Ready-to-Play must speak |
| `doctor_attention()` / `operation_attention()` (`attention.rs:373,462`) | Existing producers projecting Doctor findings and operation receipts into attention items | — | **Yes** — precedent for a `readiness_attention()` producer |
| GUI `launch_attention()` (`needs_attention.rs:280-331`) | Already converts "no ready candidate" into a `Blocking` item, names missing firmware via `FirmwareReadiness::Missing`/`RequiredFirmwareMissing`, routes BIOS to Emulator Setup and the rest to Launch Readiness, and **refuses to infer failure** when lanes are unscanned | Anything per-candidate; it fires only when *no* candidate is ready | **Yes** — the closest existing thing to Ready-to-Play; it must keep working unchanged |
| `verified_identity_cache` (dev/inode/size/mtime freshness; never authorizes launch) (`verified_identity_cache.rs:1-40`) | Cheap, honest identity reuse with staleness detection | Anything about emulator/profile/media change | **Yes** — the caching precedent and one invalidation input |
| `DatAuthoritySource` / `AuthorityFreshness::{Current, Stale, Unknown}` / `DatRefreshImpact` (`dat/authority.rs:8,18,105`) | DAT provenance, revision, sha256, and freshness vocabulary | — | **Yes** — DAT invalidation input |
| `Gathered<T>` (`diagnostics/runner.rs:67`) | Whether a lane's evidence was actually gathered | — | **Yes** — the not-gathered primitive |

### 3.7 Work that landed during this research (read, not modified)

**Coordination note.** Two commits landed while this document was being written. They overlap the
subject area, are described here so this document does not duplicate or contradict them, and
**this design does not depend on them**:

- `2625d0f` — `crates/archivefs-core/src/emulator_inventory.rs`: a bounded, read-only inventory of
  installed emulator executables (`InventoryEmulator::{Dolphin, Rpcs3, Pcsx2, Ppsspp,
  DuckStation, Xemu}`, a `PATH` scan bounded to 64 entries, a 2 s `--version` probe, 16 KiB output
  cap, `VERSION_PROBE_TIMEOUT`) that calls itself "an inventory projection, not an installer or a
  second profile/discovery system". Also `crates/archivefs-gui/src/emulator_inventory_page.rs`
  and `main.rs`/`navigation.rs` wiring.
- `f951d54` — `crates/archivefs-core/src/storage_health.rs`: read-only storage-format
  classification (`StorageFormatClass::{Chd, Rvz, Wia, Gcz, Wbfs, Cso, Zso, Iso, BinCue, Gdi,
  Cdi, Pbp, Archive, RawImage, Unknown}`) plus `crates/archivefs-gui/src/storage_health_page.rs`.

**INFERENCE:** the emulator inventory is the natural source for the "emulator installed / version
known" reasons in section 8 — and it is exactly the right shape (bounded, honest about unknown
versions, not an installer). Ready-to-Play must therefore *consume* an inventory result, and must
never scan `PATH` itself. The storage-health work is orthogonal to launch readiness (it concerns
storage formats, not launchability) and is referenced in section 23 only to confirm no overlap.

## 4. Top-level readiness state model

### 4.1 Proposed states (six, not sixty)

**Recommendation: keep exactly six top-level states.** They are named to stay consistent with
the two vocabularies already shipped (`docs/LAUNCH_SUPPORT.md`'s "Ready / Ready-planned /
Blocked", and `AttentionSeverity`'s Blocking / Action-needed / Warning / Info), and they add
only the two distinctions EmuWiz currently lacks per game: *fixable-but-required* and
*not-gathered*.

| State | Meaning | Maps from (existing type) |
|---|---|---|
| **`Ready`** | At least one candidate is launchable now, with no warnings worth showing | `LaunchReadiness::Ready` (`launch/readiness.rs:53`) |
| **`ReadyWithWarnings`** | Launchable now, but something is unverified or ambiguous | `LaunchReadiness::ReadyWithWarnings` + `LaunchWarningKind` (`:572`) |
| **`NeedsAttention`** | Something *required and specific* is missing or must be chosen, but the platform/emulator path itself is known — the user can act | **New projection**: required `LaunchBlockerKind`s that are fixable (missing firmware, missing emulator for a supported platform, incomplete media set, arcade dependency missing) |
| **`Blocked`** | EmuWiz cannot build a launch plan, or the evidence positively contradicts launchability | `LaunchBlockerKind::{IdentityUnresolved, IdentityConflict, ContentNotResolved, MediaTopologyBlocked, *PlatformMismatch, *ContentFormatUnsupported}` and the command-plan blockers |
| **`Unsupported`** | The platform, format, or emulator is not in EmuWiz's supported set at all — a permanent, honest "we do not do this", not a user error | `IdentityStatus::Unsupported`, `MediaSetState::UnsupportedSet`, absence from `launch::platform_map` |
| **`Unknown`** | Required evidence was **never gathered** (lane not scanned, discovery incomplete, evidence not loaded) and nothing proven blocks it | `Gathered<T>` absent (`diagnostics/runner.rs:67`), `CoverageStatus`, GUI `LaunchReadinessInput::{EvidenceNotLoaded, RetroArchNotScanned}` (`launch_readiness_page.rs:89`), `coverage_notes` |

### 4.2 Illustrative shape (design only — not implemented)

```rust
// DESIGN ONLY. No such type exists in the repository today.
pub struct ReadyToPlay {
    pub state: ReadyToPlayState,
    /// The single reason a user should read first; deterministic, never "highest score".
    pub primary: Option<ReadinessReason>,
    /// Every reason, ordered by severity then by a stable id — explainable, never scored.
    pub reasons: Vec<ReadinessReason>,
    /// What was actually gathered, so UNKNOWN is never silently reported as READY/BLOCKED.
    pub coverage: ReadinessCoverage,
    /// The candidate this verdict is about, when one exists.
    pub target: Option<ReadinessTarget>,
}
```

The three fields that are **new work** are exactly: `state` (the projection), `reasons` (the
shared vocabulary), and `coverage` (the not-gathered axis). `target` is a projection of an
existing `LaunchCandidate`/`LaunchTarget`, not a new type with new facts.

### 4.3 Why not more states

**INFERENCE, with evidence:** `LaunchBlockerKind` already has **198 variants**
(`launch/readiness.rs:67`) precisely because each adapter's reason is distinct and worth
preserving. That is the right design *at the evidence layer*. Promoting any of them to the
top level would recreate the fragmentation the attention model was built to end
(`attention.rs:1-8`), and would make a per-game label unstable across adapters (a missing
Dolphin binding and a missing RetroArch core are the same user-facing problem). **Rule: the
top-level enum stays small; specificity lives in `ReadinessReason` + the original typed
evidence carried as provenance.**

### 4.4 Precedence

The projection must be a **pure function with a documented priority order**, mirroring the
deterministic ordering already established in this codebase
(`platform_evidence_fusion/identity_presentation.rs:29-33`, "computed in a fixed, documented
priority order (never HashMap-iteration dependent)"):

```
Unsupported  > Unknown > Blocked > NeedsAttention > ReadyWithWarnings > Ready
```

with the explicit exception that **`Unknown` never outranks a *proven* blocker**: if a
definitive blocker exists, the state is `Blocked` even when other lanes are unscanned.
`Unknown` applies only when nothing proven blocks launch *and* a required lane was never
gathered. **INFERENCE:** this is what the existing GUI code already does for its
attention item (`needs_attention.rs:280-296` refuses to infer failure before returning
Blocking), and the per-game projection should simply make that same rule first-class.

## 5. Typed readiness reasons

### 5.1 Shape

```rust
// DESIGN ONLY. No such type exists in the repository today.
pub struct ReadinessReason {
    /// Stable, namespaced, machine-readable — same convention as `Finding::id`
    /// ("launch.identity_unresolved", "bios.missing"). Part of the CLI/JSON contract.
    pub id: ReadinessReasonId,
    pub family: ReadinessReasonFamily,
    /// Reuses the existing severity scale rather than inventing a third one.
    pub severity: ReadinessSeverity,     // Blocking | ActionNeeded | Warning | Info
    pub summary: String,                 // one plain sentence
    pub detail: String,                  // what was observed
    pub why_it_matters: Option<String>,
    pub next_action: Option<ReadinessAction>,
    pub fixability: Fixability,
    pub provenance: Vec<ReadinessProvenance>,
    /// The original typed evidence, retained — never flattened into the string above.
    pub evidence: ReadinessEvidence,
}
```

Two rules make this safe and cheap:

1. **`evidence` carries the existing typed value verbatim** (e.g.
   `LaunchBlockerKind::MameDependencyBlocked`, `MediaSetState::IncompleteSet`,
   `FirmwareReadiness::Missing`, `ArcadeDatVersionCompatibility::DatOlderThanEmulator`), so a
   reason is always traceable to the subsystem that produced it, and nothing is lost by
   grouping.
2. **`severity` reuses `AttentionSeverity`'s four levels** (`attention.rs:36`) rather than
   adding a third scale. Doctor's five-level `DoctorSeverity` (`diagnostics/mod.rs:103`) stays
   where it is; when a Doctor `Finding` is projected into a reason, `Error`/`Critical` map to
   `Blocking`, `Warning` to `Warning`, `Info` to `Info` — exactly the mapping the existing
   `doctor_attention` producer already performs (`attention.rs:462`).

### 5.2 Reason families and their existing sources

**These are the collapsing rules — the point is that 198 blockers become ~18 families.**

| Reason family (illustrative id) | Produced from (existing evidence, unchanged) | Default severity | State contribution | Fixability |
|---|---|---|---|---|
| `IDENTITY_UNRESOLVED` (`launch.identity_unresolved`) | `CanonicalIdentityStatus::Unknown`; `LaunchBlockerKind::IdentityUnresolved`; identity `Missing`/`Deferred`/`ResourceLimitReached` | Blocking | `Blocked` | EmuWizCanGuide, or UserCanFix when the cause is a misplaced file |
| `IDENTITY_CONFLICTING` (`launch.identity_conflict`) | `CanonicalIdentityStatus::Conflict`; `IdentityConflict`; `platform_evidence_fusion::IdentityStatus::Conflict`; `SameBoyIdentityConflict` | Blocking | `Blocked` | EmuWizCanGuide |
| `IDENTITY_WEAK` (`launch.identity_weak`) | `IdentityStatus::Candidate`, `IdentityConfidence::FilenameOnly` (never promoted) | Info | none (coverage note) | InformationOnly |
| `EMULATOR_MISSING` (`emulator.missing`) | `NoInstallationCandidate`, `*CandidateRequired`, `*EmulatorUnavailable` (incl. `MameEmulatorUnavailable`, `FbneoEmulatorUnavailable`, `HatariEmulatorUnavailable`), `ProfileIneligible` | ActionNeeded | `NeedsAttention` — or `Unsupported` when the platform has no adapter at all | UserCanFix + EmuWizCanGuide (Emulator Setup) |
| `EMULATOR_NOT_EXECUTABLE` (`emulator.binding_unavailable`) | `*BindingUnavailable`, `RetroArchExecutableMissing`, `AmbiguousRetroArchExecutable`, `RetroArchPathNotExact`, `AmbiguousRetroArchProfile`, `RetroArchCoreMismatch`, `CoreMissing` | Blocking | `Blocked` (fixable, but no runnable target is verified) | EmuWizCanGuide |
| `EMULATOR_VERSION_UNKNOWN` (`emulator.version_unknown`) | `ArcadeDatVersionCompatibility::Unknown`; probe returned `None` (`arcade_version_probe.rs`) | Warning | `ReadyWithWarnings` | InformationOnly |
| `EMULATOR_VERSION_INCOMPATIBLE` (`emulator.version_incompatible`) | `ArcadeDatVersionCompatibility::{DatOlderThanEmulator, DatNewerThanEmulator}` | ActionNeeded | `NeedsAttention` | UserCanFix (update DAT/emulator) + EmuWizCanGuide |
| `EMULATOR_AMBIGUOUS` (`emulator.ambiguous_preference`) | `CandidatePreference::Undetermined`, `LaunchWarningKind::MultipleEligibleProfiles`, `AmbiguousCore` | Warning | `ReadyWithWarnings` | UserCanFix (remember a profile) |
| `BIOS_MISSING` (`bios.missing`) | `FirmwareReadiness::Missing`, `RequiredFirmwareMissing`, per-adapter `Missing` states (`DuckStationBiosState::Missing`, `Pcsx2BiosVerification::Missing`, …) | ActionNeeded | `NeedsAttention` | UserCanFix + EmuWizCanGuide |
| `BIOS_UNVERIFIED` (`bios.unverified`) | `FirmwareReadiness::PresentUnverified`, `LaunchWarningKind::FirmwarePresentUnverified` | Warning | `ReadyWithWarnings` | InformationOnly unless a verifier is wired for that adapter |
| `BIOS_UNKNOWN` (`bios.unknown`) | `FirmwareReadiness::Unknown`, `Unreadable` states (`Pcsx2BiosVerification::Unreadable`) | Warning | `ReadyWithWarnings` — **never** `Missing` | InformationOnly |
| `BIOS_SELECTION_UNMODELED` (`bios.runtime_selection_not_modelled`) | `dat::dependency::BIOS_RUNTIME_SELECTION_NOT_MODELLED` (`dat/dependency/mod.rs:96`) | Info | none (coverage note) | InformationOnly |
| `FIRMWARE_MISSING` (`firmware.missing`) | `Rpcs3FirmwareUnavailable`, `Vita3kFirmwareUnavailable`, `XemuSystemFileState::Missing`, `FlycastSystemFileState::Missing`, `HatariTosMissing`, `*KickstartUnavailable`, `XRoarFirmwareMissing`, `TsugaruFirmwareMissing`, `MelonDsGameKeyMissing` | ActionNeeded | `NeedsAttention` | UserCanFix + EmuWizCanGuide |

| `MEDIA_INCOMPLETE` (`media.incomplete`) | `MediaTopologyMissingMedia`, `ConflictKind::{MissingMedium, MissingSide}`, `MediaSetState::IncompleteSet`, `MediaAvailability::Missing` | Blocking\* | `Blocked`\* | UserCanFix |
| `MEDIA_REVIEW_REQUIRED` (`media.review_required`) | `MediaTopologyReviewRequired`, `MediaSetState::{AmbiguousSet, ConflictingSet, UnverifiedSet}`, `MediaAvailability::Unverified`, `ConflictKind::UnprovenGrouping` | ActionNeeded | `NeedsAttention` | EmuWizCanGuide |
| `MEDIA_OPTIONAL_MISSING` (`media.optional_missing`) | `MediumRequirement{optional: true}` unsatisfied | Warning | `ReadyWithWarnings` | InformationOnly |
| `CONTENT_UNSUPPORTED` (`content.unsupported`) | `*ContentFormatUnsupported`, `MediaSetState::UnsupportedSet`, `ConflictKind::UnsupportedFormat`, `IdentityStatus::Unsupported`, `Vita3kContentUnsupported`, `CemuLayoutInvalid`, `CemuNotABaseTitle` | Blocking or Info | `Unsupported` when platform/format is out of scope, else `Blocked` | Unsupported |
| `CONFIG_REQUIRED` (`config.required`) | `DosBoxConfigMissing`, `DosBoxConfigMalformed`, `DosBoxConfigNoAutoexec`, `MameSearchPathUnconfigured`, `AmiberryMediaNotConfigured`, `AmiberryProfileRequired`, `CemuMlcUnavailable`, `HatariProfileUnavailable`, `TsugaruProfileUnavailable` | ActionNeeded | `NeedsAttention` | UserCanFix + EmuWizCanGuide |
| `ARCADE_SET_INCOMPLETE` (`arcade.set_incomplete`) | `SetState::{Incomplete, BadMetadata, NeedsReview}`, `MameSetIncomplete`, `FbneoSetIncomplete` | Blocking | `Blocked` | UserCanFix (acquire members) + EmuWizCanGuide |
| `ARCADE_DEPENDENCY_MISSING` (`arcade.dependency_missing`) | `DependencyState::{Missing, Ambiguous, Cycle, Contradictory, Unsupported, EvidenceUnavailable}`, `MameDependencyBlocked`, `FbneoDependencyBlocked` | Blocking / ActionNeeded | `Blocked` for `Missing` and structural states; `NeedsAttention` for `EvidenceUnavailable` | UserCanFix + EmuWizCanGuide |
| `ARCADE_CHD_REQUIRED` (`arcade.chd_required`) | `SetResolution::disks_required` vs `disks_verified`; `disks_parent_required` | ActionNeeded | `NeedsAttention` | UserCanFix |
| `ARCADE_WORKING_CLONE_ELECTED` (`arcade.working_clone`) | `ArcadeWorkingStatus::Working` elected where the parent is `NotWorking`/`Imperfect` (existing election) | Info | none | InformationOnly |
| `ARCADE_NOT_WORKING` (`arcade.parent_not_working`) | `ArcadeWorkingStatus::{NotWorking, Unknown}` with no working clone elected | Blocking / Warning | `Blocked` when `NotWorking` and nothing else is available; `ReadyWithWarnings` when a working clone *is* elected | InformationOnly (source-backed) |
| `CONTROL_REQUIREMENT_UNKNOWN` (`control.requirement_unknown`) | **No source exists today** (section 12) | Info | none (coverage note) | InformationOnly |
| `MOD_CONFLICT` / `PATCH_BASE_MISMATCH` / `MOD_DEPENDENCY_MISSING` | `mod_package` base/output hashes; `CheatCandidateClassification::{CrossPlatform, Unsupported}`; `revision_mismatch` / `region_mismatch` tiers | Warning or Blocking for a *modded* plan only | never changes unmodded readiness (section 13) | UserCanFix + EmuWizCanGuide |
| `LAUNCH_PLAN_INVALID` (`launch.plan_invalid`) | Command-plan failures: `DosBoxVariantUnsupported`, `MameLaunchArrangementUnsupported`, `Whdload*` blockers, `AmiberryMachineAmbiguous`, plus preflight/spawn error kinds | Blocking | `Blocked` | UserCanFix + EmuWizCanGuide |
| `NO_ADAPTER_FOR_PLATFORM` (`platform.unsupported`) | Absence from `launch::platform_map::launch_compatibility_for_platform` | Info | `Unsupported` | Unsupported |

\* **`MEDIA_INCOMPLETE` is `Blocked` only when the missing medium is required for *this*
launch.** `MediumRequirement::optional` (`media_set/model.rs:168`) exists precisely so optional
media can be absent without blocking; the projection must read that flag rather than treat
every `MissingMedium` conflict as fatal.

### 5.3 Naming and collapsing rules

- **Every illustrative name in the brief is either kept or deliberately merged**, as follows:
  `IDENTITY_UNCERTAIN` → split into `IDENTITY_UNRESOLVED` / `IDENTITY_CONFLICTING` /
  `IDENTITY_WEAK` (EmuWiz already distinguishes these three, and merging them would lose the
  fail-closed distinction). `EMULATOR_MISSING`, `EMULATOR_VERSION_UNKNOWN`,
  `EMULATOR_VERSION_INCOMPATIBLE`, `BIOS_MISSING`, `BIOS_AMBIGUOUS`(→ `BIOS_UNKNOWN` +
  `BIOS_UNVERIFIED`), `FIRMWARE_MISSING`, `MEDIA_INCOMPLETE`, `DISC_SET_INCOMPLETE`
  (→ `MEDIA_INCOMPLETE`/`MEDIA_REVIEW_REQUIRED`), `CHD_MISSING` (→ `ARCADE_CHD_REQUIRED` and
  the media equivalents), `DEPENDENCY_MISSING` (→ `ARCADE_DEPENDENCY_MISSING`),
  `ARCADE_ROMSET_VERSION_MISMATCH` (→ `EMULATOR_VERSION_INCOMPATIBLE`),
  `ARCADE_EMULATOR_VERSION_UNKNOWN` (→ `EMULATOR_VERSION_UNKNOWN`), `LAUNCH_PLAN_INVALID`,
  `MOD_CONFLICT`, `PATCH_BASE_MISMATCH`, `UNSUPPORTED_FORMAT` (→ `CONTENT_UNSUPPORTED` +
  `NO_ADAPTER_FOR_PLATFORM`) are kept.
- `SPECIAL_CONTROL_REQUIRED` is **replaced** by `CONTROL_REQUIREMENT_UNKNOWN` because no
  evidence source exists to justify "required" (section 12).
- `SAVE_STATE_VERSION_RISK` is **moved out of this enum entirely** (section 14) — it belongs to
  a separate sub-status, not to game readiness.
- `CONFIG_REQUIRED` is kept and consumed by `EMULATOR_VERSION_*`/`LAUNCH_PLAN_INVALID` where a
  config problem is really a binding problem.

**INFERENCE:** collapsing is only safe because `evidence` retains the original variant. A
reader who needs the Dolphin-specific reason still gets `LaunchBlockerKind::DolphinGameIdMissing`;
a user who needs the gist gets `launch.identity_unresolved`.

## 6. Evidence severity and aggregation

### 6.1 Severity mapping (E)

**The rule is: severity follows what the evidence *establishes*, not how much it worries
EmuWiz.** Uncertainty is never silently upgraded into failure, and never silently downgraded
into success.

| Evidence situation | Severity | Top-level contribution | Why |
|---|---|---|---|
| A proven blocker: identity unresolved/conflicting, content not resolved, no verified executable binding, media proven missing for a required medium, arcade set/dependency proven missing, command cannot be built | `Blocking` | `Blocked` | Execution cannot be attempted safely; a false "ready" here is a launch failure at best |
| A required component that is **absent and user-fixable**: BIOS/firmware missing, emulator not installed for a *supported* platform, arcade CHD/dependency absent, required config absent | `ActionNeeded` | `NeedsAttention` | The path is known and correct; only a component is missing. This is the state EmuWiz currently cannot express per game |
| Unknown *non-critical* information: emulator version unknown, optional firmware missing, multiple eligible profiles, BIOS present-but-unverified | `Warning` | `ReadyWithWarnings` | Launch can proceed; the user is told what is unproven |
| Unknown *required* compatibility: `DependencyState::EvidenceUnavailable`, media `Unverified`, arcade version `Unknown`, BIOS state `Unknown` | `Warning` (or `ActionNeeded` when the unknown could be resolved by an explicit user action) | `ReadyWithWarnings` / `NeedsAttention` | Compare: Doctor's own deferred checks are reported, not turned into failures (`diagnostics/mod.rs:630-661`) |
| A required lane was **never gathered** and nothing proven blocks launch | `Info` + coverage note | `Unknown` | **This is the single most important distinction in the model** |
| The platform/format/emulator is outside EmuWiz's supported set | `Info` | `Unsupported` | Honest scope boundary, not a user error |
| Nothing missing, nothing uncertain | — | `Ready` | — |

**Anti-rules (each already violated by naive implementations elsewhere):**

- **Do not treat `FirmwareReadiness::Unknown` as `Missing`.** The codebase is explicit:
  "`Unknown` is honest uncertainty, not a proven absence, so it never becomes `Missing`"
  (`launch/readiness.rs:611-616`). Ready-to-Play inherits this verbatim.
- **Do not treat `MediaAvailability::Unverified` as `Missing`** (three-way distinction,
  `media_set/model.rs:75`).
- **Do not treat `DependencyState::NotEvaluated` as `Satisfied`** — it deliberately does not
  permit `Complete` (`dat/dependency/mod.rs:241-247`).
- **Do not treat an unscanned emulator lane as "no emulator"** — the GUI already refuses this
  (`needs_attention.rs:288-296`).

### 6.2 Aggregation rules (O)

**Deterministic, ordered, evidence-typed — no scores, no percentages, no thresholds.**

```
1. If the platform/format is unsupported              → Unsupported
2. Else if any reason has severity Blocking           → Blocked
3. Else if a required evidence lane was not gathered
     and no reason is Blocking                        → Unknown
4. Else if any reason has severity ActionNeeded       → NeedsAttention
5. Else if any reason has severity Warning            → ReadyWithWarnings
6. Else                                                → Ready
```

Derived, explicit sub-rules:

- **`Unknown` is checked after `Blocked`** so a proven blocker always wins (section 4.4).
- **At least one `Ready` candidate sets the state to at most `ReadyWithWarnings`.** If *any*
  candidate is ready, the game is playable: the state must be `Ready`/`ReadyWithWarnings` and
  the unready candidates become reasons/informational entries, **not** a `Blocked` state. This
  matches the existing GUI's behaviour of only firing a Blocking attention item when *no*
  candidate is ready (`needs_attention.rs:302`).
- **`primary`** = the highest-severity reason, tie-broken by a fixed family order (never by
  iteration order), so the headline sentence is stable across runs.
- **Multi-disc is one readiness verdict per *game*, with per-medium reasons** — consistent
  with the media set model's `expected_count`/ordinal vocabulary rather than a per-file
  verdict.

**INFERENCE:** the reason a scoring model must be refused is not aesthetic. A score cannot
express "unknown" without either inventing a midpoint (which then reads as a mild failure) or
collapsing unknown into zero (which reads as a proven failure). Both are exactly the
over-claims this codebase spends its doc comments preventing.

## 7. Identity requirements (F)

### 7.1 The five identity classes Ready-to-Play needs

**Recommendation: do not invent these — project the existing evidence into five labels, and let
`CanonicalIdentityStatus` remain the only gate.**

| Ready-to-Play label | Projected from | Effect on readiness |
|---|---|---|
| **`CONFIRMED_IDENTITY`** | `IdentityStatus::Verified` for a kind in `evidence_bridge::is_identity_conferring` (`launch/evidence_bridge.rs:71-97`); optionally corroborated by `platform_evidence_fusion::IdentityStatus::{ContentAndDatAgree, VerifiedByDat}` | Contributes nothing negative; may raise the *displayed* confidence |
| **`SUFFICIENT_FOR_LAUNCH`** | `CanonicalIdentityStatus::Resolved` (`launch/planning.rs:45`) — i.e. whatever the planner already accepted | Required. Without it, `Blocked` (`IdentityUnresolved`) |
| **`AMBIGUOUS`** | `IdentityStatus::Ambiguous`; `platform_evidence_fusion::IdentityStatus::Ambiguous`; `MameSetIdentityAmbiguous`; `AmbiguousCore`/`AmbiguousRetroArchProfile` | `Blocked` (identity) or `ReadyWithWarnings` (target ambiguity) |
| **`CONFLICTING`** | `CanonicalIdentityStatus::Conflict`; `IdentityStatus::Invalid`; `platform_evidence_fusion::IdentityStatus::Conflict`; `SameBoyIdentityConflict` | `Blocked` |
| **`UNKNOWN`** | `IdentityStatus::{Missing, Deferred, Unsupported, ResourceLimitReached}`; evidence not gathered | `Unknown`/`Unsupported` — never silently `Blocked` |

### 7.2 What identity is *required* for launch

**Answer to the brief's key question: Ready-to-Play must NOT require perfect archival
identity.** It must require **launch-sufficient** identity, which EmuWiz already defines
precisely:

- The `evidence_bridge` allowlist (`launch/evidence_bridge.rs:71-97`) names the exact 16 kinds
  that may confer launch identity — a serial, a disc ID, a title ID, an executable CRC, a
  game ID, a verified loose-ROM hash, and their platform-specific peers.
- A **filename-only** identity is never sufficient and is explicitly refused by the persistence
  layer (`verified_identity_cache.rs:13-22`: `IdentityConfidence::FilenameOnly` is refused;
  a conflicting report persists neither value).
- A strong **DAT hash match** is excellent evidence but is *not* required for launch; a game
  with a verified serial and no DAT match is `Ready` for launch purposes while remaining
  "unverified" on the preservation axis. **INFERENCE:** keeping these two axes separate is what
  lets EmuWiz say "Ready to play with DuckStation" and "not confirmed against your DAT" at the
  same time without contradiction.

**Explicit non-requirements:** a DAT match, a 1G1R election, a canonical filename, a
normalized hash, or a set-level completeness verdict. Each is *evidence*, and each maps to its
own reason family, but none is a launch prerequisite.

## 8. Emulator availability and version readiness (G)

### 8.1 The seven distinctions, mapped

| The brief's distinction | Existing evidence | Reason family |
|---|---|---|
| Emulator installed | Profile discovery (`discover_*_profiles`), `LinuxEmulatorInstallationEvidence`, and the read-only `emulator_inventory.rs` projection (`2625d0f`) | `EMULATOR_MISSING` when absent |
| Executable usable | `safe_executable` (`diagnostics/profiles.rs:84-98`), `ExecutableState`/`ExecResolution`, `*BindingUnavailable` | `EMULATOR_NOT_EXECUTABLE` |
| Configured profile available | `RetroArchProfile`, per-adapter profile discoveries, `ProfileIneligible` | `EMULATOR_MISSING`/`EMULATOR_NOT_EXECUTABLE` |
| Version known | `mame -version` probe (`arcade_version_probe.rs`), the inventory `--version` probe (`VERSION_PROBE_TIMEOUT`); honest unknown when a probe fails | `EMULATOR_VERSION_UNKNOWN` |
| Version compatibility known | `ArcadeDatVersionCompatibility` (`arcade_dat_version.rs:226`) | `EMULATOR_VERSION_INCOMPATIBLE` / `_UNKNOWN` |
| Multiple emulator installs | `CandidatePreference::{SoleEligible, Undetermined}`, `LaunchWarningKind::MultipleEligibleProfiles`, `AmbiguousRetroArchExecutable` | `EMULATOR_AMBIGUOUS` |
| Preferred emulator ambiguous | `CandidatePreference::Undetermined` + `emulator_profile_memory` (`remember_emulator_profile_to`, `RememberedPreference` at `launch/planning.rs:222`) | `EMULATOR_AMBIGUOUS` |

### 8.2 Boundaries

- **No install, update, download, or channel logic** in Ready-to-Play. The existing
  `emulator_download.rs` / `managed_appimage_bootstrap` managed-install path stays the only
  install mechanism and is not invoked by a readiness projection.
- **No new version probing.** The MAME probe is the only permitted process-spawning probe in
  the arcade path, with bounded timeout and output (`arcade_version_probe.rs:51,56`); the
  the inventory's `--version` probe is the same pattern. A readiness projection **must
  consume** a probe result, never perform one during rendering.
- **Non-arcade versions are legitimately unknown.** Dolphin/PCSX2/etc. have no version
  compatibility model, and Ready-to-Play must show `EMULATOR_VERSION_UNKNOWN` as a warning —
  never as a blocker, and never as a fabricated "compatible".

## 9. BIOS / firmware readiness (H)

### 9.1 How existing BIOS evidence feeds readiness

| Situation | Evidence | Severity | State |
|---|---|---|---|
| Required BIOS verified | `FirmwareReadiness::Verified` (produced only by a real verifier, e.g. Redump-backed PS2 BIOS resolution) | — | no reason |
| Required BIOS present, unverified | `FirmwareReadiness::PresentUnverified`, `LaunchWarningKind::FirmwarePresentUnverified` | Warning | `ReadyWithWarnings` |
| Required BIOS missing | `FirmwareReadiness::Missing`, `RequiredFirmwareMissing` | ActionNeeded | `NeedsAttention` |
| BIOS state unknown | `FirmwareReadiness::Unknown` (unreadable location, not configured, or the adapter carries no evidence) | Warning | `ReadyWithWarnings` — **never** `Missing` |
| Platform needs no BIOS | `FirmwareReadiness::NotRequired` (a constant for PPSSPP, deliberately not a projection: "inventing a PSP BIOS requirement here would be exactly the kind of unreviewed assumption this module exists to avoid", `launch/readiness.rs:15-20`) | — | no reason |
| Multiple BIOS variants present / ambiguous | Per-adapter discovery can surface distinct variants; **there is no cross-adapter "ambiguous variant" state today** | Warning when detectable | `ReadyWithWarnings`; otherwise `BIOS_SELECTION_UNMODELED` coverage note |
| BIOS identity mismatch | Only where a verifier exists (PCSX2 via `FirmwareIdentityRecord` from a user-supplied Redump DAT) | ActionNeeded | `NeedsAttention` |
| Runtime BIOS *selection* correctness | `BIOS_RUNTIME_SELECTION_NOT_MODELLED = true` (`dat/dependency/mod.rs:96`) | Info | coverage note only, per the module's own rule that no state "may be read as 'this runs'" |
| Library-wide BIOS pressure | `CompletenessCounts.bios_missing` (`dat/authority.rs:71-85`), already surfaced as `Blocking` in Needs Attention | ActionNeeded/Blocking at the *library* level | already exists — do not duplicate per game |

### 9.2 Three BIOS questions that must stay separate

1. **Is storage present?** (`DependencyState`/`SetResolution` for arcade BIOS sets; file
   presence for adapters.)
2. **Is the file the verified identity?** (only where a hash verifier exists.)
3. **Would the emulator *select* the right one at runtime?** (**not modelled**, by explicit
   design.)

**INFERENCE:** Ready-to-Play may answer 1 and 2 and must answer 3 with a coverage note. A
model that silently conflates them would produce exactly the false assurance the
`BIOS_RUNTIME_SELECTION_NOT_MODELLED` marker was written to prevent.

### 9.3 Boundaries

- **Do not implement BIOS/firmware publishing, moving, copying, or downloading.** The brief
  forbids it, and EmuWiz's own rule is that firmware is never bundled
  (`dat/firmware_evidence.rs:11-18`: Redump's PS2 BIOS DAT is deliberately not embedded;
  a user must supply their own DAT).
- **Do not invent BIOS requirements** for platforms whose adapters declare none (the PPSSPP
  constant is the precedent).
- **Do not hash BIOS files during rendering.** Verification is a discrete, bounded operation
  already owned by the adapter verifiers.

## 10. Media / topology readiness (I)

### 10.1 Reuse map (no new grouping engine)

| Media shape | Existing evidence | Readiness projection |
|---|---|---|
| Single-file game | `LaunchContentRef::has_runnable_path()` (`launch/planning.rs:127`) | ready unless a blocker exists |
| Multi-track disc (BIN/CUE, GDI, CCD/IMG/SUB) | `media_set` adapters + `launch::topology` projection; per-adapter `*ContentFormatUnsupported` | `CONTENT_UNSUPPORTED` if the adapter cannot take it; otherwise no reason |
| Multi-disc game | `MediaSet` with ordinals; `MediaTopologyLaunchProjection{start_media, media_sequence, swap_plan}`; `MediaSwapPlan` (`media_set/model.rs:333`) | `Ready`/`ReadyWithWarnings`; missing required disc → `MEDIA_INCOMPLETE` |
| Companion files (side files, M3U/playlist) | `platform_evidence_fusion/cue_m3u_parsing`; `MediaRole::{BootMedia, PlayMedia, InstallMedia, DataMedia, …}` | playlist/side-file absence is a warning unless declared required |
| CHD | `chd_identity`/`chd_logical_media`; `dat/archive/chd.rs`; `DiskAuditVerdict::Exact` in `dat/disk_audit` | present+verified CHD → no reason; missing required disk → `ARCADE_CHD_REQUIRED`/`MEDIA_INCOMPLETE` |
| Partial / incomplete set | `MediaSetState::IncompleteSet`, `MediaAvailability::Missing`, `SetState::Incomplete` | `MEDIA_INCOMPLETE` (Blocked when required) or `NeedsAttention` |
| Wrong media topology | `MediaSetState::{AmbiguousSet, ConflictingSet}`, `ConflictKind::{OrdinalConflict, RoleConflict, CountConflict}` | `MEDIA_REVIEW_REQUIRED` |
| Unverified topology | `MediaSetState::UnverifiedSet`, `MediaAvailability::Unverified`, `MediaSetConfidence::Unverified` | **`MEDIA_REVIEW_REQUIRED`, never `MEDIA_INCOMPLETE`** |
| Topology needs review before choosing a start medium | `LaunchBlockerKind::MediaTopologyReviewRequired`, `MediaTopologyLaunchProjection.action_safety` (`ActionSafety`) | `MEDIA_REVIEW_REQUIRED` |

### 10.2 Boundaries

- **No second media grouping engine.** `media_set/mod.rs:1` states the contract: "A plan is
  data, never launch authority." Ready-to-Play consumes `MediaSet`/`MediaSwapPlan` and does no
  grouping, no ordinal inference, and no filename-based member matching
  (`EvidenceKind` explicitly ranks `Filename` low, `media_set/model.rs:55`).
- **A missing *optional* medium never blocks** (`MediumRequirement::optional`).
- **A start-medium cannot be guessed**: the topology projection already refuses to turn
  lexical/member order into a start disc (`launch/topology.rs:46-63`), and that refusal must
  surface as `MEDIA_REVIEW_REQUIRED`, never as an invented start disc.

## 11. Arcade readiness and its projection into the generic model (J)

### 11.1 Mapping each arcade concern onto generic reason families

**No arcade-specific readiness model is needed, and none should be built** — arcade blockers
already live inside the shared `LaunchBlockerKind` vocabulary (`launch/mame_command.rs:51`;
`launch/fbneo_command.rs`), which is itself the strongest available evidence that the generic
model is the right home.

| Arcade concern | Existing evidence | Generic reason family |
|---|---|---|
| Elected parent/clone | The existing arcade election path (`PlayingLibraryPolicyMode::Arcade`, explicit reasons, no opaque score; `playing_library/matching.rs`) | `ARCADE_WORKING_CLONE_ELECTED` (Info) or nothing |
| Working / imperfect / not working | `ArcadeWorkingStatus::{Working, Imperfect, NotWorking, Unknown}`: "source-backed… deliberately not inferred from a title, filename, or popularity list", and `Unknown` "is never eligible for a working-clone fallback" (`playing_library/model.rs:42-53`) | `ARCADE_NOT_WORKING` (Blocked/Warning) |
| Dependency completeness | `DependencyState` + downgrade-only `apply_dependency_state`; `DependencyKind` (8 distinct kinds: `cloneof`, `romof`, merged member, BIOS, device, sample, CHD parent, …); `MameDependencyBlocked`/`FbneoDependencyBlocked` | `ARCADE_DEPENDENCY_MISSING` |
| BIOS (as a set) | BIOS resolves as *storage provision* (`DependencyKind` + `SetResolution`), with runtime selection explicitly unmodelled (`BIOS_RUNTIME_SELECTION_NOT_MODELLED`) | `BIOS_MISSING` (storage) + `BIOS_SELECTION_UNMODELED` (Info) |
| Device ROMs | `device_ref` is "a device requirement, not ROM borrowing" (`dat/dependency/mod.rs:42`), resolving through the same dependency vocabulary | `ARCADE_DEPENDENCY_MISSING` |
| CHD dependencies | `SetResolution::{disks_required, disks_verified, disks_parent_required}`; CHD header identity via `DiskAuditVerdict::Exact`; a CHD `parent_sha1` is "a format-level delta dependency… **not** the DAT's `disk merge=`" (`dat/dependency/mod.rs:44-45`) | `ARCADE_CHD_REQUIRED` |
| ROMset vs emulator version (MAME) | `ArcadeDatVersionCompatibility::{Matching, DatOlderThanEmulator, DatNewerThanEmulator, Unknown}` from a bounded `mame -version` probe | `EMULATOR_VERSION_INCOMPATIBLE` / `_UNKNOWN` |
| ROMset vs emulator version (FBNeo) | Documented as **deliberately un-probed**: "FBNeo is therefore left un-probed here: its compatibility stays `Unknown` unless a version string is supplied some other way" (`diagnostics/arcade_version_probe.rs:20-26`) | `EMULATOR_VERSION_UNKNOWN` (always) |
| Set storage completeness | `SetState`/`SetResolution` with `nodump`/`baddump` fail-closed rules (`dat/set.rs`) | `ARCADE_SET_INCOMPLETE` |
| Controller/control-panel requirements | **Not modelled anywhere** (section 12) | `CONTROL_REQUIREMENT_UNKNOWN` (Info) |

### 11.2 Cross-check against the in-tree arcade audit

The concurrent in-tree audit (`docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md`) recommends a
near-term read-only arcade vocabulary: `READY`, `READY_WITH_WORKING_CLONE`,
`MISSING_DEPENDENCY`, `ROMSET_VERSION_MISMATCH`, `EMULATOR_VERSION_UNKNOWN`,
`KNOWN_NOT_WORKING`, `IMPERFECT`, `REVIEW_REQUIRED`. **INFERENCE: that vocabulary is fully
expressible in the generic model proposed here**, which is a deliberate design goal:

| Arcade audit state | Generic projection |
|---|---|
| `READY` | `Ready` |
| `READY_WITH_WORKING_CLONE` | `ReadyWithWarnings` + `ARCADE_WORKING_CLONE_ELECTED` (Info) |
| `MISSING_DEPENDENCY` | `NeedsAttention`/`Blocked` + `ARCADE_DEPENDENCY_MISSING` |
| `ROMSET_VERSION_MISMATCH` | `NeedsAttention` + `EMULATOR_VERSION_INCOMPATIBLE` |
| `EMULATOR_VERSION_UNKNOWN` | `ReadyWithWarnings` + `EMULATOR_VERSION_UNKNOWN` |
| `KNOWN_NOT_WORKING` | `Blocked` + `ARCADE_NOT_WORKING` |
| `IMPERFECT` | `ReadyWithWarnings` + `ARCADE_NOT_WORKING` (Imperfect) |
| `REVIEW_REQUIRED` | `NeedsAttention` + `MEDIA_REVIEW_REQUIRED`/identity reasons |

**Coordination note:** that audit is by concurrent work and may propose arcade-layer types. If
it lands as a *separate* arcade status, the rule must be that the arcade status is **one
evidence producer** among several, never a parallel top-level readiness — otherwise EmuWiz
would ship two answers to the same question.

## 12. Controller / input readiness (K)

### 12.1 The honest finding: there is no evidence source today

**CONCLUSION FROM SOURCE:** a repository-wide search finds **no** modelling of lightguns,
wheels, pedals, trackballs, keyboard/mouse requirements, or per-game controller profiles in
core, CLI, or GUI. The only hits for "wheel" are mouse-wheel UI scroll tests
(`crates/archivefs-gui/src/tests/*`), and there is no arcade control-panel metadata anywhere.

### 12.2 Recommendation: `UNKNOWN` — never a blocker, never a warning

| Option | Verdict |
|---|---|
| **BLOCK** on a special-control requirement | **Rejected.** EmuWiz has no evidence of which games require which controls; blocking would be fabricated — and wrong in the common case (most arcade titles whose *cabinet* used a wheel are playable on a pad). |
| **WARN** that a special control is required | **Rejected for now.** A warning still asserts a requirement EmuWiz cannot prove. |
| **UNKNOWN, as a coverage note only** | **Recommended.** EmuWiz states plainly: "Control requirements are not modelled; EmuWiz cannot tell you whether this game needs a specific controller." |

This mirrors two existing precedents: the PPSSPP firmware constant refuses to invent a
requirement (`launch/readiness.rs:15-20`), and `DEFERRED_CHECKS` names what Doctor deliberately
does not check rather than implying absence of a problem (`diagnostics/mod.rs:630-661`).

### 12.3 What a future, evidence-backed model would look like

**INFERENCE, later phase only:** a `ControlRequirement` must come from a real catalogue with
provenance (a curated list or DAT-like source), and must carry the same tiering EmuWiz already
uses for compatibility elsewhere (`CheatCandidateClassification`'s Weak→VerifiedExact ladder,
`patch_manager/cheat_candidates.rs:101-140`). Only a *verified* hard requirement would warn;
a verified "standard gamepad is sufficient" fact would clear the note. **Explicitly: arcade
cabinet control metadata alone must never be used to mark an ordinary gamepad as proven
sufficient** — the brief says so, and nothing in the current evidence contradicts it.

## 13. Mods / patches readiness (L)

### 13.1 Mods must not change *game* readiness by default

**Recommendation:** readiness is computed for the **unmodded** launch first, and a modded plan
is a *variant* with its own reasons. A game with a broken mod is still `Ready` to play
unmodded; it becomes a mod-specific problem, not a game-readiness problem.

| Mod layer state | Evidence | Projection |
|---|---|---|
| `READY_UNMODDED` | Base game readiness (`Ready`/`ReadyWithWarnings`) | the game's state, unchanged |
| `READY_MODDED` | A mod plan whose `required_source_sha256` (base match) **and** `expected_result_sha256` (output verification) both pass, per `mod_package` semantics (`mod_package.rs:1-40`) | `ReadyWithWarnings` + Info "mod applies cleanly"; the modded variant is launchable |
| `MOD_CONFLICT` | Two mods writing the same destination, or `CheatCandidateClassification::CrossPlatform`/`Unsupported` for the selected archive | mod-variant warning/blocker; **never** lowers base game readiness |
| `PATCH_BASE_MISMATCH` | Manifest `supported_game.identities`/`revision` mismatch; the `revision_mismatch` tier (`patch_manager/cheat_catalogue.rs:1127,1433`) | blocker for the *modded* variant only |
| `MOD_DEPENDENCY_MISSING` | A declared dependency/operation whose required source is absent | blocker for the modded variant only |

### 13.2 Boundaries

- **No implementation of mods/patches** here (per the brief).
- **The mod layer never invents application.** `mod_package` deliberately refuses `patch`
  operations and only permits `create`/`replace`/declared `delete`; Ready-to-Play must present
  that refusal as `MOD_UNSUPPORTED_INPUT`, not as a general failure.
- **Cheats/patches installed state is informational.** `CheatInstalledState`/
  `SharedApplyStatus` say what is installed; they do not affect launchability, and a
  "cheats disabled" state must never read as "not ready".

## 14. Save state / Save Vault interaction (M)

### 14.1 Two questions, two sub-statuses

| Question | Sub-status | Evidence today |
|---|---|---|
| "Can this game launch?" | **`ReadyToPlay`** (this document) | Rich (section 3) |
| "Can I resume from a save state here?" | **`ResumeReadiness`** (separate, future) | **None** — no save/save-state code exists in `crates/` |

**Recommendation:** keep them strictly separate; never let a save-state concern reduce the
game readiness state. A game is `Ready` even if its save states are unknown.

### 14.2 Why the distinction is load-bearing

- Save states are **emulator-version- and core-version-sensitive**: a state written by a
  different build can fail to load, or load incorrectly. That is a real risk — but it is a risk
  *to a state*, not to launching.
- EmuWiz has no version-compatibility model for save states today (no Save Vault implementation;
  the only in-tree trace is `docs/research/APOLLO_PS3_SAVE_VAULT_AUDIT.md`).
- Therefore the honest states are: `Unknown`/`NotEvaluated` (no evidence) and, later, a
  dedicated vocabulary such as `ResumeReady` / `ResumeUnverified` / `ResumeVersionRisk` /
  `NoSaveState` / `Unknown` — **owned by Save Vault**, not by Ready-to-Play.

**Direction of dependency (important):** Save Vault may *consume* Ready-to-Play (a state is
only resumable if the game is launchable); Ready-to-Play must **not** consume Save Vault.
That keeps the two subsystems independently shippable and avoids a cycle. **No Save Vault code
was read beyond the absence check confirmed above, and none is proposed here.**

## 15. The launch plan as the final gate (N)

### 15.1 Readiness is not "the plan exists" — it is "the plan passed its own checks"

**Recommendation:** `Ready`/`ReadyWithWarnings` may only be produced when EmuWiz could actually
build a launch plan for that candidate, i.e. all of the following hold. Each is already a
typed condition in the codebase; none is new:

1. **Emulator executable resolved** — a `LaunchTarget` with a verified binding, not a
   `*BindingUnavailable` (`launch/readiness.rs:67`; `diagnostics/profiles.rs`).
2. **Launch media resolved** — `LaunchContentRef::has_runnable_path()` (`launch/planning.rs:127`),
   and for a disc set a proven start medium from the topology projection
   (`launch/topology.rs:46-63`).
3. **Platform adapter ready** — the platform maps to the target
   (`launch/platform_map::launch_compatibility_for_platform`), and the adapter-specific
   identity requirement is satisfied (serial / disc ID / game ID / title ID / game key).
4. **Required inputs projected** — `launch/input_projection.rs` produced the adapter's own
   request type rather than an explicit unavailable result.
5. **No ambiguous argument or path state** — no `AmbiguousCore`, `AmbiguousRetroArchProfile`,
   `AmbiguousRetroArchExecutable`, `RetroArchPathNotExact`, `AmiberryMachineAmbiguous`,
   `AmiberryBindingAmbiguous`, `HatariBindingAmbiguous`, `WhdloadProfileAmbiguous`, and no
   preflight/spawn failure kind.
6. **No drift detected** — `SameBoyDriftBeforeSpawn`/`CemuDriftBeforeSpawn` exist precisely
   because a file can change between plan and spawn; a readiness verdict must be built from a
   plan whose inputs are still current (section 20).

### 15.2 If launch planning fails, the state is not `Ready`

**Hard rule:** if the planner cannot produce a candidate, the state is `NeedsAttention`,
`Blocked`, `Unsupported`, or `Unknown` — never `Ready`/`ReadyWithWarnings`. The existing GUI
already behaves this way and, importantly, treats *unscanned* lanes as `Unknown`-like rather
than as failure (`needs_attention.rs:288-296`).

**And the inverse must also hold:** a readiness verdict is **not** execution authority.
Execution re-derives everything (fresh identity re-inspection, fresh environment discovery,
freshly rebuilt plan/command) — see `launch/execution.rs`, `launch/process_spawn.rs`,
`verified_identity_cache.rs:1-14`, and `docs/LAUNCH_SUPPORT.md`'s "Safety boundaries".
Ready-to-Play must explicitly document that it is a *report*, not a token.

## 16. Fixability model (Q)

```rust
// DESIGN ONLY.
pub enum Fixability {
    /// The user can resolve it outside EmuWiz (supply a BIOS, install an emulator,
    /// locate a missing disc, choose a profile).
    UserCanFix,
    /// EmuWiz can take the user exactly to the existing surface that addresses it
    /// (Emulator Setup, Doctor, DAT authority, Multi-disc review, Cheats & Mods).
    EmuWizCanGuide,
    /// An existing, journaled EmuWiz repair already covers it. Today this is exactly the
    /// four Doctor repairs; nothing new may claim this variant.
    EmuWizCanRepairSafely,
    /// Platform/format/emulator outside EmuWiz's scope. Not fixable by design.
    Unsupported,
    /// True statement with no action (e.g. runtime BIOS selection not modelled).
    InformationOnly,
}
```

| Reason family | Fixability |
|---|---|
| `IDENTITY_UNRESOLVED`, `IDENTITY_CONFLICTING` | `EmuWizCanGuide` (identity review surfaces) |
| `EMULATOR_MISSING`, `CONFIG_REQUIRED` | `UserCanFix` + `EmuWizCanGuide` (Emulator Setup) |
| `EMULATOR_NOT_EXECUTABLE`, `LAUNCH_PLAN_INVALID` | `EmuWizCanGuide` |
| `BIOS_MISSING`, `FIRMWARE_MISSING` | `UserCanFix` + `EmuWizCanGuide` |
| `BIOS_UNVERIFIED`, `BIOS_UNKNOWN`, `BIOS_SELECTION_UNMODELED` | `InformationOnly` |
| `MEDIA_INCOMPLETE`, `ARCADE_CHD_REQUIRED`, `ARCADE_SET_INCOMPLETE` | `UserCanFix` + `EmuWizCanGuide` |
| `MEDIA_REVIEW_REQUIRED` | `EmuWizCanGuide` |
| `EMULATOR_VERSION_INCOMPATIBLE` | `UserCanFix` + `EmuWizCanGuide` |
| `EMULATOR_VERSION_UNKNOWN`, `EMULATOR_AMBIGUOUS`, `ARCADE_WORKING_CLONE_ELECTED` | `InformationOnly` (ambiguity may also be `UserCanFix` by remembering a profile) |
| `ARCADE_DEPENDENCY_MISSING` | `UserCanFix` (when `Missing`) or `EmuWizCanGuide` (when `EvidenceUnavailable`) |
| `ARCADE_NOT_WORKING` | `InformationOnly` (source-backed fact) |
| `CONTENT_UNSUPPORTED`, `NO_ADAPTER_FOR_PLATFORM` | `Unsupported` |
| `CONTROL_REQUIREMENT_UNKNOWN` | `InformationOnly` |
| `MOD_*`, `PATCH_BASE_MISMATCH` | `UserCanFix` + `EmuWizCanGuide` (Cheats & Mods) |

**Hard rules:**

- **`EmuWizCanRepairSafely` may only be claimed where a real repair exists** — today, only
  Doctor's four actions (`repair.rs`, and `DEFERRED_CHECKS`' explicit statement that
  "everything else - permission changes, remounting, database repair, removing managed cheat
  entries, rolling back an interrupted install - is reported and explained, never performed",
  `diagnostics/mod.rs:657-661`).
- **A reason must never imply that EmuWiz will act.** Following `KnownRecovery`'s contract,
  guidance carries no callable, no closure, and no command ("carries no callable, no closure,
  and no path to execute against", `diagnostics/mod.rs:36-40`).
- **`UserCanFix` is not a promise the user has the material.** Licensing, availability, and
  legality are the user's domain; EmuWiz must not offer acquisition (§26).

## 17. User-facing explanations (P)

### 17.1 Required shape of every result

Every readiness result carries four layers, in this order:

1. **Short summary** — one sentence, no jargon, naming the emulator when known.
2. **Typed reasons** — the ordered `ReadinessReason` list (family + severity + fixability).
3. **Technical evidence/details** — the retained typed evidence and provenance, shown on
   demand (a "Details"/"Technical" disclosure), never in the headline.
4. **Recommended next action** — a single concrete step plus a **navigation-only destination**,
   reusing `AttentionDestination` (`attention.rs:111`) so the user lands on the existing
   surface rather than a new one.

### 17.2 Worked examples (the brief's cases, filled in honestly)

| State | Summary | Reasons | Next action |
|---|---|---|---|
| `Ready` | "Ready to play with PCSX2." | none | "Launch" (existing typed launch action) |
| `ReadyWithWarnings` | "Ready to play with RetroArch, but emulator version compatibility has not been verified." | `EMULATOR_VERSION_UNKNOWN` (Warning, InformationOnly) | "Play anyway" / "Details" |
| `NeedsAttention` | "Missing required PS2 BIOS." | `BIOS_MISSING` (ActionNeeded, UserCanFix) | "Open Emulator Setup" → `AttentionDestination::EmulatorSetup` |
| `NeedsAttention` | "Dolphin is not installed, and this GameCube game needs it." | `EMULATOR_MISSING` (ActionNeeded, UserCanFix) | "Open Emulator Setup" |
| `Blocked` | "Disc 2 is missing from this media set." | `MEDIA_INCOMPLETE` (Blocking, UserCanFix) — with the exact expected ordinal | "Review media set" → `MainView::Selected`, or the multi-disc review surface |
| `Blocked` | "EmuWiz cannot tell which game this is — identity evidence conflicts." | `IDENTITY_CONFLICTING` (Blocking, EmuWizCanGuide) | "Review identity evidence" |
| `Unsupported` | "EmuWiz has no launch path for this platform yet." | `NO_ADAPTER_FOR_PLATFORM` (Info, Unsupported) | none (documentation) |
| `Unknown` | "Launch readiness has not been checked for this game yet." | coverage note only | "Check now" (runs the *existing* discovery/preflight, never a new probe) |
| `Blocked` (arcade) | "This ROM set is missing required dependencies: neogeo." | `ARCADE_DEPENDENCY_MISSING` (Blocking, UserCanFix) | "Review set dependencies" |
| `ReadyWithWarnings` (arcade) | "The parent set is known not to work; a working clone is selected instead." | `ARCADE_NOT_WORKING` + `ARCADE_WORKING_CLONE_ELECTED` (Info) | "Play `<clone>`" |

**INFERENCE:** the `Unknown` row is the one no comparable tool in section 22 produces, and it
is the difference between "we checked and it is fine" and "we have not looked" — a distinction
users of ROM managers routinely misread.

### 17.3 Copy rules

- Never say "verified" for `PresentUnverified`, and never say "missing" for `Unknown`.
- Never say a game "should work" — either EmuWiz has evidence, or it says it does not.
- Never show a percentage, star rating, or confidence score.
- Always name the emulator when the verdict is about a specific candidate.
- Keep the existing guarantees intact: no sentence may imply that a repair, download, or
  install will happen automatically.

## 18. The Ready-to-Play view (R)

### 18.1 Read-only page, sections derived from the state model

**Proposal:** one read-only page whose sections are the six states (not a re-scored list):

| Section | Contents |
|---|---|
| **Ready** | Games with a launchable candidate |
| **Ready with warnings** | Launchable, with the warning reasons inline |
| **Needs attention** | Missing-but-fixable components, grouped by reason family (BIOS, emulator, media, config) |
| **Blocked** | Proven blockers, grouped by reason family |
| **Unsupported** | Out-of-scope platforms/formats, collapsed by default |
| **Unknown / not checked** | Games whose evidence was never gathered, with an explicit "check now" affordance |

### 18.2 Filters (matching the brief's list)

Platform · emulator · reason family · severity · BIOS missing · emulator missing · media
incomplete · version mismatch · controller (present but permanently "not modelled") ·
mod conflict. **INFERENCE:** filters should be driven by the same typed reason ids the CLI/JSON
emits, so a filter can never diverge from the model.

### 18.3 Behavioural constraints

- **Read-only.** The page renders a projection; it does not scan, probe, or launch.
- **Navigation-only actions**, exactly like the existing launch-readiness panel whose only
  action is `OpenDoctor` (`launch_readiness_page.rs`, `LaunchReadinessPageAction::OpenDoctor`).
- **Bounded rendering** — see section 19; the page must never force an unbounded recomputation.
- **Never implies auto-fix.** No button may be labelled "Fix", "Repair", or "Install" unless it
  navigates to an existing surface that owns that action.
- **Reuses `AttentionDestination`** so the Ready-to-Play page and Needs Attention agree about
  where a problem is solved.

## 19. Performance and scale (S)

### 19.1 The 100k-entry constraint

**Requirement:** the library may exceed 100k entries; readiness must never require a full
recompute, and must never rehash large media to answer a UI question.

Existing precedent (**CONCLUSION FROM SOURCE**): `AttentionSnapshot` is explicitly bounded
(`ATTENTION_PAGE_SIZE = 50`, `ATTENTION_GROUP_LIMIT = 1024`, a `limited` flag, and `counts()`
computed over unresolved items only — `attention.rs:10-11,228-296`), and it carries a
worst-item eviction rule so "historical receipts must not crowd out a newly observed blocker"
(`:257-268`). `verified_identity_cache` exists specifically so consumers "can explain identity /
launch readiness without re-inspecting the content on every access"
(`verified_identity_cache.rs:1-10`).

### 19.2 Recommended computation model

| Concern | Recommendation |
|---|---|
| Where readiness is computed | **Per game, on demand** (selected game / open detail view), by a *pure* function over already-gathered inputs — the same contract as `build_launch_plan` and `run_doctor_scan`. Never a whole-library sweep on startup |
| Library-wide list | Built from **cached per-game summaries** (state + reason ids + counts), never by recomputing every plan. Mirrors `AttentionItem`'s "summary card can represent many objects, without loading those objects" (`attention.rs:174-175`) |
| Hashing | **No new hashing.** Identity comes from the existing verified-identity cache and its freshness snapshot; BIOS hashing only where an adapter verifier already does it; CHD uses header identity (`chd_identity.rs`), never a full read |
| Tool probing | **Never during render.** The MAME version probe and any inventory `--version` probe are discrete, bounded, explicitly triggered operations whose *results* are consumed |
| Bounded refresh | Recompute only what a change invalidates (section 20), in dependency order: emulator/environment → firmware → media → identity → plan |
| Progress and limits | Match the existing pattern: a `limited` flag plus coverage notes when a bound was hit, so a partial view never reads as complete |

### 19.3 Cost profile of a single projection (INFERENCE)

Cheap by construction: it reads already-computed structs (a `LaunchPlan`, a media projection, a
firmware state, an authority row) and produces a small value. The only non-trivial inputs are
ones the existing UI already computes when a game is selected. **The risk is therefore not CPU
cost but accidental *redundant* recomputation**, which the caching rules in section 20 exist to
prevent.

## 20. Staleness and invalidation (T)

### 20.1 What must invalidate what

| Change | Invalidates | Existing signal to key on |
|---|---|---|
| Game file replaced/renamed/removed | identity, media, plan | `verified_identity_cache`'s `(device, inode, size, mtime)` freshness; `CapturedFileIdentity` (`launch/process_spawn.rs`); the `*DriftBeforeSpawn` precedent |
| Media/disc set changed (disc added, renamed, moved) | media, plan | media-set membership/revision; recomputed `MediaSetState` |
| Emulator installed, removed, or version changed | emulator reasons, version reasons | profile discovery results; `LinuxEmulatorInstallationEvidence`; arcade version probe result; the `emulator_inventory.rs` projection |
| Emulator profile/config changed | binding/executable reasons | profile discovery + `emulator_profile_memory`; config states (`DosBoxConfig*`, `AmiberryMediaNotConfigured`) |
| BIOS/firmware file added, removed, replaced | firmware reasons | firmware state enums and their path/hash evidence |
| DAT imported, updated, or authority changed | arcade set/dependency, media evidence, BIOS evidence | `AuthorityFreshness`, `DatAuthoritySource{revision, sha256, imported_at}`, `DatRefreshImpact` |
| Arcade election / playing-library policy changed | arcade reasons | the plan/election id and its inputs |
| Mod/cheat applied, rolled back, or package changed | mod reasons only | `SharedApplyJournal`/`SharedApplyStatus`, `CheatInstalledState` |
| Storage root or source folder changed | everything (paths may no longer resolve) | config identity check + root-migration records (`source_root_migration`, `source_folder_migration`) |
| Mount state changed | media/content resolution | the library-view/mount state that already gates content access |

### 20.2 Rules

1. **Readiness is a projection with a generation, not stored truth.** Anything persisted for
   speed must carry the generation inputs it was derived from — the discipline
   `verified_identity_cache` already applies — and must be re-derived (not merely re-displayed)
   when an input changed.
2. **Stale never means "shown as true".** A stale fact may remain *visible for explanation* and
   must not authorize a launch — verbatim the verified-identity rule
   (`verified_identity_cache.rs:33-39`: "A stale fact stays visible for explanation; it must
   never authorize a launch").
3. **Unknown beats stale.** If freshness cannot be established, the honest reason wins over a
   stale positive.
4. **Invalidation is per-reason, not global.** A BIOS change must not invalidate identity; a
   media change must not invalidate emulator evidence. This is what keeps 100k-scale refreshes
   bounded (section 19).

## 21. Provenance (U)

### 21.1 Per-reason provenance

Every reason retains provenance where the underlying evidence has it — hidden from the headline
but available in the technical details and the JSON contract:

| Reason family | Provenance carried |
|---|---|
| `IDENTITY_*` | the identity kinds/values used, their `IdentityStatus`, the file-identity snapshot, and whether (and by which source) a DAT match corroborated it |
| `EMULATOR_*` | installation form (`MANAGED_APPIMAGE_INSTALLATION_FORM` or plain/Flatpak/PATH/config-only), profile id/path, executable path + resolution state, probe time and raw version string |
| `BIOS_*` / `FIRMWARE_*` | which adapter enum produced the state, the verified path, and — where a verifier ran — the `FirmwareIdentityRecord` hashes and its `dat_version` |
| `MEDIA_*` | the `MediaSet` id, `Provenance`/`EvidenceKind` for each claim, expected-vs-observed ordinals, and the topology projection's own explanation |
| `ARCADE_*` | DAT source id + `revision`/`sha256`, `SetResolution` member/disk lists, `DependencyState`, and the CHD header identity that matched |
| `MOD_*` | package manifest id/version, the required base hash, and the expected result hash |
| `CONFIG_REQUIRED` | the config path inspected and its parsed state |
| `LAUNCH_PLAN_INVALID` | the failing adapter and the preflight error kind |

### 21.2 Rules

- **Provenance is per reason, never per game**, so one verdict can cite several independent
  sources without ambiguity.
- **No provenance means no claim.** Following the codebase's rule that evidence is "only ever
  emitted for a comparison this module actually performed against data both sides really
  declared" (`patch_manager/cheat_candidates.rs:38-42`), a reason with no source must be a
  coverage note instead.
- **Provenance is available but not required in the headline** — `AttentionItem.provenance`
  already models that split (`attention.rs:173`).
- **DAT provenance must include the DAT's own version/sha256**, because a DAT revision is exactly
  what can invalidate an arcade or BIOS claim (section 20).

## 22. Comparable-tool findings (V)

**Scope note:** not a competitor catalogue — only what each project does about *telling a user
something is playable, and what is missing*.

| Tool | How it answers "is this playable?" | Missing-BIOS reporting | Emulator readiness | Notes for EmuWiz |
|---|---|---|---|---|
| **ES-DE** | At **launch time**: a popup is shown when a game fails to start, and "likewise a notification will be shown if the defined emulator core is not installed. The es_log.txt file will also provide additional details" (**DOCUMENTED FACT**: ES-DE `USERGUIDE.md:756`) | Missing BIOS surfaces as the **emulator's own error message** at launch ("Attempting to launch a game without enabling the access will simply display an error message in the emulator that the BIOS files are missing", `:312`); requirements are documented per system (`:1940,1977,2052`) | No pre-flight check; core presence is reported only when it matters | **INFERENCE:** ES-DE is the *runtime-notification* posture. EmuWiz's pre-flight per-game verdict is the differentiator, and must not degrade into "try it and see" |
| **EmuDeck** | Install-time setup plus documentation: a "BIOS and ROMs Cheat Sheet" telling users "what BIOS files you need and where to place your BIOS and ROMs" (**DOCUMENTED FACT**: `emudeck.github.io`) | Documentation-driven (cheat sheet + a single `Emulation/bios` folder). A bundled BIOS-checker utility is widely referenced but **was not verified in this session** (**UNCERTAIN**) | Emulator installation is EmuDeck's core competency | **INFERENCE:** keep the cheat-sheet model as *guidance text* while adding machine-checked evidence |
| **Batocera** | Documentation per system, with a BIOS drop directory and an "add games/BIOS files" page (**DOCUMENTED FACT**: `wiki.batocera.org/bios`; `wiki.batocera.org/systems` links to `add_games_bios`) | Documented per system; **no per-game machine-checked pre-flight readiness was found** in the pages inspected | Emulator selection is configuration-driven | **INFERENCE:** "document it and let the emulator fail" is the posture EmuWiz avoids |
| **RomM** | Server-side scan/enrich/browse/play: "Scan, enrich, browse and play your ROM collection… 400+ platforms", with saves/states sync, ROM patcher, permissions (**DOCUMENTED FACT**: RomM README) | Not a per-game launch-readiness concept; BIOS is platform documentation | Server-side library, not per-game pre-flight | **INFERENCE:** RomM is EmuWiz's *destination*; EmuWiz can supply the pre-flight answer RomM lacks |
| **Provenance** | Frontend listing systems/games; "Some systems require BIOS files. See BIOS Requirements" with a dedicated wiki page, and "(Optional) Add BIOS files" as an install step (**DOCUMENTED FACT**: Provenance README `:220,235-237`) | Documentation page plus a user step; BIOS synced across devices via iCloud | No per-game pre-flight verdict | Same documentation posture as EmuDeck/Batocera |
| **EmuHaven** | Installs/updates/launches emulators; **no readiness model at all**, and downloads are verified structurally rather than by cryptographic signature or pinned checksum — **DOCUMENTED FACT** via the in-tree audit `docs/research/EMUHAVEN_EMULATOR_MANAGER_AUDIT.md` | Not modelled | Emulator lifecycle management only | **Must not copy** its download/launch posture; its useful ideas belong to an Emulator Manager task |
| **Arcade Manager** | Curated arcade filtering/lists plus per-system config; **not** an authoritative ROM-set validator — **DOCUMENTED FACT** via the in-tree audit `docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md` | Arcade BIOS/devices/CHDs handled as dependency closure | Arcade curation + filters, version-bound to an emulator family | **Adopt the structural ideas only** (parent/clone separation, dependency closure, ROMset↔emulator version binding) — EmuWiz already has each as evidence |

**Cross-cutting finding (INFERENCE):** every comparable tool either (a) documents requirements
and lets the emulator fail at launch, or (b) checks a *library* rather than a *game*. None of the
tools inspected publishes a per-game pre-flight verdict separating *proven blocker* from
*missing-and-fixable* from *not yet checked*. That separation is the novel part of this design —
and it costs little, because the evidence already exists.

## 23. Gap analysis (W)

| Feature / evidence area | Current EmuWiz | Genuine gap? | Value | Risk | Recommendation |
|---|---|---|---|---|---|
| Per-game top-level readiness value | `LaunchPlan` with per-candidate 3-state verdict; no single value | **Yes** | High | Low (pure projection) | **DO NEXT** |
| Shared reason vocabulary across per-game + library axes | Two vocabularies: 198 `LaunchBlockerKind`s and 4 `AttentionSeverity`s | **Yes** | High | Low–medium (naming is a contract) | **DO NEXT** |
| Not-gathered (`Unknown`) per game | Exists library-wide (`Gathered`, `CoverageStatus`, `coverage_notes`); absent per game | **Yes** | High | Low | **DO NEXT** |
| Fixability classification | Doctor distinguishes guide vs repair in prose/fields; readiness has none | **Yes** | Medium–high | Low | **DO NEXT** |
| Identity sufficiency definition | Already precise (`CanonicalIdentityStatus` + 16-kind allowlist + `FilenameOnly` refusal) | **No** | — | — | **ALREADY COVERED** — project it, do not redefine |
| BIOS/firmware state | `FirmwareReadiness` + 8 adapter projections + `FirmwareIdentityRecord` | **No** | — | — | **ALREADY COVERED** |
| Emulator install form / executable usability | `LinuxEmulatorInstallationEvidence`, `safe_executable`, binding resolvers | **No** | — | — | **ALREADY COVERED** |
| Emulator inventory across emulators | Landed as a read-only projection (`emulator_inventory.rs`, `2625d0f`) | **No** | Medium | Low | **ALREADY COVERED** — consume it; do not duplicate. `storage_health.rs` (`f951d54`) is orthogonal (storage formats, not launchability) |
| Emulator version compatibility | Arcade only (`ArcadeDatVersionCompatibility`); MAME-only probe | **Yes, narrowly** | Medium | Medium (needs a version model per emulator) | **RESEARCH MORE** |
| Media topology and completeness | `media_set` + `launch::topology` projection | **No** | — | — | **ALREADY COVERED** |
| Arcade set/dependency/version evidence | `SetState`, `SetResolution`, `DependencyState`, election, version compatibility | **No** | — | — | **ALREADY COVERED**; project it (section 11) |
| Controller/input requirements | **Nothing** | **Yes, and unbuildable today** | Low until a source exists | **High** (fabrication risk) | **DO NOT BUILD** (coverage note only) |
| Mods/patches readiness | `mod_package` base/output verification; cheat candidate tiers; journals | Partially (no readiness projection) | Medium | Medium | **HIGH VALUE LATER** |
| Save-state/resume readiness | **Nothing** (no Save Vault) | Yes, but out of scope | Medium (future) | Medium | **HIGH VALUE LATER** (separate sub-status) |
| Library-wide attention + navigation | `attention` + Needs Attention GUI | **No** | — | — | **ALREADY COVERED** |
| Doctor findings and repairs | `diagnostics` + 4 repairs + deferred checks | **No** | — | — | **ALREADY COVERED** |
| Explanation copy (summary/reasons/evidence/action) | `AttentionItem` fields; `Finding.why_it_matters`/`next_step` | Partially (no per-game unified copy) | High | Low | **DO NEXT** (with R1) |
| Read-only Ready-to-Play view | Launch Readiness panel + Needs Attention page | **Yes** (a unified per-game list) | Medium–high | Low–medium (UI cost) | **HIGH VALUE LATER** (R2) |
| Fix guidance / deep links | `AttentionDestination` navigation | Partially | Medium | Low | **HIGH VALUE LATER** (R3) |
| Incremental invalidation/caching | `verified_identity_cache` freshness; `AuthorityFreshness`; `Gathered` | Partially (no readiness-level generation) | High at 100k scale | Medium | **HIGH VALUE LATER** (R4) |
| Provenance per reason | Rich per-subsystem provenance exists; not unified | Partially | Medium | Low | **DO NEXT** (cheap: reuse existing fields) |
| Overall scoring / ranking | None, by design | **No gap** | — | — | **DO NOT BUILD** |

### 23.1 Ranking summary

- **DO NEXT (R0+R1):** the projection, the reason vocabulary, the `Unknown` axis, fixability, and
  provenance — all pure, all read-only, all reusing existing evidence.
- **HIGH VALUE LATER:** mods projection (R5), resume sub-status (separate task), the Ready-to-Play
  view (R2), fix guidance (R3), incremental invalidation (R4), non-arcade version compatibility
  (research first).
- **ALREADY COVERED (do not rebuild):** identity sufficiency, firmware/BIOS state, emulator
  install/executability, media topology, arcade set/dependency/version evidence, attention,
  Doctor findings/repairs, emulator inventory.
- **RESEARCH MORE:** per-emulator version-compatibility models beyond arcade; whether a
  BIOS-variant ambiguity state is derivable from existing per-adapter discovery; controller
  evidence sources (only if a citable catalogue is found).
- **DO NOT BUILD:** scores/percentages; a controller requirement model without an evidence
  source; a second media grouping engine; a second emulator scanner; any acquisition or install
  flow; readiness-driven auto-repair.

## 24. Implementation roadmap (X)

Every phase reuses existing evidence systems, and each has a **hard boundary** that must not be
crossed regardless of how useful the next step looks.

### PHASE R0 — aggregation model (design artefacts only)

**Deliverable:** the typed model from sections 4–6 and 16, with its pure projection function and
a fixture-based test suite. No I/O, no UI, no CLI surface beyond a debug dumper.

- `ReadyToPlayState` (six states), `ReadinessReason` + family enum, `Fixability`,
  `ReadinessCoverage`.
- The pure aggregator: `(LaunchPlan, Option<MediaTopologyLaunchProjection>, firmware states,
  arcade evidence, coverage inputs) → ReadyToPlay`.
- Deterministic ordering tests (mirroring `tests::status_priority_is_deterministic` in
  `platform_evidence_fusion/identity_presentation.rs`).
- **Boundary:** pure functions only. No new I/O, no new probe, no new database table, no GUI, no
  change to any existing enum in `launch/`, `media_set/`, `dat/`, or `attention/`.

### PHASE R1 — read-only core projection + CLI/JSON

**Deliverable:** the projection is callable from the existing read-only paths (selected-game
flows and JSON output), producing stable reason ids.

- Wire the aggregator where a `LaunchPlan` is already built, and where Doctor already gathers
  inputs (`DoctorScanInputs`).
- Emit `ReadyToPlay` in the existing JSON contract for the already-planned game (no new command
  required initially; a `readiness` sub-command is optional).
- Provide a `readiness_attention()` producer so a not-ready game yields exactly one attention
  item **consistent with today's `launch_attention()`** (`needs_attention.rs:280-331`) rather
  than a second, differently-worded item.
- **Boundary:** consumes only already-gathered evidence. No new probes, no new hashing, no
  background scanning, no writes, no change to launch preflight.

### PHASE R2 — Ready-to-Play view (read-only GUI)

**Deliverable:** the page in section 18, sections driven by the six states, filters driven by
typed reason ids, navigation-only actions.

- Reuse `AttentionDestination`; no new destinations.
- The **Unknown / not checked** section must exist from day one, or the model's central
  distinction is invisible to users.
- **Boundary:** render-only. No scanning on page open beyond what the existing selected-game
  flow already does; no launch action on the page that is not already an existing typed action;
  no auto-fix buttons.

### PHASE R3 — fix guidance and deep links

**Deliverable:** each reason's `next_action` navigates to the existing surface that owns the fix
(Emulator Setup, Doctor, DAT authority, multi-disc review, Cheats & Mods, identity review).

- Copy follows the section 17 rules; nothing implies automatic repair.
- `Fixability::EmuWizCanRepairSafely` may only be set for the four existing Doctor repairs.
- **Boundary:** navigation only. No execution of repairs from the readiness view, no acquiring of
  BIOS/ROMs, no config writes.

### PHASE R4 — incremental invalidation and caching

**Deliverable:** a readiness generation keyed on the inputs in section 20, with per-reason
invalidation and bounded recomputation.

- Persist nothing that cannot be re-derived; carry the input generation with anything cached.
- Reuse `verified_identity_cache` freshness and `AuthorityFreshness` rather than inventing a
  second freshness vocabulary.
- **Boundary:** no eager whole-library recomputation; no hashing sweeps; no stale fact may ever
  authorise a launch (existing rule).

### PHASE R5 — controller / mod / version enrichment (evidence-dependent)

**Deliverable:** only what new *evidence* justifies:

- Mods/patch readiness projection over `mod_package` + cheat-candidate tiers (no new mod engine).
- Per-emulator version-compatibility research → a version reason for non-arcade emulators, only
  if a citable compatibility source exists.
- Controller requirements **only** if a real, provenance-carrying catalogue is identified; until
  then the coverage note stays.
- **Boundary:** every addition must arrive with its evidence source and its "does not prove"
  statement. No speculative requirements.

### 24.1 Ordering rationale

The brief's suggested order (R0 → R1 → R2 → R3 → R4 → R5) is kept **unchanged**, for three
concrete reasons:

1. **R0/R1 first** because they are pure and cheap, and because they *prevent* the duplicate
   vocabulary that R2 would otherwise hard-code into the UI.
2. **R2 before R3** because guidance without a stable reason vocabulary produces hand-written
   strings that drift from the model.
3. **R4 after R2** because caching should be designed once real usage shows which inputs change
   most often — and because a premature cache bakes in an invalidation model that is hard to
   correct.

**Note on `ResumeReadiness`:** it is deliberately *not* a phase here. It belongs to Save Vault
(section 14) and depends on Save Vault's own evidence existing.

## 25. The ten specific questions, answered (Y)

**1. Does EmuWiz already have enough primitives for Ready-to-Play?**
**Yes — comprehensively.** Identity sufficiency (`CanonicalIdentityStatus` + a 16-kind allowlist
+ a `FilenameOnly` refusal), firmware readiness with adapter projections, emulator profile
discovery and per-adapter readiness assessments, media topology with a launch projection, arcade
set/dependency/version evidence, mod base/output verification, Doctor's finding + repair +
deferred-check model, the bounded attention model, and the freshness primitives for caching all
already exist (sections 3.1–3.6). **The missing pieces are a projection and a shared reason
vocabulary — not evidence.**

**2. What is the smallest new core model required?**
Three types, all pure data, plus one pure function: `ReadyToPlayState` (six variants),
`ReadinessReason` (closed family enum + severity + fixability + retained evidence + provenance),
and `ReadinessCoverage` (which lanes were gathered), aggregated by a single pure function
(§4.1–4.4, §5.1, §6.2). Everything they carry already exists; nothing new is probed, scanned,
hashed, or stored.

**3. What evidence should BLOCK launch?**
Only proven contradictions or unbuildable plans: identity unresolved or conflicting; content not
resolved; no verified executable/binding for **every** candidate; media proven **missing** for a
required medium of **every** candidate; an arcade set/dependency proven missing or structurally
broken for every candidate; a content format unsupported by every available adapter; a command
plan the planner refuses to build. Nothing else. In particular, unknown is **not** a blocker
(§6.1).

**4. What evidence should WARN only?**
Unverified-but-present BIOS; BIOS state unknown; optional firmware missing; emulator version
unknown or compatibility unmodelled; multiple eligible profiles / ambiguous preference; optional
media missing; media topology unverified; an arcade title marked imperfect (or a working clone
substituted for a non-working parent); mods not installed; controller requirements not modelled;
and any lane that was gathered but inconclusive without proving harm. These produce
`ReadyWithWarnings` (§6.1).

**5. Should Ready-to-Play require perfect identity?**
**No.** It must require *launch-sufficient* identity, which EmuWiz already defines through
`CanonicalIdentityStatus::Resolved` plus `evidence_bridge::is_identity_conferring`
(`launch/evidence_bridge.rs:71-97`). Perfect archival identity (DAT match, canonical hash, 1G1R
election) is a *preservation* axis and must not gate launch. Conversely, filename-only evidence
must never be treated as confirmed identity (`verified_identity_cache.rs:13-22`) (§7).

**6. How should arcade readiness project into the generic model?**
Directly, with no arcade-specific top-level state: `SetState`/`SetResolution` → set/CHD reasons;
`DependencyState` → dependency reasons; `ArcadeWorkingStatus` + the existing election → working
and working-clone reasons; `ArcadeDatVersionCompatibility` → version reasons; and the BIOS
storage-vs-runtime-selection split honoured via `BIOS_RUNTIME_SELECTION_NOT_MODELLED`. The in-tree
arcade audit's proposed vocabulary maps one-to-one onto the generic states (§11).

**7. Should save-state compatibility be separate from game launch readiness?**
**Yes — strictly separate.** `ReadyToPlay` answers "can this launch?"; a future `ResumeReadiness`
(owned by Save Vault) answers "can this state be resumed?", with its own vocabulary. A save-state
concern must never reduce game readiness, and Ready-to-Play must never depend on Save Vault
(§14).

**8. What should be the first implementation slice?**
**R0 + R1 (§24):** the pure model and its read-only projection, wired where a `LaunchPlan` is
already built and where Doctor already gathers inputs, emitted in the existing JSON contract,
plus a `readiness_attention()` producer that stays consistent with today's `launch_attention()`.
No GUI, no new probe, no new table, no changes to existing enums.

**9. What should Ready-to-Play explicitly NOT try to solve?**
Emulator installation/updating/channels; BIOS/firmware acquisition or publishing; save-state
compatibility truth; controller/input requirement truth; mod curation or application; DAT
authoring or repair; launch execution authority; scoring/ranking; media grouping; identity
resolution (§2.2, §26).

**10. Which existing subsystems should remain authoritative rather than duplicated?**
`launch::planning` + `launch::readiness` (plan, candidates, blockers, firmware vocabulary);
`launch::execution`/`process_spawn` (the only execution authority); `media_set` +
`launch::topology` (grouping and topology); `dat::set` + `dat::dependency` (arcade storage and
dependency closure); `diagnostics` (findings, repairs, deferred checks); `attention`
(library-wide aggregation, severity, destinations); `platform_evidence_fusion` + `game_identity`
+ `verified_identity_cache` (identity and identity freshness); `patch_manager`/`mod_package`
(mods/cheats and their journals); `platform::PLATFORMS` + `launch::platform_map` (platform
vocabulary); Doctor's profile discovery plus the emulator inventory projection (emulator
availability); `dat::authority` (DAT freshness and BIOS counts).

## 26. DO-NOT-BUILD list

Ready-to-Play must **not**:

1. **A second readiness system.** No parallel per-game status, no parallel reason vocabulary, no
   parallel library-wide store. The projection consumes existing evidence and speaks
   `AttentionSeverity`/`AttentionDestination` (§4.3, §6.2).
2. **Any new probe, scan, or hash during rendering.** Reuse profile discovery, the bounded MAME
   version probe, existing BIOS verifiers, CHD header identity, and the verified-identity cache.
   Never hash large media to answer a UI question (§8.2, §19).
3. **Any install, update, download, or acquisition flow** — emulators, BIOS/firmware, ROMs, mods,
   or DATs. EmuWiz does not distribute copyrighted content, and readiness is a report, not a fetch
   (§2.2, §8.2, §9.3).
4. **Any auto-fix, auto-repair, or "fix all" action.** Guidance navigates; only the four existing
   Doctor repairs may be described as repairs (§16).
5. **Any scoring, percentage, star rating, or confidence number.** Unknown cannot be scored without
   becoming a lie (§6.2, §17.3).
6. **Any controller/input requirement model without a real evidence source.** Today it is a
   coverage note, deliberately (§12).
7. **Any BIOS runtime-selection claim.** `BIOS_RUNTIME_SELECTION_NOT_MODELLED` stays true (§9.2).
8. **Any change to arcade election, Publisher Profiles, Save Vault, or launch planning** (per the
   brief). This design projects their output; it does not alter it.
9. **Any second media grouping or ordinal inference.** `media_set`/`launch::topology` own it
   (§10.2).
10. **Treating `Unknown` as `Missing`, or unverified as absent** — in code, copy, counters, or
    filters (§6.1, §17.3).
11. **Launching from the readiness view** except through the existing typed launch actions, and
    never from a readiness verdict alone (§15.2).
12. **Persisting readiness as truth.** It is a projection with a generation; anything cached must
    be re-derived when inputs change, and stale facts never authorise a launch (§20).

## 27. Sources inspected

### 27.1 This repository (CONCLUSION FROM SOURCE)

Cited at `faa6f48` (see the status block):

- **Launch planning/readiness:** `crates/archivefs-core/src/launch/mod.rs:1-45`;
  `launch/planning.rs:1-20,45,73,95,115,127,134,157,169,181,194,222,509`;
  `launch/readiness.rs:15-20,32,38-44,53,67,555,572,611-616,617,631,721,743`;
  `launch/topology.rs:1-4,28,46-63`; `launch/evidence_bridge.rs:1-30,71-97`;
  `launch/mame_command.rs:18,51`; `launch/fbneo_command.rs`; `launch/input_projection.rs`;
  `launch/platform_map.rs`; `launch/execution.rs`; `launch/process_spawn.rs`.
- **Identity:** `game_identity.rs:175,204,614`;
  `platform_evidence_fusion/identity_presentation.rs:20-60`;
  `platform_evidence_fusion/archive_set_identity.rs`; `verified_identity_cache.rs:1-40`.
- **Media:** `media_set/mod.rs:1`; `media_set/model.rs:39,55,75,80,163,168,284,303,309,333`;
  `media_set/{engine,inspect,naming,plan}.rs`.
- **Arcade/DAT:** `dat/set.rs:1-60,269,290`;
  `dat/dependency/mod.rs:1-52,90-96,107,236,241-247,257-265`; `dat/authority.rs:8,18,48,71-107`;
  `dat/disk_audit.rs`; `dat/archive/chd.rs`; `playing_library/model.rs:42-53,70-105`;
  `playing_library/matching.rs`.
- **Diagnostics/Doctor:** `diagnostics/mod.rs:1-52,103-140,155,231,336-368,369-420,422,582,623,630-661`;
  `diagnostics/runner.rs:67,103,143-149,248-398`;
  `diagnostics/profiles.rs:1-30,70,76-98,422,476,1568,1789,1970,2128`;
  `diagnostics/arcade_dat_version.rs:226`; `diagnostics/arcade_version_probe.rs:1-40,51,56`.
- **Attention/operations:** `attention.rs:1-11,36,60,111,149,156,174-175,228-296,373,462`;
  `operation.rs:32-105`.
- **Emulator environment:** `emulator_environment/retroarch.rs:135,147,372,390,563,593,604,684,704`;
  `emulator_inventory.rs` (landed as `2625d0f`, read-only); `emulator_download.rs`;
  `managed_appimage_bootstrap`.
- **Mods/cheats:** `mod_package.rs:1-40`; `patch_manager/cheat_candidates.rs:1-42,101-140,176`;
  `patch_manager/cheat_catalogue.rs:1127,1433`; `patch_manager/*` journals.
- **GUI:** `crates/archivefs-gui/src/launch_readiness_page.rs:1-40,88-135`;
  `crates/archivefs-gui/src/needs_attention.rs:1-10,29-47,280-331,496`;
  `crates/archivefs-gui/src/doctor_page.rs:1-30`.
- **Docs:** `docs/LAUNCH_SUPPORT.md`; `docs/ADAPTER_SUPPORT_MATRIX.md`;
  `docs/research/CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md`;
  `docs/research/UNIFIED_EVIDENCE_RESOLUTION.md`;
  `docs/research/MEDIA_TOPOLOGY_LAUNCH_INTEGRATION.md`;
  `docs/research/PRE_RC_LAUNCH_PROJECTION_AUDIT.md`.

### 27.2 In-tree audits by concurrent work (cited as in-tree evidence)

- `docs/research/EMUHAVEN_EMULATOR_MANAGER_AUDIT.md` (committed) — EmuHaven install/update/launch
  lifecycle; no cryptographic verification of downloads; Save Vault and Publisher Profiles
  re-confirmed absent from `crates/`.
- `docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md` (untracked at read time) — Arcade Manager as
  curation/filters rather than a validator; the recommended arcade readiness vocabulary; the
  `BIOS_RUNTIME_SELECTION_NOT_MODELLED` boundary.

### 27.3 External sources (DOCUMENTED FACT where cited)

- **ES-DE** `USERGUIDE.md` (`:312`, `:756`, `:1940`, `:1977`, `:2052`) — launch-time notification
  for missing cores/BIOS; per-system BIOS documentation.
- **EmuDeck** — `emudeck.github.io`: BIOS and ROMs cheat sheet; `Emulation/bios` folder.
- **Batocera** — `wiki.batocera.org/bios`; `wiki.batocera.org/systems` (→ `add_games_bios`):
  per-system BIOS documentation.
- **RomM** — project README: server-side scan/enrich/browse/play, 400+ platforms, saves sync,
  ROM patcher, permissions.
- **Provenance** — project README (`:220`, `:235-237`): BIOS Requirements wiki page; optional BIOS
  step; iCloud BIOS sync.
- **`dat/firmware_evidence.rs:11-18`** cites Redump's PS2 BIOS DAT licensing as the reason EmuWiz
  never bundles BIOS hashes (an internal citation of an external constraint).

**Known limitations of this research:** no implementation was attempted, no emulator was launched,
and no new probe was run. Claims about *what a user sees today* come from reading code and in-tree
docs rather than from exercising a build; claims about comparable tools come from their published
documentation and the two in-tree audits, not from running them. Items flagged **UNCERTAIN** in the
text are not to be relied on without verification.

---

## 28. Validation record for this research task

- `git status --short` and `git diff --check` were run before committing.
- The only file added by this task is `docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md`; no
  production Rust file, GUI page, launch planner, Publisher Profile, Save Vault, or arcade
  election file was modified, and no Ready-to-Play UI was added.
- Two commits from concurrent work landed during this research (`2625d0f` emulator inventory,
  `f951d54` storage health). Their files were **read but not modified**, and every anchor cited
  in this document was re-verified afterwards at `f951d54`.
- No auto-fix was performed, no emulator was launched, no file was written outside this document,
  and nothing was pushed.
- Implementation targets identified here (R0–R5 in section 24) are explicitly **not** implemented
  and must be raised as separate tasks.
