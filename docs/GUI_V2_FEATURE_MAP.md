# GUI v2 feature map — milestone 1

Baseline: `be21648dc70da3a67fbb5d821c4fe58be4bea961`. This is a presentation
migration map, not permission to widen a backend's supported formats or safety
rules. The original GUI, CLI and all engines remain authoritative and intact.

## Status and navigation contract

- **Native**: implemented in the parallel `emuwiz-v2` interface now.
- **Handoff**: an explicit action opens the existing workflow in a separately
  labelled Legacy / Advanced window. V2 stays open. Selection is carried through
  for game actions. Merely visiting a v2 section never opens the legacy window.
- **Planned**: a specific future home, not a new nonfunctional v2 button. Use the
  named existing legacy tool in the meantime. Nothing is removed.

Permanent sidebar: Home; Library (Games, Platforms, Check Games, Problems &
Repair, Build Library); Play (Launch, Emulator Setup); Mods & Cheats; Artwork &
Metadata; Sources; Activity; History; Settings; Advanced. Narrow windows retain a
scrollable text sidebar. There is no Simple/Advanced navigation mode switch.

Every native page has a purpose, location, Back/Home controls, a primary next
action and secondary details. Game routes preserve the originating browser
filters and scrolling. Back is also Alt+Left or Escape; Alt+Home returns Home;
Tab/Shift+Tab and Enter operate sidebar buttons. The persisted location/filter
file is `gui-v2.json`, separate from legacy preferences.

Each future task must migrate its **whole journey**: discovery, explanation,
setup discovery, action, progress, result, recovery, back route and advanced
inspection. A row marked Planned is not a claim that that journey exists in v2.

## Library, checking and repair

Backend paths below are relative to `crates/archivefs-core/src` unless prefixed
`GUI:` (then relative to `crates/archivefs-gui/src`). Grouped format observers are
one user capability, not separate pages or new engines.

