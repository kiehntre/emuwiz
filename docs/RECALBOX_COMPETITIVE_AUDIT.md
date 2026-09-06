# Recalbox Competitive Audit for EmuWiz

Research only. No production Rust, tests, or Cargo runs were touched to
produce this document. Authoritative HEAD inspected:
`7c5f2f242f2b2754d69ab7e26ca83cc09a9d5053`
(`docs(launch): audit openMSX standalone adapter`) on
`feature/archivefs-unified-platform`.

All Recalbox claims below are sourced from Recalbox's own public site/wiki/
GitHub, retrieved **2026-09-06**, and are quoted or paraphrased with a URL.
Anything from a forum thread or third-party blog is explicitly labelled
"community source" and treated as corroborating, not authoritative. Where a
page could not be fetched in full (several current `wiki.recalbox.com` /
`wiki-next.recalbox.com` pages are JS-rendered and returned no static body to
this session's fetch tool), the claim is sourced from the site's own search
snippet/summary instead and marked as such — nothing below is invented.

## Executive summary

Recalbox is a turnkey **appliance OS**: it owns the boot sequence, the
session, the network file server, and a curated 100+/144-system emulator
matrix with an automatic "pick the best core" default. That ownership is
exactly what makes it easy for a beginner and exactly what EmuWiz — a
Linux-first, local-first *orchestrator* over a user's existing ROM library,
DAT verification, RomM, ES-DE, and installed emulators — cannot and should
not copy wholesale. The useful transfer is narrower than "be more like
Recalbox": it is a small set of **presentation and sequencing** ideas
(a real first-run wizard with a visible step count; per-system BIOS pages
naming exact expected files; a visible, plain-language override precedence
for emulator/core choice; a documentation taxonomy organized by novice path
first) applied on top of EmuWiz's already-stronger evidence machinery (DAT
identity, hash-verified firmware catalogues, transactional cheat
install/rollback, read-only-first philosophy). EmuWiz should keep refusing
Recalbox's core simplification tool — automatic "best" selection with no
evidence requirement — because that is precisely the class of hidden policy
EmuWiz's launch planner is built to avoid.

## Product-model differences

| | Recalbox | EmuWiz |
| --- | --- | --- |
| What it is | A Linux distribution/appliance image users flash to an SD card or install as the machine's only OS | A desktop application that coordinates an existing Linux system's ROM folders, DAT catalogues, installed emulators, RomM, and ES-DE |
| Owns boot/session? | Yes — Recalbox *is* the OS session (`recalbox.com`: "THE all-in-one retro gaming console") [recalbox.com, 2026-09-06] | No — EmuWiz never owns boot, display session, or the user's other applications |
| Owns networking/file serving? | Yes — ships its own Samba/FTP server and a fixed `/roms/<system>` layout users mirror onto a NAS [wiki.recalbox.com "Load roms on a network share with Samba", via search summary, 2026-09-06] | No — EmuWiz reads whatever source folders the user already has; it never runs a file server |
| Owns emulator lifecycle? | Yes — bundles and updates every emulator/core as part of the OS image | No — EmuWiz discovers and launches emulators the user already installed; it never bundles, updates, or auto-configures them |
| Default selection policy | Automatic: "Recalbox picks one [core] by default — the one that gives the best result on your machine" [wiki-next.recalbox.com "Emulators" via search summary, 2026-09-06] | Explicit: multiple candidates shown side by side, no automatic winner unless a caller-supplied remembered preference exists (`crate::launch::planning::apply_preference`) |
| Identity/verification model | Scraper-driven metadata (ScreenScraper); no DAT-style hash verification surfaced in user docs found | DAT/No-Intro-style hash identity, verified-identity facts per platform, hash-verified firmware catalogues |

## First-run UX

Recalbox's documented first-boot sequence [wiki.recalbox.com "First use and
configuration" / "Using a Recalbox controller for the first time", via
search summaries, 2026-09-06]:

1. **Setup wizard on first boot**: "greets you with options to choose your
   language, time zone, and basic network settings (Ethernet is easiest!)."
2. **Controller**: a USB controller Recalbox doesn't already know
   auto-starts the configuration wizard; Bluetooth controllers are paired via
   `START → CONTROLLER SETTINGS → PAIR BLUETOOTH CONTROLLERS`, then mapped
   button-by-button.
3. **Adding games**: no in-OS "add a folder" dialog — the user copies ROMs
   into a fixed `/roms/<system>` tree over the box's own Samba/FTP share or a
   USB drive, mirroring the exact per-system folder names Recalbox expects.
4. **Scraping**: an internal scraper "fully exploits your ScreenScraper
   account," run on demand from the menu, with thread-count/quota options.
5. **First launch**: select a scraped game from EmulationStation's list and
   press the launch button; Recalbox picks the default emulator/core itself.
