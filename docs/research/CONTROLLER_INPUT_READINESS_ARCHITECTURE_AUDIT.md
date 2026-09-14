# Controller / Input Readiness Architecture Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot.** This document records research and design reasoning only. Nothing
> in it is implemented. No production Rust file, GUI page, launch planner, emulator adapter,
> controller config, Steam Input integration, RetroArch mapping, Save Vault code, or database
> code was created or modified while producing it. It proposes a conceptual evidence model
> in prose and tables; where illustrative code appears it is clearly marked non-normative
> pseudocode, never a real type in this repository.

**Scope:** EmuWiz, `feature/archivefs-unified-platform`, worktree
`/home/davedap/emuwiz-main-release-fix`. **Method:** direct reading of this repository's own
source (`file:line` citations, tagged **CONCLUSION FROM SOURCE**), plus external web research
on SDL/evdev/Steam Input/RetroArch/per-emulator controller config formats and MAME's
`-listxml` input metadata (tagged **DOCUMENTED FACT** where a specific external source was
read, **GENERAL KNOWLEDGE** where the claim reflects broadly-published, stable technical
convention that was cross-checked by search but not read from one canonical primary document
in this pass). Reasoning original to this document is tagged **INFERENCE**. Anything not
independently confirmed is tagged **UNCERTAIN**.

**Tagging key**

| Tag | Meaning |
|---|---|
| **DOCUMENTED FACT** | Stated by an external, cited source read in this pass, or by a `file:line` in this repository. |
| **GENERAL KNOWLEDGE** | Widely and consistently published technical convention (cross-checked by search, not read from one single primary source end-to-end in this pass). |
| **INFERENCE** | Reasoning drawn by this document; not directly asserted by a source. |
| **UNCERTAIN** | Explicitly flagged as unverified; not to be built on without further reading or a probe. |

## 0. Purpose, scope, and explicit non-goals

**Purpose.** EmuWiz's Ready-to-Play projection (`crates/archivefs-core/src/ready_to_play.rs`)
already declares a `ReadinessReasonFamily::Controller` variant but has no evidence source that
constructs it. This document asks: what would a trustworthy, honest, bounded
controller/input-readiness evidence model for EmuWiz actually look like, what could it prove
today from static configuration and metadata alone, what must it never claim, and how — if at
all — should it connect to Ready-to-Play, Doctor, and the Attention model.

**Explicit non-goals (this task).** This document does **not** modify, and its reasoning must
not be read as authorizing changes to:

- `ready_to_play.rs` or any Ready-to-Play state/reason type
- Any emulator adapter (`launch/`, `diagnostics/profiles.rs`, per-adapter modules)
- Any controller configuration file EmuWiz writes (none exist today — confirmed below)
- Steam Input configuration, RetroArch input/remap files, or any other emulator's input `.ini`/`.cfg`
- Save Vault (still unimplemented — see the Ready-to-Play audit §14, reconfirmed here)
- Launch planning (`launch::planning`, `launch::readiness`)
- Any GUI page
- The database schema or any migration

Everything below is a **design sketch for a future phase**, explicitly not scheduled or
authorized by this document.

## 1. Current EmuWiz inventory

All rows are **CONCLUSION FROM SOURCE**, read directly in this pass.

### 1.1 `ReadinessReasonFamily::Controller` is declared but dead

`crates/archivefs-core/src/ready_to_play.rs:29-44` declares:

```rust
pub enum ReadinessReasonFamily {
    Identity, Content, MediaTopology, Firmware, Emulator, Configuration,
    Dependency, Arcade, DatCompatibility, ModOrPatch, Controller, LaunchPlan,
    Unsupported, UnknownEvidence,
}
```

A repository-wide grep for `ReadinessReasonFamily::Controller` finds exactly **one** hit outside
its own declaration: `crates/archivefs-gui/src/ready_to_play_page.rs:174`, inside the exhaustive
`family_label()` match arm that maps it to the display string `"Controller"`. **There is no
constructor call site anywhere in `crates/` that ever produces a `ReadinessReason` with this
family.** The `reason()` helper (`ready_to_play.rs:155-173`) and every call to it in
`project_ready_to_play` (`ready_to_play.rs:243-371`) are enumerated in full in this file, and
none references `Controller`. This is concrete, verified evidence — not a paraphrase of the
task brief's claim — that controller/input state currently has no evidence source feeding
Ready-to-Play: the enum variant and its display label exist purely so that a future producer
has somewhere to land, and the GUI match arm exists only because Rust requires the match to be
exhaustive over every declared variant, not because anything populates it.

### 1.2 `launch::input_projection` is a false-positive name match — not gamepad input

`crates/archivefs-core/src/launch/input_projection.rs` (796 lines) is named in a way that
invites confusion with controller/gamepad "input," but its own module documentation and
contents (read in full) show it has **nothing to do with controller or keyboard input**. Its
actual job is routing an already-verified `VerifiedIdentityFact` (a confirmed PS1/PS2 serial, a
PSP disc ID, a PS3 title ID, a Dolphin game ID, etc.) into the correct field of a per-adapter
launch-request struct (e.g. populating `Pcsx2GameRequest`'s serial field, or `DolphinGameRequest`'s
game-ID field) so the right adapter-specific launch command can be built. "Input" here means
"input to the launch-request-building function" — i.e. a data-flow sense of "input," not a
device sense. **This is stated explicitly here so a future contributor searching the repo for
"input" does not mistake this module for a controller evidence source or attempt to extend it
for that purpose.** Any future `InputReadiness`/`InputRequirement`/`InputCapability` work
belongs in a new module, not here.

### 1.3 Prior Ready-to-Play research already reached — and this document confirms — "Unknown, never a blocker"

`docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md` §12 ("Controller / input readiness (K)",
lines 681-711) is the prior research this task's brief paraphrases. Reading it in full, its
actual conclusion is more precise than "no trustworthy evidence source" alone — it is a
three-way decision table (§12.2, quoted verbatim below) that explicitly **rejects both BLOCK and
WARN** and recommends **UNKNOWN as a coverage note only**:

> | Option | Verdict |
> |---|---|
> | **BLOCK** on a special-control requirement | **Rejected.** EmuWiz has no evidence of which games require which controls; blocking would be fabricated — and wrong in the common case (most arcade titles whose *cabinet* used a wheel are playable on a pad). |
> | **WARN** that a special control is required | **Rejected for now.** A warning still asserts a requirement EmuWiz cannot prove. |
> | **UNKNOWN, as a coverage note only** | **Recommended.** EmuWiz states plainly: "Control requirements are not modelled; EmuWiz cannot tell you whether this game needs a specific controller." |

It grounds this in two existing EmuWiz precedents cited by `file:line`: the PPSSPP firmware
constant's refusal to invent a requirement (`launch/readiness.rs:15-20`), and Doctor's
`DEFERRED_CHECKS`, which names what is deliberately not checked rather than implying the
absence of a problem (`diagnostics/mod.rs:630-661`). It also explicitly flags (§12.3) that
**any future evidence-backed model must never treat arcade cabinet control metadata alone as
proof that an ordinary gamepad is sufficient** — i.e. even a rich MAME `<control>` catalogue is
not, by itself, a "standard gamepad is fine" proof; it is only ever evidence of what a *cabinet*
had, and cabinet-original controls are frequently more specialized than what makes a game
playable on a pad. This document adopts and extends that same posture rather than re-deriving
it independently — its own reasoning throughout (sections 3-9, 16) is compatible with and
builds on that prior conclusion, not a departure from it.

The wider audit also independently confirms **no Save Vault or save-state code exists anywhere
in `crates/`** (its §14.1, cross-referencing `docs/research/APOLLO_PS3_SAVE_VAULT_AUDIT.md`),
which matters for section 14 below (server-side controller evidence is not more trustworthy
just because a hypothetical future Save Vault or remote-play feature might want it).

### 1.4 No controller/gamepad code exists anywhere in `crates/` today

A repository-wide search for gamepad/joystick/evdev/libinput/SDL-game-controller/XInput/
DualShock/DualSense terms across `crates/` finds:

- `crates/archivefs-gui/src/gamer_view/rail.rs:281` — a doc-comment mentioning "keyboard/gamepad"
  navigation for a UI list-jump widget; no gamepad code, just a comment describing that the GUI's
  own keyboard focus behavior is meant to feel natural alongside a future gamepad-navigable UI.
  Not evidence of any controller subsystem.
- `crates/archivefs-core/src/patch_manager/amiga_whdload_local.rs:787` — a substring match on
  `"joystick"` inside heuristic text classification for WHDLoad install-script content
  (deciding whether a script mentions joystick ports as part of Amiga game metadata text
  matching), not a controller-capability model.
- `crates/archivefs-core/src/patch_manager/hatari_local.rs:252,658` — `joystick_ports:
  Vec<Option<String>>`, a field describing the **Atari ST/STE's physical joystick port
  wiring** as part of Hatari's machine-configuration surface (which physical port an Atari
  joystick would plug into on the emulated machine), not a model of what controller the user
  has connected to their PC or whether EmuWiz can see it.