| Feature | Backend/component | GUI v2 home | Primary action | User-visible result | Background activity | Failure/recovery | Migration |
|---|---|---|---|---|---|---|---|
| Task dashboard | GUI: `gui_v2/routes.rs`, `pages.rs` | Home | Choose one of six tasks | Next task and signpost | Existing library load | Add games / Activity / retry | Native |
| Catalogue browsing, known health, recent scan evidence | `Database::load_archives`, `library_visibility` | Games | Open a game | Virtualized grid/list, written status | Read-only catalogue load | Retain last usable list; reload | Native |
| Search, system and attention filters | GUI: `gui_v2/library.rs` | Games | Search / choose system | Matching real games | Worker-side projection | Clear filters; add missing folder | Native |
| Platform browsing | `PersistedArchive::platform`, platform registry | Platforms | View system's games | Filtered browser | Loaded snapshot only | Add games / return to all systems | Native |
| Selected game information | `game_identity`, `launch::evidence_bridge`, database saved checks | Games → Game | Play | Evidence, file presence, installed-emulator discovery, contextual actions | Read-only selected-file stat, database lookup and discovery | Verify / setup / reconnect folder / retry | Native summary; execution handoff |
| Open game folder | Existing desktop file manager | Games → Game | Open Folder | Containing folder opens | `xdg-open` with argv, no shell | Reconnect drive / review Sources | Native |
| Refresh configured folders | `scan_all_enabled_sources_default` | Sources | Scan configured folders (confirm) | Updated game list; partial-folder failures remain explicit | Existing scanner, indeterminate until complete | Reconnect folder and rescan; old entries preserved by core | Native |
| Folder discovery and onboarding | Collection discovery, GUI: `collection_discovery_page`, `onboarding` | Sources → Add My Games | Find game folders | Review discovered folders before adding | Existing discovery scan | Choose manually / cancel | Handoff |
| Add, enable, disable and migrate source folders | Source folder config, `source_folder_migration`, `source_root_migration` | Sources → Change setup | Review folder change | Explicit configured sources | Preview / source scan | Cancel; existing migration recovery | Planned; legacy Sources |
| Read-only verification, including local MAME arcade data | `dat::sources`, `dat::audit`, GUI: `check_games` | Check Games → Arcade/system | Verify collection | Matched, unknown and needs-attention summary | Existing audit progress | Setup relevant verification data; choose if ambiguous | Handoff; first milestone-2 native journey |
| MAME local-data assignment and optional installed program | `dat::sources`, `identity_source::providers::mame`, GUI: `check_games` | Check Games → Arcade → Change setup | Confirm “Use it for Arcade?” | Arcade ready independently of software lists/program | Existing structural inspection | Ask on ambiguity; reject DAT/XML as executable | Handoff; preserve existing Simple Mode joins |
| Arcade dependencies, BIOS/CHD completeness and compatibility | `dat::set`, `dat::dependency`, `arcade_*`, `mame_input_requirements` | Check Games → Arcade → Results / Game → Verify | Check Arcade games | Required files / recommended safe next step | Audit and dependency resolution | Show exact missing requirements; no automatic download | Planned; legacy verification/selected evidence |
| Coverage, expected/missing/full-set review | `dat` expected inventory, GUI: `dat_coverage_panel` | Check Games → Results → Collection completeness | Review missing games | Relevant gaps, not provider lifecycle | Catalogue coverage query | Set up relevant data / resolve ambiguity | Planned; legacy DAT coverage |
| Identify & Rename | `dat::rename_apply`, GUI: `identify_rename` | Problems & Repair → Names | Preview names | Before/after proposals | Read-only audit and rename plan | Confirm chosen changes; history/undo | Planned; legacy Identify & Rename |
| Attention overview and diagnostics | `attention`, `diagnostics`, GUI: `needs_attention`, `doctor_page` | Problems & Repair | Review problems | Explained problems and actions | Existing doctor checks | Retry check / choose explicit repair | Handoff |
| Repair plan review/apply/rollback | `repair`, operation/transaction engines | Problems & Repair → Repair | Preview a fix | Scoped proposal, receipt and recovery | Existing transactional operation | Cancel preview; recover/undo through history | Planned; legacy Repair Center |
| Exact and equivalent duplicates | `repair::exact_duplicate`, N64/optical equivalence | Problems & Repair → Duplicates | Find duplicate copies | Evidence-backed retained/quarantined selection | Duplicate scan | Review conflicts; quarantine/undo, not silent deletion | Planned; legacy Duplicate Finder |
| Catalogue-relative duplicate review | Database/`dat` duplicate reports | Check Games → Results → Duplicates | Review matches | Why multiple entries match | Read-only query | Refine evidence; never treat as exact duplicates | Planned; legacy Library → Duplicates |
| CUE/BIN → CHD conversion | `repair` conversion planner/executor, `optical_fingerprint` | Game → Manage files → Convert | Preview conversion | Verified new copy and storage effect | Conversion and fingerprint verification | Refuse unsupported content; transaction rollback | Planned; legacy Disc Conversion |
| Storage usage / conversion opportunities | `storage_health`, `storage_conversion`, `conversion_planner` | Build Library → Storage | Analyse storage | Read-only usage and eligible opportunities | Analysis | Reconnect storage; unsupported stays explained | Planned; legacy Storage Health |
| PSP shrinking / restoration and XISO analysis | `psp_game_slimmer`, `psp_reversible_shrink`, `xiso_reversible_shrink` | Game → Manage files → Save space | Analyse safely | Existing supported plan; analysis-only where no writer exists | Bounded analysis / existing explicit executor | Retain originals; restore only with proven reconstruction | Planned; existing advanced/CLI workflows |
| Optical, cartridge, floppy, tape and package structural evidence | `game_identity`, `*_boot_evidence`, `*_header_evidence`, `content_evidence`, `logical_media` | Game → Advanced details → Recognition | Inspect information | Actual evidence, uncertainty and limits | Existing bounded observers | Explain unknown/unsupported; no filename promotion | Planned detailed view; native saved identity summary |
| Tape blocks, timing/audio and disk structure inspection | `tape_analysis`, `tape_audio`, disk/tape format modules | Game → Advanced details → Media inspection | Inspect media | Read-only structure and diagnostic findings | Bounded analysis | Explain unsupported/damaged input | Planned; legacy Tape Inspector/selected evidence |
| Multi-disc/disk/tape sets and sides | MediaSet, MediaSwapPlan, `launch::topology`, `launch::media_handoff` | Game → Media; Play → automatic | Review set / Play | Proven order and supported whole-set handoff | Existing topology/launch checks | Missing/stale/ambiguous evidence refused; emulator handles swapping | Planned detail; existing launch retained via handoff |

## Play, setup and saves