6. **Recovery guidance**: a dedicated FAQ section exists (structure
   confirmed via the documentation index, page bodies not retrievable this
   session) alongside per-system wiki pages.

### Comparison with current EmuWiz onboarding

EmuWiz's actual current onboarding state, from `docs/FIRST_RUN_ONBOARDING_PLAN.md`
(a prior source-level, read-only audit of this repo, not this document's own
claim): there is **no dedicated onboarding wizard, no step tracker, no
persisted onboarding-progress flag** today. A first-run user sees a
"Welcome to EmuWiz" banner in Setup/Diagnostics and a task-card grid on Home
(`home_page.rs`), with the "Add your games" card promoted when no source
folder exists, but nothing sequences "now check DAT Sources → now check
Emulator Setup → now verify" — the user has to infer the order themselves.
That plan already recommends a 5-step guided overlay (Welcome → Add a source
→ optional DAT setup → Emulator setup → Verify) that deep-links into existing
pages rather than duplicating them.

| Idea | Classification | Why |
| --- | --- | --- |
| A visible, sequenced first-run path (step N of M) | `BORROW_CONCEPT` | EmuWiz has the pieces but no sequencing shell; `docs/FIRST_RUN_ONBOARDING_PLAN.md` already proposes exactly this |
| OS-level setup wizard (language/timezone/network) | `NOT_RELEVANT` | EmuWiz runs on an already-configured OS; it has no boot/network to configure |
| Auto-starting a wizard when new hardware (a controller) appears | `CONFLICTS_WITH_EMUWIZ_MODEL` | EmuWiz does not own input devices or launch external configuration tools uninvited |
| Fixed per-system folder taxonomy for "where do I put files" | `ALREADY_STRONG_IN_EMUWIZ` | EmuWiz already scans arbitrary existing folder layouts via alias/evidence detection rather than requiring one fixed tree; requiring Recalbox's tree would be a regression |
| Built-in network file server for adding games | `CONFLICTS_WITH_EMUWIZ_MODEL` | EmuWiz is local-first over files that already exist on the host; running a file server is appliance-OS territory |
| On-device scraping as an explicit, separate "later" step, not forced during first run | `ALREADY_STRONG_IN_EMUWIZ` | `docs/FIRST_RUN_ONBOARDING_PLAN.md` already keeps DAT/RomM/ES-DE optional and skippable in the proposed flow, matching this instinct |

## Platform/system model

Recalbox's public claim: "Recalbox 10.1 emulates 144 systems across seven
official images" [search summary of wiki-next.recalbox.com "Emulators",
2026-09-06], organized in documentation by informal category — Arcade
(MAME, FBNeo, Daphne, Naomi), Consoles (Nintendo/Sega/Sony/others),
Computers, Ports/Games — per the documentation table of contents
[`recalbox.gitbook.io/documentation/llms.txt`, 2026-09-06]. No confirmed
structured "manufacturer" or "generation" field was found in any fetched
page; the categorization is presentational (per-doc-section), not a queried
data field.

EmuWiz's canonical `Platform` struct
(`crates/archivefs-core/src/platform/mod.rs`, unchanged, inspected not
modified) has: `id`, `display_name`, `folder_aliases`, `filename_aliases`,
`strong_extensions`, `weak_extensions`, `magic`, `layout`, `conflicts_with`,
`preferred_emulator`, `explanation`. It has **no manufacturer or generation
field either** — both projects present systems by name/category rather than
by a structured manufacturer/generation taxonomy. This is a genuine
similarity, not a gap to close on EmuWiz's side from this evidence.

`crate::platform_artwork` does define artwork **category IDs** (`console`,
`handheld`, …) for local artwork normalization — closer to Recalbox's
per-system theme/artwork concept than the `Platform` struct itself is.

### Discrepancies worth later platform research (not resolved here)

- Recalbox's per-system wiki pages document **exact expected BIOS filenames
  and MD5 checksums** per system [search summary of wiki-next.recalbox.com
  "Emulators", 2026-09-06]. EmuWiz's own firmware catalogues
  (`crate::patch_manager::pcengine_cd_firmware::KNOWN_SYSTEM_CARDS`, and the
  DuckStation/PCSX2 firmware modules) already carry **size + CRC32 + SHA-1**
  per known file — stronger than filename+MD5 alone — but EmuWiz's coverage
  is adapter-by-adapter, not a single browsable per-system reference page.
  Worth a future audit: does every EmuWiz firmware-aware adapter have a
  catalogue as complete as its Recalbox-documented counterpart?
