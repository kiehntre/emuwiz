# EmuWiz — Full Duplication / Completeness Audit

Read-only audit. No production code, branches, worktrees, or refs were modified.

- **Date of audit:** 2026-09-02
- **Authoritative repo:** `saltbox26:/home/davedap/archivefs`
- **Authoritative branch:** `feature/archivefs-unified-platform`
- **Authoritative HEAD inspected:** `ddf121276ea7130a4c6a25248e3c20619c4c2fc7` — `feat(gui): explain DAT identity and catalogue filename`
- **`git status --short` at audit start:**
  ```
   M crates/archivefs-core/src/launch/mod.rs
   M crates/archivefs-core/src/launch/tests.rs
   M crates/archivefs-gui/src/launch_readiness_page/tests.rs
  ?? crates/archivefs-core/src/launch/plan_to_spawn_tests.rs
  ?? amiga  c64  gb  gba  gbc  n64  nes  ps2  saturn  static
  ```
  The four `launch` entries are **test-only** additions from the immediately-preceding "Launch E2E validation" task in this same session (uncommitted, no production behaviour change). `amiga … static` are pre-existing untracked scratch directories.

**LOCAL IS AUTHORITATIVE.** GitHub is many hundreds of commits behind and contributes nothing not already local.

---

## 1. Local repository inventory

| Metric | Value |
|---|---|
| Local branches | 130 |
| Git worktrees | 94 |
| Authoritative branch commit count | ~1060 |
| Unreachable commits (`git fsck --no-reflogs --unreachable`) | 318 — orphaned rebase/amend/WIP noise from the replay-integration workflow; spot-checked, none carry unique feature work |
| Reflog | authoritative HEAD advances **only** by clean `--ff-only` merges of validated integration commits (`… → b07d51c → 0420fc1 → decc035 → ddf1212`). No lost work. |

### Branch topology

The repo uses a **replay-integration** workflow: feature work is developed on a per-topic branch/worktree, then replayed (rebased/squashed) onto `feature/archivefs-unified-platform` and the origin branch abandoned as a pointer. Consequently **"branch not merged" ≠ "work not landed"** — ~90 of the 130 local branches are stale snapshots whose content is a strict subset of authoritative HEAD (each diffs *net-negative* vs HEAD with zero new files). This was verified exhaustively in two prior audits this session and re-spot-checked here.

### Live, non-stale worktrees (the only ones that matter)

| Path | Branch | HEAD | vs `ddf1212` | State | What it is |
|---|---|---|---|---|---|
| `/home/davedap/archivefs` | `feature/archivefs-unified-platform` | `ddf1212` | — | test-only dirty (see above) | **Authoritative.** |
| `/home/davedap/archivefs-dat-gui-slice` | `feature/dat-gui-vertical-slice` | `ddf1212` | 0 / 0 | clean | **Concurrent session — DAT → GUI/library workflow.** Its last landed work *is* `ddf1212`. Currently synced/idle. |
| `/home/davedap/archivefs-generic-noncheat-mod-workflow` | `codex/generic-noncheat-mod-workflow` | `decc035` | 0 ahead committed, **dirty** | modified: `mod_package.rs`, `patch_manager/mod.rs`, `shared_transaction.rs`, `cheats_mods_preview.rs`, `gui/main.rs`, `gui/tests/mod.rs`; **new: `crates/archivefs-gui/src/local_mod_package_page.rs`** | **Concurrent session — generic non-cheat Mods apply + GUI, actively in progress.** |
| `/home/davedap/archivefs-stale-recovery-fixit` | `fix/stale-recovery-fixit-here` | `37ca447` | +3 / −1 | clean | **Superseded duplicate.** A *second* implementation of stale-recovery-archive (adds `dat/rename_apply/stale_archive.rs`), branched from `b07d51c` before the winning implementation (`decc035`, `history.rs`) landed. Its approach did not win. |
| `/home/davedap/archivefs/.claude/worktrees/missing-library-fixit` | `fix/missing-library-fixit-here` | `0420fc1` | ancestor of HEAD | clean | Work already landed (`0420fc1`). Worktree redundant. |
| `/home/davedap/archivefs/.claude/worktrees/standalone-emu-readiness` | `fix/standalone-emu-readiness` | `b07d51c` | ancestor of HEAD | clean, **0 commits** | Plan-only investigation of AppImage standalone launch. Never implemented. |
| `/tmp/archivefs-gamer-typed-launch` | detached `043fb4c` | — | — | prunable | Orphan. |

### Source-tree keyword survey (authoritative HEAD)

Every term from the brief resolves to real, present modules:

