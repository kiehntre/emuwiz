# Cheats & Mods: V1 discovery-journey audit

> **Audit snapshot** — this document records what exists in the repository at
> the time it was written (`feature/archivefs-unified-platform`, see git log
> for the exact commit at write time). It is read-only research: no source
> file was changed to produce it. Where later work supersedes a claim here,
> trust the code and update this document rather than the reverse.

This audit exists because a substantial amount of Cheats & Mods
infrastructure already shipped, incrementally, across many small reviewed
milestones (25+ existing design docs under `docs/`, several thousand lines
of `patch_manager` code, and a mature GUI test suite). **The task is not to
build Cheats & Mods from scratch** — it is to identify the specific,
bounded gaps between what already works end-to-end and the ten-step novice
journey stated in the brief, then sequence closing them.

---

## 1. Executive summary

EmuWiz's Cheats & Mods system is **far along, not "partial-to-missing."**
Three complete emulator-specific journeys already run end-to-end with real
identity matching, preview, explicit confirmation, atomic apply, journalled
rollback, and honest failure states:

- **RetroArch cheats** (`.cht`) — trusted-source download → local catalogue
  → candidate ranking → per-cheat selection → install → history → rollback.
- **PCSX2 PNACH patches** — GameHacking.org PS2 catalogue (serial+CRC
  matched) → merge into a managed PNACH block → shared apply/rollback.
- **Dolphin Gecko/Action Replay codes** — three converging providers
  (Dolphin's own upstream GameSettings, GameHacking.org GameCube/Wii,
  BSFree GameCube/Wii) → strict game-ID+revision matching → enable/disable
  inside the real `GameSettings/<GameID>.ini` → shared apply/rollback.
- **Xenia Canary patches** (`.patch.toml`) — Title ID/Media ID/module-hash
  matched → merge → shared apply/rollback, wired into `main.rs`.

Genuinely **missing or stubbed for V1**:

- **General mod installation** beyond one narrow Dolphin texture slice
  (single PNG, or an explicit multi-file JSON manifest — no archives, no
  catalogue, no PCSX2/Riivolution/widescreen-patch equivalent at all). The
  "Mods" section of the GUI is, today, a compact **"Planned" banner** for
  every adapter except that one Dolphin path — this is documented and
  intentional (`docs/CHEATS_MODS_FUNCTIONAL_REPAIR.md`), not an oversight.
- **User-supplied/local cheat file import has no install path.**
  `user_cheat_import.rs`/`user_cheat_import_page.rs` are explicitly a
  read-only *index* — provenance and match evidence only, never an apply
  button.
- **GameHacking.org GameCube is preview-only** (no install), unlike its PS2
  and Wii siblings.
- **GameHacking.org is currently Cloudflare-blocked** in this environment
  for automated requests on both PS2 and GameCube endpoints; the
  browser-assisted manual-paste fallback exists and is real, but is not a
  substitute for automatic discovery.
- **No generalized `EmulatorAdapter` trait** — every emulator's
  provider→install pipeline is its own hand-built module family. This is a
  known, explicit, reviewed decision (`adapter.rs`, `matching.rs`), not a
  bug, and the recommendation below is to keep extending it that way for
  V1 rather than generalize first.

Nothing here found a safety defect. Preview, explicit confirmation,
backup/journal, rollback, symlink/traversal defenses, and idempotency are
handled by one shared, heavily-tested pipeline (`shared_preview.rs`,
`shared_transaction.rs`, `destination_safety.rs`) that every emulator-
specific installer reuses rather than reimplements.

---

## 2. Current end-to-end journey (mapped against the brief's 11 steps)