- A third-party project, `Abdess/retrobios` ("Source-verified BIOS and
  firmware packs … with emulator source code as the deciding authority"
  [github.com/Abdess/retrobios, 2026-09-06]), targets Recalbox, RetroArch,
  Batocera, RetroPie, and others with the same "verify against emulator
  source" philosophy EmuWiz's own catalogues already apply independently.
  It is a **firmware-pack redistribution project** — EmuWiz must not depend
  on, scrape, or bundle it; it is noted only as evidence that "verify a
  system-ROM claim against the emulator's own source" is a shared,
  reasonable standard, which EmuWiz already meets on its own.
- No evidence was found that Recalbox exposes RetroArch core hints or
  extension lists as a stable, versioned public API — its "systems database"
  is documentation pages, not a queryable dataset (see next section).

## BIOS experience

Recalbox: "Some machines need their original BIOS, which Recalbox cannot
ship for legal reasons, and those files go into `/recalbox/share/bios/`,
sometimes in a system-specific sub-folder, with each system page listing
exactly what is expected, with file names and MD5 checksums." [search
summary of wiki-next.recalbox.com "Emulators", 2026-09-06]. This is the
single strongest UX idea found in this audit: **one page per system that
names the exact expected file(s) and their checksum**, so a user troubleshoots
against a documented, specific expectation rather than a generic "BIOS
missing" message.

EmuWiz's comparable machinery is stronger evidence, weaker presentation:

- `crate::patch_manager::pcengine_cd_firmware::KNOWN_SYSTEM_CARDS` verifies
  against **size + CRC32 + SHA-1** of the file actually found on disk, never
  a filename alone.
- `crate::launch::readiness::FirmwareReadiness` (`Verified` /
  `PresentUnverified` / `Missing` / `Unknown` / `NotRequired`) is a shared,
  reviewed vocabulary every firmware-aware adapter projects onto, and Doctor
  (`crate::diagnostics::profiles`) already surfaces per-adapter firmware
  labels (e.g. xemu's `flash_bios` / MCPX / EEPROM breakdown).
- What EmuWiz does **not** yet have is a single, plain-language,
  per-platform "here is exactly what firmware this needs, and here is
  whether EmuWiz found it" page a beginner can read before ever touching
  Doctor's technical findings list.

EmuWiz must keep its evidence bar (hash-verified, never filename-only,
never downloaded or copied by EmuWiz) while borrowing Recalbox's
*presentation* idea: name the exact expectation, per platform, in one place.

## Emulator/core selection

Recalbox: a system is "often handled by several emulators (also called
cores), which do not offer the same compatibility, the same performance or
the same options, and Recalbox picks one by default — the one that gives the
best result on your machine." [search summary of wiki-next.recalbox.com
"Emulators", 2026-09-06]. Override is available at two levels — "you can
replace it for a whole system from that system's options, or game by game
from the game menu in the list" — with a documented, explicit precedence:
**"Game > System > RetroArch"** for which config file wins when the same
setting exists in more than one [search summary of wiki.recalbox.com
"RetroArch" / "Configuration override", 2026-09-06].

EmuWiz's deliberate position (`crate::launch::planning`,
`crate::launch::platform_map`): every discovered standalone adapter and
every matching RetroArch core is surfaced as its own candidate; nothing is
auto-selected unless it is the sole eligible candidate or a caller-supplied
`RememberedPreference` names it; two genuinely different RetroArch cores for
one platform are marked `Ambiguous`/blocked rather than guessed. This is not
a UX gap — it is a safety property, grounded in "no automatic winner where
evidence does not justify it," and Recalbox's auto-"best" policy is exactly
the pattern EmuWiz's architecture exists to refuse.

The transferable idea is narrow and does not require adopting
auto-selection: **a visible, plain-language override precedence** — "your
choice for this game overrides your choice for this platform, which
overrides the default" — is a useful mental model for EmuWiz's own
remembered-preference UI, once remembered preferences exist at more than one
scope. Today EmuWiz has per-profile remembered choices; it does not yet have
a documented cascade the way Recalbox's Game > System > RetroArch is
documented.

## Metadata and scraping

Recalbox's internal scraper is ScreenScraper-first: it "fully exploits your
ScreenScraper account" (thread count and quota are user-configurable), pulls
box art (2D/3D), screenshots, title screens, and produces a Recalbox-branded
"mix" composite image; a third-party companion tool, Skraper, adds maps,
manuals, and pad-to-keyboard configuration scraping [search summaries of
wiki.recalbox.com "Internal Scraper" / "Scraps management" and
`recalbox.com` blog, 2026-09-06]. Region/language priority is a configurable
scraper option.

EmuWiz's own metadata position is RomM- and ES-DE-mediated rather than a
built-in scraper: EmuWiz's job is DAT-verified identity plus read-only
RomM/ES-DE interoperability, not running its own art-scraping pipeline. That
split is intentional and should not change: EmuWiz does not need to add a
ScreenScraper-account-driven scraper to be competitive — RomM already
exists in the ecosystem for that role, and EmuWiz's advantage is that its
identity resolution (DAT hash matches) is independent of and prior to any
art-scraping step. No proposal here suggests violating ScreenScraper's terms
or building an anti-scraping-control workaround; if EmuWiz ever wants richer
per-game metadata directly, the reviewed path is RomM's own provider
integration, not a parallel scraper.