- **DAT / No-Intro / Redump / TOSEC / MAME / identity / rename / 1G1R / Playing Library** — `dat/` (parsers `logiqx`, `clrmamepro`, `mame_listxml`; `sources/`, `updates.rs` 4.7k L, `managed_sources.rs`, `rename_plan/`, `rename_apply/` incl. `history.rs`, `rom_organisation/`, `set.rs::classify_disk_only_sets`, `divergence`, `policy/`); `identity_source/` (`no_intro/`, `redump/`, `tosec/`, `mame_listxml`, `mame_software_list`, `fbneo`, `whdload`, `hasheous`, `artwork`, `verification`, `path_map`, `stale`); `playing_library/` (`model`, `matching`, `apply_adapter`, `romm_library_plan`, `romm_projection`, `retrodeck_projection`), `playing_library_page.rs` "Build Playing Library (1G1R)".
- **RomM / artwork / cover / media / screenshot / manual / enrichment / import** — `identity_source/romm/` (`client`, `capability`, `enrichment`, `import`, `normalise`), `identity_source/artwork.rs`, `gamer_artwork.rs`, `romm_game.rs`, `romm_browse.rs`, `romm_config.rs`, `romm_source.rs`. **No screenshot/manual handling** (covers only).
- **cheat / mods / Dolphin texture / PCSX2 / RetroArch / GameHacking / BSFree / Action Replay** — `patch_manager/` (~70 files: `cht_document`, `gecko_document`, `pcsx2_pnach`, `dolphin_code`, `xenia_patch_document`, `dolphin_texture_mod`, `dolphin_texture_pack`, `bsfree*`, `gamehacking_*`, `cheatbase`, `user_cheat_import`, `cheat_installer/plan/rollback/history`, `shared_transaction`); `mod_package.rs` (inspect-only). AR licensing = research draft (blocked).
- **launch / AppImage / DuckStation / PPSSPP / RPCS3 / Ryujinx / ES-DE** — `launch/` (12 adapters: RetroArch+Dolphin+PCSX2+PPSSPP+DuckStation+RPCS3+Flycast+xemu+Xenia+ScummVM+DOSBox+MAME; `planning`, `readiness`, `platform_map`, `input_projection`, `evidence_bridge`, `integration`, `execution`, `process_spawn`, `es_de_export`, `es_de_publish`). **No Ryujinx / Switch launch.** AppImage discovered but not launch-authorized.
- **Doctor / repair / duplicate / quarantine / recovery / mount root / source ingestion / library organisation / BIOS / firmware** — `diagnostics/` (`runner`, `profiles`, `managed`, `environment`, `verified_identity`, `arcade_*`, `repair`); `repair/` (`duplicate_scan`, `quarantine`, `proposal`, `executor`, `rollback`); `ingestion/`; mount-root reconciliation in `lib.rs`/GUI; firmware evidence in `patch_manager/*_firmware`, `dat/firmware_evidence.rs`.
- **Dreamcast GDI/CDI/CHD / PS3 / PS4 / Xbox / Xbox360 / Wii / GameCube** — `dreamcast_cdi.rs`, `dreamcast_boot_evidence.rs`, `chd_optical_specialist.rs`, `chd_logical_media.rs`, `ps3_boot_evidence.rs`, `ps3_disc_evidence.rs`, `ps4_layout_evidence.rs`, `param_sfo.rs`, `xbox_boot_evidence.rs`, `xbox360_boot_evidence.rs`, `xbox360_stfs_evidence.rs`, `gamecube_wii_boot_evidence.rs`, `xdvdfs_*`. **No PS5. No Wii U / Switch specialist parsers** (registry ids only). **3DS registry id only.**

---

## 2. Local branch / worktree audit (only the ones with any live relevance)

| Branch / worktree | HEAD | merge-base w/ `ddf1212` | ahead/behind | clean? | In authoritative HEAD? | Unique unmerged work? | Verdict |
|---|---|---|---|---|---|---|---|
| `feature/archivefs-unified-platform` (`/home/davedap/archivefs`) | `ddf1212` | self | — | test-only dirty | — | — | **AUTHORITATIVE** |
| `feature/dat-gui-vertical-slice` (`archivefs-dat-gui-slice`) | `ddf1212` | `ddf1212` | 0 / 0 | yes | **yes (identical)** | none | Concurrent DAT session, synced. Do not touch. |
| `codex/generic-noncheat-mod-workflow` (`archivefs-generic-noncheat-mod-workflow`) | `decc035` | `decc035` | 0 committed | **dirty** | base yes; WIP no | **yes — generic non-cheat Mods apply + `local_mod_package_page.rs`**, in progress | **CONCURRENT — do not assign generic mods work.** |
| `fix/stale-recovery-fixit-here` (`archivefs-stale-recovery-fixit`) | `37ca447` | `b07d51c` | +3 / −1 | yes | **feature yes, via a different file** | no (superseded) | **OBSOLETE / SUPERSEDED** — `decc035` shipped `dat/rename_apply/history.rs` for the same behaviour; this branch's `stale_archive.rs` lost. |
| `fix/missing-library-fixit-here` (`.claude/worktrees/missing-library-fixit`) | `0420fc1` | `0420fc1` | ancestor | yes | **yes** | none | Landed. Redundant worktree. |
| `fix/standalone-emu-readiness` (`.claude/worktrees/standalone-emu-readiness`) | `b07d51c` | `b07d51c` | ancestor, **0 commits** | yes | n/a | **plan only, no code** | Investigation notes only. AppImage-launch gap still open. |
| `codex/stale-recovery-final-current-tip` / `codex/stale-recovery-cleanup` | `decc035` / `aae040c` | — | — | — | **yes** | none | Landed as `decc035`. Obsolete pointers. |
| `feature/non-cheat-mod-foundation` (`emuwiz-codex-mods`) | `3a55df3` | old | 0 committed divergence | — | foundation yes | no | Superseded by `mod_package.rs` in HEAD + the active `codex/generic-noncheat-mod-workflow`. Obsolete. |
| `feature/ps4-identity-phase1` | `c7bd006` | old | +1 | — | **yes** (tree identical, minus 267 later-added lines) | no | Landed (`f36e799`, `fecd7da`). Obsolete. |
| `feature/tape-identity-phase2` / `-integrated` | `c362cc1` / `39838d6` | old | +1 | — | **yes** (`tape_identity.rs` byte-identical) | no | Landed (`10be8f0`, `1276122`). Obsolete. |
| All other `feature/*`, `integration/*`, `integrate/*`, `codex/*-modernize`, `audit/*` (~85 branches / ~80 worktrees) | various | various | +1 / −N | — | **yes** — every one diffs net-negative vs HEAD, 0 new files | no | **OBSOLETE / SUPERSEDED.** Content is in `ddf1212`. Safe to delete in a deliberate cleanup pass; not a backlog concern. |
| `design/*`, `docs/preserved-emuwiz-research`, `research/encrypted-action-replay-licensing`, `feature/gui-history-recovery-view` | various | — | — | — | reference only | no | **RESEARCH / HISTORICAL REFERENCE** — keep, never replay wholesale. |
| `main` | `1235d2f` | — | 0 / 375 | — | superseded | no | v0.7.x era. Fully behind. |

No branch conflicts with newer architecture except `fix/stale-recovery-fixit-here` (would collide with the landed `history.rs` design) and `feature/non-cheat-mod-foundation` (would collide with the active generic-mods worktree).

---

## 3. GitHub audit — `kiehntre/emuwiz` (was `kiehntre/archivefs`, renamed; not archived)