**None of these constitute a controller-presence, controller-capability, or controller-mapping
evidence source.** There is no SDL binding, no evdev/libinput binding, no `/dev/input` reading,
no RetroArch-autoconfig parsing, no per-emulator pad-profile parsing, and no MAME `<input>`/
`<control>` element parsing anywhere in `crates/` today.

### 1.5 A naming collision to flag explicitly: GUI "controller" files are MVC controllers, not gamepads

A repository search for `controller.rs` turns up several GUI files —
`crates/archivefs-gui/src/emulator_setup/controller.rs`,
`crates/archivefs-gui/src/doctor_repair/controller.rs`,
`crates/archivefs-gui/src/romm/controller.rs`,
`crates/archivefs-gui/src/cheats_mods/controller.rs`, and others — that use "controller" in the
**software-architecture (MVC) sense**: a module that mediates between a page's view state and
its model/business logic. **These have nothing to do with game controllers.** This is a pure
naming collision, noted here explicitly so a future `grep -r controller` pass over this codebase
is not mistaken for turning up gamepad-input code. None of these files were read in depth for
this audit beyond confirming, by path and by their being paired with a `view.rs`/`model.rs` in
the same directory, that they are MVC controllers.

### 1.6 MAME metadata EmuWiz already models — and the gap in it

**CONCLUSION FROM SOURCE:** EmuWiz already has a MAME `-listxml` importer
(`crates/archivefs-core/src/identity_source/mame_listxml/import.rs`,
`identity_source/mame_listxml/convert.rs`) and a read-only installed-MAME compatibility
projection (`crates/archivefs-core/src/arcade_mame_compatibility.rs`, HEAD revision read via
`git show HEAD:...` since this file has uncommitted, unrelated in-progress changes in this
shared worktree that were not read). `import_mame_listxml()` (`import.rs:81-111`) routes the
raw `-listxml` XML through the **generic DAT parser** (`parse_dat_file`, shared with every other
DAT ecosystem EmuWiz supports), producing a `ParsedDat` built from the generic `DatGameEntry`
model (`crates/archivefs-core/src/dat/model.rs:331-361` — full field list read: `name`, `id`,
`description`, `roms`, `clone_of`, `rom_of`, `sample_of`, `is_bios`, `is_device`, `runnable`,
and further ROM/checksum/software-list fields). **`DatGameEntry` has no field for `<input>`,
`<control>`, `players`, `coins`, or any input-related MAME XML element.** The generic DAT parser
is not an XML-schema-aware MAME parser in that sense — it extracts the game/ROM/dependency
subset of the schema that DAT-style tooling needs (identity, checksums, clone/parent/BIOS/device
relationships) and, as far as this reading shows, does not retain `<input>` at all.

**INFERENCE, with evidence:** this means MAME's per-machine `<input>` metadata is not merely
"unprojected" today — it is very likely **discarded at parse time**, because the shared parser's
output type has nowhere to put it. Turning MAME `-listxml` into a controller/input evidence
source would therefore require **new parsing** (extending the DAT model or building a
parallel, input-specific MAME XML reader), not just a new Ready-to-Play projection over data
that is already sitting in memory. This is a materially different (larger) first step than
"write a projection," and is stated explicitly in section 4 below.

### 1.7 Doctor / diagnostics and the Attention model — how a future signal would plug in

`crates/archivefs-core/src/diagnostics/mod.rs` defines `Finding{id, category, subsystem,
severity, title, explanation, why_it_matters, next_step, evidence, affected, recovery, repair,
measurements}` (read at `mod.rs:369-420` per the Ready-to-Play audit's own citation, reconfirmed
here) and `DoctorSeverity::{Healthy, Info, Warning, Error, Critical}` with an `ACTIONABLE`
ranking and an `is_blocking()` predicate (`mod.rs:103-113` and onward). `diagnostics/` is
organized as bounded per-subsystem checks (`arcade_dat_version.rs`, `arcade_version_probe.rs`,
`environment.rs`, `managed.rs`, `profiles.rs`, `repair/`, `verified_identity/`) aggregated by a
pure `run_doctor_scan` in `runner.rs`, with an explicit `CoverageStatus`/`DEFERRED_CHECKS`
not-checked vocabulary (cited by the prior audit at `diagnostics/mod.rs:582-661`).
`crates/archivefs-core/src/attention.rs` (520 lines) defines `AttentionSeverity::{Blocking,
ActionNeeded, Warning, Info}` and a 13-member `AttentionCategory` enum (`Sources, Identity, Dat,
Duplicates, Repair, Emulator, Launch, Publication, CheatsMods, Recovery, Conversion,
Unsupported, Operations` — `attention.rs:60-74`), plus producer functions such as
`doctor_attention()`/`operation_attention()` that project Doctor findings and operation receipts
into a bounded, deduplicated `AttentionSnapshot`. `crates/archivefs-gui/src/needs_attention.rs`
(517 lines) is the GUI's consumer/aggregator; it already shows the pattern a future input signal
would need to follow — e.g. `append_dat_attention()` (`needs_attention.rs:12-60`, read in
full for its shape) turns a `DatAuthorityDashboard` row into an `AttentionItem` with an explicit
severity derivation and a `source_workflow`/`source_records` provenance trail, never inventing
severity from absence of data.

**INFERENCE:** a future `InputReadiness` producer would plug in exactly the same way an
existing family does today — as a pure projection function analogous to `doctor_attention()` or
`append_dat_attention()`, emitting `AttentionItem`s tagged with a category. **No new
`AttentionCategory` variant is obviously required**: `Emulator` (a controller-profile
configuration problem is arguably an emulator-configuration problem) or `Launch` (a per-game
readiness concern) both fit the existing 13-category vocabulary without extension, mirroring
how Ready-to-Play's own reason families reuse `AttentionSeverity` rather than inventing a third
severity scale (Ready-to-Play audit §5.1). This document does not pick one category over the
other — that is an implementation-phase decision — but notes that **no evidence found in this
pass suggests the Attention model needs a new category to carry an input-readiness signal**,
which lowers the cost of a future, cautious rollout.

## 2. Input requirement model — evidence tiers, not booleans

The task brief's own framing is right to insist that requirement types be tiered rather than
assumed. This document classifies each requirement type against four evidence tiers:

| Tier | Meaning |
|---|---|
| **AUTHORITATIVE** | A primary, machine-readable source directly states the requirement for this exact game/machine (e.g. a MAME `<control type="lightgun">` element for that specific `<machine>`). |
| **STRONG** | A reputable secondary source states it with enough specificity to trust for warning-level UX (e.g. a curated compatibility database explicitly listing "requires light gun" for a named game), but is not the emulator/platform's own primary metadata. |
| **HEURISTIC** | Inferable only via pattern-matching, genre inference, or platform-wide generalization (e.g. "this is a PS2 racing game, therefore probably wants analog triggers") — **never strong enough to warn or block, at most an internal hint for future curation work.** |
| **UNKNOWN** | No source consulted; the honest default for everything until a real source is wired. |

**Explicit rejection, stated in the task brief and independently endorsed here:** "every PS2
game requires DualShock 2 analog features" is an unsafe HEURISTIC-tier generalization, not a
requirement fact. Concretely: many PS2 titles support digital-only D-pad play, some require
analog for specific mechanics (DualShock 2's pressure-sensitive/analog face buttons matter for a
minority of titles, e.g. certain fighting or rhythm titles reading button pressure), and the
correct per-title answer is only ever knowable from a real source (a game's own manual/box
requirements text, a curated compatibility note, or in rare cases explicit `SCE_CONFIG`-style
in-disc metadata this project does not currently parse) — never from "this is a PS2 game."
Generalizing from platform to per-title requirement is exactly the failure mode Ready-to-Play's
own `IdentityStatus::Candidate` vs `Verified` distinction and its filename-only-evidence refusal
(§1.3 above, and Ready-to-Play audit §3.1) already teach EmuWiz not to make elsewhere; the same
discipline applies here.

Candidate requirement *types* (not yet tiered per-game — this is the taxonomy, not a data set):