## Library/navigation

Recalbox's browsing surface (EmulationStation) provides: per-system game
lists, favourites, a "last played"/recently played list, search, and basic
filters, following the wider EmulationStation-family convention that
Recalbox itself is built on. Genre/player-count fields are populated from
scraped metadata rather than derived locally.

EmuWiz deliberately does **not** duplicate this: it hands curated library
views (Playing Library / RomM / ES-DE export) to those existing frontends
rather than building a parallel in-app game browser. The audit finds no
evidence this posture should change — building a Recalbox-style browsing UI
inside EmuWiz would duplicate ES-DE/RomM/EmulationStation rather than add
value, and EmuWiz's actual differentiator (DAT-verified identity, 1G1R
planning, safe export) sits *upstream* of browsing, not inside it.

## Collection management

No evidence was found that Recalbox has a first-class duplicate/region/1G1R
management feature comparable to EmuWiz's — its documentation and forum
threads discuss folder organization and manual ROM curation, not an
automated identity-driven 1G1R planner. This is an area to flag as an
EmuWiz strength (see below) rather than something to borrow.

## Cheats/mods

Recalbox's cheat workflow is entirely RetroArch's own mechanism, manually
operated: "You need to download a file with the `.cht` extension containing
the game's cheats and place it in the `SHARE\CHEATS` folder," then toggle it
on/off from RetroArch's own Quick Menu (`Hotkey+B → Core Options` or
`Quick Menu → Cheats → Cheats File Load`); "Cheat codes are only available
for libretro cores" — non-libretro cores (e.g. mupen64plus for N64) have no
cheat support at all [search summary of wiki.recalbox.com/forum.recalbox.com
"Cheats", 2026-09-06 — **community-source-heavy; the .cht mechanism itself is
RetroArch's, corroborated by multiple forum threads, but no single official
Recalbox wiki page was fully retrievable this session**]. There is no
curated cheat browser, no per-game compatibility pre-filtering, and no
install/rollback safety layer — the user finds a `.cht` file themselves and
places it by hand. No evidence of a first-class "mods" (texture packs, ROM
hacks) feature was found; the pattern would presumably be the same manual
file-drop.

EmuWiz's local cheat installer (`crate::patch_manager::*_install_plan`,
`crate::patch_manager::shared_transaction`, per the beginner-workflow
simplification in `docs/CHEATS_MODS_BEGINNER_WORKFLOW.md`) is materially
more capable: provider-sourced and identity-matched candidates, a preview →
confirm → apply → verify → rollback pipeline, and — as of the beginner
simplification — a plain checkbox list with "Install selected" / "Undo
installation" hiding that machinery by default. Recalbox has nothing
comparable to borrow here; the only transferable idea is presentation
minimalism, which EmuWiz has already implemented independently.

## Updates

Recalbox's update flow: in-menu check (`START → UPDATES`), online or fully
offline (copy a new SD-card image file into a directory visible from any OS,
reboot, and the system detects and applies it), and a config-file switch
(`updates.type` in `recalbox.conf`) to move from the stable to the beta
channel [search summary of multiple wiki.recalbox.com pages, 2026-09-06].
No rollback mechanism was found in official documentation; community
guidance for "can't update" scenarios is generally "reinstall and restore
your ROMs/BIOS/saves backup," which is not meaningfully different in
mechanism from EmuWiz's own manual download-and-replace AppImage approach —
it is the same "copy a new artifact over the old one" idea, just packaged
into the appliance's own boot sequence.

**Conclusion: EmuWiz does not need a built-in updater merely because
Recalbox has one.** The genuinely transferable pieces are messaging ideas,
not mechanism:

- an explicit, user-visible **channel** concept (stable vs beta) if/when
  EmuWiz ever ships pre-release builds;
- a simple **in-app "check for a newer AppImage" indicator** (read-only
  version comparison, no auto-download) that mirrors "you're on the
  latest version" / "a newer version is available" without adding any
  install/replace automation, which is out of scope for V1's manual
  download-and-replace model.

## Doctor/error recovery