| # | Step | Status | Evidence |
|---|---|---|---|
| 1 | User selects a game | **DONE** | Existing library selection/context seam (`selected_game_panel.rs`, `cheats_mods_context_*` tests) |
| 2 | EmuWiz identifies it with strong evidence | **DONE** (per adapter) | PCSX2: `pcsx2_identity.rs` (`Pcsx2IdentityState::{Verified,MissingCrc,Deferred,Ambiguous}`). Dolphin: `dolphin_local::match_dolphin_inventory` (exact GameID+revision). Xenia: Title ID/Media ID/module-hash (`xenia_install_plan.rs`). RetroArch: `cheat_candidates.rs`'s 6-tier evidence model |
| 3 | User opens Cheats & Mods | **DONE** | `crates/archivefs-gui/src/cheats_mods_preview.rs`, `navigation.rs`; reachable from Gamer View, Library, and directly (`cheats_mods_is_a_primary_active_navigation_destination` test) |
| 4 | EmuWiz shows relevant available cheats/mods/patches | **DONE for cheats/patches; MISSING for general mods** | Per-adapter candidate lists (`cheat_candidates.rs`, `gamehacking_*_provider.rs`, `xenia_install_plan.rs`) render real cards; `show_mods_section` renders only a banner |
| 5 | Each result explains source/identity/region/format/target/freshness | **DONE** | `cheats_mods_preview.rs`'s `preview_state_human_label`/`proposed_action_human_label`; `CheatProviderProvenance{source,maintainer,origin,distribution_status,verification}`; `cheat_source_labels_cover_every_freshness_and_status` test |
| 6 | Unsafe/ambiguous results blocked or warned | **DONE** | `PreviewState::{Conflict,Ambiguous,NotEligible,Unsupported,UnsafeDestination,...}`; `cheat_candidates.rs`'s ambiguous/cross-platform/unsupported classes are never installable |
| 7 | User previews exact changes | **DONE** | `shared_preview.rs` (`SharedPreviewReport`, bounded hashing, `PREVIEW_MAX_*` limits) |
| 8 | User explicitly confirms | **DONE** | `SharedApplyOptions`/confirmation gating in `shared_transaction.rs`; "Nothing installs...until you explicitly confirm it" (`docs/CHEATS_MODS_USER_POLICY.md`) |
| 9 | EmuWiz applies safely | **DONE** | `execute_shared_apply` — atomic write, verification, journal (per-adapter installers: `cheat_installer.rs`, `pcsx2_install_plan.rs`, `dolphin_gecko_install_plan.rs`/`gamehacking_gamecube_install_plan.rs`, `xenia_install_plan.rs`) |
| 10 | User can undo/recover | **DONE** | `cheat_rollback.rs`, `execute_shared_rollback`, `cheat_history.rs`; GUI: `successful_undo_clears_the_matching_installed_state`, `undo_from_gamer_view_actually_switches_to_the_review_screen` |
| 11 | Offline/provider-failure states remain understandable | **DONE** | `beginner_view_shows_neutral_state_for_missing_upstream_game_without_retry`, `wii_cloudflare_state_offers_offline_import_without_retry`, `sources_retained_snapshot_stays_visibly_usable_after_update_failure` |

**The only structural hole in the 11-step journey is step 4 for the "mods"
half of "Cheats & Mods."** Everything else is a real, tested pipeline for
at least one emulator per format family.

---

## 3. Capability matrix

### A. Discovery