| Requirement type | Typical evidence source if one existed | Notes |
|---|---|---|
| Digital d-pad/buttons only | Near-universal default; rarely worth asserting | Baseline, not a distinguishing fact |
| Dual analog sticks | Genre/manual/compat-DB; MAME `stick`/`joy` controls for arcade | Common on 3D-era consoles, not universal |
| Analog triggers | Manual/compat-DB; rare in MAME arcade metadata | Racing/shooter titles most commonly |
| Pressure-sensitive buttons | Manual/compat-DB only; not modelled by MAME (console-specific) | DualShock 2-era titles only, a minority |
| Mouse | MAME `control type="mouse"`; DOS/PC platform convention; compat-DB | Common on PC-platform adapters (DOSBox) |
| Keyboard | Platform convention (Amiga/PC/DOS/home-computer adapters) | Not "controller" evidence per se |
| Lightgun | MAME `control type="lightgun"` (AUTHORITATIVE-tier candidate) | Strongest MAME-modelled special peripheral |
| Wheel / pedals | MAME `control type="pedal"`/`paddle`-adjacent; compat-DB | Cabinet-original wheel often optional vs pad |
| Trackball | MAME `control type="trackball"` | Historically some titles genuinely require it (e.g. *Centipede*-class) |
| Spinner/dial | MAME `control type="dial"` | Similar to trackball |
| Touchscreen | Platform convention (3DS/DS/NDS-family, Switch handheld) | Emulator-side touch-injection is a separate concern from requirement |
| Microphone | Compat-DB / genre only; no MAME element seen for this in this pass | Niche (e.g. DS/Wii microphone minigames) |
| Motion/gyro | Platform convention (Wii/Switch); compat-DB | Emulated-vs-real distinction matters (Dolphin/Ryubing sections) |
| Multitap / multi-controller count | MAME `players` count (AUTHORITATIVE for arcade); console convention elsewhere | Distinct from "requires 2+ controllers to *play*" vs "*supports* up to 4" |
| Special peripheral (guitar/drums/dance mat/steering yoke/arcade-specific panel) | Compat-DB only; no generic MAME element models these individually beyond generic control types | Highest-specificity, lowest-coverage tier |

## 3. Game-level requirement sources

| Source | What it can prove | License / access | Machine-readable? | Granularity |
|---|---|---|---|---|
| MAME `-listxml` `<input>`/`<control>` | Per-machine (not per-ROM-set-clone-variant in all cases) cabinet control requirements, AUTHORITATIVE for what the *original cabinet* had | MAME itself is GPL-2.0; the `-listxml` output is generated locally by the user's own installed MAME, not redistributed by EmuWiz | Yes, XML | Per-machine |
| DAT metadata (No-Intro/Redump/MAME) | Identity/checksum only for No-Intro/Redump; MAME DATs (via `-listxml`) are the only DAT-family source with input metadata at all, per §1.6/§4 | Varies by publisher; No-Intro/Redump conventionally CC0/public-domain-style dat text, verify per-publisher | Yes, XML/DAT | Per-title/per-machine |
| Compatibility databases (e.g. community-run wikis, RetroAchievements-adjacent notes, LaunchBox/ES-DE community metadata) | Curated, often-accurate per-title notes when present, but coverage is partial and provenance/trust varies by contributor | Typically community-licensed or informal; **not independently checked in this pass for a specific redistribution-safe source** — flagged **UNCERTAIN** | Sometimes (structured fields), often prose | Per-title, inconsistent |
| Emulator compatibility DBs (e.g. RPCS3/Dolphin/PCSX2 compat lists) | Whether a title *runs*, sometimes annotates input quirks in freeform notes | Project-specific, generally viewable but not a machine-readable input-requirement schema | Mostly no (prose) | Per-title, inconsistent |
| LaunchBox/ES-DE metadata (gamelist.xml, LaunchBox XML) | Community-curated per-game metadata fields exist (genre, players, rating) but **no dedicated input-requirement field was found in this pass**; ES-DE's own controller-icon-in-UI feature is a display convenience, not a requirement source — flagged **UNCERTAIN, not confirmed against a specific schema version this pass** | GPL-family (both projects) | Yes for the schema itself, no for a controller-requirement field specifically | Per-title where present |
| RetroAchievements metadata | Achievement/game metadata, not input-requirement metadata as far as searched | RetroAchievements site terms, not independently reviewed this pass | Yes (API) | Per-title, wrong domain |
| Game manuals / box text | Historically the actual authoritative *human* source ("requires analog controller (DualShock)" stickers on PS1/PS2 boxes are real and well known) | Not machine-readable without OCR/transcription, and copyright status varies per publisher | No, unless transcribed by a curated project | Per-title |

**INFERENCE:** of everything surveyed, **MAME `-listxml` is the only source in this table that
is (a) already partially plumbed into EmuWiz's importer pipeline, (b) machine-readable, (c)
locally generated by the user's own trusted installed MAME rather than fetched from a third
party, and (d) AUTHORITATIVE-tier for the one thing it actually claims (what the original
cabinet's control panel had)** — which supports the task brief's instinct that it's the
strongest per-machine candidate. This document nonetheless pushes back on treating it as
sufficient on its own (§4, §12.3 above): a cabinet's original controls are a fact about the
cabinet, not a proof about what's required for *emulated, pad-based play today*, and MAME's own
documentation and long-standing frontend convention (e.g. `controls.dat`-style community
projects layering a second "recommended alternate control" annotation on top of raw
`-listxml` data) reflects exactly this gap — the raw XML is necessary but not sufficient.

## 4. MAME input metadata deep-dive

**Depth of this section:** MAME's own canonical `-listxml`/DTD documentation page
(`docs.mamedev.org/commandline/commandline-all.html`) was fetched directly in this pass and
does **not** itself spell out the full `<input>`/`<control>` element schema inline — it
describes `-listxml`'s purpose and shows only an abbreviated example. The structure below
reflects **GENERAL KNOWLEDGE**: the `<input>`/`<control>` schema is long-standing, stable, and
consistently described across MAME's own source-level DTD comments and the many independent
frontend/tooling projects built against it over two decades (e.g. the `controls.dat`
project referenced by arcade-cabinet-building communities, and numerous ROM-manager/frontend
parsers). It was **not** independently re-verified byte-for-byte against MAME's XML-generation
source in this pass — flagged **UNCERTAIN at the byte-for-byte level**, though the shape below
is widely corroborated and low-risk to state at this level of generality.

Per-machine, `-listxml` emits an `<input>` element roughly of the shape:

```xml
<input players="2" coins="2" service="no" tilt="no">
  <control type="joy8way" player="1" buttons="1" ways="8"/>
  <control type="joy8way" player="2" buttons="1" ways="8"/>
</input>
```

- `players` — how many simultaneous players the machine supports (a count, not "requires N to
  start").
- `coins` — coin slots, arcade-specific, not meaningful for EmuWiz's home-emulation context.
- `service`/`tilt` — service-panel/tilt-switch presence, operational metadata, not player-input
  requirement.
- Nested `<control>` elements, one or more per machine, each with a `type` attribute drawn from
  a closed vocabulary that (GENERAL KNOWLEDGE, cross-referenced across multiple independent
  descriptions of the schema) includes at least: `joy` (generic joystick), `joy2way`, `joy4way`,
  `joy8way` (digital joystick with a `ways` restriction), `doublejoy8way` (twin-stick), `stick`
  (analog joystick), `paddle`, `dial`, `trackball`, `lightgun`, `pedal`, `positional`, `mouse`,
  `keypad`, `keyboard`, `mahjong`, `hanafuda`, and gambling-specific control types. Attributes
  commonly include `player`, `buttons`, `ways` (rotation granularity for digital sticks), and
  sometimes `minimum`/`maximum`/`sensitivity`/`keydelta` for analog/dial-type controls.

**What this metadata proves (if parsed):** exactly and only what the **original arcade cabinet's
control panel physically had**, per machine, as declared by MAME's own driver source (MAME
driver authors hand-declare this per machine as part of getting the driver accepted upstream —
GENERAL KNOWLEDGE, not independently re-verified against MAME driver source this pass).

**What it does not prove:**

1. That the *emulated* experience on a generic gamepad is unplayable without the original
   peripheral — many `trackball`/`dial`/`paddle` games are commonly played, imperfectly but
   functionally, with an analog stick; some (documented, real exceptions — classic
   trackball/spinner titles being the most commonly cited) genuinely degrade badly without the
   real peripheral. MAME's own metadata does not distinguish these two cases; that distinction
   would need a second, curated layer (exactly what community "recommended controls" databases
   exist to provide, per §3).
2. Anything about non-arcade platforms — this metadata is arcade-only; it says nothing about a
   PS2, GameCube, or Switch title's controller requirements.
3. Whether the currently-installed MAME build's driver for a given machine has the input
   correctly declared, is a placeholder/skeleton driver, or has known input-emulation bugs.
4. Per §1.6, whether EmuWiz's own importer currently retains any of this at all — it does not.

**Verdict on the task brief's suggestion that this is EmuWiz's strongest per-machine evidence
source: largely confirmed, with one material qualification.** It is the strongest *candidate*
by the four properties named in §3's inference (local, trusted, structured, already
half-plumbed), but "strongest candidate" is not the same as "sufficient by itself" — see §12.3
of the prior Ready-to-Play audit (quoted in §1.3 above) and the two-layer point in item 1 just
above. A future implementation should treat raw `-listxml` `<control>` presence as AUTHORITATIVE
evidence of **cabinet-original control hardware**, and keep that conceptually and in the type
system separate from any (currently nonexistent) "requires this to be playable on a pad" claim.

## 5. SDL game-controller evidence