Confirmed, official per-system pages exist for BIOS/compatibility, and a
dedicated FAQ section exists in the documentation index; the individual
FAQ Q&A bodies were not retrievable in full this session (the current
`wiki-next.recalbox.com/en/faq` page returned no static content to this
session's fetch tool). What is confirmed from adjacent pages is the
*pattern*: each system's own wiki page states BIOS expectations
concretely (filenames + checksums) rather than only via a generic runtime
error, and general troubleshooting content is reachable from the same
documentation tree rather than scattered across forum threads.

EmuWiz's Doctor (`crate::diagnostics`, `doctor_page.rs`,
`docs/BEGINNER_UX_AUDIT.md`) already follows a deliberate
simple-answer → next-action → technical-details ordering and a
read-only-findings-then-explicit-confirm-to-repair pattern
(`DoctorPageAction::ConfirmRepair`). `docs/BEGINNER_UX_AUDIT.md` (H1–H5 and
beyond) already catalogues specific novice/advanced separation problems
(e.g. "Doctor" vs "Problems & Repair" naming drift, DAT-acronym exposure)
independent of this Recalbox research. The one Recalbox idea worth adding to
that existing backlog: **name the exact expected artifact** (a specific BIOS
filename/hash, a specific missing folder) inside the plain-language answer,
the same way a Recalbox system page does, rather than only in "technical
details."

## Controller UX

Recalbox: USB controllers Recalbox doesn't recognise auto-launch a mapping
wizard; Bluetooth pairing is a menu action (`START → CONTROLLER SETTINGS →
PAIR BLUETOOTH CONTROLLERS`) with a "put your controller into pairing mode"
prompt; button mapping is done once per controller and reused system-wide,
with per-system/per-emulator quirks documented separately [search summary of
wiki.recalbox.com "Using a Recalbox controller for the first time" /
"First use and configuration", 2026-09-06].

This is squarely appliance-OS territory: Recalbox owns input-device
enumeration, Bluetooth pairing, and global hotkey/mapping state because it
*is* the OS session. **EmuWiz should not own controller pairing or mapping.**
The Linux desktop, the window/input manager, and each launched emulator
already own that; EmuWiz taking it over would duplicate and likely conflict
with those owners. The one legitimate EmuWiz role is **diagnosis and
delegation**: Doctor could, in principle, report "no game controller was
detected" or "the resolved emulator profile has no configured input
mapping" as a read-only finding pointing at the relevant emulator's own
settings — never a mapping UI inside EmuWiz itself. No current EmuWiz code
does this; it is listed as P2 below, not assumed necessary.

## Documentation

Recalbox's documentation taxonomy [`recalbox.gitbook.io/documentation/llms.txt`,
2026-09-06]: **Welcome → Presentation → Basic Manual → Advanced User →
Emulators → Hardware Compatibility → Tutorials → FAQ**, translated into at
least English, French, German, and Italian, with per-emulator/system pages
grouped by category (Arcade, Consoles, Computers, Ports) and tutorials
grouped by task (video/audio configuration, controller setup, ROM
management, scraping, networking).

The transferable structural idea for EmuWiz 1.0 docs is the **ordering**:
lead with a novice path (Welcome/Basic Manual equivalent) before any
reference material, keep per-topic reference pages (BIOS-per-system
equivalent: firmware-per-adapter) separate from the narrative guide, and put
troubleshooting in one discoverable place rather than only in commit
messages/design docs (EmuWiz currently has excellent *design-time* docs —
this repository's own `docs/` tree — but no confirmed end-user-facing
documentation site yet). This is a 1.0 gap independent of Recalbox, which
Recalbox's structure simply illustrates well.

## What EmuWiz should borrow

1. A real, sequenced first-run path with a visible step count — already
   planned in `docs/FIRST_RUN_ONBOARDING_PLAN.md`; this audit corroborates
   the idea from an outside reference rather than introducing it.
2. Per-platform, plain-language firmware pages naming the *exact* expected
   artifact (filename/hash), matching Recalbox's per-system BIOS pages, but
   built from EmuWiz's own stronger hash-verified catalogues.
3. A documented, visible override-precedence model (game > platform >
   default) as the mental model for EmuWiz's own remembered-emulator-choice
   UI, once/if a platform-level remembered default is added alongside the
   existing per-profile one.
4. A novice-first documentation ordering (Welcome/basic path before
   reference material) for EmuWiz's eventual 1.0 user-facing docs.
5. Lightweight, read-only "check for a newer version" messaging, without any
   auto-download/auto-replace automation.

## What EmuWiz should NOT copy

Verified, not assumed:

- **Appliance-OS ownership of boot/session.** Recalbox *is* the OS
  (`recalbox.com`: "all-in-one retro gaming console"). EmuWiz runs inside a
  user's existing Linux session and must never assume it owns startup.
- **Owning the network file server.** Recalbox ships its own Samba/FTP share
  and a fixed `/roms/<system>` tree users must mirror. EmuWiz reads whatever
  folders already exist; adding a file server or requiring a fixed tree
  would regress its flexible folder-alias detection.
- **Automatic "best" emulator/core selection with no evidence
  requirement.** Directly documented Recalbox behaviour, and directly
  opposed to `crate::launch::planning::apply_preference`'s "no automatic
  winner without a remembered preference or a sole eligible candidate" rule.