| Feature | Backend/component | GUI v2 home | Primary action | User-visible result | Background activity | Failure/recovery | Migration |
|---|---|---|---|---|---|---|---|
| Ready-to-play overview | `ready_to_play`, launch planner | Launch | Choose a game | One coherent game home | Library load | Play opens existing readiness checks | Native browser; handoff execution |
| Typed emulator launch and process feedback | `launch` typed requests/preflight/execution | Game → Play | Continue to Play, then existing Launch | Existing safe launch or explained blocker | Existing preflight/process tracking in legacy window | Setup/review blocker; never guess argv or bypass evidence | Handoff |
| Automatic executable/profile discovery | `diagnostics::profiles`, `emulator_environment`, existing adapters | Emulator Setup / Game status | Find installed emulators | Discovered installations; not a false ready verdict | Existing read-only discovery | Confirm/change or choose manually | Native limited game summary; full setup handoff |
| BIOS, firmware, TOS and core readiness | `bios_projection`, firmware verifiers, adapter profiles | Emulator Setup → System | Check setup | Actual required files and safe next step | Existing inventory/verifiers | Choose legitimate local firmware; no automatic download | Planned detail; legacy Emulator Setup/BIOS tools |
| Versions, channels, managed emulator installs/updates | `emulator_inventory`, `emulator_download`, `emulator_update`, `managed_emulator_install` | Emulator Setup → Manage programs | Review installation/update | Version, source, proposed changes | Existing download/verify/install operation | Explicit confirmation; managed rollback/retry | Planned; legacy installed-emulator tools |
| RetroArch cores, resources and profiles | `emulator_environment::retroarch`, `launch::retroarch_resource_projection` | Emulator Setup → RetroArch | Review setup | Discovered cores and applicable game targets | Existing profile/core discovery | Resolve multiple candidates explicitly | Planned; legacy RetroArch setup |
| Standalone adapters (all currently supported) | Existing `patch_manager::*_local` and `launch::*` | Emulator Setup → Emulator; Game → Play | Review / launch | Existing adapter-specific capabilities and limits | Existing discovery/preflight | Preserve every adapter's safety gate; no new adapters | Handoff, no backend changes |
| PS2 Save Vault and memory cards | `memory_card_inventory`, GUI: `pcsx2_page` | Game → Saves; Platforms → PlayStation 2 → Saves | Review saves | Card inventory and existing supported actions | Existing read-only inventory | Choose another card; confirm any supported write | Planned; legacy Tools → Saves |
| Emulator profile backups/repairs | `diagnostics::repair`, managed transaction/recovery | Emulator Setup → Recovery | Preview repair | Exact affected profile and recovery path | Existing repair checks/operation | Cancel; use recorded backup/rollback | Planned; legacy Doctor |
| Archive mounting/inspection and cleanup | Archive workflow, mount/process modules, archive inspector | Game → Advanced details → Archive; Advanced → Active mounts | Inspect / explicitly mount | Members or active mount details | Existing bounded inspection/mount job | Refuse unsafe member; unmount safely | Planned; legacy Mount/Active mounts/Archive Inspector |

## Mods and cheats

All actual installs, activation changes and rollback remain behind the existing
preview/confirmation/transaction paths. Opening Game → Mods does not install,
activate or silently choose a profile.