| Capability | Status | Evidence |
|---|---|---|
| Automatic provider discovery (RetroArch) | DONE | `cheat_sources.rs` — HTTPS-only, hashed, atomically published immutable snapshots |
| Automatic provider discovery (PCSX2) | DONE | `patch_manager::mod.rs` `BUILT_IN_SOURCE_URL`/`BUILT_IN_SOURCE_ID` (compiled-in, reviewed endpoint) |
| Automatic provider discovery (Dolphin) | DONE (two paths) | `dolphin_gecko_provider.rs` (per-game fetch) + `dolphin_cheat_catalogue.rs` (bulk pinned-commit archive, offline-searchable) |
| Automatic provider discovery (Xenia) | DONE | `xenia_provider.rs` — GitHub Git Trees API index, per-file immutable cache by (commit, path) |
| Automatic provider discovery (GameHacking.org PS2/Wii) | PARTIAL | Real HTTP crawler exists (`gamehacking_provider.rs`, `gamehacking_wii_provider.rs`) but is **currently Cloudflare-blocked** in this environment (403 on every request) |
| Automatic provider discovery (GameHacking.org GameCube) | PARTIAL | Crawler exists (`gamehacking_gamecube_provider.rs`) but is preview-only (no install) **and** blocked, and its numeric `system_id` for per-game fetch is still unconfirmed |
| Automatic provider discovery (BSFree) | DONE | `bsfree.rs` — immutable third-party SQLite, read-only |
| Automatic provider discovery (CheatBase) | DONE (browse-only) | `cheatbase.rs` — immutable pinned SQLite, no install/conversion API by design |
| Local files/packages import | PARTIAL | `user_cheat_import.rs` indexes RetroArch/PCSX2 files with provenance+match evidence, but **never installs** |
| Browser-assisted import | DONE, but narrow-by-design | `gamehacking_browser_import.rs` — opens the real page via `xdg-open` in the user's own browser; user manually saves/pastes bytes. Explicitly **not** automation: no fingerprint spoofing, no cookies/sessions read, no headless browser, no CAPTCHA solving |
| Provider search | DONE (per-game, not full-text) | Each provider matches one selected game's identity against its own catalogue; no cross-provider free-text search UI found |
| Provider health/failure handling | DONE | `CheatProviderSourceState::{Downloading,Validating,Ready,UpdateAvailable,Invalid,UnsupportedSchema,DownloadFailed,ValidationFailed,Disabled}`; `GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE` distinguishes a challenge from an ordinary failure |
| Offline behavior | DONE | Retained snapshots stay usable after an update failure (`sources_retained_snapshot_stays_visibly_usable_after_update_failure`); Cloudflare-blocked Wii path still offers manual import without a dead retry loop |
| Cached/snapshotted results | DONE | `cheat_cache_lock.rs` (cross-process advisory lock) + `cheat_cache_maintenance.rs` (2700 lines: inventory, pinning, pruning with pre-delete revalidation) |

**Notable design-intent quotes:**
> "This module is the deliberately *unclever* answer to that: the person
> opens the exact game page in their own ordinary browser... and hands the
> resulting bytes to ArchiveFS." — `gamehacking_browser_import.rs`

> "This milestone is preview-only: there is no install/apply path here at
> all, unlike the PS2 provider." — `gamehacking_gamecube_provider.rs`

### B. Identity / compatibility

| Field | Used by | Evidence |
|---|---|---|
| Platform | All adapters | Cross-platform candidates are explicitly listed but never installable (`cheat_candidates.rs`) |
| Title/game identity | All adapters | Normalized-title tier in `cheat_candidates.rs`; `CatalogueGameEvidence` shared with PCSX2 matching |
| Region | RetroArch, Dolphin, Xenia | Part of the "verified exact" tier (`cheat_candidates.rs` doc comment: "normalized title + canonical platform + region") |
| Version/revision | Dolphin | `dolphin_gecko_provider::revision_applicability`, `GeckoApplicabilityDecision` — rules out wrong region/revision explicitly |
| Serial/Title-ID/Game-ID | PCSX2, Dolphin, Xenia | `Pcsx2GameIdentity`, Dolphin `GameID`, Xenia Title ID |
| Executable CRC | PCSX2 | `normalize_crc`, `Pcsx2IdentityState::MissingCrc` as its own honest state (never silently downgraded to a title match) |
| Module hash | Xenia | `xenia_install_plan.rs` — module-hash is part of the strict match tuple alongside Title ID/Media ID |
| Ambiguity handling | All adapters | Never auto-resolved: `CheatJourneyIdentityState::Conflicting`, `Pcsx2IdentityState::Ambiguous`, `PreviewState::Ambiguous`, `cheat_candidates.rs`'s explicit "two or more sharing the top score" rule |
| Unsupported content | All adapters | `PreviewState::Unsupported`; cross-emulator-targeted records are shown for transparency, never installable |

**Notable design-intent quote:**
> "A **verified exact** candidate...may be selected automatically, but only
> when it is the single best candidate. A **strong** candidate is shown as
> the recommended choice and is installable, but never auto-selected."
> — `cheat_candidates.rs`

### C. Cheat types