- **Owning controller pairing/mapping.** Confirmed as an OS-level Recalbox
  responsibility; EmuWiz has no input-device ownership today and should not
  acquire it — diagnosis/delegation only, if anything.
- **Bundling and lifecycle-managing emulator binaries/cores.** Recalbox
  ships and updates every emulator as part of its OS image. EmuWiz
  deliberately discovers and launches *externally installed* emulators
  (`crate::patch_manager::*_local` discovery modules) and never bundles or
  auto-updates one — this is a foundational EmuWiz architectural choice, not
  an oversight to close.
- **Fixed, appliance-defined filesystem layout as the only supported
  shape.** Recalbox requires `/roms/<system>`. EmuWiz's alias/evidence-based
  detection exists specifically so a user's *own* existing organization
  works.
- **SD-card/image-based update mechanism.** Not applicable to a desktop
  application; noted only as a mechanism, not something to imitate.

## Where EmuWiz is already stronger

Grounded in current EmuWiz code, not marketing framing:

- **DAT/identity evidence.** EmuWiz resolves canonical identity from
  hash-verified DAT matches and platform-specific header/boot evidence
  (`crate::game_identity`, `crate::dat::identity`,
  `crate::platform_evidence_fusion`) before any launch candidate is
  considered legitimate. No equivalent was found in Recalbox's documented
  model, which identifies games by scraped metadata, not verified hashes.
- **Hash-verified firmware catalogues.**
  `crate::patch_manager::pcengine_cd_firmware::KNOWN_SYSTEM_CARDS` (and the
  DuckStation/PCSX2 firmware modules) check size + CRC32 + SHA-1 against the
  actual file on disk. Recalbox's documented BIOS verification is
  filename + MD5 per its own wiki pages.
- **No-automatic-winner launch planning.**
  `crate::launch::planning::{CandidatePreference, apply_preference}`
  surfaces every eligible standalone adapter and RetroArch core as its own
  candidate and only elects a winner from a sole-eligible case or an
  explicit remembered preference — the opposite of Recalbox's documented
  "Recalbox picks one by default."
- **Transactional, reversible cheat installation.** EmuWiz's local cheat
  install pipeline (`crate::patch_manager::shared_transaction`,
  `*_install_plan`, `*_install_result`) is preview → confirm → apply →
  verify → rollback, with a beginner-simplified default view
  (`docs/CHEATS_MODS_BEGINNER_WORKFLOW.md`). Recalbox's cheat workflow is a
  manual `.cht`-file drop with no comparable safety layer.
- **Read-only-first philosophy across Doctor/DAT/RomM.** DAT Sources
  (`dat_sources_page.rs`) is explicitly documented as never renaming,
  moving, or deleting a ROM; RomM integration is explicitly "nothing in
  your RomM library is ever changed"; Doctor's findings are read-only by
  construction, with mutation gated behind an explicit confirm step
  (`DoctorPageAction::ConfirmRepair`). No Recalbox source reviewed makes an
  equivalent explicit non-mutation guarantee for its own scraper/BIOS/config
  writes.
- **RomM/ES-DE interoperability instead of a competing frontend.** EmuWiz
  exports to and imports from tools that already do library browsing well
  (`crate::launch::es_de_export`/`es_de_publish`,
  `crate::identity_source::romm`) rather than building a parallel game
  browser the way Recalbox's own EmulationStation front-end does. This is a
  role difference, not an incompleteness.
- **External emulator coexistence.** Every EmuWiz standalone adapter
  (Snes9x, Mesen, Stella, RMG, etc.) discovers and launches an emulator the
  *user* installed and keeps a RetroArch candidate as a genuinely separate,
  coexisting option (`crate::launch::integration::DiscoveredStandaloneProfile`).
  Recalbox bundles one copy of each emulator it supports; there is no
  "which of my several installed emulators" question for it to answer.

## Prioritised roadmap recommendations

### P0 — before 1.0

1. **Ship the already-planned first-run step tracker.**
   - Recalbox inspiration: a visible, sequenced setup wizard.
   - Current EmuWiz state: Home + Setup/Diagnostics banners exist but no
     sequencing shell (`docs/FIRST_RUN_ONBOARDING_PLAN.md`).
   - Exact gap: a novice has to infer the order of "add source → optional
     DAT → emulator setup → verify" from Home's card grid.
   - Proposed EmuWiz-native solution: the 5-step deep-linking overlay
     `docs/FIRST_RUN_ONBOARDING_PLAN.md` already specifies — reuse existing
     pages, add only the thin step-tracker shell and a persisted
     `onboarding_state` sidecar file.
   - Cost/risk: SMALL per that plan's own estimate; low risk since it wraps
     existing, already-reviewed pages rather than adding new logic.
   - Backend needed: no (pure GUI shell over existing calls).