| Feature | Backend/component | GUI v2 home | Primary action | User-visible result | Background activity | Failure/recovery | Migration |
|---|---|---|---|---|---|---|---|
| Per-game cheats/mod discovery and preview | `patch_manager`, GUI cheat journey | Mods & Cheats → Game | Choose a game / open its improvements | Applicable providers, candidates, destinations | Existing catalogue/profile lookup | Explain unavailable/ambiguous evidence; review setup | Native chooser; contextual handoff |
| PCSX2 cheats and texture packs | `pcsx2_*`, `pcsx2_texture_pack` | Mods & Cheats → PlayStation 2 → Cheats / Texture Packs | Preview pack | Existing verified destination and change plan | Existing inspection/staging | Correct serial/profile; cancel/rollback | Planned native; legacy per-game panel |
| RPCS3 patches and ordinary mods | `rpcs3_*`, `rpcs3_ordinary_mod` | Mods & Cheats → PlayStation 3 → Patches / Mods | Preview mod | Scoped plan and provenance | Existing staging/transaction | Refuse conflicts; rollback | Planned native; legacy per-game panel |
| PPSSPP textures / ordinary mod packages | Existing PPSSPP adapter and local-mod package controller | Mods & Cheats → PSP → Texture Packs | Preview texture pack | Applicable game/profile and existing plan | Existing package inspection | Resolve identity/profile; cancel/rollback | Planned native; legacy per-game panel |
| Cemu graphic packs | `cemu_graphic_pack`, `cemu_local` | Mods & Cheats → Wii U → Graphic Packs | Preview graphic pack | Existing supported pack/rule plan | Existing inspection/staging | Explain unsupported rule/target; rollback | Planned native; legacy per-game panel |
| Dolphin Gecko/AR/OnFrame cheats and texture packs | `dolphin_*`, `gecko_*`, `bsfree_*` | Mods & Cheats → GameCube/Wii → Cheats / Textures | Preview improvement | Existing game-ID-bound plan | Existing retrieval and inspection | Disambiguate codes/profile; transactional undo | Planned native; legacy per-game panel |
| Xenia patches | `xenia_*` | Mods & Cheats → Xbox 360 → Patches | Preview patch | Title-ID-bound proposal | Existing provider/local lookup | Explain compatibility/identity conflict; undo | Planned native; legacy per-game panel |
| RetroArch cheats | `retroarch_cheat_*`, `retroarch_materialization` | Mods & Cheats → Game → RetroArch Cheats | Preview codes | Existing core/profile-compatible plan | Existing discovery/catalogue read | Choose correct profile; preserve existing files | Planned native; legacy per-game panel |
| Other existing emulator-specific local codes/mods | Current adapter capability/preview contracts | Mods & Cheats → System → Game | Review available improvements | Only actually supported actions | Existing adapter inspection | Unsupported shown honestly, not hidden or fabricated | Planned native; legacy per-game panels |
| Local cheat import and format compatibility | `user_cheat_import`, `cheat_conversion`, `cheat_ir` | Mods & Cheats → Game → Import | Preview local file | Parsed compatible codes and safety findings | Existing bounded parse | Correct type; no invented conversion support | Planned; legacy import tool |
| CheatBase, GameHacking, BSFree, Gecko and other registered cheat sources | `cheat_source_registry`, providers, caches | Mods & Cheats → Find improvements; Advanced → Cheat Sources | Search / explicitly refresh | Candidates and source status | Existing fetch/cache progress | Retry/offline cache/manual supported import | Planned; legacy Cheat Sources/CheatBase |
| Cheat reconciliation, activation, uninstall and rollback | `cheat_reconciliation_plan`, `shared_transaction`, `cheat_history` | Game → Installed improvements; History → Mods | Preview change | Exact before/after state | Existing journalled operation | Explicit approval; conflict-safe rollback | Planned; legacy review/history |

## Playing library, metadata, advanced tools and history