- `origin/main` = `1235d2f` (local `main` == this). `origin/feature/archivefs-unified-platform` = **`4fbaebc`** — roughly mid-history of the local integration branch; **GitHub has not seen ~600 of the local unified-platform commits.**
- **PRs #1–#66**, all targeting `main`. Status: **all MERGED** except #30 (DRAFT, docs), #27 & #32 (CLOSED, docs/imgbot). No open feature PRs. No drafts with code. **No issues** in either repo name.
- Remote `origin/feature/*` branches (`launch-input-projections`, `launch-plan-wiring`, `retroarch-command-plan`, `pcsx2-cheat-gui-apply`, `ppsspp-duckstation-doctor-gui`, `repair-center-core`, `library-repair-planner`, `rar-7zz-provider`, `content-media-evidence`, `duplicate-quarantine-gui-review`, `esde-integration`, `esde-installation-discovery`, `frontend-profiles-stage1-2`, `ps2-loose-iso-identity`, `no-intro-sms-semantics-fix`, `malformed-cheat-parsing`, `cheat-journey-orchestration`, `cheatbase-provider-stage1`) — each maps to a merged PR / landed feature and is **superseded locally**.

| Remote item | Classification |
|---|---|
| PR #5 "Show RomM cover artwork in Gamer View" (merged 2026-08-05) | **MERGED LOCALLY** — `gamer_artwork.rs`, `identity_source/artwork.rs` present in HEAD |
| PR #66 "EmuWiz 0.8 frontend profiles, media intelligence, repair workflow" (merged 2026-08-17) | **MERGED LOCALLY** |
| PRs #1–#66 (feature/repair/DAT/BSFree/etc.) | **MERGED LOCALLY** — all present, most then rewritten/extended in the unified branch |
| All `origin/feature/*` topic branches | **SUPERSEDED LOCALLY** |
| PR #30 `research/encrypted-action-replay-licensing` (DRAFT) | **RESEARCH ONLY / BLOCKED** — encrypted AR licensing; no implementation intended |
| `origin/imgbot`, PR #32, PR #27 | **ABANDONED** (housekeeping / closed docs) |
| Every unified-platform commit after `4fbaebc` (~600) | **UNIQUE LOCAL, not on GitHub** — a push-lag, not missing work |

**No unique remote work exists.** The only remote-only item is stale docs draft #30.

---

## 4. Feature matrix (authoritative HEAD `ddf1212`)

Legend: **DONE** · **PARTIAL** · **ELSEWHERE** (exists on another branch, next step = promotion) · **FAILED** · **BLOCKED** · **NOT STARTED** · **UNKNOWN**

### A. DAT
| Feature | Status | Evidence |
|---|---|---|
| Import (Logiqx / clrmamepro / MAME listxml) | **DONE** | `dat/parsers/*`, `dat/model.rs`, `trusted_dtd.rs` |
| No-Intro | **DONE** | `identity_source/no_intro/` (`import`, `convert`, `registry`, `pack_import` incl. aftermarket "Love Pack") |
| Redump | **DONE** | `identity_source/redump/`, `chd_identity.rs`, `chd_logical_media.rs` |
| TOSEC | **DONE** | `identity_source/tosec/`, `dat/tosec_release_pack/` |
| DAT packs | **DONE** | `no_intro/pack_import`, `dat/tosec_release_pack` |
| Auto-update / download / rollback | **DONE** | `dat/updates.rs::{check_managed_dat_update, update_managed_dat, rollback_managed_dat_to_previous}`, wired in `dat_sources_page.rs` (`ManagedDatUpdatePolicy`, status views) |
| Identity evidence / persistence | **DONE** | `verified_identity_cache/`, `diagnostics/verified_identity.rs`, migrations `0008`, `0010`, `dat/library_identity_summary/` |
| GUI DAT identity | **DONE** (+ concurrent polish) | `dat_identity_panel.rs`, `dat_sources_page.rs`, `selected_evidence_no_intro.rs`, `game_metadata.rs`; **`ddf1212` "explain DAT identity and catalogue filename"** is the current concurrent DAT-→-GUI session's latest landed work |
| Filename comparison | **DONE** | `dat/policy/candidate.rs`, `identity_source/matching.rs`, "Exact local filename" states in GUI |
| Rename preview / apply / rollback / exact-resume | **DONE** | `dat/rename_plan/`, `dat/rename_apply/` (`executor`, `journal`, `rollback`, `exact_resume`, `reconcile`, `noclobber`, `preflight`, `history`) |
| Headered / headerless | **DONE** | `header_normalization.rs`, `smd_normalization.rs`, per-platform `*_header_evidence.rs` |
| Language / region / revision | **PARTIAL** | region/revision carried in identity records & DAT model; no dedicated language-preference policy surface beyond `dat/policy/` |
| Parent / clone | **DONE** | No-Intro `cloneofid`, `dat/divergence`, 1G1R election consumes clone graph |
| BIOS / dependency sets | **DONE** | `dat/dependency/`, `dat/set.rs`, `s2d` dependency engine, `dat/firmware_evidence.rs` |
| 1G1R | **DONE** | `playing_library/matching`, `model`, `playing_library_page.rs` "Build Playing Library (1G1R)" |
| Playing Library | **DONE** | `playing_library/` full, GUI page, apply transactions |
| Disk-only DAT sets | **DONE** | `dat/set.rs::classify_disk_only_sets`, `dat/disk_audit.rs` |