2. **Add a plain-language "what firmware does this platform need, and did
   EmuWiz find it" summary per platform.**
   - Recalbox inspiration: per-system BIOS pages naming exact
     filenames/checksums.
   - Current EmuWiz state: hash-verified firmware catalogues exist per
     adapter (`pcengine_cd_firmware`, DuckStation/PCSX2 firmware modules,
     `FirmwareReadiness` projections) but are surfaced only inside Doctor's
     technical findings and per-adapter setup pages, not as one legible
     per-platform summary.
   - Exact gap: a beginner cannot see, in one place and before opening
     Doctor, "PS2 needs a BIOS; here's whether one was found" in plain
     language.
   - Proposed EmuWiz-native solution: a read-only projection (no new
     evidence gathering) that renders each platform's existing
     `FirmwareReadiness` state as one sentence + a "why" link into Doctor's
     existing technical detail — never inventing a new firmware requirement
     for a platform that has none.
   - Cost/risk: SMALL–MEDIUM; purely presentational over existing readiness
     data.
   - Backend needed: no (a view over already-computed `FirmwareReadiness`).

### P1 — after core QA

3. **Document EmuWiz's own emulator-choice precedence in-product.**
   - Recalbox inspiration: documented "Game > System > RetroArch" override
     precedence.
   - Current EmuWiz state: per-profile remembered preference exists
     (`RememberedPreference`); there is no platform-level default distinct
     from a per-profile remembered choice, and no in-GUI explanation of how
     a remembered choice relates to "no automatic winner."
   - Exact gap: a user who sets a remembered preference has no on-screen
     explanation of when it applies versus when EmuWiz still shows multiple
     candidates.
   - Proposed EmuWiz-native solution: a one-line explanation on the launch
     readiness / emulator setup pages: "Remembered for this game" vs
     "Multiple options available — nothing was assumed," reusing existing
     `CandidatePreference` values; no new preference *mechanism*.
   - Cost/risk: SMALL; GUI copy + existing enum, no new selection logic.
   - Backend needed: no.

4. **Start an end-user-facing documentation site with a novice-first
   ordering.**
   - Recalbox inspiration: Welcome → Basic Manual → Advanced → per-system
     reference → Tutorials → FAQ ordering.
   - Current EmuWiz state: extensive design/engineering docs in this
     repository's `docs/` tree; no confirmed public, novice-oriented
     documentation site.
   - Exact gap: a new user has nowhere public to read "what EmuWiz will
     never do silently" or "here's how to add your first source folder"
     outside the app itself.
   - Proposed EmuWiz-native solution: publish a small, novice-first doc site
     mirroring the already-drafted onboarding wording
     (`docs/FIRST_RUN_ONBOARDING_PLAN.md` step 1) plus per-adapter firmware
     reference pages generated from the same data the P0 firmware summary
     uses.
   - Cost/risk: MEDIUM; mostly content work, not code.
   - Backend needed: no.

5. **Add a read-only "newer AppImage available" check.**
   - Recalbox inspiration: in-menu update check with stable/beta messaging.
   - Current EmuWiz state: V1 manual download-and-replace AppImage
     (`docs/APPIMAGE_PACKAGING.md`); no in-app version check.
   - Exact gap: a user has no signal inside EmuWiz that a newer build
     exists.
   - Proposed EmuWiz-native solution: an optional, explicit "Check for
     updates" action that compares the running version against a published
     version file/feed and only displays the result — no download, no
     replace, no channel switching automation.
   - Cost/risk: MEDIUM; needs a versioned publication point and clear
     opt-in messaging (must not phone home silently).
   - Backend needed: yes, small (an HTTP check behind an explicit user
     action) — flag for privacy/opt-in review before building.

### P2 — nice to have

6. **Doctor: report "no controller detected" as a read-only diagnostic
   pointing at the OS/emulator's own settings.**
   - Recalbox inspiration: controller pairing is a first-class flow.
   - Current EmuWiz state: no controller awareness at all.
   - Exact gap: none confirmed as blocking; purely a diagnostic nicety.
   - Proposed EmuWiz-native solution: a Doctor finding only, never a mapping
     UI — "delegate," not "own," matching §12's conclusion.
   - Cost/risk: MEDIUM (needs a safe, read-only way to enumerate input
     devices without taking ownership).
   - Backend needed: yes (a new, bounded read-only probe).

### DO NOT BUILD

- An EmuWiz-owned network file server or fixed ROM folder taxonomy.
- Automatic "pick the best emulator/core" selection.
- Controller pairing/mapping owned by EmuWiz.
- Bundling, updating, or auto-configuring any emulator binary.
- A built-in art scraper duplicating RomM's role.
- A full in-app game browser duplicating ES-DE/RomM/EmulationStation.
- An automated updater that downloads and replaces the running AppImage
  without an explicit user action.