| Format | Parser | Malformed-input handling | Install wired end-to-end? |
|---|---|---|---|
| RetroArch `.cht` | `cht_document.rs` (full-fidelity) + two lighter metadata-only readers for indexing | "Never panics on catalogue input... no slicing on a non-char boundary" | **Yes** — `cheat_installer.rs` |
| PCSX2 PNACH | `pcsx2_pnach.rs` | Explicit error kinds: `TooLarge`, `InvalidUtf8`, `MalformedManagedBlock`, `DuplicateManagedBlock`, `InvalidPatchLine`, `TooManyManagedBlocks` | **Yes** — `pcsx2_install_plan.rs`, lossless preservation of existing file content outside EmuWiz's managed block |
| Dolphin Gecko / Action Replay | `gecko_document.rs` | Per-line attribution to a code, a `*Note`, or a malformed-line warning — never a parse abort | **Yes** — surgical in-place `[Gecko]`/`[ActionReplay]` section edit (`replace_gecko_enabled_section`), preserving `[Core]`/`[Video_*]` and everything else in the same shared `.ini` byte-for-byte |
| GameHacking.org (PS2) | Feeds PCSX2 PNACH path | Same as PCSX2 | **Yes** |
| GameHacking.org (GameCube) | Feeds Gecko/AR classifier | Same as Dolphin | **No** — preview-only |
| GameHacking.org (Wii) | Feeds Gecko/AR classifier | Same as Dolphin, plus `WiiCheatSafety::UnverifiedFormatLabel` | **Yes** |
| BSFree (GameCube/Wii) | Reuses `dolphin_code.rs`'s shared AR/Gecko line decoder | Only a proven-safe subset is ever offered (no master/zero/self-modifying codes, no placeholders) — everything else is preview-only forever, "not merely 'not yet supported'" | **Yes**, for the proven subset only |
| Xenia `.patch.toml` | `xenia_patch_document.rs` (mirrors Xenia Canary's own upstream `PatchDB` schema) | Bounded (`MAX_PATCH_FILE_BYTES`/`MAX_PATCHES_PER_FILE`/`MAX_WRITES_PER_PATCH`); strict TOML-schema validation, not a general TOML interpreter | **Yes** |
| User-supplied RetroArch/PCSX2 files | `user_cheat_import.rs` | Bounded, reuses existing parsers | **No** — index/review only |

### D. Mod types

- **Real, working, non-cheat mod installation exists for exactly one case:**
  Dolphin texture replacement, via `dolphin_texture_mod.rs` (single PNG,
  verified GameID-scoped destination) and `dolphin_texture_pack.rs` (an
  explicit, fully-enumerated multi-file JSON manifest — sizes and SHA-256
  digests included by the caller, never inferred from a folder/archive
  name). Both reuse the shared preview/apply/rollback pipeline unchanged.
  Both are deliberately **not** archive importers: "This module does not,
  and must not grow to: unpack ZIP/RAR/7z texture packs, import a whole
  directory, or read a pack manifest [beyond the explicit JSON contract]."
- **Everything else labeled "mod" in the product today is a relabeled
  cheat/patch**, not a distinct mod workflow: PCSX2 widescreen patches and
  Dolphin's own Riivolution content have no adapter at all yet.
- **The GUI's general "Mods" section is an explicit placeholder.** Per
  `docs/CHEATS_MODS_FUNCTIONAL_REPAIR.md`, `show_mods_section` renders a
  single `MODS_UNAVAILABLE_BODY` "Planned" banner for every adapter except
  the Dolphin texture path, and a dedicated test
  (`mods_section_has_no_fake_user_actions`) proves no Install/Browse/
  Download/Apply/Remove button is ever shown where nothing real backs it.
- **Dependency/conflict handling**: `dolphin_dedup.rs` provides real,
  content-digest-keyed (never display-name-keyed) duplicate/conflict
  detection, but only within the Dolphin Gecko/AR cheat family it already
  serves (BSFree GameCube, BSFree Wii, GameHacking Wii) — there is no
  equivalent for the one real mod workflow (texture files don't conflict
  the same way codes do) and no cross-mod dependency graph of any kind.
- **A research document already exists** (`docs/MOD_SOURCES_AND_SAFETY_RESEARCH.md`)
  proposing the shape of a real mod-provider system (verified catalogue,
  shared preview/apply reuse, executable installers "never executed —
  surfaced instead as a deep link"), explicitly marked "no provider
  integration is authorized by it."

### E. Safety

| Capability | Status | Evidence |
|---|---|---|
| Preview | DONE | `shared_preview.rs` — bounded hashing (`PREVIEW_MAX_BYTES_PER_FILE`=1MiB, `PREVIEW_MAX_TOTAL_BYTES_HASHED`=32MiB, `PREVIEW_MAX_ENTRIES`=512) |
| Explicit confirmation | DONE | Required at the `shared_transaction.rs` apply boundary; never implied by preview alone |
| Backups | DONE | Every real installer backs up the pre-existing destination before writing (`shared_transaction.rs`) |
| Transaction journal | DONE | `CHEAT_ROLLBACK_RUNS_DIRECTORY_NAME`, versioned schemas (`CHEAT_ROLLBACK_RUN_SCHEMA_VERSION`, `SHARED_APPLY_SCHEMA_VERSION`) |
| Rollback | DONE | `cheat_rollback.rs` — explicit `CheatRollbackOutcome` per entry (`RemovedInstalledFile`, `RestoredBackup`, `AlreadyRestored`, `FailedBackupChanged`, `FailedUnsafeDestination`, ...); fault-injection test hooks (`FaultPoint::{TempWrite,Verification,Rename,Removal,JournalWrite}`) prove partial-failure recovery is actually tested |
| Malformed input | DONE | Every format parser is documented panic-free with bounded reads; `cht_document.rs`: "Never panics on catalogue input" |
| Archive/path traversal | DONE | `destination_safety.rs` — "deliberately rejects every symlink used as a destination root, parent directory, or final destination, including symlinks whose targets remain beneath the validated root" |
| Symlink handling | DONE | Same module; `cheat_history.rs` independently never follows a final symlink when reading attacker-controlled journal data |
| Overwrite behavior | DONE | `PreviewState::ReplaceDifferent`/`Conflict` distinguish "safe to replace" from "needs a decision"; Dolphin texture mod has its own explicit `DolphinTextureModPlan::Conflict` |
| Idempotency | DONE | `CheatRollbackOutcome::{AlreadyRestored,NoChangeRequired}`; `offline_cached_snapshot_reuse_applies_and_is_recorded` test; GameCube install plan's dedicated "Idempotency and conflicts" design section |
| TOCTOU awareness | DONE (documented limit, not solved) | `destination_safety.rs`: "cannot eliminate time-of-check/time-of-use races. A future write-capable caller must revalidate immediately before writing" — and `cheat_installer.rs` confirms it actually does this via `assess_destination` immediately before every write |

**No safety gap was found in this audit.** The one honestly-stated residual
risk (TOCTOU) is explicitly documented and mitigated by immediate
revalidation at every real write site inspected.

### F. User experience

- **What renders today**: per-adapter candidate cards with human-readable
  state labels (never raw enum `Debug` output — `proposed_action_is_a_sentence_not_debug_formatted_enum_output`
  test exists specifically to guard this), identity/region/version
  evidence, freshness/provenance, and a dedicated Cheat Sources page
  (`cheat_sources_page.rs`) surfacing all ~9 registered providers with
  per-platform priority/override state that previously required hand-
  editing a TOML file.
- **What exists but may be under-surfaced**: `CheatProviderLicence`/
  `CheatProviderLicenceStatus` (`Established`/`NotEstablished`/`Unknown`)
  is modeled in `cheat_provider.rs` but this audit did not confirm every
  provider's licence status is rendered on its own card (worth a follow-up
  read, not re-litigated here).
- **Dead-end/incomplete actions**: none found that lack an explicit,
  honest label — the one place a button could have been a dead end (the
  general Mods section) is instead a labeled "Planned" banner with a test
  proving no fake action button exists there.
- **Where a novice would get stuck today**: attempting to install a PCSX2
  widescreen patch, a Dolphin Riivolution mod, or any non-texture mod —
  the UI correctly tells them this isn't available yet rather than
  offering a broken flow, but there is genuinely nothing to click through
  to a working outcome.
- **User-supplied cheat files**: a beginner following "download a cheat
  file from a forum, then import it" will reach a real review screen
  (`user_cheat_import_page.rs`) but then have no confirm/install button —
  this is the one place the current UX could read as a dead end rather
  than an honest "not yet."

### G. Providers (repository evidence only)

| Provider | Supplies | Transport | Auth | Rate limits | Provenance stored | Failure mode | Licensing note in repo |
|---|---|---|---|---|---|---|---|
| RetroArch cheat catalogue (libretro DB) | `.cht` files | HTTPS zip download, hashed | None | Not found in code (single bounded download, not polling) | `CheatSourceManifest`/schema-versioned metadata | `CheatSourceError`, retained-snapshot fallback | Not located in this pass |
| PCSX2 official patches tree | PNACH | Compiled-in `BUILT_IN_SOURCE_URL` (HTTPS) | None | Not found | `CheatProviderProvenance` | Standard `ProviderResponse`/error path | Not located in this pass |
| Dolphin upstream GameSettings | Gecko/AR `.ini` per game | HTTPS, single-game fetch | None | `GECKO_PROVIDER_MAX_RESPONSE_BYTES` bounds one response, no explicit rate limiter found | `DOLPHIN_UPSTREAM_LICENSE = "GPL-2.0-or-later"`, `DOLPHIN_UPSTREAM_ATTRIBUTION` | Bounded error, no crash | **Yes** — GPL-2.0-or-later, explicit attribution constant |
| Dolphin upstream catalogue (bulk) | Same, pre-indexed | HTTPS, one pinned-commit archive | None | Single download per refresh | Same GPL attribution | Same | Same as above |
| GameHacking.org (PS2/GameCube/Wii) | Exported cheat codes | HTTP scrape (HTML parsing via `scraper` crate) + manual export endpoint | None (public site) | `CLOUDFLARE_COOLDOWN = 15 min` after a detected challenge; `MAX_RETRIES = 3` | Provider ID `"gamehacking.org"`, per-platform | `GameHackingErrorKind::CloudflareBlocked`/`UnsupportedSystem`; **currently blocked in this sandbox for all requests** | Custom, honest `USER_AGENT` naming the project; no scraping-ToS discussion found in code |
| GameHacking.org browser-assisted | Same content, manual | None (user's own browser via `xdg-open`) | N/A | N/A | Provenance record explicitly has no field for cookies/session | User-driven; a challenge page is still rejected on import | N/A |
| BSFree Archive | SQLite cheat DB | HTTPS download of a pinned artifact, hash-verified | None | Not found (single download) | `CheatProviderProvenance`, upstream attributed to Andrew Mackrodt | `CheatSourceError` | "The upstream database remains an immutable third-party artifact" — read-only by policy |
| CheatBase | SQLite cheat DB | HTTPS, pinned commit + expected SHA-256 (`CHEATBASE_EXPECTED_SHA256`) | None | Not found | `CHEATBASE_UPSTREAM_PROJECT` (GitHub URL), pinned commit | `ProviderValidationStatus` | Immutable, browse-only by design; no conversion/install API |
| Xenia Canary game-patches | `.patch.toml` | GitHub Git Trees API (index) + `raw.githubusercontent.com` (content), per-(commit,path) immutable cache | None (public repo) | Not found beyond the two-step index-then-fetch design minimizing requests | Commit + path keyed cache | Standard error path | Not located in this pass |
| User-supplied local files | RetroArch/PCSX2 format files | None (local disk only) | N/A | N/A | `UserCheatDiagnostic`/`UserCheatMatchState` | Bounded read errors | N/A — user's own files |

**Do not treat the "Not located in this pass" cells as "absent."** They
mean this audit did not find an explicit rate-limit/licence constant for
that provider in the files inspected — a targeted follow-up read (not a
new provider integration) would confirm either way before V1 sign-off.

---

## 4. Existing provider matrix

See table in §3.G above — this is the canonical version; do not duplicate
it elsewhere in future docs.

---

## 5. Safety/recovery matrix

See table in §3.E above.

---

## 6. Real mods vs. cheats/patches distinction

| Label used in product | What it actually is | Real mod or relabeled cheat/patch? |
|---|---|---|
| Dolphin texture mod (single PNG) | Genuine non-cheat asset replacement, verified-GameID-scoped, shared apply/rollback | **Real mod** |
| Dolphin texture pack (JSON manifest) | Same mechanism, multiple files, explicit manifest | **Real mod** |
| PCSX2 "widescreen patch" (as commonly understood upstream) | Not present as an adapter at all yet | **N/A — not implemented** |
| Dolphin Riivolution | Not present as an adapter at all yet | **N/A — not implemented** |
| Everything else under "Cheats & Mods" | RetroArch `.cht`, PCSX2 PNACH, Dolphin Gecko/AR, Xenia `.patch.toml` | **Cheat/patch**, correctly labeled as such in the UI, never mis-called a "mod" |

**Conclusion**: EmuWiz does not currently mislabel any cheat/patch as a
mod. The "Mods" section is honest about having almost nothing behind it
yet, which is the correct interim state — the gap is real functionality,
not mislabeling.

---

## 7. Exact V1 gaps

Ranked by how directly each blocks the stated novice journey:

1. **No install path for user-supplied/local cheat files.**
   `user_cheat_import.rs` + `user_cheat_import_page.rs` stop at review.
   Closing this reuses `cheat_installer.rs`'s existing write path — it is
   an install-plan/UI-wiring gap, not a new safety mechanism.
2. **GameHacking.org GameCube has no install path** (PS2 and Wii do). The
   classifier/matching work already exists
   (`gamehacking_gamecube_provider.rs`); what's missing is an install
   plan analogous to `gamehacking_gamecube_install_plan.rs`'s own sibling
   for Wii, or confirming that sibling already covers GameCube (needs a
   direct read to settle, not assumed here).
3. **GameHacking.org is provider-blocked in this environment** for
   automated PS2/GameCube/Wii requests. This may be sandbox-specific
   rather than universal — needs verification from a real network
   environment before being treated as a permanent constraint. The
   browser-assisted fallback already covers the "provider unreachable"
   case honestly.
4. **General mod support does not exist beyond Dolphin textures.** This is
   the single largest genuine feature gap and the one explicitly deferred
   by `docs/CHEATS_MODS_FUNCTIONAL_REPAIR.md` and scoped by
   `docs/MOD_SOURCES_AND_SAFETY_RESEARCH.md`. Whether this is in scope for
   "V1" depends on product definition of V1 — see §10.
5. **Provider licensing/rate-limit facts for RetroArch's libretro DB,
   PCSX2's official tree, GameHacking.org, and Xenia's `game-patches` repo
   were not located in the files this audit read.** This is a
   documentation-completeness gap, not necessarily a code gap — worth a
   short, targeted verification pass before any public V1 claim about
   provider licensing.
6. **Provider-licence UI surfacing** (`CheatProviderLicence` →
   is it shown per-card everywhere?) was not fully confirmed either way.

---

## 8. Recommended implementation order

Sliced for independent, non-overlapping hand-off to separate coding-agent
tasks. Each is scoped to *finish an existing seam*, per the brief's
priority — none proposes a new abstraction.

**A. `user-cheat-import-install-bridge`** (small)
Wire `user_cheat_import.rs`'s existing `UserCheatCandidate`/match evidence
into the existing `cheat_installer.rs`/`shared_preview.rs` pipeline for the
RetroArch case specifically (the format that pipeline already installs).
Non-goals: PCSX2 local-file install (separate task, below), any new parser.

**B. `user-cheat-import-pcsx2-bridge`** (small)
Same bridge for the PCSX2 PNACH case, reusing `pcsx2_install_plan.rs`.
Kept separate from A because the two installers have different destination
rules and this keeps each task's review small.

**C. `gamehacking-gamecube-install-parity`** (small–medium)
Confirm whether `gamehacking_gamecube_install_plan.rs` genuinely does not
exist (this audit read the *provider* file's own doc comment claiming
preview-only; a direct listing of `gamehacking_gamecube_install_plan.rs`
would confirm), and if it truly doesn't, build it as a thin wrapper mirror
of the Wii install plan, reusing `stage_gamecube_gamehacking_install`
exactly as `bsfree_gamecube.rs` already does.

**D. `gamehacking-provider-health-verification`** (tiny, research-only)
Re-run the existing crawler against the real `gamehacking.org` from a
network environment without the sandbox's apparent block, to confirm
whether the Cloudflare-blocked state is environment-specific or a real,
ongoing upstream posture EmuWiz must design around permanently. No code
change; informs whether task E below is urgent.

**E. `provider-licence-and-rate-limit-audit`** (tiny, research-only)
Grep/read pass to fill the "Not located in this pass" cells in §3.G for
RetroArch's libretro DB, the PCSX2 official tree, GameHacking.org, and
Xenia's `game-patches` repo. Produces a documentation update, not code.

**F. `cheat-provider-licence-ui-audit`** (tiny)
Confirm whether `CheatProviderLicence`/`CheatProviderLicenceStatus` is
rendered on every provider card in `cheat_sources_page.rs`; if not, wire
it — this is presentation-only, reusing an existing modeled field.

**G. `mod-catalogue-foundation`** (large, deferred — see §9)
The first real step toward general mod support per
`docs/MOD_SOURCES_AND_SAFETY_RESEARCH.md`'s own recommended shape: a
verified mod-source list mirroring `CheatSourceList`, for exactly one
well-scoped format (e.g., PCSX2 widescreen patches, since PCSX2 already
has the most mature identity-matching and install-plan precedent to
mirror). Only attempt after A–F land, and only as a separate, explicitly
re-reviewed design (per that research doc's own closing caveat).

---

## 9. Explicit post-V1 deferrals

- Riivolution support (Dolphin).
- PCSX2 widescreen/texture-patch mod adapter (unless promoted into V1 via
  task G above by product decision).
- Any archive-format (ZIP/RAR/7z) mod or texture-pack ingestion — both
  existing mod modules explicitly refuse to grow this way.
- A generalized `EmulatorAdapter` trait covering RetroArch/Dolphin/Xenia —
  `adapter.rs`/`matching.rs` are explicit that this needs its own
  separately-reviewed generalization effort, not a byproduct of a V1
  feature task.
- Full-text/cross-provider cheat search (today's discovery is exclusively
  per-selected-game).
- Executing any downloaded installer binary for a mod — explicitly ruled
  out in `docs/MOD_SOURCES_AND_SAFETY_RESEARCH.md`'s own recommendation
  ("never executed — surfaced instead as a deep link").

---

## 10. Definition of Done for Cheats & Mods V1

V1 is done when, for **at least the four already-working cheat/patch
families** (RetroArch, PCSX2, Dolphin, Xenia):

1. Every discovery path in §3.A that is marked DONE remains DONE with no
   regression (covered by the existing GUI test suite in
   `cheats_mods_workflows.rs`).
2. User-supplied local cheat files (RetroArch and PCSX2 shapes) can be
   previewed **and installed**, not merely indexed (tasks A+B).
3. GameHacking.org GameCube reaches install parity with its PS2/Wii
   siblings, or an explicit, reviewed decision states why it should not
   (task C).
4. Provider licensing and rate-limit facts are documented for every
   provider in §3.G, with no "not located" cells remaining (task E).
5. No safety regression against §3.E's matrix — every item there must
   still read DONE.
6. The general "Mods" section either (a) still honestly shows "Planned"
   with no fake actions, or (b) has shipped task G's first real mod format
   under the same safety bar as cheats — a deliberate product decision,
   not a default.

**General mod support (task G) is explicitly optional for this Definition
of Done** unless product direction says otherwise — the brief's own novice
journey is satisfiable today for cheats/patches, and closing tasks A–F is
sufficient to call the *existing, already-substantial* system V1-complete
without inventing new abstractions.
