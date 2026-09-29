# EmuDeck 2.7 ideas: EmuWiz audit and setup portability preview

Audit baseline: `54d0503b8fa2eaa48f3cf5a0ca71fe935a126384`, 29 September 2026.
Authoritative checkout: `/home/davedap/emuwiz-main-release-fix`. `git fetch origin`
completed; local `main` equalled `origin/main` and had no tracked changes.
Existing untracked research files were left alone. Recent main commits concern
Browse & Play usability and artwork connectivity/layouts.

Implementation branch: `feature/setup-portability-preview`, isolated worktree
`/home/davedap/emuwiz-setup-portability-preview`, created **after** the audit
selected setup portability. No main modification, promotion or push.

## External inspiration, bounded claims

The official [EmuDeck 2.7 EA release](https://github.com/dragoonDorise/EmuDeck/releases/tag/2.7EA)
confirms the release but supplies only a short announcement, not a detailed
feature contract. The six ideas in the task are treated as audit prompts;
this lane does not claim all were introduced in that release.
The official [screen resolution guide](https://manual.emudeck.com/using-app/5_screen-resolution/)
describes changing emulator internal resolution, while the official
[Cemu controls/motion guide](https://github.com/EmuDeck/emudeck.github.io/blob/main/docs/emulators/steamos/cemu/cemu-native.md)
describes SDL and optional DSU/Steam Input steps. These illustrate that render
resolution, output dimensions, controller backends and motion dependencies
must remain distinct. None of EmuDeck's settings, scripts or configurations
are copied into EmuWiz.

## Overlap audit

All registered worktrees/branches were inspected. Relevant separate lanes:
`research/organisation-export-parity`, `research/release-portability-matrix`,
`research/save-state-portability`, `research/upgrade-config-migration`,
`feature/save-migration-readiness-wiring`, `feature/mame-arcade-setup-journey`, save inventory/compatibility/CLI,
and source-root migration. Their topics concern published libraries, release
binaries, selected saves and existing path references; none provides this
setup export/import-preview workflow. Read-only inspection found the four
research/save-migration checkouts tracked-clean.

Current cheat parser/conflict/provenance/import-preview lanes and artwork/GUI
lanes were also identified. This implementation does not change their modules,
configurations or behavior. GUI v2 gains only a new Settings feature module,
one render hook, one state field and its initialization, plus the matching
existing headless test-fixture initialization. No library browsing,
artwork, cheat or emulator adapter logic is edited.

## Controller AutoMap audit

Repository searches covered controller/gamepad/input, SDL, XInput, evdev,
Steam Input, numbered player slots, mappings/remaps/profiles, gyro/DSU,
DualSense, Xbox and 8BitDo. `launch/input_projection.rs` projects verified
**game identity** into adapter requests; it is not hardware input projection.

Common result across the adapters below: no connected-controller probe,
controller identity/backend capability model, hotplug layer, generated or
imported controller-map workflow, player-slot assignment, or deterministic
multi-controller binding was found. Controller presence flags describe
configuration observations; they do not prove working hardware.

| Adapter | Existing input evidence | Player/type/backend/motion support in EmuWiz | Controller writes, preview, rollback |
|---|---|---|---|
| RetroArch | Environment/config/core discovery; explicit resource planning | No connected device/slot/capability model; remapping content is outside the current environment inventory | No controller-map writer or controller preview. Per-launch resource overrides and other safe transactions are separate capabilities |
| Dolphin | `DolphinSettings.controller_profile_present`; global/per-game INI inspection | Presence of a profile field only; no device/backend or motion proof | No controller configuration workflow; existing scoped mod transactions do not authorise controller writes |
| RPCS3 | `Rpcs3Settings.controller_profile_present` in bounded per-game YAML inspection | A config declaration only; no device/slot/backend capability evidence | No controller mapping apply/rollback surface |
| PCSX2 | `Pcsx2ControllerInfo` retains observed controller section names | No attached device identity; section names are not deterministic player bindings | Read-only config inspection; no controller map generator/importer |
| PPSSPP | `PpssppSettings.controller_config_present` | Config declaration only, no connected devices or map interpretation | No controller mapping writer/preview |
| xemu | `XemuConfig.controller_config_present` from an input table | An input table is not a controller detection result | No controller map write/rollback workflow |
| Xenia | Local patch/profile and executable evidence | No typed controller inspection or hardware capability layer | No controller mapping workflow |
| Azahar / Citra family | Azahar profile/config existence and launch evidence | No typed input backend/device/motion model; no separate Citra mapping adapter found | No controller mapping workflow |
| MAME | Preserved listxml `<input>/<control>`; `ArcadeInputRequirements` families and supported-player metadata | Strong static **machine-control** evidence, explicitly not attached hardware, launch blockers or Ready-to-Play controller verdicts | No connected-controller AutoMap; native controls must be configured separately |
| Switch / Ryujinx | Switch naming/front-end export coverage | No Ryujinx/Switch controller adapter found | No mapping workflow |
| Other profile adapters | DuckStation/Flycast profile-presence fields; Hatari joystick modes; Amiga/WHDLoad controller declaration | Read-only adapter-specific config evidence, no shared connected-controller model | No general mapping generation or controller configuration import |

Emulator profiles distinguish roots/installations; they are not device mapping
profiles. Existing global/per-game config inspection is read-only. Safe writes
elsewhere (cheats, mods, bezels, save restore) are scoped to those adapters and
transactions. They cannot be borrowed as evidence that global input rewrites
are safe. A real AutoMap vertical slice needs hardware discovery, stable
identifiers and per-emulator map/version contracts first.

## Setup import/export audit

| Setup area | Baseline capability | Portability gap / first-slice disposition |
|---|---|---|
| Emulator selection/paths/versions | Manual executable/config-root override files; profile discovery; typed `InventoryEmulator`/version evidence; remembered Dolphin/Xenia profiles | No setup bundle. Export the existing seven supported manual path channels and RetroArch core-folder override. Versions, automatic detections and per-game remembered profiles require rediscovery/review |
| ROM/library roots | `Config`, structured sources including disabled entries, mount root and master ROM root | Export root **references** and enablement, never files |
| DAT/source preferences | Typed `DatSourcesConfig`, source kinds/ownership, global/platform `DatPolicyConfig`; managed-source lifecycle | Export selected registration fields and known policy fields; omit catalogues, health snapshots, arbitrary future fields and free-text origin |
| Provider preferences | RomM non-secret `ProviderSettings` separate from token files, mappings and media mapping | Export enablement, origin, mappings, paging/timeout and declared mapping style. Re-enter full address/base path and credentials; other providers omitted explicitly |
| Artwork | Local/provider caches, source indexes and settings in their own workflows | No common portable setup contract; omitted with guidance to review Sources & Providers |
| Controllers | Adapter-specific config-presence observations only | No central settings to transfer; explicitly omitted |
| Launch preferences | Proven launch profiles/resource contracts; per-game recommendation planner and runtime choices | No reviewed portable apply schema; omitted, require destination review |
| Conversion preferences | Existing focused planners and runtime jobs | Not a portable persisted profile; explicitly omitted |
| Cheats/mod preferences | Separate registries, remembered profiles, per-game choices/review state and scoped installation histories | No safe whole-setup transfer boundary; explicitly omitted and untouched |
| Selected saves/config | Save snapshot manifests, evidence-bound restore preview/receipts/rollback; save migration planner | Existing selected-game save workflow retained; no save/config attachment copying in this slice |
| UI state/history | GUI preference JSON with route/filter/doc associations and database-local IDs; recovery journals | Not portable simply by copying; omitted and never re-keyed or rewritten |

Other exports already exist: ES-DE/publisher playing-library output and
`platform_evidence_fusion/library_plan_export`. They export games/evidence or
frontend projections, not application setup. Existing source-root migration
uses exact component containment and preserved history; it cannot import
foreign config documents or automatically reinterpret unrelated host paths.

Reused infrastructure: `app_dirs` EmuWiz-first/legacy fallback directory
resolution; existing library config/source parsers; DAT/provider models;
`InventoryEmulator`; migration `MigrationProposal`/`MigrationClassification`
for evidence in preview; feature-owned asynchronous workers. This lane adds
one setup-manifest boundary rather than another configuration store, archive
extractor, general migration engine or transaction journal. A future apply
lane must reuse relevant transactions, approvals and rollback. This preview
has no apply API and needs no mutation journal.

## Resolution/display profile audit

| Adapter | Existing display evidence | Missing user-facing configuration seam |
|---|---|---|
| PPSSPP | Internal resolution, backend and other global/per-game settings inspection | No Native/1080p/1440p/4K policy projector or display apply/rollback |
| xemu | Renderer and fullscreen observations | No proven shared internal-scale/output projection |
| Vita3K | Profile/config presence and launch argument contracts | No typed display capability/profile mapping |
| Azahar | Local installation/config presence and launch evidence | No typed display profile/writer |
| Dolphin | Renderer, internal resolution, aspect and related INI observations | No version-bound resolution projection or display write workflow |
| PCSX2 | Renderer/internal resolution/filtering/vsync in bounded global/per-game inspection | Per-game recommendation classifier conservatively treats resolution/renderer as host-specific; no safe display apply surface |
| RPCS3 | Renderer/resolution scale/frame limit/vsync per-game YAML evidence | No shared user profile → adapter setting projection/apply |
| RetroArch | Config environment/resource planning, bezel/overlay infrastructure | No shared display-resolution capability; overlay output dimensions are not render scaling |
| PrimeHack | No separate PrimeHack display adapter found | Must not assume Dolphin settings are interchangeable without a contract |

`game_profile_planner` already classifies host-specific display choices and
rejects insufficient identity, stale version evidence and conflicts. This is
a useful policy seam, but it deliberately exposes no writer. Launch command
builders explicitly avoid adding unproven graphics/controller flags. Native
render scaling, window size, fullscreen and output resolution have different
semantics and must not become one generic emulator setting.

## Gyro/controller capability audit

No shared runtime model of gyro, accelerometer, rumble, touchpad, analogue
triggers, motion/DSU server availability or emulator backend restrictions was
found. No hardware scanner or testable Steam Input/duplicated-input detection
exists. Emulator settings/profile-presence flags cannot establish any of
these capabilities. No automatic DSU installation or motion-stack seam is
nearly complete. Handheld versus external device identity is also unmodeled.

Consequently Steam Input masking native motion, duplicate virtual/native
controllers and emulator-specific backend needs are **unknown**, not inferred
as working or conflicting. Diagnostics-first would be the right later lane,
with explicit observed/unknown capability evidence rather than model-name
assumptions. This implementation adds neither diagnostics nor a gyro stack.

Low-priority seams: `retroachievements` already parses/caches read-only metadata
and legacy Gamer View displays cached results; no almost-finished account/
authentication achievement workflow justifies taking precedence. Existing
manual/document viewing and beginner guidance exist; embedded video support
is not required to complete this target.

## Qualitative value comparison

| Area | User pain | Already implemented / readiness | Implementation and safety risk | GUI value / reuse |
|---|---|---|---|---|
| Controller AutoMap | High for multiplayer and new setups | Config observations exist; hardware discovery and stable binding contracts absent | High: OS/backends, device ordering, global configuration ownership and rollback | High eventual value, but little proven cross-emulator map reuse today |
| Setup portability | High when replacing a computer or setting up a second device | Typed settings, directory resolution and preview patterns ready | Low for bounded export + read-only preview; apply remains separate | Immediate Settings benefit across emulator/library/DAT/provider areas |
| Display profiles | Moderate/high for handheld/docked transitions | Good observations in several adapters; host-specific classifier ready | Medium/high: version-specific scale semantics, write scope and preservation still needed | Clear benefit, narrower safe adapter coverage initially |
| Motion capability diagnostics | High for games requiring motion, narrower overall audience | No device/backend capability model or probes | Medium even read-only; false capability claims would be costly | Useful diagnostic benefit, hardware/backend work required before reuse |

**CHOSEN TARGET: setup export + import-preview foundation.** It is the only
candidate with enough existing safe settings plumbing for a complete local
GUI slice in one lane without new OS integrations or persistent emulator
config writes. This is a limited setup summary, not complete migration.
Next best target: version-aware display profile planning for an adapter such
as PPSSPP, only after proving native scaling semantics and safe write/rollback
boundaries. Controller diagnostics/discovery must precede AutoMap.

## Implementation contract and GUI

Settings → **Move your setup to another device**:

1. **Prepare setup export** collects only allowlisted known configuration.
2. Export preview explains selected fields, omissions and review needs.
   Absolute paths and the server origin appear under Advanced details.
3. **Save setup file…** writes a new JSON file, private permissions on Unix,
   and refuses overwrite. Existing source settings remain byte-for-byte intact.
4. **Preview setup file…** bounds reads to 1 MiB, rejects unsupported schema
   versions/fields and resets all prior file-specific location choices.
5. Reusable settings, location reviews, missing selected executables,
   credential/server-address attention and optional-state exclusions are shown.
6. Choosing or confirming a location triggers a worker metadata check. Source
   paths remain unchanged; remaps are exact field replacements, not prefix or
   fuzzy matches. No original/imported path is probed until explicit selection.
7. There is no Apply control/API. The GUI explains that import application is
   unavailable and directs the user to existing Setup/Sources controls.

Known-field projection excludes arbitrary unknown DAT/provider fields, DAT
health/validation history, URL userinfo/path/query/fragment and token-file
references. Token files are never read. Malformed source settings produce
explicit omission notices without echoing parser input. File reads are bounded,
regular-file-only, and refuse leaf symlinks; chosen local path checks refuse
intermediate links and parent traversal. Metadata existence is not verified
content, a verified installation or emulator compatibility. Missing selected
executables are reported as missing **at that location**, not proof that the
emulator is absent everywhere. Automatic discovery/version probing is left to
existing Setup; imported executable paths are never run.

The manifest reuses DAT policy ordering including meaningful region/language
preference order, registration ownership and emulator identity vocabulary.
Ownership remains source evidence and grants no update authority on another
device. Format version 1 is a new interchange schema; existing persistence and
SQLite are unchanged. No migration is required.

Deferred: import apply, prefix remaps, save/config attachments, arbitrary
provider/UI/cheat preference transfer, ROM copies, config writers, network
connections, downloads, controller AutoMap, motion stacks, display profiles,
RetroAchievements expansion and embedded video.

## Validation

Focused portability integration tests and GUI feature tests cover the new
boundary. Existing config/profile/path-migration tests are selected as affected
architecture checks. Targeted core/GUI compilation, formatting, whitespace and
scope/boundary guards are required. No full workspace/release build or repeated
GUI suite is needed for this preview-only foundation. Interactive GUI/local
file-dialog smoke remains a review action; headless tests do not validate
platform-native dialog behavior.

Final results (30 September 2026):

| Command (offline, isolated target directory) | Result |
|---|---|
| `cargo test -p archivefs-core --test setup_portability_preview` | 26 passed |
| `cargo test -p archivefs-gui --lib setup_portability` | 7 passed, including Settings routing and narrow/wide headless rendering |
| `cargo test -p archivefs-gui --lib emulator_setup_overrides` | 13 passed |
| `cargo test -p archivefs-core --lib source_root_migration` | 6 passed |
| `cargo test -p archivefs-core --lib dat::policy` | 51 passed |
| `cargo test -p archivefs-core --lib source_folders` | 11 passed |
| `cargo check -p archivefs-gui --lib` (checks core dependency too) | Passed; five existing GUI warnings, no new feature warnings |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` and staged diff check | Passed |
| Task scope and GUI-root boundary guards | Passed; no GUI-library-root growth |

114 tests passed across the selected targets (33 new-feature tests, 81 existing
compatibility checks). The initial new integration-test run exposed a fixture
using single-quoted strings where EmuWiz's existing config parser requires
double quotes. The fixture was corrected, the no-write test strengthened to
require an actual collected source, and all 26 integration cases passed on
rerun; production parser behavior was not changed.

No full workspace/GUI suite, release build, network provider smoke or live GUI
smoke was run. Native file dialogs and local GUI interaction still require
manual review. The candidate is a local feature commit only.