| Feature | Backend/component | GUI v2 home | Primary action | User-visible result | Background activity | Failure/recovery | Migration |
|---|---|---|---|---|---|---|---|
| Playing Library and 1G1R selection | `playing_library`, `library_views`, GUI: `playing_library_page` | Build Library | Build my library | Reviewable selection/output plan | Existing planning | Change policy; cancel preview | Handoff to existing Make a Playing Library workspace |
| Canonical organisation / rename-move proposals | Organisation and repair transaction engines | Build Library → Organise | Preview organisation | Proposed safe destinations | Existing plan/apply | Collisions refused; explicit approval/undo | Planned; legacy Library Organisation |
| RomM publisher | `publisher_profile`, `identity_source::romm` integration | Build Library → RomM | Preview output | Existing publisher plan; no invented writes | Existing projection | Correct mapping/setup; keep original media | Planned; legacy publisher profiles |
| ES-DE publisher, launch recipes and metadata export | `publisher_profile`, `launch::es_de_export`, `es_de_publish` | Build Library → ES-DE | Preview output | Supported frontend export/launch recipes | Existing planning/publish | Existing path/evidence refusals and recovery | Planned; legacy publishers/ES-DE tools |
| Other registered publisher profiles | `publisher_profile` | Build Library → Choose destination | Preview destination | Existing capability-specific projection | Existing planner | Unsupported operation explained | Planned; legacy publisher profiles |
| Library-view history/removal | `library_view_history` | History → Playing Libraries | Review an output | Durable apply/remove history | Read-only history load | Existing remove/restore safeguards | Planned; legacy Library View History |
| Covers and screenshots | `identity_source::artwork`, ES-DE and LaunchBox local indexes | Games / Game | Browse / Show screenshots | Immediate placeholder, lazy real pictures | Bounded local/network lanes, persistent thumbnails | Written unavailable state; Retry picture | Native |
| Descriptive metadata already imported | `ExternalIdentityRecord`, RomM cache | Game → About this game | Read game information | Existing synopsis/genres/year/players | One background index; no identity promotion | Missing remains absent; manage metadata | Native limited exact-path projection |
| RomM import, linking, mappings, media roots and server checks | `identity_source::romm`, `settings`, `net_policy` | Artwork & Metadata → RomM; Advanced → Identity Sources | Review connection / explicitly refresh | Existing linkage and enrichment | Existing network/import jobs | Approved-origin policy, offline cache, explicit mapping review | Planned; handoff to existing settings |
| ScreenScraper single/batch enrichment | `screenscraper_enrichment`, ScreenScraper client | Artwork & Metadata → Find information | Preview information | Review metadata before accepting | Existing bounded/rate-limited lookup | Retry/auth/setup; originals unaffected | Planned; existing ScreenScraper pages |
| Hasheous identity lookup | `identity_source::hasheous` | Game → Verify → More evidence | Look up game | Existing hash-based candidates | Existing approved lookup | Explain uncertainty/failure; do not guess | Planned; existing identity tools |
| Platform artwork overrides | `platform_artwork`, GUI: `platform_artwork_manager` | Artwork & Metadata → System pictures | Preview picture | Existing managed custom artwork | Existing bounded image load | Reset override / choose valid picture | Planned; legacy Settings |
| Museum / curated collection view | GUI: `museum_page` | Games → Browse collections | Explore collection | Read-only curated information | Existing loaded evidence/media | Return to Games; missing art placeholder | Planned; legacy Museum |
| Achievements and completion-time metadata | `retroachievements`, imported enrichment fields | Game → More information | View progress/information | Existing display-only metadata | Existing permitted lookups | Missing/offline explicitly labelled | Planned; legacy supported panels |
| Session work/progress | GUI: `gui_v2/activity.rs` | Activity + permanent footer | View Activity / View result | Queue, running, elapsed, known counts, success/failure | Real catalogue/detail/artwork/scan jobs | Safe artwork cancellation; non-cancellable scan clearly marked | Native; legacy jobs stay in their window |
| Durable repair/mod/config history and recovery | `operation`, repair/cheat/library journals | History → By task | Review previous changes | Existing receipts and supported recovery | Existing history queries | Review before rollback; preserve conflict refusals | Handoff overview; native durable history planned |
| DAT and catalogue management for all existing ecosystems | `dat`, `identity_source::{no_intro,redump,tosec,fbneo,mame_listxml,mame_software_list,whdload}` | Sources → Verification Data / DATs; Advanced | Inspect / explicitly import | Detailed sources, coverage and lifecycle | Existing bounded validation/audit and managed-update workers | Missing/stale/ambiguous states, existing safe recovery | Native; shared DAT page and engines, no ecosystem changes |
| Provider snapshots, activation, provenance and lifecycle | `identity_source::{status,managed_snapshot,freshness,verification}` | Sources → Verification Data / DATs | Inspect details | Full technical evidence | Existing managed-provider checks and snapshot stores | Explicit activation/deactivation only | Native through the shared DAT/provider page |
| MAME software lists / ScummVM / WHDLoad setup | Existing provider discovery/conversion | Check Games → Relevant system → Advanced setup | Review setup | Separate applicable verification data | Existing provider checks | Arcade data never shown missing because software lists are absent | Planned native; legacy setup retained |
| Platform aliases, assignment overrides and evidence resolution | Platform registry/fusion, `evidence_resolution` | Game/System → Advanced details; Advanced → Platform mapping | Review assignment | Current evidence and proposed override | Existing read-only resolution | Ask when ambiguous; explicit change only | Planned; legacy aliases/assignment tools |
| Database status, backup, restore and migrations | Database health/backup/restore, recovery migrations | Advanced → Application data | Check data / preview recovery | Existing health and recoverable plan | Existing checks/transaction | Refuse unsafe/newer schema; confirm restore | Planned; legacy Database Status/CLI |
| Logs, technical diagnostics, mount/storage environment | `diagnostics`, logging | Advanced → Diagnostics | Run a read-only check | Technical findings and privacy-aware details | Existing doctor/diagnostics | Explain failure; retry/copy details | Planned; legacy diagnostics |
| Application settings, theme/readability, About | GUI configuration/presentation | Settings | Review settings | Independent v2 location; existing app settings via handoff | Async v2 preference persistence | Failure does not block browsing; legacy config unchanged | Native v2 defaults; other settings handoff |
| CLI automation and batch operations | `archivefs-cli` | Advanced → Automation documentation | View existing CLI help | Existing command contracts, not GUI reimplementations | Only when explicitly run | Existing exit codes/recovery contracts | Unchanged; future documentation link |

## Milestone-2 ordering

First migrate **Home → Check Games → Arcade → choose/accept folder → Verify →
results**, preserving the local MAME structural-evidence joins already in main.
Native verification must share v2 Activity, expose the useful result counts and
provide a direct recovery action. Software-list setup stays secondary. Then
migrate a complete discovered-emulator Play journey and one complete per-game
Mods journey, retaining existing preflight/transaction authority throughout.

No feature is considered product-complete by this map or unit tests alone.
Milestone 1 stays on its review branch until live visual and exploration
acceptance is performed. See `GUI_V2_MILESTONE_1.md` for architecture, measurements
and the launch/review checklist.