## Research-source opportunities

- Recalbox's **wiki/documentation pages** (`wiki.recalbox.com`,
  `wiki-next.recalbox.com`, `recalbox.gitbook.io/documentation`) are
  human-curated prose, not a queryable dataset — useful as a **secondary,
  cross-check reference** when researching an unfamiliar platform's expected
  BIOS filename or folder convention, but not something to scrape or import;
  several current pages are also JS-rendered and were not fully retrievable
  by this session's tooling, which is itself a reason not to depend on them
  as a live data source.
- The upstream **`recalbox/recalbox-emulationstation`** GitHub repository
  (mentioned for completeness; not fetched in full this session) is the
  more promising *structured* reference: EmulationStation-family
  distributions typically ship a machine-readable `es_systems.cfg`
  (per-system name/path/extension/command/platform/theme). If EmuWiz ever
  wants a second opinion on a platform's canonical folder name or extension
  set, reading that file as a **cross-check, never an authority**, and
  always independently verified against EmuWiz's own evidence rules, is
  legally and technically feasible (Recalbox's `recalbox-emulationstation`
  is an open-source fork in the EmulationStation lineage). Do not adopt any
  name or extension from it without EmuWiz's own review.
- Do **not** use ScreenScraper's database as an EmuWiz data source without
  separately reviewing its own terms of use; this audit does not evaluate
  or endorse that.
- Do **not** use `Abdess/retrobios` or any BIOS-pack project as a source of
  files; noted above only as evidence for a shared verification philosophy.

## Definition of Done

This audit is complete when:

1. `docs/RECALBOX_COMPETITIVE_AUDIT.md` exists with every required section
   and the decision table below, every Recalbox claim cited to a public
   Recalbox source (or explicitly labelled a community source) with an
   access date.
2. No claim compares EmuWiz to Recalbox by asserting EmuWiz should become an
   appliance OS, own boot/session/network/controllers, or auto-select an
   emulator without evidence.
3. Every roadmap recommendation names the exact current EmuWiz state, the
   exact gap, and a proposed EmuWiz-native (not Recalbox-native) solution.
4. No production Rust, test, or Cargo command was touched; only this
   documentation file is committed.

## Decision table

| RECALBOX IDEA | EMUWIZ CURRENT STATE | VALUE | FIT | PRIORITY | BACKEND NEEDED? | RECOMMENDATION |
| --- | --- | --- | --- | --- | --- | --- |
| Sequenced first-run wizard with step count | Banners only, no sequencing (`docs/FIRST_RUN_ONBOARDING_PLAN.md`) | High | Strong | P0 | No | Ship the already-planned 5-step overlay |
| Per-system BIOS page (exact filename + checksum) | Hash-verified per-adapter catalogues, no unified summary | High | Strong | P0 | No | Add a read-only per-platform firmware summary view |
| Documented Game > System > default override precedence | Per-profile `RememberedPreference` only | Medium | Strong | P1 | No | Add explanatory copy reusing `CandidatePreference` |
| Novice-first public documentation ordering | Rich internal `docs/`, no public novice site | Medium | Strong | P1 | No | Publish a small novice-first doc site |
| Read-only "check for updates" | Manual AppImage download/replace only | Medium | Strong (if opt-in, no auto-replace) | P1 | Yes (small) | Add explicit, opt-in version check only |
| ScreenScraper-driven internal scraper | RomM/ES-DE interoperability, no built-in scraper | Low for EmuWiz's role | Conflicts (duplicates RomM) | DO NOT BUILD | Yes | Leave scraping to RomM |
| Fixed `/roms/<system>` + built-in Samba/FTP server | Flexible folder-alias detection over existing folders | Negative | Conflicts | DO NOT BUILD | Yes | Do not adopt |
| Automatic "best" emulator/core selection | No-automatic-winner planner (`apply_preference`) | Negative | Conflicts | DO NOT BUILD | Yes | Do not adopt |
| OS-level controller pairing/mapping | None; not EmuWiz's role | Low direct value; some diagnostic value | Mostly conflicts | P2 (diagnostic only) / DO NOT BUILD (mapping UI) | Yes (diagnostic only) | Diagnose/delegate only, never own |
| Manual `.cht`-file cheat workflow | Transactional, identity-matched, rollback-capable installer | Negative (would be a regression) | Conflicts | DO NOT BUILD | N/A | Keep EmuWiz's stronger model |
| Bundled, appliance-updated emulator/core lifecycle | External-emulator discovery only, by design | Negative | Conflicts | DO NOT BUILD | Yes | Do not adopt |
| `recalbox-emulationstation`'s `es_systems.cfg` as a cross-check reference | EmuWiz's own reviewed `Platform` registry | Low-medium, corroborative only | Neutral | P2 (research aid only) | No | Use only as a secondary cross-check, never authoritative |