**DOCUMENTED FACT** (SDL2 wiki, fetched via search-result summary in this pass): SDL's
GameController API identifies each connected device by a GUID (from
`SDL_JoystickGetGUIDString()`), matches it against a mapping database (the community-maintained
`SDL_GameControllerDB`, MIT-family/public-domain-style licensing, GPL-compatible for EmuWiz's
purposes since EmuWiz would only ever *read* such a database, not redistribute SDL itself
differently than upstream), and exposes axis/button capability once a mapping is found.
`SDL_GameControllerAddMapping`/`AddMappingsFromRW` load mappings at runtime; hotplug is
supported via `SDL_CONTROLLERDEVICEADDED`/`REMOVED` events, and a mapping loaded before
`SDL_Init` still fires an added-event retroactively for already-plugged-in matching devices.

**What SDL evidence can prove:** a device is currently connected to *this machine* (not
necessarily this session's compositor/seat — see §6), a mapping exists in the loaded database
for its GUID (or not — "unmapped" is itself informative), and which axes/buttons the mapping
claims are present.

**What it cannot prove:**

- That the specific *emulator* EmuWiz would launch is actually bound to that device — SDL
  enumeration is orthogonal to any given emulator's own device-selection state (its own
  autoconfig/profile file, which EmuWiz does not currently read either — §7-§11).
- That Steam Input, if active, has not already transformed what the emulator actually sees
  (§6).
- That the device will still be connected at the moment of launch (a later section addresses
  static-vs-runtime timing).
- Anything about a *specific game's* requirement — SDL is capability evidence about a device,
  never requirement evidence about a game.

## 6. evdev/libinput — deliberately not a default source

**GENERAL KNOWLEDGE**, cross-checked via freedesktop.org's libinput documentation and systemd's
own device-permission issue tracker in this pass: evdev exposes raw `/dev/input/eventN` device
nodes; libinput classifies devices via udev properties (`ID_INPUT_JOYSTICK`,
`ID_INPUT_KEYBOARD`, `ID_INPUT_MOUSE`, etc.) and assigns a "seat" (`seat0` by default).
Permission to read a raw evdev node is normally gated by the `input` group or a udev
`uaccess`/logind ACL rule tied to an active graphical session — i.e. it is more
permission-sensitive than SDL's higher-level enumeration, and a process without an active seat
grant (headless, over SSH, inside certain sandboxes) may simply not be able to open the device
node at all, producing a false "absent" read rather than an honest "cannot check."

**Recommendation: do not use evdev/libinput directly as a default EmuWiz evidence source.**
Justification:

1. **Redundant with SDL for EmuWiz's purpose.** Everything a controller-*presence-and-capability*
   check needs (device present, button/axis count, GUID identity) is already exposed at a
   higher, more portable level by SDL's GameController API, which EmuWiz would need for any
   direct device check regardless of platform.
2. **Permission-sensitive in a way that produces misleading negatives, not honest unknowns.** A
   permission failure on `/dev/input/eventN` looks, at the raw syscall level, similar enough to
   "no such device" that a naive implementation risks reporting `DEVICE_NOT_PRESENT` when the
   true state is `EVIDENCE_UNAVAILABLE` (no permission to check) — precisely the "unknown
   collapsed into missing" failure mode the Ready-to-Play audit already warns against
   elsewhere (its §4.4, the `Unknown`-never-outranks-a-proven-blocker rule, and its explicit
   distinction between "not gathered" and "missing").
3. **Lower-level than EmuWiz needs for anything in this document's proposed scope.** Nothing in
   §2's requirement taxonomy needs raw evdev capability bits (e.g. `EV_ABS`/`EV_KEY` bitmaps);
   SDL's already-classified button/axis model is sufficient for every use case surveyed here.

If a future maintainer finds a concrete case SDL cannot cover (e.g. distinguishing a genuine
analog trigger from a digital-only button on a device SDL maps ambiguously), that would be a
narrow, justified exception — not a reason to make evdev a default enumeration path.

## 7. Steam Input

**DOCUMENTED FACT / GENERAL KNOWLEDGE**, cross-checked across Steamworks' own gamepad-emulation
best-practices documentation and independent community sources describing the double-input
problem in this pass: Steam Input intercepts a physical controller and, when active, presents
games with a **virtual, remapped device** instead — on Windows via overlay injection into
XInput/DirectInput/RawInput/Windows.Gaming.Input APIs (not a real virtual driver at the OS
level the way it is on Linux/macOS), and on Linux/macOS via an actual virtual gamepad device.
The practical, well-documented consequence: **a game (or, by extension, an emulator) that reads
"what controller is connected" while Steam Input is active in most configurations sees a
Steam-virtualized Xbox- or DualShock-shaped device, not the physical controller's own GUID,
name, or exact capability set.** Community sources describe a "double input" failure mode
specifically because a naive enumeration can see *both* the raw physical device and the Steam
virtual device simultaneously, and tooling like HidHide exists precisely to force games to see
only one of the two by hiding the raw HID device from everything except the Steam client.

**Confirming the task brief's flagged finding:** physical controller identity does **not**
reliably survive Steam Input's virtualization layer. Any EmuWiz evidence step that enumerates
"connected controllers" while Steam Input is active for that session risks reporting a
Steam-virtual device's identity/capabilities (or, if the raw device is hidden, nothing at all)
rather than the user's actual physical hardware — and per-game Steam Input *layouts* add a
further transformation on top (a physical d-pad might be remapped to virtual analog input, or
vice versa, entirely outside any evidence EmuWiz could gather from the device side).

**What is provable:** that a controller was detected by SDL/the OS *before* any Steam Input
transformation, if EmuWiz's own check runs independently of Steam Input's hook layer (feasible
in principle, since SDL enumeration happens against the OS device layer Steam Input intercepts
downstream of, not upstream of — **UNCERTAIN**, this ordering is plausible from the sources read
but not independently verified against Steam Input's exact hook point in this pass).

**What is not provable:** what the *game/emulator* ultimately receives once Steam Input's
per-game layout has been applied. This is a hard boundary, not a gap to be closed by more
enumeration — it would require reading Steam's own runtime layout state, which is out of
EmuWiz's scope entirely by this document's own non-goals.

## 8. RetroArch input model

**DOCUMENTED FACT**, Libretro's own controller-autoconfiguration documentation (fetched via
search-result summary, `docs.libretro.com/guides/controller-autoconfiguration/` and the
`libretro/retroarch-joypad-autoconfig` repository, both read at summary depth in this pass):
RetroArch identifies a controller for autoconfig purposes by **vendor ID, product ID, and
device name** together; matching autoconfig `.cfg` files (three parts: a match header, a
RetroPad-button mapping, and optional display-label input descriptors) live per
`[profile directory]/[joypad driver]/[device index].cfg`. Per-core and, separately, per-game
**remap files** exist as a further mapping layer on top of the base RetroPad abstraction.
RetroArch's "RetroPad" is itself an abstraction layer — every physical device is mapped *into*
RetroPad's fixed button/axis set before a libretro core ever sees input, meaning **the same
physical controller, differently autoconfigured, can present entirely different capabilities to
a core** depending on the loaded `.cfg`.

**Three distinct evidence axes this document recommends never collapsing into one:**

1. **"A device is currently connected"** — OS/SDL-level fact, ephemeral, runtime.
2. **"A configured mapping exists for this device"** — a static fact about whether an
   autoconfig `.cfg` (or a manually-bound `input_player*_*` entry in `retroarch.cfg`) exists on
   disk for that vendor/product/name combination. This is checkable **without** the device being
   connected right now.
3. **"The game/core declares support for this input shape"** — a libretro core's own declared
   input capability (RetroPad basic vs analog vs pointer/lightgun-class cores), which is a
   property of the core, not the device or the mapping.

These three are independently knowable and independently absent-able; a device can be connected
with no mapping configured, a mapping can exist for a device that's not currently plugged in,
and a core can lack analog/lightgun support regardless of what's connected or configured. Any
future EmuWiz model should keep these as separate typed facts (mirroring the six-way split in
§10 below), never one boolean "controller ready" flag.

## 9. Per-emulator config-only evidence surveys

Each entry below states what can be determined **from configuration files alone, without
launching the emulator** — consistent with Ready-to-Play's own "no probing, no I/O beyond
already-gathered evidence" discipline (§0 of this document; `ready_to_play.rs:1-5`). Depth is
GENERAL KNOWLEDGE cross-checked by search in this pass unless otherwise noted; none of these
formats were read from a canonical upstream schema document end-to-end.

### 9.1 Dolphin (GameCube/Wii)

`GCPadNew.ini` and `WiimoteNew.ini` hold global GameCube-pad and Wiimote profiles; per-game
overrides live in a game's own `<GameID>.ini` under a `[Controls]`-style section (e.g.
`PadType0`) that can point at a named profile. **What's provable from config alone:** whether a
profile is assigned per-game vs relying on the global default, and whether the profile names a
real vs Bluetooth-passthrough Wiimote source. **Not provable:** whether the *game itself*
requires motion control that an emulated (non-motion-capable) Wiimote profile cannot satisfy —
that is a per-title fact Dolphin's own compatibility notes sometimes carry, not something in the
INI itself.