### B. RomM
| Feature | Status | Evidence |
|---|---|---|
| Library import | **DONE** | `identity_source/romm/import.rs` (adaptive paging, gated recovery), `client.rs` (`/api/roms`, `/api/platforms`, `/api/heartbeat`) |
| Metadata enrichment | **DONE** | `romm/enrichment.rs` |
| Platform mapping | **DONE** | `romm/config.rs`, catalogue-platform → RomM-slug overrides in GUI (`main.rs`) |
| Covers / artwork ingest | **DONE** (since PR #5, 2026-08-05) | `identity_source/artwork.rs` — SSRF-guarded fetch of `path_cover_small` **from the RomM instance only** (`url_cover`/IGDB/RetroAchievements deliberately refused), 2 MiB response cap, 32 MiB decompression-bomb guard, thumbnail resize, on-disk **1 GiB LRU cache** with eviction + "last used" index, keys from server-identity + record-id + RomM `ts` |
| Covers display in GUI | **DONE** | `gamer_artwork.rs` — Gamer-View game-list cover worker, `CoverPriority` scheduler, `CoverSlot`/`CoverJob`/`CoverReply`, typed `NoCover` reasons (`NoRommIdentity` / `NoArtwork` / `PublicOnly` / `Unavailable` / `Failed`); Details-panel single-cover load on button press via `RommOperation`; `romm_game::decode_thumbnail` / `CoverImage` |
| `ArtworkReference` parse | **DONE** | `romm/normalise.rs:164` parses `url_cover`, `path_cover_large`, `path_cover_small`; `capability.rs` reports `available_artwork_fields` |
| Screenshots | **NOT STARTED** | no `screenshot` handling anywhere |
| Manuals | **NOT STARTED** | no `manual` handling anywhere |
| `path_cover_large` (higher-res cover) | **NOT STARTED** | only `path_cover_small` is ever fetched (deliberate: card-sized thumbnails) |
| Local RomM media-path reuse (read RomM's `resources/` off a mounted disk instead of HTTP) | **NOT STARTED** | `identity_source/path_map.rs` maps RomM *ROM* paths to local files for identity matching; there is no equivalent for reading RomM's *media* files locally — covers are always fetched over HTTP from the instance |
| Library creation / export | **DONE** | `playing_library/romm_library_plan.rs` (`build_romm_library_plan`, `build_romm_library_apply_transactions`), `romm_projection.rs`, wired in `playing_library_page.rs` (`build_romm_projection_with_visibility`) |
| RomM scan-folder compatibility (RetroDECK/ES-DE-style layout) | **DONE** | `playing_library/{romm_projection, retrodeck_projection}.rs` |

### C. Mods
| Feature | Status | Evidence |
|---|---|---|
| Generic non-cheat package **inspection** | **DONE** | `mod_package.rs::{inspect_local_mod_package, inspect_local_mod_package_candidates}`, `LocalModPackagePlan` (conflict/blocker/compat model) |
| Generic **apply** | **ELSEWHERE — in progress concurrently** | no `apply`/`install` fn in `mod_package.rs` at HEAD; `codex/generic-noncheat-mod-workflow` worktree is actively adding it (dirty: `mod_package.rs`, `shared_transaction.rs`, `patch_manager/mod.rs`) |
| Generic **rollback** | **ELSEWHERE — in progress** | same worktree, via `shared_transaction` reuse |
| Generic **GUI** | **ELSEWHERE — in progress** | same worktree, new file `crates/archivefs-gui/src/local_mod_package_page.rs` |
| Dolphin texture mods | **DONE** | `patch_manager/dolphin_texture_mod.rs`, `dolphin_texture_pack.rs`, `dolphin_texture_mod_page.rs` GUI with `execute_dolphin_texture_pack_apply` + `execute_shared_apply` + rollback; single-PNG hires-texture install + texture-pack apply |
| Provider / download support | **NOT STARTED** | no mod downloader; GUI states "Texture packs, Riivolution assets … remain unavailable" |

### D. Cheats
| Feature | Status | Evidence |
|---|---|---|
| RetroArch cheats | **DONE** | `patch_manager/{retroarch_cheat_library, retroarch_cheat_setup, retroarch_inventory, retroarch_materialization}.rs`, e2e test |
| PCSX2 (PNACH) | **DONE** | `pcsx2_pnach.rs`, `pcsx2_install_plan.rs`, e2e test; ordinary-cheat *provider* still needs a licensed source (see below) |
| Action Replay | **BLOCKED** | encrypted-AR licensing research draft (PR #30); no implementation |
| BSFree (GC/Wii) | **DONE** | `patch_manager/bsfree*`, `bsfree_gamecube`, `bsfree_wii`, e2e tests |
| GameHacking.org import | **DONE** | `patch_manager/gamehacking_*`, browser-assisted import, e2e tests; GUI `user_cheat_import_page.rs` + `e5c26da` |
| Malformed-cheat handling | **DONE** | bounded/fail-closed parsing in `cht_document`, `gecko_document`, `pcsx2_pnach`, `xenia_patch_document`; `import_safety.rs` |
| Licensed ordinary-cheat provider (PCSX2) | **BLOCKED** | ROADMAP milestone #1 — apply/preview/merge/rollback done, blocker is a legally-reviewed source, not code |

### E. Launch
| Feature | Status | Evidence |
|---|---|---|
| Unified planning | **DONE** | `launch/planning.rs::build_launch_plan`, `integration.rs::build_launch_plan_from_results` |
| Input projection | **DONE** | `launch/input_projection.rs` (12 adapters) |
| Platform mapping | **DONE** | `launch/platform_map.rs` (`LAUNCH_COMPATIBILITY`, `.info` alias resolution) |
| RetroArch launch (real spawn) | **DONE** | `launch/retroarch_command.rs` + `execution.rs` + `process_spawn.rs`; GUI `launch_readiness_page.rs`; `5c12985` |
| Standalone launch (Dolphin/PCSX2/DuckStation/PPSSPP/RPCS3/Flycast/xemu/Xenia/ScummVM/DOSBox/MAME) | **PARTIAL** | all 12 have `*_command` + `*_execution` and are wired into Gamer View. **Native-binding resolvers accept only `InstallationType::Native` (a distro binary on `PATH`).** |
| **AppImage / Flatpak standalone launch** | **NOT STARTED** | `resolve_pcsx2_native_launch_binding` / `resolve_ppsspp_native_launch_binding` (`ppsspp_local.rs:353`) / `resolve_dolphin_native_launch_binding` reject any non-`Native` install → `UnsupportedInstallationType`. PPSSPP/PCSX2/Dolphin AppImages **are discovered** (classified `Portable`) but never launch-authorized, so a game whose only emulator is an AppImage (incl. one EmuWiz's own downloader installed) falls back to RetroArch-core ambiguity. Investigation exists (`.claude/worktrees/standalone-emu-readiness`, **0 commits**). |
| Fallback standalone → RetroArch | **DONE** (behaviour) + **TESTED this session (uncommitted)** | `planning.rs::apply_preference`; the fallback boundary is covered by the uncommitted `launch/tests.rs` additions from the preceding task |
| PPSSPP / DuckStation | **DONE** (command+execution+Doctor rows) | `ppsspp_*`, `duckstation_*`, `ppsspp-duckstation-doctor-gui` |
| Dolphin | **DONE** (incl. explicit-root `-u` binding, full preflight→spawn E2E) | `dolphin_command/execution`, `dolphin_local.rs` |
| RPCS3 | **DONE** | `rpcs3_command/execution`, `rpcs3_local.rs`, `rpcs3_page.rs` |
| Ryujinx / Switch | **NOT STARTED** (deliberate) | no adapter, no `.nsp`/`.xci` parser; ROADMAP defers Switch (keys/firmware) |
| ES-DE | **DONE** | `launch/es_de_export.rs`, `es_de_publish.rs`, `emulator_environment/es_de.rs` (`discover_es_de_environment`, local-install discovery), `playing_library/retrodeck_projection.rs` |
| GUI launch | **DONE** | `launch_readiness_page.rs` (background `preflight_and_launch_*` workers), `gamer_view.rs` `GamerPlayAction::Launch` |
| E2E validation | **PARTIAL — landed this session, uncommitted** | per-adapter `*_execution/tests.rs` dense; cross-module continuity + fallback-boundary + `gamer_play_action`-on-mixed-plan tests added in the immediately-preceding task and sitting uncommitted in the authoritative worktree (`launch/tests.rs`, `launch/plan_to_spawn_tests.rs`, `launch_readiness_page/tests.rs`) |

### F. Repair / recovery
| Feature | Status | Evidence |
|---|---|---|
| Duplicate repair / detection | **DONE** | `repair/duplicate_scan.rs`, `FilenameDuplicateDetector`, `exact_duplicate_review_page.rs` |
| Quarantine (rollbackable) | **DONE** | `repair/quarantine.rs` (`plan_duplicate_quarantine`, `QuarantineGroupPlan`, `quarantine_destination`), PRs #62–65 |
| Rollback / exact-resume | **DONE** | `dat/rename_apply/{rollback, exact_resume, journal, reconcile}.rs`, `repair_history_page.rs`, `repair_review_page.rs` |
| Stale-recovery archive ("fix-it-here" for dead recovery records) | **DONE** | `dat/rename_apply/history.rs` (`RecoveryHistoryState`, `StaleRecoveryReason`, `RecoveryCleanupClassification`), `repair_history_page.rs` + `dat_sources_page.rs` wiring (`decc035`) |
| Missing-library "fix-it-here" | **DONE** | `library_view.rs::{confirmed_missing_catalogue_paths, show_missing_library_fixit_card}` (`0420fc1`) |
| Mount-root reconciliation | **DONE** | `2a6831c`, `b07d51c`, `3728743` — core unsafe-target rejection + GUI setup + reconciliation, order/verify/spacing/feedback |

### G. Library organisation
| Feature | Status | Evidence |
|---|---|---|
| Canonical platform folders | **DONE** | `rom_organisation_page.rs`, `dat/rom_organisation/`, canonical master-ROM-root (PR #18) |
| Source ingestion | **DONE** | `ingestion/` (`discover_source`, `DiscoveryStats`, content registry) |
| Rename transactions | **DONE** | `dat/rename_apply/` |
| 1G1R / Playing Library | **DONE** | `playing_library/`, `playing_library_page.rs` |
| RomM export / library creation | **DONE** | `romm_library_plan.rs`, `romm_projection.rs` |
| Managed Library Views (symlink views + repair) | **DONE** | `library_views.rs`, `library_view_history.rs`, `library_view_history_page.rs` |

### H. Platform-specific identity
| Platform | Status | Evidence / gap |
|---|---|---|
| Dreamcast GDI / CDI | **DONE** | `dreamcast_cdi.rs` (`.gdi`/`.cdi` data-track selection, "exactly one candidate or refuse"), `dreamcast_boot_evidence.rs` |
| Dreamcast CHD | **DONE** | `chd_optical_specialist.rs`, `chd_logical_media.rs` |
| PS1 / PS2 / PS3 / PSP | **DONE** | `playstation_boot_evidence.rs`, `ps2_boot_evidence.rs`, `ps3_boot_evidence.rs`, `ps3_disc_evidence.rs`, `psp_boot_evidence.rs`, `psp_pbp_evidence.rs`, `param_sfo.rs` |
| PS4 | **PARTIAL** (Phase 1) | `ps4_layout_evidence.rs` — bounded `sce_sys/param.sfo` CUSA identity in an *extracted* dir; encrypted `.pkg` identity + launch deferred |
| PS5 | **NOT STARTED** | not in platform registry |
| Switch | **PARTIAL** (registry only) | `Switch` platform id + folder/artwork; no `.nsp`/`.xci` parser, no launch (deliberate) |
| Xbox / Xbox 360 | **DONE** | `xbox_boot_evidence.rs`, `xbox360_boot_evidence.rs`, `xbox360_stfs_evidence.rs`, `xdvdfs_traversal.rs`, `xdvdfs_signature.rs`, xemu/Xenia adapters |
| Wii / GameCube | **DONE** | `gamecube_wii_boot_evidence.rs`, Dolphin Game-ID mapping |
| Wii U | **NOT STARTED** (registry only) | `WiiU` platform id; no `.wud`/`.wux` parser |
| 3DS | **NOT STARTED** (registry only) | `Nintendo 3DS` id; no `.3ds`/`.cia` parser |

### I. GUI
| Area | Status | Evidence |
|---|---|---|
| DAT info | **DONE** (+ concurrent polish) | `dat_identity_panel.rs`, `dat_sources_page.rs`, `selected_evidence_no_intro.rs`, `identity_sources_page.rs`, `dat_catalogue_picker.rs` |
| Cheats & Mods | **DONE** (generic-mods GUI concurrent) | `cheats_mods_preview.rs`, `cheat_sources_page.rs`, `cheatbase_page.rs`, `user_cheat_import_page.rs`, `dolphin_texture_mod_page.rs`, `pcsx2_page.rs` |
| Sources / Discovery | **DONE** | `sources_page.rs`, `collection_discovery_page.rs` |
| Library Organisation | **DONE** | `rom_organisation_page.rs`, `playing_library_page.rs`, `library_view_history_page.rs` |
| Doctor | **DONE** | `doctor_page.rs`, `problems_repair_page.rs` |
| Repair | **DONE** | `repair_review_page.rs`, `repair_history_page.rs`, `exact_duplicate_review_page.rs`, `plan_preview_page.rs` |
| Novice / Gamer mode | **DONE** (open UX polish) | `gamer_view.rs`, `gamer_platform_shelf.rs`, `home_page.rs`, `view_mode.rs`; `docs/BEGINNER_UX_AUDIT.md` lists still-open small items (G1/G5/L2/L4/L5/H4) |
| Artwork / media | **DONE** (covers) | `gamer_artwork.rs`, `romm_game.rs`, `game_presentation.rs`, `ui/platform_artwork.rs` |
| Launch | **DONE** | `launch_readiness_page.rs`, `retroarch_core_setup.rs`, `emulator_setup_focus.rs`, `emulator_download_page.rs` |

---

## 5. Special RomM artwork audit

Deep search across current source, all local branches/worktrees, git history, and GitHub PRs/branches (`romm`, `artwork`, `cover(s)`, `boxart`, `media`, `screenshot`, `manual`, `thumbnail`, `image_url`, `url_cover`, `path_cover`, `media_path`, `cover_path`, `enrichment`, `import`).

**1. Can EmuWiz already ingest / reuse RomM cover artwork?** — **YES.** `identity_source/artwork.rs` fetches `path_cover_small` from the configured RomM instance, validates and resizes it, and stores it in a durable on-disk LRU cache (1 GiB cap). `url_cover` (IGDB / RetroAchievements public hosts) is parsed for provenance but **deliberately never fetched** (SSRF policy).

**2. Can it already display RomM covers in the GUI?** — **YES.** Two independent paths: (a) `gamer_artwork.rs` runs a background, priority-scheduled cover worker that draws a cover in each Gamer-View list row; (b) the Selected-game Details panel loads one full cover on explicit button press via `RommOperation`. Rows fall back to a labelled placeholder with a typed reason (`NoRommIdentity` / `NoArtwork` / `PublicOnly` / `Unavailable` / `Failed`).

**3. Does local RomM media-path support exist?** — **NO.** Covers are always fetched over HTTP from the RomM instance. There is no path to read RomM's own `resources/roms/.../cover/*.png` files directly off a locally-mounted RomM data directory. (`identity_source/path_map.rs` maps RomM *ROM* paths to local files, but only for identity/hash matching, not for media reuse.)

**4. Is there a previous branch/commit implementing it?** — **YES, long landed.**
- `9c9c95c` "RomM identity Stage 1D (part 1): bounded provider-owned artwork cache" → created `identity_source/artwork.rs`.
- `2e753aa` "Show RomM cover artwork in the Gamer View game list" → created `gamer_artwork.rs`.
- `594312a` "Add the artwork comparison audit and pin cover/platform precedence".
- **GitHub PR #5 "Show RomM cover artwork in Gamer View", MERGED 2026-08-05.**
All present in `feature/archivefs-unified-platform` today.

**5. If incomplete, what EXACT seam remains?** — The cover pipeline is complete. Genuinely-missing, RomM-media-adjacent seams (all small, all optional):
- **RomM screenshots** — never modelled or fetched. Seam: add a `screenshots` field to `ArtworkReference` in `romm/normalise.rs`, a `Screenshot*` fetch variant in `artwork.rs` (same SSRF + cache rules), and a viewer in `selected_game_panel.rs` / `game_presentation.rs`.
- **RomM manuals** — never modelled. Seam: `path_manual`/`url_manual` parse + a bounded PDF/HTML fetch-and-store, plus an "Open manual" action.
- **`path_cover_large`** — parsed but never fetched (only `_small`). Seam: a second cache tier fetched on the Details-panel button press.
- **Local RomM media-path reuse** — seam: a `RommMediaRoot` config + a "read local file instead of HTTP" branch in `artwork.rs`'s fetch, gated on a verified same-host path bind (mirroring `romm_projection::verified_same_path_bind`).

None of these is on any branch. None is currently assigned.

---

## 6. Recent duplicate-suggestion audit

| Feature | Status | Where implemented | Commit / branch / PR | What remains | Reassignment would be duplicate? |
|---|---|---|---|---|---|
| **Stale recovery archive / fix-it-here** | **DONE** | `crates/archivefs-core/src/dat/rename_apply/history.rs`; `repair_history_page.rs` + `dat_sources_page.rs` wiring | `decc035` (in HEAD). Superseded rival: `fix/stale-recovery-fixit-here` @ `37ca447` (`stale_archive.rs`, not landed) | Nothing. Optionally delete the superseded branch. | **YES — do not reassign.** |
| **Missing-library fix-it-here** | **DONE** | `crates/archivefs-gui/src/library_view.rs` (`confirmed_missing_catalogue_paths`, `show_missing_library_fixit_card`), `database.rs::remove_missing_archives` | `0420fc1` (in HEAD) | Nothing. | **YES — do not reassign.** |
| **DAT identity GUI** | **DONE**, concurrent polish in flight | `dat_identity_panel.rs`, `dat_sources_page.rs`, `selected_evidence_no_intro.rs`, `game_metadata.rs` | `34f016c` … `ddf1212`; active worktree `feature/dat-gui-vertical-slice` | Whatever the concurrent DAT session is still doing. | **YES — do not reassign; a session already owns it.** |
| **Generic non-cheat mods (apply + GUI)** | **ELSEWHERE — actively in progress** | `mod_package.rs` (inspect only, in HEAD); apply + `local_mod_package_page.rs` in worktree `codex/generic-noncheat-mod-workflow` (dirty) | branch `codex/generic-noncheat-mod-workflow` @ `decc035` + uncommitted | The concurrent session is building it. | **YES — a session already owns it.** |
| **Dolphin texture mods** | **DONE** | `patch_manager/dolphin_texture_mod.rs`, `dolphin_texture_pack.rs`, `dolphin_texture_mod_page.rs` | landed (PR #66 era) | Only non-goals (texture packs beyond single-PNG, Riivolution) — explicitly deferred. | **YES — do not reassign.** |
| **RomM artwork** | **DONE** (covers) | `identity_source/artwork.rs`, `gamer_artwork.rs`, `romm_game.rs` | `9c9c95c`, `2e753aa`, PR #5 (merged 2026-08-05) | Covers: nothing. Screenshots / manuals / `path_cover_large` / local-media-path: not started (optional). | **YES for "RomM covers".** A *screenshots/manuals* task would be new work, not a duplicate. |
| **AppImage launch** | **NOT STARTED** (investigation only) | — | `.claude/worktrees/standalone-emu-readiness` @ `b07d51c` — **plan/notes, 0 commits** | The whole implementation: an AppImage/managed-install native-binding path for PCSX2/PPSSPP/Dolphin (fed from `install.json`-provenanced managed installs or validated `~/Applications/*.AppImage`), + a planner "prefer ready standalone over RetroArch" rule, + tests. | **NO — genuine unstarted work.** |
| **Launch E2E** | **PARTIAL — landed this session, uncommitted** | `crates/archivefs-core/src/launch/tests.rs` (+ new `plan_to_spawn_tests.rs`), `crates/archivefs-gui/src/launch_readiness_page/tests.rs` | uncommitted in the authoritative worktree (this session's preceding task) | Commit it; then standalone-adapter (non-Dolphin/RetroArch) full-chain-to-spawn E2E and a GUI background-worker test remain. | **YES for the slice just done.** The remaining slices are new. |
| **RomM library creation / export** | **DONE** | `playing_library/romm_library_plan.rs` (`build_romm_library_plan`, `build_romm_library_apply_transactions`), `romm_projection.rs`, `playing_library_page.rs` | landed | Nothing structural. | **YES — do not reassign.** |
| **No-Intro auto-update** | **DONE** | `dat/updates.rs` (`check_managed_dat_update`, `update_managed_dat`, `rollback_managed_dat_to_previous`, `ManagedDatUpdatePolicy`), wired in `dat_sources_page.rs` | landed | Nothing. | **YES — do not reassign.** |
| **Playing Library / 1G1R** | **DONE** | `playing_library/` (full), `playing_library_page.rs` "Build Playing Library (1G1R)" | landed | Nothing structural. | **YES — do not reassign.** |

**Every "recent suggestion" except AppImage launch (and the not-yet-committed launch-E2E slice) would be duplicate work.**

---

## 7. True backlog — genuinely unfinished, genuinely useful

Excludes: the ~85 stale replay branches; docs drift; anything already landed; the two features a concurrent session already owns (generic mods, DAT-→-GUI polish); deliberate non-goals (Switch/PS5/Wii U/3DS parsers, Action-Replay decryption, mod downloader).

### P0

**P0-1 — Commit the launch-E2E test slice already produced this session.**
- **Missing seam:** the cross-module launch continuity + fallback-boundary tests exist as uncommitted changes in the authoritative worktree (`launch/tests.rs` +218, new `launch/plan_to_spawn_tests.rs`, `launch_readiness_page/tests.rs` +85). They are green but unpersisted; a `git checkout` or a bad rebase would lose them.
- **Evidence not already done:** `git status --short` shows them modified/untracked; no commit references `plan_to_spawn_tests`.
- **Modules/files:** exactly those three + one `#[cfg(test)] mod` line in `launch/mod.rs`.
- **Overlap risk:** none — no concurrent session is in `launch/`.
- **Parallel Codex:** no — trivial, do it in the owning session.
- **Task type:** PROMOTION (commit).

### P1

**P1-1 — Standalone-emulator AppImage / managed-install launch readiness.**
- **Missing seam:** `resolve_{pcsx2,ppsspp,dolphin,rpcs3,xemu}_native_launch_binding` accept only `InstallationType::Native` (a distro binary on `PATH`). An AppImage — including one EmuWiz's own download catalogue installs with `install.json` provenance — is discovered but classified `Portable`/`Explicit` and refused with `UnsupportedInstallationType`, so PS2/GC/Wii/PSP silently fall back to RetroArch-core ambiguity. Needs: (a) a trusted-executable channel from `diagnostics::profiles::discover_managed_emulator_installations` (`install.json`) and/or validated `~/Applications/<N>/<N>.AppImage`; (b) an AppImage native-binding arm per adapter that never guesses a path; (c) a `planning::apply_preference` rule that prefers a *ready* standalone over RetroArch cores; (d) the fail-closed test matrix.
- **Evidence not already done:** `ppsspp_local.rs:353` `if profile.installation_type != PpssppInstallationType::Native { return Err(UnsupportedInstallationType) }`; `pcsx2_local.rs:816` `Native => resolve_default_native_binding` (all others rejected); `.claude/worktrees/standalone-emu-readiness` has **0 commits**.
- **Modules/files:** `patch_manager/{pcsx2,ppsspp,dolphin,rpcs3,xemu}_local.rs`, `launch/planning.rs`, `launch/{pcsx2,ppsspp,dolphin}_execution.rs`, `diagnostics/profiles.rs` (read-only reuse), plus `launch_readiness_page.rs` request derivation.
- **Overlap risk:** low. No concurrent session touches `launch/` or `patch_manager/*_local`. Some proximity to emulator-download work but that is landed.
- **Parallel Codex:** yes (self-contained, well-scoped, has an investigation to build from).
- **Task type:** IMPLEMENTATION (from the existing plan-only investigation).

**P1-2 — Standalone-adapter full-chain launch E2E (the adapters Dolphin/RetroArch already prove).**
- **Missing seam:** only Dolphin and RetroArch have a test that runs `preflight_*` all the way to a produced `*Command` and spawns a fake executable. PPSSPP/PCSX2/DuckStation/RPCS3/xemu/Xenia `*_execution` fixtures deliberately stop at `BindingUnavailable` (their fake executables are `Explicit`, never `Native`), and their spawn tests use `hand_built_command`. So "profile discovery → native binding → real command → spawn" is untested for 6 of 8 standalone adapters.
- **Evidence not already done:** `ppsspp_execution/tests.rs::a_fully_valid_request_reaches_binding_resolution` asserts `BindingUnavailable`; `pcsx2_execution/tests.rs` likewise; spawn tests use `hand_built_command`.
- **Modules/files:** `launch/{ppsspp,pcsx2,duckstation,rpcs3,xemu,xenia}_execution/tests.rs`, needs a per-adapter fixture that yields a `Native` binding (e.g. fake `pcsx2-qt` on an injected `PATH`).
- **Overlap risk:** would conflict with P1-1 (both touch `*_local.rs` binding classification). **Do P1-1 first, then this, or the same session does both.**
- **Parallel Codex:** no (serialise with P1-1).
- **Task type:** IMPLEMENTATION (tests).

### P2

**P2-1 — RomM screenshots (and, secondarily, manuals).**
- **Missing seam:** `romm/normalise.rs` models only `url_cover` / `path_cover_{small,large}`. No screenshot or manual field, no fetch, no viewer. Add a `screenshots: Vec<String>` (RomM-instance paths only) to `ArtworkReference`, a `Screenshot` fetch variant in `identity_source/artwork.rs` reusing the exact SSRF + size-bomb + LRU rules, and a small gallery in `selected_game_panel.rs` / `game_presentation.rs`.
- **Evidence not already done:** `rg -i "screenshot|\bmanual\b"` over `identity_source/` and GUI returns nothing.
- **Overlap risk:** none.
- **Parallel Codex:** yes.
- **Task type:** IMPLEMENTATION.

**P2-2 — `path_cover_large` (Details-panel high-res cover).**
- **Missing seam:** only `path_cover_small` is ever fetched. The Details panel already loads "one cover on button press" — point that button at `path_cover_large` with a second cache tier.
- **Evidence not already done:** `artwork.rs` fetch path only references `path_cover_small`.
- **Overlap risk:** touches `artwork.rs` + `gamer_artwork.rs` / Details panel — small; would lightly overlap P2-1 (same file). Bundle P2-1 + P2-2.
- **Parallel Codex:** yes (bundled with P2-1).
- **Task type:** IMPLEMENTATION.

**P2-3 — Beginner-UX audit follow-through (`docs/BEGINNER_UX_AUDIT.md`).**
- **Missing seam:** the audit doc (preserved, not yet actioned) lists concrete open items — G1 "Open location" is a no-op; G5 launch-blocker copy still leaks planner/core/executable jargon; L2/L4/L5 raw paths + backend state + dead-end empty states in beginner views; H4 names a stale route. Each is a small, isolated GUI fix.
- **Evidence not already done:** the doc itself is the checklist; per-item status is UNKNOWN until re-checked against `ddf1212`.
- **Overlap risk:** medium — touches `gamer_view.rs`, `library_view.rs`, `home_page.rs`, `selected_game_panel.rs`, some of which the concurrent DAT-GUI session may also touch. Coordinate.
- **Parallel Codex:** no (coordinate with the DAT-GUI session).
- **Task type:** AUDIT first (triage the checklist against HEAD), then IMPLEMENTATION of the survivors.

### P3

**P3-1 — Local RomM media-path reuse.**
- **Missing seam:** covers are always HTTP-fetched. For a user who has RomM's data directory mounted locally, add a `RommMediaRoot` config and a "read the local file instead of HTTP" branch in `artwork.rs`, gated on a verified same-host path bind (mirror `romm_projection::verified_same_path_bind`).
- **Evidence not already done:** no `media_path` / `resources_path` / local-media handling anywhere.
- **Overlap risk:** low. Bundle with P2-1/P2-2 or do standalone.
- **Parallel Codex:** yes.
- **Task type:** RESEARCH (does the RomM API expose a stable on-disk layout worth binding to?) → IMPLEMENTATION.

**P3-2 — GUI background-worker launch test.**
- **Missing seam:** no test drives `RetroArchLaunchState` / `Pcsx2LaunchState`'s background `preflight_and_launch_*` worker from a plan produced by `build_launch_plan_from_results` (GUI launch tests hand-build the `LaunchPlan`).
- **Overlap risk:** conflicts with P0-1 / P1-2 file (`launch_readiness_page/tests.rs`). Serialise.
- **Parallel Codex:** no.
- **Task type:** IMPLEMENTATION (tests).

**P3-3 — Deliberate branch/worktree cleanup.**
- ~85 obsolete branches / ~80 obsolete worktrees (all net-negative vs `ddf1212`, 0 new files) plus the superseded `fix/stale-recovery-fixit-here` and `feature/non-cheat-mod-foundation`. Not a feature gap; a hygiene task.
- **Overlap risk:** must NOT touch the 3 live parallel worktrees or the `research/`/`design/`/`docs/preserved-emuwiz-research` reference branches.
- **Parallel Codex:** no (destructive; needs explicit owner sign-off).
- **Task type:** AUDIT → cleanup (out of scope for parallel feature agents).

---

## 8. Summary

| | |
|---|---|
| **Authoritative HEAD inspected** | `ddf121276ea7130a4c6a25248e3c20619c4c2fc7` (`feature/archivefs-unified-platform`) |
| **Local branches inspected** | 130 (≈85 confirmed obsolete/superseded; ≈40 reference/replay; 3 live parallel; 1 authoritative) |
| **Local worktrees inspected** | 94 (3 live: authoritative + DAT-GUI + generic-mods; 1 superseded stale-recovery; 2 spent `.claude` worktrees; ~87 obsolete; 1 prunable) |
| **GitHub branches / PRs reviewed** | `kiehntre/emuwiz` (== `kiehntre/archivefs`): all remote branches + PRs #1–#66 + 0 issues. All feature PRs MERGED. 1 draft (#30, AR-licensing docs). `origin` is ~600 commits behind local. No unique remote work. |
| **Unique unpromoted implementations found** | **1 (needs commit): the launch-E2E test slice** already produced this session, uncommitted in the authoritative worktree. Everything else classified "elsewhere" is a concurrent session's *in-progress* work, not something to promote. |
| **Duplicate task suggestions identified** | **9 of 11** checked: stale-recovery archive, missing-library fix-it-here, DAT identity GUI, generic non-cheat mods, Dolphin texture mods, RomM artwork (covers), RomM library creation/export, No-Intro auto-update, Playing Library/1G1R — all already done or already owned by a running session. Only **AppImage launch** and the **remaining launch-E2E slices** are genuine new work. |
| **True backlog count** | **10 items** — P0: 1 · P1: 2 · P2: 3 · P3: 4. Only P1-1 (AppImage launch) is a substantial, unstarted, parallel-safe feature. |
| **`git status --short` (audit end)** | `M launch/mod.rs`, `M launch/tests.rs`, `M launch_readiness_page/tests.rs`, `?? launch/plan_to_spawn_tests.rs` (this session's uncommitted launch-E2E work), plus untracked `amiga c64 gb gba gbc n64 nes ps2 saturn static` (pre-existing scratch). No tracked file was modified by this audit; only `EMUWIZ_FULL_DUPLICATION_AUDIT.md` was created. |

### One-line guidance for the frustrated user

> Almost everything being re-suggested is **already built** (RomM covers since Aug 5; stale-recovery archive, missing-library fix-it, DAT identity GUI, No-Intro auto-update, 1G1R/Playing Library, RomM export all landed) **or already being built right now** by a parallel session (generic non-cheat mods; DAT-→-GUI polish). The only genuinely unstarted, worthwhile, parallel-safe feature is **standalone-emulator AppImage launch readiness** (P1-1). Also: commit the launch-E2E tests already sitting uncommitted in the tree (P0-1).

EMUWIZ FULL DUPLICATION AUDIT COMPLETE