### 9.2 PCSX2 (PS2)

Pad configuration selects a backend (SDL/XInput/DirectInput-family) and per-pad mapping, with
per-game override support (GENERAL KNOWLEDGE; not independently re-verified against current
PCSX2 Qt config schema in this pass — flagged **UNCERTAIN at the exact-current-format level**,
PCSX2's config format has changed across major UI rewrites). **What's provable:** which backend
and mapping a game's override (if any) selects. **Not provable:** whether a title actually needs
DualShock 2 pressure-sensitivity or analog-only modes (§2's explicit warning against assuming
this from platform alone) — that is a per-title fact, not visible in the pad-config file itself.

### 9.3 RPCS3 (PS3)

**DOCUMENTED FACT**, RPCS3's own wiki controller-configuration page (search-result summary read
in this pass): RPCS3 exposes multiple pad handler backends (SDL, DualShock 3/4, DualSense,
XInput, evdev on Linux) through one Pad Settings dialog, and **the handler choice itself gates
feature availability** — e.g. the XInput handler is explicitly documented as working for basic
input but **not** supporting motion controls or pressure-sensitive buttons, while the
DualShock4/SDL-family handlers do. RPCS3 supports per-game pad configuration (its own config
profile can be overridden per title). **What's provable from config alone:** which handler is
selected, and therefore — because handler choice is itself documented to gate motion/pressure
support — a config-only fact can legitimately say "this handler cannot deliver motion/pressure
even if the game wants it," which is a genuinely stronger, provable claim than most of the other
emulators surveyed here. **Not provable:** whether the specific game requires motion/pressure at
all (still a per-title requirement fact EmuWiz has no source for, per §3).

### 9.4 PPSSPP (PSP)

Control mapping is a general key/button-to-PSP-button map, typically global rather than
per-game in common usage (GENERAL KNOWLEDGE, not independently re-verified against current
PPSSPP config schema in this pass — **UNCERTAIN**). **What's provable:** whether any mapping is
configured at all. **Not provable:** per-game requirement (PSP titles have far less input
diversity than console/arcade platforms generally, lowering the practical stakes here).

### 9.5 xemu (original Xbox)

**GENERAL KNOWLEDGE:** xemu is SDL-based and, by design and community convention, targets an
Xbox-controller-shaped default mapping (unsurprising, given original-Xbox software assumed an
Xbox controller). **What's provable from config alone:** essentially nothing beyond "an SDL
mapping is or isn't configured" — xemu's controller model is comparatively simple relative to
RPCS3/Dolphin's per-game complexity. **Not independently verified in this pass beyond
search-summary depth — UNCERTAIN on exact config file format/location.**

### 9.6 Azahar (3DS, Citra-family fork)

**GENERAL KNOWLEDGE, low confidence — flagged UNCERTAIN, not independently read this pass
beyond general awareness that Azahar is a maintained Citra-derived 3DS emulator.** 3DS-family
input includes a touchscreen and, on New3DS hardware, a second analog "C-stick" plus gyro —
config-only evidence would at best show whether a touch/gyro binding exists, never whether a
given title's use of the touchscreen/gyro is optional (most 3DS titles use touch for menus only)
or load-bearing (a minority of titles use gyro or touch as a core mechanic). This document does
not have enough independently-verified depth on Azahar specifically to say more than that its
config format was not read in this pass.

### 9.7 Ryubing (Switch, Ryujinx-derived fork)

**DOCUMENTED FACT (from Ryubing's own site and search-summary, read in this pass) with an
important honesty caveat the task brief specifically asked to surface:** Ryubing describes
itself as a continuation of the Ryujinx codebase after Ryujinx's original project ended in
October 2024, under an MIT license, focused on "maintenance, compatibility work and practical
quality-of-life improvements." It supports keyboard, mouse, touch, and "nearly all controllers,"
including Joy-Con input, with gyroscope-based motion control configurable in its input-config
menu (adjustable sensitivity/deadzone) — but **dual-JoyCon motion specifically is documented as
still needing a third-party bridge (DS4Windows or BetterJoy)**, i.e. the project's own
documentation admits a real gap rather than claiming full native dual-JoyCon motion support.
**Public documentation for Ryubing specifically (as opposed to its Ryujinx ancestor) is thin** —
most search results resolve to either the Ryubing project's own marketing/download pages or to
older Ryujinx-era guides; no independent third-party technical writeup of Ryubing's exact config
file format was found in this pass. **Honest depth statement:** everything above is
search-summary depth (page titles/snippets), not a full read of Ryubing's own documentation
site or source; this document does not claim more precision than that.

## 10. Special peripherals — proposed states (design only)

For lightgun, wheel, pedals, guitar/drums, microphone, dance-mat, multitap, motion, touch,
mouse/keyboard, and arcade-specific control-panel peripherals, this document proposes (design
only, no implementation) a small closed state set distinct from Ready-to-Play's own states, to
avoid conflating "can this game launch" with "does the ideal peripheral exist":

| State | Meaning |
|---|---|
| `SUPPORTED` | The emulator/core is capable of this input class in principle (e.g. RPCS3's SDL handler supports motion) — says nothing about this specific game or device |
| `CONFIGURED` | A profile/mapping exists on disk that binds *something* to this input class for this emulator/game |
| `NOT_CONFIGURED` | No such binding exists, though the emulator could support one |
| `DEVICE_NOT_PRESENT` | A device capable of this input class is not currently detected (runtime fact, never a static one) |
| `SPECIAL_PERIPHERAL_REQUIRED` | A real, provenance-carrying source (AUTHORITATIVE or STRONG tier, §2) asserts this game specifically requires this peripheral class |
| `UNKNOWN` | No evidence gathered — the honest default, per §1.3's inherited posture |

**Rule, mirroring §1.3's inherited conclusion:** `SPECIAL_PERIPHERAL_REQUIRED` may only ever be
set from an AUTHORITATIVE or STRONG source (§2's tiers); it must never be derived from
HEURISTIC-tier inference (genre, platform, or cabinet-original-controls-alone per §4's caveat).

## 11. Physical presence vs configuration — six independent axes

The task brief's six-way distinction is correct and this document designs each as an
independent, separately-knowable fact — never collapsed into one boolean:

| Axis | Question | Static or runtime | Who could prove it (future) |
|---|---|---|---|
| **A. Requirement known** | Does *this game* need a specific input class at all? | Static (per-title metadata) | §3 sources, tiered per §2 |
| **B. Emulator profile configured** | Does the target emulator have a profile/mapping on disk for this input class, for this game or globally? | Static (config-file read) | §9's per-emulator surveys |
| **C. Controller capability known** | Is there a device (connected now or previously seen) whose *capabilities* are known (buttons/axes/motion)? | Either (SDL enumeration is runtime; a remembered device's last-known capability could be cached) | §5 (SDL) |
| **D. Controller physically present now** | Is a capable device connected *right now*? | Strictly runtime | §5 (SDL), never evdev by default (§6) |
| **E. Controller accessible to the emulator** | Even if present, can the emulator actually see/claim it (not hidden by Steam Input, not claimed by another process, permission granted)? | Runtime, and specifically **not provable server-side in a remote-streaming topology** (§14) | Bounded, on-demand only |
| **F. Game-specific mapping valid** | Does the concrete button/axis mapping for *this* device, in *this* emulator, for *this* game, make sense (e.g. an analog-only game bound to a digital-only device)? | Static once B and C are both known, but requires cross-referencing both | Combination of B + C, never inferred from either alone |

**INFERENCE:** collapsing any two of these into one boolean is exactly the failure mode this
document exists to prevent. In particular, A and D are the two most commonly conflated in
consumer UX ("this game needs a wheel" + "no wheel is plugged in" → naive "Controller missing"
message) even though neither alone justifies the message, and their conjunction still only
justifies a *warning*, not a block, per §13.

## 12. Proposed evidence model (design only — conceptual, not implemented)

**Explicit decision, argued below: this model should stay a deliberately distinct vocabulary
from Ready-to-Play's own types, connected only through a thin, optional projection — not
merged into `ReadinessReasonFamily`/`ReadyToPlayState` internals.**

Justification, grounded in `ready_to_play.rs` itself: Ready-to-Play's existing per-family
evidence-state enums (`IdentityEvidenceState`, `MediaEvidenceState`, `EmulatorEvidenceState`,
`ModEvidenceState`, `ready_to_play.rs:63-96`) are each *small and specific to what that family
can actually prove* — they are not one shared shape reused verbatim. Input evidence has a
materially different shape from any of them: it is the *only* family in this document's scope
with the six-axis structure of §11, and the *only* one where a proven-absent runtime fact (D)
must never promote to a blocking state (§13) even when a requirement is AUTHORITATIVELY known
(A) — a combination none of the existing evidence-state enums need to express, because none of
Ready-to-Play's other families have a legitimate "known-required, known-currently-absent, still
must not block" case (missing firmware, by contrast, *should* block once known, because firmware
doesn't reconnect on its own the way a controller can). A single dedicated `InputReadiness` type
family is more honest than stretching an existing enum to cover a case it wasn't designed for.

Illustrative, **non-normative** pseudocode (no such types exist in the repository):

```text
// DESIGN ONLY — illustrative shape, not Rust to be compiled or copied verbatim.

InputRequirement {
    requirement_type: InputRequirementType,   // one of §2's taxonomy
    evidence_tier: EvidenceTier,              // Authoritative | Strong | Heuristic | Unknown
    provenance: ProvenanceRef,                // e.g. "MAME -listxml <control> for <machine>"
}

InputCapability {
    // what a *device* can do, from SDL enumeration only (§5) — never evdev by default
    device_guid: Option<String>,
    buttons: Option<u8>,
    has_dual_analog: bool,
    has_motion: bool,
    mapping_known: bool,                      // SDL_GameControllerDB match, yes/no
}

InputProfileEvidence {
    // axis B of §11 — static, config-file-only
    emulator: EmulatorId,
    game_specific: bool,
    configured_input_classes: Vec<InputRequirementType>,
}

InputDeviceEvidence {
    // axis D/E of §11 — runtime, on-demand only, never persisted as truth (§15)
    observed_at: Timestamp,
    present: bool,
    capability: Option<InputCapability>,
    steam_input_active: Option<bool>,         // Unknown by default, never assumed false
}

InputReadiness {
    state: InputReadinessState,               // Ready | ReadyWithWarnings | NeedsAttention
                                               // | Unsupported | Unknown  (mirrors ReadyToPlayState's
                                               // vocabulary deliberately — see justification below)
    reasons: Vec<InputReadinessReason>,
    coverage: InputEvidenceCoverage,          // which of A-F above were actually gathered
}
```

**On state-name vocabulary specifically: mirror `ReadyToPlayState`'s five relevant names
(`Ready`, `ReadyWithWarnings`, `NeedsAttention`, `Unsupported`, `Unknown`) but omit `Blocked`.**
This is a deliberate, narrow divergence, not a wholesale distinct vocabulary: reusing the same
words avoids the "three spellings for the same concept" problem the Ready-to-Play audit itself
identified as a pre-existing library-wide problem worth fixing (its executive summary, point 2).
`Blocked` is omitted because §13 concludes input evidence must never independently produce a
launch-blocking state — carrying the word `Blocked` in this type's own vocabulary would invite a
future caller to wire it in as a blocker by habit. Everything else about the type
(`InputRequirement`, `InputCapability`, `InputProfileEvidence`, `InputDeviceEvidence`) is new and
specific to input's six-axis shape, per the justification above — those are not modeled anywhere
in Ready-to-Play today and would gain nothing from trying to fit Ready-to-Play's existing
per-family shapes.

## 13. Ready-to-Play boundary — the explicit A/B/C answer

The task brief poses three options: fully separate, warnings-only contribution, or
block-only-when-a-proven-required-peripheral-is-known-absent. **Answer: option (c), narrowly
construed, with (b) as the default day-to-day behavior and (a) as the fallback whenever evidence
is thin.**

Reasoning:

- **Never (a) fully separate, forever.** A `ReadinessReasonFamily::Controller` variant already
  exists in shipped code (§1.1) specifically so that a bounded, honest input signal has
  somewhere to surface *inside* the existing reason vocabulary users already read. Leaving it
  permanently unused wastes that existing design intent without a compelling reason.
- **(b) warnings-only is correct for the overwhelming common case:** whenever a requirement is
  known (axis A) but the device's runtime presence (axis D) is merely unproven (not positively
  disproven) — e.g. "controller not detected right now" — this must be, at most,
  `ReadyWithWarnings`, because axis D is inherently transient (a controller can be plugged in
  seconds later) and Ready-to-Play must never punish a user for evidence that is honestly
  time-bound rather than proven false. This mirrors Ready-to-Play's own precedent that
  `Unknown` never masks or downgrades into a false negative (`ready_to_play.rs` tests,
  `unknown_does_not_become_missing_or_mask_a_blocker`, lines 501-526).
- **(c) `NeedsAttention` (never `Blocked`) applies only in the narrow, fully-proven case**: axis
  A is AUTHORITATIVE-or-STRONG-tier (never heuristic, per §2/§10), **and** the requirement is for
  a peripheral class no reasonable substitute exists for (a lightgun is the clearest real
  example — a standard gamepad genuinely cannot substitute for light-gun aiming in most
  lightgun titles), **and** axis B (no profile configured for that class) or axis D (device class
  never observed) is itself proven, not merely unscanned. Even then, this document recommends
  landing at `NeedsAttention`, never `Blocked` — consistent with `ReadyToPlayState`'s own
  definition of `NeedsAttention` as "something required and specific is missing... the user can
  act" (Ready-to-Play audit §4.1) rather than `Blocked`'s "EmuWiz cannot build a launch plan... or
  evidence positively contradicts launchability." A missing lightgun does not prevent EmuWiz from
  building and executing a launch plan; it only means the resulting session may not be usably
  playable — a materially different, softer claim.

**Two worked examples, as required by the brief:**

1. *No controller evidence gathered at all for a title.* → `InputReadiness::Unknown`, which,
   if surfaced through Ready-to-Play at all, must render as `UnknownEvidence`/coverage-note
   territory (Ready-to-Play's own `ReadinessReasonFamily::UnknownEvidence`), **never**
   `NeedsAttention` and certainly never `Blocked`. This is the default state for essentially
   every title today, since no evidence source is wired (§1.1, §1.6).
2. *MAME `<control type="lightgun">` proves a lightgun is part of the machine's original
   control panel, and no lightgun-capable device/profile is configured.* → This is the one case
   in this entire document where `NeedsAttention` (not `Blocked`) is justified, **and only once
   the §4 caveat is satisfied** — i.e. only after confirming (via a second, curated layer per §3,
   not raw `-listxml` alone) that this specific title is not one of the many lightgun-cabinet
   titles commonly played acceptably on a pad. Raw MAME metadata alone is necessary but,
   per §4's explicit qualification, not sufficient for this example to fire safely.
3. *A generic gamepad requirement is known and a gamepad is simply not connected right now* (the
   brief's third example) → `ReadyWithWarnings` at most, never `Blocked`, never even
   `NeedsAttention` — the device may connect at any moment before the user actually launches, and
   Ready-to-Play's own pure-projection contract (`ready_to_play.rs:1-5`, "deliberately performs
   no discovery, probing, hashing, I/O") means it would be evaluating stale evidence anyway; the
   live check, if any, belongs at pre-launch time (§14), not in the cached Ready-to-Play view.

## 14. Runtime vs static evidence

| Evidence | Static or runtime | When to collect |
|---|---|---|
| Requirement source (MAME `<input>`, curated compat notes) | Static | Ingested once, alongside other DAT/identity ingestion; cached like every other DAT-derived fact |
| Emulator profile/mapping configuration (axis B) | Static | Read alongside existing emulator-environment discovery (`diagnostics/profiles.rs`'s existing scan cadence), not a new independent scan |
| Device presence/capability (axis C/D) | Runtime | **On-demand only** — either an explicit user-triggered "check my controller" action, or immediately pre-launch as one more preflight fact alongside the emulator adapters' own existing pre-launch re-validation (`launch::execution`/`process_spawn` already re-validate identity fresh at launch time per the Ready-to-Play audit's §2.1 point 3 — a device check belongs in the same place, philosophically, not a new subsystem) |
| Device-accessible-to-emulator (axis E) | Runtime, and only ever locally meaningful | Never gathered server-side in a remote-streaming topology (§16) |

**Explicit rejection of a permanent polling daemon**, and the reasoning holds up under scrutiny:
a background poller watching for controller hotplug events would (a) contradict Ready-to-Play's
own stated purity/no-discovery contract if it fed the cached projection directly, (b) be the
first persistent background process of its kind in a codebase whose diagnostics model is
explicitly bounded, on-demand, and pull-based (`run_doctor_scan` is a pure function called on
demand, not a service), (c) provide no value over "check immediately before you need the
answer" for a UI concern whose whole purpose is answering "can I play *now*," and (d) introduce
exactly the kind of persistent physical-device-state tracking this document's DO-NOT-BUILD list
(§20) independently rejects on privacy/scope grounds. No source read in this pass — SDL's own
hotplug-event model included — gives a reason a *frontend application like EmuWiz* needs to run
a background poll rather than simply enumerating on demand; SDL's hotplug events are relevant to
a running game/emulator process that wants to react live, which is a different problem from
EmuWiz's own pre-launch UI check.

## 15. Save/cloud/remote-play implications

Per the Ready-to-Play audit's §14 (reconfirmed in §1.3 above), no Save Vault or remote-play code
exists in `crates/` today — this section is pure forward-looking research, unattached to any
existing EmuWiz feature.

**Core finding, confirmed by this pass's research (§ Sunshine/Moonlight search results in the
tool-use record above), and stated plainly because the task brief specifically flags it as the
key point:** in a Sunshine (host) + Moonlight (client) remote-play topology, **the host
("server") does not see the client's physical controller at all.** Moonlight captures input on
the *client* device and streams it to Sunshine, which recreates it on the host as a **virtual
gamepad** via ViGEm (Windows) or `uinput` (Linux) — from the host's own OS and any locally-running
emulator's perspective, what's "connected" is Sunshine's virtual Xbox/PlayStation/Switch-shaped
controller, not the user's actual physical hardware, and its GUID/capability profile is
Sunshine's own virtual device identity, not the client's real device's. **This means: any
server-side (host-side) controller/input evidence EmuWiz might gather in a remote-streaming
topology is not merely incomplete — it is actively describing the wrong device.** A host-side
check would see a generic virtual gamepad (or nothing, before a Moonlight session connects) and
could easily produce a confident-sounding but wrong answer ("a standard gamepad is connected" —
true of the virtual device, irrelevant to whether the actual streaming client has anything
plugged in, or has a lightgun-incapable phone touchscreen as its only input, or is a different
person's client entirely).

**Consequence for this document's design:** any future runtime device check (§14) must be
scoped explicitly to "the machine EmuWiz's own process is running on," and must never be
presented to the user as "your controller" without qualifying that, in a remote-play session,
that machine is the streaming host, not necessarily where the user is physically sitting. This
document does not propose any remote-play-aware controller detection — it is explicitly out of
scope — but flags this as the sharpest concrete example of why server-side runtime device
enumeration, generalized carelessly, is actively misleading rather than merely stale.

## 16. Real-machine case study — this host's own remote-streaming topology

Using the topology described for this environment (Sunshine/Moonlight, an Xbox Series
controller connected via an Xbox Wireless Adapter, Linux/Nobara clients, server-side emulator
launching) as a reasoning exercise — **not** an inspection of this host's actual live hardware
state, per the task's own instruction:

- If EmuWiz ran on the Sunshine **host** and attempted a live SDL enumeration at pre-launch time
  (§14), what it would see is governed entirely by whether a Moonlight session is currently
  connected: with no session connected, no virtual gamepad exists yet, so EmuWiz would
  (correctly, if it reported honestly) see `DEVICE_NOT_PRESENT`/`UNKNOWN` even though the user's
  real Xbox Series controller may be sitting right next to their Moonlight client, fully
  functional. This would be a **false negative** if EmuWiz's UX phrased it as "no controller
  found" rather than "no controller found on this machine."
- With a Moonlight session connected and actively streaming, EmuWiz's host-side SDL enumeration
  would see Sunshine's virtual gamepad device — generically Xbox-360-shaped regardless of the
  user's actual Xbox Series controller model — meaning any capability check (e.g. "does this
  device support motion/gyro") would report the **virtual** device's (likely absent) gyro
  capability, not the physical Xbox Series controller's actual capability, which is itself
  limited (base Xbox Series controllers lack gyro) but for an entirely different, coincidental
  reason unrelated to the virtualization layer — a case where two independent "no gyro" facts
  could combine to look more confidently wrong than either alone.
- The Xbox Wireless Adapter detail matters only on whichever machine the physical controller is
  actually paired to (almost certainly the Moonlight **client**, not the Sunshine host, in a
  typical setup) — which is further, concrete illustration of §15's point: the host has no
  visibility into the client's pairing/adapter state at all, wireless-adapter-specific quirks
  included.
- **Conclusion for this case study:** in this exact topology, essentially all host-side runtime
  controller evidence is either stale-empty (no session) or describing Sunshine's virtual device
  rather than the real hardware (session active) — reinforcing §15's finding with a concrete
  instance rather than an abstract one, and reinforcing why this document does not propose any
  server-side controller-presence feature for a remote-play-aware EmuWiz without first solving
  (out of scope here) how the *client's* own input state, if ever exposed at all, would need to
  travel back through Moonlight/Sunshine's own protocol rather than through host-side
  enumeration.

## 17. Security / privacy

- **Bounded device enumeration only** — GUID, name, button/axis count/capability class. Nothing
  more.
- **Explicitly reject:** keystroke logging, input-event recording of any kind, capturing actual
  button-press values or timing outside an explicit, user-initiated diagnostic test mode (e.g. a
  "press a button to confirm this is your controller" UI flow the user deliberately triggers,
  which is materially different from passive background capture), and persisting any record of
  *which buttons were pressed* or *when* beyond the lifetime of that one explicit test
  interaction.
- **Never persist device presence as a durable fact about the user's setup** — see §14's
  on-demand-only recommendation; a stored "last known controller" record edges toward exactly
  the kind of persistent physical-device tracking this document's DO-NOT-BUILD list rejects.

## 18. UX

Plain-language examples, extending the task brief's own framing and consistent with Ready-to-Play's existing phrasing conventions (`ready_to_play_page.rs`'s existing `state_label`/`family_label` strings, §1.1):

- "Control requirements are not modelled for this game; EmuWiz cannot tell you whether a
  specific controller is needed." (§13, example 1 — `Unknown`)
- "A controller was not detected just now. If you plan to use one, connect it before launching —
  this does not stop the game from launching." (§13, example 3 — `ReadyWithWarnings`, never
  "Controller missing")
- "This machine's original cabinet used a light gun. EmuWiz has not confirmed whether this title
  plays acceptably on a standard controller." (§13, example 2 — `NeedsAttention`, carefully
  hedged, never a bare "Light gun required")

**Explicitly avoid** ever asserting "Controller missing" as a bare claim — every example above
is phrased to state exactly what was and wasn't checked, mirroring Ready-to-Play's own existing
discipline of naming evidence gaps rather than asserting failure from absence
(`needs_attention.rs`'s documented refusal to infer failure from an unscanned lane, cited by the
prior audit at lines 280-296).

## 19. Performance

- No per-library-item runtime device enumeration at render/library-load time — this would scale
  linearly with library size for a fact (device presence) that is global to the *machine*, not
  per-game, and is exactly the kind of expensive, needless per-item probe Ready-to-Play's own
  design principles already reject for every other evidence family it models.
- Cache per-game requirement metadata (§2/§3, static) exactly like existing DAT-derived facts are
  cached.
- Cache per-emulator profile/mapping evidence (§9, static) alongside existing emulator-
  environment discovery's own cache lifetime (`diagnostics/profiles.rs`'s existing scan cadence),
  not a separate cache with separate invalidation rules.
- Device presence/capability (runtime) is checked **once, on demand**, not polled, and its result
  is used immediately, not stored as a cache entry with its own staleness policy — because unlike
  every other cached fact in this codebase, "is the device still there" changes on a timescale
  cache invalidation cannot meaningfully track (§14).
- No render-time evdev crawling, no render-time SDL re-enumeration on every frame of a controller
  status widget, if one is ever built — an on-demand explicit check triggered by user action or
  pre-launch, not by paint cycles.

## 20. Comparable projects — concept-level only

All GPL-family (RetroArch, ES-DE, Batocera, EmuDeck's own scripts, Pegasus, RetroDECK) unless
noted; no source code from any of these was read or copied in this pass — everything below is
concept-level, drawn from public documentation/search-summary depth.

- **ES-DE / Pegasus / LaunchBox / RetroDECK** — general-purpose frontends that primarily surface
  *configured* controller mappings (their own input-config UI) and rely on RetroArch/emulator-
  native config underneath for actual runtime binding; none surveyed in this pass appear to
  model per-game controller *requirements* as distinct metadata (consistent with §3's
  "UNCERTAIN, not confirmed against a specific schema" note on LaunchBox/ES-DE metadata).
- **RetroArch** — already covered in depth (§8); the three-axis separation (device / mapping /
  core-declared-support) is the most mature model surveyed and the strongest conceptual precedent
  for §11's six-axis design.
- **Batocera / EmuDeck** — distribution/setup-script projects that automate applying
  SDL_GameControllerDB-style mappings and per-emulator config generation at *install* time, not
  at *readiness-check* time; their model is closer to "make the configuration correct once" than
  "continuously assess whether it's still correct," which is a genuinely different problem from
  what this document scopes (a read-only readiness *view*, never a configurator).
- **Steam Input** — already covered in depth (§7); its per-game-layout model is the most complex
  surveyed and the clearest illustration of why device-side evidence cannot see through a
  virtualization layer.

**INFERENCE:** no comparable project surveyed here attempts a bounded, honest, per-game
input-*readiness* projection in the sense this document designs (i.e., distinct from "apply a
mapping" or "declare RetroPad support") — most either automate configuration (Batocera/EmuDeck)
or defer entirely to the emulator's own input layer (ES-DE/Pegasus/LaunchBox/RetroDECK). This
lowers the risk of this document's design accidentally duplicating an existing, better-tested
pattern, but also means there is no directly comparable prior art to validate the six-axis model
of §11 against — it is original reasoning specific to EmuWiz's own evidence-honesty conventions,
not adapted from any single surveyed project.

## 21. Final decisions

### 21.1 Should EmuWiz build any controller/input evidence gathering today?

**No.** Every source surveyed in §3/§9 either requires new parsing work not yet started (MAME
`<input>`, §1.6/§4) or is UNCERTAIN-depth/low-coverage (compat databases, per-emulator config
schemas). The one exception — SDL device enumeration (§5) — is buildable today but proves only
axis C/D of §11, never axis A (requirement), and per §13 a requirement-less presence check alone
justifies nothing beyond an `Unknown` coverage note, which is already achievable today with zero
new code by simply leaving `ReadinessReasonFamily::Controller` unused, exactly as it is now.

### 21.2 If/when built, should it start from MAME or from a curated database?

**MAME `-listxml`, but only as the first of two required layers, never alone.** §4's verdict:
strongest available per-machine AUTHORITATIVE source and already half-plumbed into the importer
pipeline, but explicitly insufficient by itself (the cabinet-vs-playable-on-a-pad gap). The
correct first phase is: (1) extend the MAME listxml parser to retain `<input>`/`<control>`
elements (new parsing work, §1.6), (2) build the "recommended alternate control" curation layer
only as a second, later phase once real per-title curation capacity exists — never ship phase 1
alone as if it answered "is a pad sufficient."

### 21.3 Should Ready-to-Play gain a Controller-family producer now?

**No, not now — but the type should stay exactly as it is (declared, unused) rather than being
removed.** Its exhaustive-match cost (one GUI label arm) is negligible, and removing it would
just mean re-adding it later once a real evidence source exists. Building a producer today, with
no real evidence behind it, risks exactly the fabrication failure mode §1.3 and §2 already
reject — better to leave `Controller` honestly idle than to wire it to a HEURISTIC-tier guess
just to make the variant "used."

### 21.4 Should the evidence model mirror `ReadyToPlayState`'s vocabulary or be distinct?

**Mirror four of five names, deliberately omit `Blocked`.** Full reasoning and justification in
§12 above.

### 21.5 What should the Ready-to-Play boundary be (A/B/C)?

**(c), narrowly construed, with (b) as the default.** Full reasoning and the three worked
examples in §13 above.

### 21.6 Is evdev/libinput ever appropriate as a default evidence path?

**No.** Full reasoning in §6: redundant with SDL for every use case surveyed, and materially more
likely to turn an honest "cannot check" into a dishonest "not present."

### 21.7 What is the correct behavior in a remote-play (Sunshine/Moonlight-style) topology?

**Server-side runtime device evidence must never be gathered or presented as if it described the
streaming client's actual input hardware.** Full reasoning in §15/§16: the host, in this
topology, structurally cannot see the client's real device — only Sunshine's own virtual
gamepad, which is not the same fact and must never be labeled as if it were.

## 22. Roadmap

The task brief's I0-I5 shape holds up under this research with no structural changes needed —
this document confirms the ordering rather than revising it, because each phase's prerequisite
is exactly what the preceding sections found missing:

- **I0 — Typed vocabulary only.** Define `InputRequirementType`, `EvidenceTier`, the §10 special-
  peripheral states, and the §12 type sketch's shape as real (but unwired) types. No parsing, no
  producer, no UI. Matches §21.3's "leave it honestly idle" conclusion.
- **I1 — Static game requirements, MAME-first.** Extend the MAME listxml parser to retain
  `<input>`/`<control>` per §1.6/§4/§21.2. This is the first phase that produces any real,
  provenance-carrying `InputRequirement` data, and it is arcade-only at this stage — console/
  handheld platforms remain `Unknown` until a curated source is identified (§3's UNCERTAIN rows).
- **I2 — Emulator profile inspection.** Read (never write) per-emulator config/profile evidence
  per §9, populating `InputProfileEvidence` (axis B of §11) statically, cached alongside existing
  `diagnostics/profiles.rs` discovery per §19.
- **I3 — Optional on-demand runtime inventory.** SDL-only device enumeration (§5), triggered
  on-demand or pre-launch only (§14), never polled (§14's explicit rejection), never evdev (§6),
  and explicitly scoped to "this machine" with the §15/§16 remote-play caveat documented in any
  UI that surfaces it.
- **I4 — `InputReadiness` projection.** Combine I1-I3 into the §12 typed model, following §13's
  boundary rules exactly (warnings-only in the common case, `NeedsAttention` only in the narrow
  proven-and-curated case, never `Blocked`).
- **I5 — GUI/Doctor presentation.** Wire a producer into Attention (§1.7's `Emulator`-or-`Launch`
  category choice, deferred to this phase) and into Ready-to-Play's existing `Controller` family
  (§21.3, now finally justified once I4 exists), using the §18 UX phrasing conventions.

**No phase reordering found justified by this research** — each phase strictly depends on its
predecessor's evidence existing first, and no shortcut (e.g. skipping straight to I3's device
enumeration without I1's requirement data) would produce anything more useful than what's already
achievable today (an `Unknown` coverage note) per §21.1.

## 23. Explicit DO-NOT-BUILD list

- No permanent polling daemon for controller hotplug (§14).
- No keystroke logging, input-event recording, or button-press capture outside an explicit,
  user-initiated diagnostic test mode (§17).
- No guessing controller support from platform alone (§2's explicit PS2/DualShock-2 rejection).
- No blocking launch on a background scan simply being absent/incomplete — absence of a scan is
  `Unknown`, never `Blocked` (§13, example 1).
- No treating a Moonlight/Sunshine client's controller state as if it were server-side hardware
  state — the host cannot see it; do not fabricate that visibility (§15, §16).
- No auto-remapping any emulator's controller configuration without explicit user confirmation —
  this document proposes read-only evidence gathering only, never a configurator (§20's Batocera/
  EmuDeck distinction is deliberate: EmuWiz's scope here is assessment, not automation).
- No changing any emulator's config files, ever, as part of this feature area (repeated from §0's
  non-goals for emphasis).
- No persistent physical-device "truth" record — device presence is checked on demand and used
  immediately, never stored as a durable fact about the user's setup (§14, §17).
- No controller scoring, percentages, or compatibility ratings of any kind — consistent with
  Ready-to-Play's own existing, explicit rejection of quality/scoring signals anywhere in its
  model (Ready-to-Play audit §2.2, "A quality, rating, or 'best games' signal" — no scoring model
  exists or is proposed there, and none is proposed here either).

## Closing footer

No code changes were made anywhere in this repository while producing this document. No
production Rust file, GUI page, launch planner, emulator adapter, controller config, Steam Input
integration, RetroArch mapping, Save Vault code, or database code was modified. The only file
created is this document, at `docs/research/CONTROLLER_INPUT_READINESS_ARCHITECTURE_AUDIT.md`.
No other file in this shared worktree — including the other contributor's in-progress,
uncommitted changes to `arcade_mame_compatibility.rs`, `archive_workflow.rs`,
`repair/optical_conversion.rs`, `storage_health_page.rs`, `database.rs`, `lib.rs`, or the
untracked `docs/research/ARCADE_MANAGER_EMUWIZ_AUDIT.md` — was read beyond what `git status`
already reported, or touched in any way.

**What remains genuinely unresolved / not independently verified in this pass:**

1. The exact byte-for-byte MAME `-listxml` `<input>`/`<control>` DTD/schema (§4) — described from
   GENERAL KNOWLEDGE and cross-referenced search results, not read from MAME's own XML-generation
   source or a fetched, complete DTD document.
2. Whether any specific, redistribution-safe, machine-readable per-title compatibility database
   exists that EmuWiz could legally and reliably consume for non-arcade platforms (§3) — flagged
   UNCERTAIN; no specific source was identified and verified as fit for this purpose in this
   pass.
3. LaunchBox/ES-DE metadata schemas — whether either currently carries any dedicated
   input-requirement field (§3, §20) — not confirmed against a specific current schema version.
4. PCSX2's and PPSSPP's exact current per-game config file formats (§9.2, §9.4) — described at
   GENERAL KNOWLEDGE depth only; both projects have had UI/config rewrites and the precise
   current on-disk shape was not independently verified.
5. Azahar's controller/touch/gyro configuration specifics (§9.6) — acknowledged as the thinnest
   section in this document; not independently read beyond general project awareness.
6. Ryubing's exact configuration file format and the full extent of its motion/gyro support
   beyond what its own marketing/summary pages state (§9.7) — public documentation for this
   specific fork (as opposed to its Ryujinx ancestor) is thin, and this document is honest that
   only search-summary depth was reached.
7. The exact hook-point ordering between OS-level SDL device enumeration and Steam Input's own
   interception layer (§7) — the claim that SDL enumeration could in principle run "before" Steam
   Input's transformation is plausible from the sources read but not independently verified
   against Steam Input's own implementation.
8. Whether `AttentionCategory::Emulator` or `AttentionCategory::Launch` is the better-fitting
   category for a future input-readiness attention item (§1.7) — left as an open,
   implementation-phase choice, not resolved here.
