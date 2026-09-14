# MAME Static Input Metadata Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot.** This document records research and design reasoning only. Nothing in
> it is implemented. No production Rust file, GUI page, launch planner, Ready-to-Play type, MAME
> importer, DAT model, controller-profile code, SDL/evdev runtime code, or database code was
> created or modified while producing it. Where illustrative code appears it is clearly marked
> non-normative pseudocode, never a real type in this repository.

**Scope:** EmuWiz, `feature/archivefs-unified-platform`, worktree
`/home/davedap/emuwiz-main-release-fix`. **Method:** direct reading of this repository's own
source (`file:line` citations, tagged **CONCLUSION FROM SOURCE**); direct interrogation of a
locally installed MAME 0.264 binary (`mame -listxml <shortname>`, read-only, no ROM files
required or read — `-listxml` emits driver-declared metadata regardless of whether ROMs are
present) for both its embedded DTD and real per-machine `<input>`/`<control>` XML, tagged
**DOCUMENTED FACT (local MAME 0.264)**; direct reading of MAME's own upstream `infoxml.cpp`
source (fetched from `github.com/mamedev/mame`, `master` branch) for the exact code that
generates this XML, tagged **DOCUMENTED FACT (MAME source)**; supplementary web research tagged
**DOCUMENTED FACT** (specific source read) or **GENERAL KNOWLEDGE** (cross-checked, not read
end-to-end from one canonical source). Reasoning original to this document is tagged
**INFERENCE**. Anything not independently confirmed is tagged **UNCERTAIN**.

This document is the direct successor to
`docs/research/CONTROLLER_INPUT_READINESS_ARCHITECTURE_AUDIT.md` (the "prior audit"), which
already established several conclusions this document does not re-litigate:

- `ReadinessReasonFamily::Controller` (`ready_to_play.rs`) has zero producers and should stay
  that way for now (prior audit §1.1, §21.3).
- MAME `-listxml` is the strongest per-machine candidate evidence source, but cabinet-original
  controls are a fact about the cabinet, never a proof about what a modern gamepad needs (prior
  audit §4, §12.3, §21.2).
- Runtime server-side controller evidence must never be treated as proof of a Moonlight/Sunshine
  client's actual hardware (prior audit §15/§16).
- No permanent polling daemon, no runtime SDL/evdev enumeration, no Ready-to-Play wiring (prior
  audit §14, §21.1, §23).

This document narrows in on one question the prior audit deliberately left at survey depth: what,
precisely, does MAME's static `<input>`/`<control>` schema contain, what can EmuWiz safely and
honestly preserve and normalize from it, and where exactly does the cabinet-fact/modern-requirement
line sit — with **zero runtime-controller claims anywhere in this document.**

**Tagging key**

| Tag | Meaning |
|---|---|
| **DOCUMENTED FACT (local MAME 0.264)** | Read directly from this installed MAME 0.264 binary's own `-listxml` output (DTD or per-machine XML) in this pass. |
| **DOCUMENTED FACT (MAME source)** | Read directly from MAME's own upstream `infoxml.cpp`/git history on `github.com/mamedev/mame` in this pass. |
| **DOCUMENTED FACT** | Stated by another external, cited source read in this pass, or by a `file:line` in this repository. |
| **GENERAL KNOWLEDGE** | Widely and consistently published technical convention, cross-checked but not read end-to-end from one canonical source. |
| **INFERENCE** | Reasoning drawn by this document; not directly asserted by a source. |
| **UNCERTAIN** | Explicitly flagged as unverified; not to be built on without further reading. |

## 0. Purpose, scope, and explicit non-goals

**Purpose.** MAME `-listxml` per-machine `<input>`/`<control>` metadata is currently discarded
(or reduced to a near-empty stub, per §1 below) by EmuWiz's MAME listxml importer. This document
asks: what is the exact shape of that metadata, what can it honestly prove, and what would a
minimal, honest, purely-static preservation-and-normalization layer look like — stopping well
short of any Ready-to-Play wiring, runtime device model, or "is a gamepad enough" claim.

**Explicit non-goals.** This document does **not** modify, and its reasoning must not be read as
authorizing changes to:

- Ready-to-Play (`ready_to_play.rs`) or any Ready-to-Play state/reason type
- The MAME listxml importer (`identity_source/mame_listxml/`, `dat/parsers/mame_listxml.rs`)
- The generic DAT model (`dat/model.rs`)
- Launch planning, controller profiles, SDL/evdev runtime code
- Any GUI page
- The database schema or any migration

Everything below is a design sketch for a future phase, not scheduled or authorized here.

## 1. Current EmuWiz inventory

All rows are **CONCLUSION FROM SOURCE**, read directly in this pass.

### 1.1 The generic DAT model has no field for `<input>`/`<control>`

`crates/archivefs-core/src/dat/model.rs:331-420` — `DatGameEntry`'s full field list, read in
full this pass — has `name`, `id`, `description`, `roms`, `clone_of`, `rom_of`, `sample_of`,
`is_bios`, `is_device`, `runnable`, `supported`, `disks`, `device_refs`, `samples`, `bios_sets`,
`parts`, `board`, `rebuild_to`, `year`, `manufacturer`, `source_file`, `comment`,
`original_metadata` (a free-form `DatOriginalMetadata { fields: BTreeMap<String, String> }`
catch-all bucket), and `content_classification`. **There is no dedicated `input`/`control`
field, and none is implied by any existing field.** This confirms the prior audit's §1.6 finding
still holds against the current source.

### 1.2 More precise than the prior audit found: `<input>` is not fully discarded — it is reduced to a near-useless stub, and `<control>` is fully dropped

The prior audit inferred (§1.6, "very likely discarded at parse time") without reading
`dat/parsers/mame_listxml.rs` in this depth. Reading it directly in this pass
(`crates/archivefs-core/src/dat/parsers/mame_listxml.rs:283-370`, `handle_empty_like`) shows the
actual behavior is more specific and worth flagging explicitly:

```rust
"slot" | "slotoption" | "chip" | "display" | "sound" | "input" | "dipswitch" => {
    let key = format!("mame.{tag}");
    m.metadata
        .fields
        .entry(key)
        .or_insert_with(|| attr(e, b"name").unwrap_or_default());
}
```

(`mame_listxml.rs:360-366`.) This *is* reached for `<input>` — it appears in the tag match arm —
but it reads the `name` attribute of the element, and **`<input>` never has a `name` attribute**
(its real attributes are `players`/`coins`/`service`/`tilt`, confirmed by the DTD in §2 below).
The result: `original_metadata.fields["mame.input"]` is set to an empty string on every MAME
import that has an `<input>` element — a stub key with no real content, present only because
`<input>` happens to share a tag-name match arm with `<slot>`/`<sound>`/`<display>`/`<dipswitch>`,
none of which have children this parser walks into either. **`<control>` is not in this match arm
at all** — its `type`/`player`/`buttons`/`ways`/analog attributes are never read, matched, or
stored anywhere, not even in the generic string bucket. Because `<input>`'s children
(`Event::Start` for `<input>`, then nested `Event::Empty` for each `<control>`) are processed by
this same `handle_empty_like` dispatcher regardless of nesting, and `"control"` never appears in
its match arms, every `<control>` element is silently discarded on every MAME import performed by
this codebase today.

**INFERENCE:** this is a meaningfully different (and more precise) finding than "discarded" —
EmuWiz's importer does not merely lack a place to put input evidence; it actively writes one
misleading, empty placeholder key (`mame.input = ""`) into `original_metadata` for every machine
that has an `<input>` element, which is worse than absence because a future reader of
`original_metadata.fields` could mistake the *presence* of the `mame.input` key for evidence that
input data was captured, when in fact it carries nothing. Any future preservation work (§13, §20)
should treat this stub as a bug to fix in the same change that adds real capture, not build
alongside it.

### 1.3 `ReadinessReasonFamily::Controller` is still dead

`crates/archivefs-core/src/ready_to_play.rs:40` still declares the `Controller` variant with no
constructor call site (reconfirmed by a fresh grep in this pass). No change from the prior
audit's §1.1 finding.

### 1.4 Two existing "arcade" modules are not the right home, and neither models input

Two modules were read in this pass specifically to check whether either is a plausible home for
input evidence, since both sit in "arcade compatibility" territory:

- **`crates/archivefs-core/src/diagnostics/arcade_dat_version.rs`** — its own module doc states
  its job precisely: "does the arcade DAT I audited against come from the same emulator build I
  have installed?" It parses and orders MAME/FBNeo *version strings* (`0.NNN`, `u`/`b` suffixes)
  for a Doctor advisory about DAT-vs-installed-build mismatch. This is entirely about *version
  provenance*, not per-machine input content — not a plausible home for `<input>`/`<control>`
  evidence.
- **`crates/archivefs-core/src/arcade_mame_compatibility.rs`** (read via
  `git show HEAD:crates/archivefs-core/src/arcade_mame_compatibility.rs`, since the working-tree
  copy has another session's uncommitted, unrelated changes not read here) — its own module doc:
  "Read-only per-set compatibility against one pinned installed MAME build... does not execute
  MAME, walk a rompath, hash files, or mutate a catalogue." Its `MameSetCompatibilityState`
  (`Compatible`/`CompatibleWithWarnings`/`Incompatible`/`Unknown`/`Unsupported`) and
  `MameMismatchReason` (`RequiredFileMissing`, `WrongSize`, `CrcMismatch`, `BiosMissing`,
  `ChdMissing`, `NeedsRedump`, etc.) are entirely about **ROM/CHD dump completeness and
  checksum/version matching against the pinned MAME build** — a totally different concern from
  what the original cabinet's control panel had. Also not a plausible home.

**CONCLUSION FROM SOURCE:** neither existing "arcade" module models or even adjacently touches
control/input semantics. A future evidence type needs either a new home or an addition to the
generic DAT model — decided explicitly in §13.

## 2. Authoritative MAME input XML — the exact schema

**Primary source for this section: this installed MAME 0.264 binary's own embedded DTD**, printed
verbatim by `mame -listxml <anything>` inside its `<!DOCTYPE mame [...]>` preamble — this is
stronger evidence than any secondary web page, because it is the literal schema declaration this
exact binary emits, not a description of some other version's behavior:

```
<!ELEMENT input (control*)>
    <!ATTLIST input service (yes|no) "no">
    <!ATTLIST input tilt (yes|no) "no">
    <!ATTLIST input players CDATA #REQUIRED>
    <!ATTLIST input coins CDATA #IMPLIED>
<!ELEMENT control EMPTY>
    <!ATTLIST control type CDATA #REQUIRED>
    <!ATTLIST control player CDATA #IMPLIED>
    <!ATTLIST control buttons CDATA #IMPLIED>
    <!ATTLIST control reqbuttons CDATA #IMPLIED>
    <!ATTLIST control minimum CDATA #IMPLIED>
    <!ATTLIST control maximum CDATA #IMPLIED>
    <!ATTLIST control sensitivity CDATA #IMPLIED>
    <!ATTLIST control keydelta CDATA #IMPLIED>
    <!ATTLIST control reverse (yes|no) "no">
    <!ATTLIST control ways CDATA #IMPLIED>
    <!ATTLIST control ways2 CDATA #IMPLIED>
    <!ATTLIST control ways3 CDATA #IMPLIED>
```

Two structural facts worth stating precisely: `<input>` can contain zero or more `<control>`
children (`control*` — an `<input>` with no `<control>` at all is valid and real, seen in this
pass for `vindictr`'s ambiguous cocktail-cabinet secondary set and for several machines with only
coin/service input); and `type` is declared `CDATA #REQUIRED` — **free text, not a closed,
schema-enforced enumeration.** The actual vocabulary of `type` strings is determined entirely by
what MAME's C++ driver/frontend code emits (confirmed directly against source in §2.1), not
constrained by the DTD itself. A future parser must not assume it has captured every possible
`type` string just because it has captured every string this document's bounded sample produced.

### 2.1 Attribute semantics — read directly from MAME's own `infoxml.cpp` generator

**DOCUMENTED FACT (MAME source):** this document fetched `src/frontend/mame/infoxml.cpp` from
`github.com/mamedev/mame` (`master` branch — a newer development snapshot than the installed
0.264, see the version-drift note below) and read the exact code that computes every XML
attribute this section describes, rather than relying on a description of the schema.

- **`players`** — a plain running maximum: `if (nplayer < field.player() + 1) nplayer = field.player() + 1;` over every input field in the machine's port list (`infoxml.cpp:1470-1471`), emitted as `players="%d"` (`infoxml.cpp:1771`). It is a *count of distinct player-numbered input slots the driver declares*, computed purely from how many player indices exist across all fields — not a "how many must be connected to start" requirement in the code itself. §6 below reasons about what this means for EmuWiz's own claims.
- **`coins`** — the highest coin-slot index seen (`IPT_COIN1`..`IPT_COIN12`, `infoxml.cpp:1668-1681`), arcade-operational metadata (coin mechanisms), not a player-input requirement.
- **`service`/`tilt`** — booleans set the moment any `IPT_SERVICE`/`IPT_TILT` field exists anywhere in the machine's ports (`infoxml.cpp:1699-1705`); service-panel/tilt-switch presence, not player-input.
- **`buttons`** — for a digital control, `nbuttons`, incremented once per `IPT_BUTTON1`..`IPT_BUTTON16` field seen for that player/control-type slot (`infoxml.cpp:1663-1664`); for the special cases of `type="stick"` (analog) and `type="joy"` (digital), the code instead emits `maxbuttons` — the *highest numbered* `IPT_BUTTONn` seen, not a count — via a ternary keyed on the type string (`infoxml.cpp:1793`, `1815`: `strcmp(elem.type, "stick") ? elem.nbuttons : elem.maxbuttons`, and the parallel joy case). **INFERENCE:** this means `buttons` is not perfectly uniform across control types — for most types it is a literal count of distinct button fields, but for `stick`/`joy` it is instead the highest button *index* declared, which only differs from a count when button numbering has gaps (uncommon but not guaranteed absent) — a real, if narrow, source of imprecision a future parser should not silently assume away.
- **`reqbuttons`** — **DOCUMENTED FACT (MAME source), pinned precisely, resolving the prior audit's open question:** this attribute was added in MAME commit `9707fdbf0964edeb9e9d844bdbfb6e51402204ad` (AJR, 2016-05-23), whose message states its exact purpose: a `PORT_OPTIONAL` flag marks "controls that are not required for normal operation and may not be hooked up on actual hardware, but are still worth emulating because the hardware does respond to them in some way," and the commit's own message names `"reqbuttons"` explicitly as the XML field this flag feeds — i.e. **`reqbuttons` is `buttons` minus any buttons the driver has explicitly flagged `PORT_OPTIONAL`.** A gap between `buttons` and `reqbuttons` on a real machine therefore means: the cabinet had extra buttons wired up but the game does not require them to be pressed for normal play (the commit's own motivating example, `gijoe` and its clones, is a concrete case). **Version-drift finding, not in the task brief's ground truth and independently discovered in this pass:** `reqbuttons` is present in the installed MAME 0.264 binary's own DTD (quoted above) but **is absent entirely from the current `master`-branch `infoxml.cpp`'s DTD block and its attribute-writing code** (grepped and read in full — no `reqbuttons` token appears anywhere in the current upstream source at all). This means `reqbuttons` was a real, well-defined attribute as of 0.264 and has since been removed from MAME's own `-listxml` output in a later development version (exact removal version/commit not identified in this pass — flagged **UNCERTAIN** for the specific version boundary, though the *fact* of removal is DOCUMENTED FACT from the current source diff against 0.264's own DTD). A future EmuWiz parser must not assume `reqbuttons` will always be present even from a "modern" MAME build — it is version-conditional, unlike `buttons`.
- **`minimum`/`maximum`** — emitted only `if (elem.min != 0 || elem.max != 0)` (`infoxml.cpp:1794-1795`) as the raw analog input range the driver declared for that axis (`ioport_field`'s own configured min/max) — a hardware-calibration fact (e.g. a trackball's counter range, a lightgun's screen-coordinate range), not itself a claim about physical device requirements.
- **`sensitivity`/`keydelta`** — `field.sensitivity()`/`field.delta()` (`infoxml.cpp:1739-1742`), MAME's own internal analog-input tuning defaults (how much a keyboard-simulated analog nudge moves per keypress, and how sensitive the analog axis is scaled) — **INFERENCE:** these describe MAME's *keyboard-as-analog-input emulation calibration*, not a property of the original arcade hardware in the way `ways`/`minimum`/`maximum` are; they are meaningful mainly for MAME's own keyboard-substitute analog controls, and this document recommends treating them as lower-value, MAME-internal-tuning evidence rather than cabinet-hardware fact (a nuance not previously flagged in the prior audit's GENERAL KNOWLEDGE-tier description of this schema).
- **`reverse`** — a boolean, whether the analog axis's polarity is inverted from its natural reading direction by default (`elem.reverse`, `infoxml.cpp:1800-1801`) — a calibration default, same class as `sensitivity`/`keydelta`.
- **`ways`/`ways2`/`ways3`** — `elem.ways` is `field.way()` — **DOCUMENTED FACT (MAME source)**, `infoxml.cpp:1481` etc. — MAME's own internal restricted-direction count for a digital joystick (2/4/8-way, or a rotated variant). Critically, this document confirms via direct source reading (`infoxml.cpp:1807-1829`) that `ways`/`ways2`/`ways3` are **not simply "first stick/second stick/third stick" labels** — they are positionally assigned by a `helper[0..2]` array that tracks which of up to three independent directional-input clusters (`IPT_JOYSTICK_*`, `IPT_JOYSTICKLEFT_*`, `IPT_JOYSTICKRIGHT_*`) the driver declared, and the control's `type` string itself gets a `"double"`/`"triple"` prefix (`doublejoy`/`triplejoy`) based on how many clusters exist (`infoxml.cpp:1807-1811`) — confirming `doublejoy` (twin-stick, e.g. `robotron`) and, rarer, a `triplejoy` type exist and are structurally distinct from a single `joy` with an unusual `ways` value. **A real, non-numeric `ways` value was observed directly in this pass** — `vindictr` (Vindicators Part II) emits `ways="vertical2" ways2="vertical2"` (§16 table) — confirming `ways`/`ways2`/`ways3` are genuinely free-text-shaped in practice (a 2-way stick restricted to the vertical axis, distinct from a horizontal 2-way), not merely integers 2/4/8 as the prior audit's GENERAL KNOWLEDGE-tier description implied.

**Honesty note on source-version alignment:** the DTD block and attribute-writer code read in this
section came from two slightly different points in time — the DTD quoted at the top of §2 is from
the locally installed MAME 0.264 binary, while the `infoxml.cpp` source read for semantics in
§2.1 is the current upstream `master` (a materially newer development build, given `reqbuttons`'s
removal between the two). The core attribute semantics (`players`/`coins`/`service`/`tilt`/
`buttons`/`minimum`/`maximum`/`sensitivity`/`keydelta`/`reverse`/`ways`/`ways2`/`ways3`/`type`)
are stable across this gap — none of that logic differs in a way relevant to this document's
conclusions, and the real per-machine XML samples in §16 were pulled from the same 0.264 binary
the DTD came from, so schema and sample data are version-matched even though the *generator source
code* read for semantic explanation is a later snapshot. `reqbuttons` is the one attribute where
version matters and is called out explicitly above.

## 3. Control types inventory

Types below are drawn from this pass's real, bounded `mame -listxml <shortname>` sample (§16) and
cross-referenced directly against the `CTRL_*` enum and `IPT_*` mapping in `infoxml.cpp`
(**DOCUMENTED FACT (MAME source)**, `infoxml.cpp:1400-1427` for the enum, `1474-1727` for the
`IPT_*` → type-string mapping).

| Type string | Represents | Digital/analog | Ordinary modern gamepad plausible substitute? |
|---|---|---|---|
| `joy` (+ `2way`/`4way`/`8way` via `ways`) | Restricted-direction digital joystick | Digital | **Yes, trivially** — this is exactly what a d-pad or analog stick's digital read already does. |
| `doublejoy` / `triplejoy` | Twin- or triple-stick digital control (independent directional clusters), e.g. `robotron` | Digital | **Yes for twin-stick** — a modern pad's two analog sticks read digitally map naturally; triple-stick has no natural 3-stick modern-pad equivalent, though most triple-stick MAME machines are vanishingly rare. |
| `stick` | Analog joystick axis | Analog | **Yes** — this is precisely what a modern analog stick axis is. |
| `only_buttons` | Pure button panel, no directional control at all (e.g. `quizshow`) | Digital | **Yes, trivially.** |
| `paddle` | Analog rotary knob (Arkanoid-class spinner-adjacent, but distinct from `dial`) | Analog | **Contentious.** Commonly played on an analog stick with a real but tolerable precision loss; some titles (precision-timing games) degrade more. |
| `dial` | Analog spinner/rotary encoder (e.g. `tempest`, `arkanoid`) | Analog | **Contentious**, same nuance as `paddle` — playable on a stick in practice for most titles, genuinely worse for a minority. |
| `trackball` | Relative-motion rolling ball (e.g. `centiped`, `atarifb`) | Analog (relative) | **Contentious**, most explicitly flagged nuance in the prior audit (§4/§9 there): many trackball games are commonly played, imperfectly but functionally, on an analog stick; a genuine minority (precision-aiming trackball titles) degrade meaningfully. |
| `pedal` | Analog foot pedal (throttle/brake), e.g. `outrun`, `wecleman` | Analog | **Contestable but generally yes** — an analog trigger is a reasonable functional substitute for most pedal use. |
| `lightgun` | Screen-position analog pointer read as absolute coordinates, e.g. `lethalen`, `cheyenne` | Analog (absolute pointer) | **No** — fundamentally different interaction model (aiming at a physical point on a CRT/display), not a directional-input problem a stick can approximate. |
| `positional` | Absolute-position rotary control (not observed directly in this pass's sample — GENERAL KNOWLEDGE from the `IPT_POSITIONAL`/`IPT_POSITIONAL_V` mapping confirmed in source, `infoxml.cpp:1603-1610`) | Analog | **UNCERTAIN**, no real example gathered this pass; conceptually similar to `dial` (a bounded rotary read), same contentious tier expected. |
| `mouse` | Relative-motion analog pointer, e.g. `a2000` (Amiga), `ibm5150` (PC) | Analog (relative) | **Contentious**, same class as trackball — degrades but often functional with an analog stick, genuinely poor for precision-pointer use cases (e.g. GUI-driven computer software). |
| `keypad` | Small fixed digital button grid (e.g. `c64`'s `1541` disk-drive-adjacent keypad) | Digital | **No meaningful gamepad equivalent for authentic use**, though technically remappable to a handful of buttons for reduced functionality. |
| `keyboard` | Full computer keyboard, e.g. `c64` (66), `pet2001` (74), `a2000` (94), `ibm5150` (121) | Digital | **No** for authentic use — a full alphanumeric keyboard is not a gamepad-shaped problem at all; this is a platform-convention fact (§9 of the prior audit already flags "Keyboard — Not 'controller' evidence per se"), reaffirmed here specifically for MAME-hosted home-computer drivers. |
| `mahjong` | Dedicated mahjong-tile panel, e.g. `akiss` (19 buttons), `mjsiyoub` (26 buttons/player) | Digital | **No meaningful gamepad equivalent for authentic play** — a game-specific button scheme with no standard modern-pad mapping convention, though technically playable via a large reduced button map. |
| `hanafuda` (not observed directly this pass — `IPT_HANAFUDA_FIRST`/`LAST` confirmed in source, `infoxml.cpp:1715-1719`) | Dedicated hanafuda-card panel, same family as mahjong | Digital | Same tier as `mahjong` — **no meaningful gamepad equivalent for authentic play.** |
| `gambling` (not observed directly this pass — `IPT_GAMBLING_FIRST`/`LAST` pattern implied by the same `default:` range-check style seen for mahjong/hanafuda; not individually re-confirmed in source read this pass, flagged **UNCERTAIN at the exact enum-range level**, though the task brief's own `3cdpoker` example, buttons="9", is a real gambling-panel machine) | Dedicated gambling-cabinet button panel | Digital | Same tier as `mahjong`/`hanafuda` — **no meaningful gamepad equivalent for authentic play.** |

**INFERENCE:** the honest three-tier grouping this table supports is: (1) **trivially fine on a
pad** — `joy`, `doublejoy`/`triplejoy`, `stick`, `only_buttons`; (2) **contentious, commonly
tolerated in practice but genuinely degraded for a minority of titles** — `paddle`, `dial`,
`trackball`, `mouse`, `positional`, `pedal`; (3) **no meaningful gamepad substitute for authentic
play** — `lightgun`, `keypad`, `keyboard`, `mahjong`, `hanafuda`, `gambling`. This grouping is
carried through to §8's requirement-strength classification.

## 4. Ways/direction semantics — cabinet fact, not requirement proof

`ways`/`ways2`/`ways3` (§2.1) are, per direct source confirmation, MAME's record of **how many
discrete directions the original joystick hardware could physically register** (`field.way()`),
one value per independent stick cluster. This document reasons through the requirement-vs-cabinet
question carefully, per the task's own instruction not to overstate it:

- `ways="4"` is a **hardware fact about the original control**: a genuine 4-way arcade stick
  physically cannot register diagonal positions at all (it has a cross-shaped gate that mechanically
  blocks them), so the *original game logic* only ever reads up/down/left/right for that input.
- This **does** constrain what the game's own logic actually consumes — a `ways="4"` game was
  never designed to distinguish a diagonal input from its nearest orthogonal neighbor, so nothing
  is "lost" by playing it with a modern 8-way-capable stick; MAME's own input-emulation layer
  (not modeled by this document, out of scope per §0) is what maps a physical 8-way input down to
  the driver's 4-way read, not a property the *player's device* needs to match.
- It does **not** mean a modern 8-way stick "won't work" — the opposite is true in the overwhelming
  common case: an 8-way-capable device is a strict superset of what a 4-way game needs. The
  practical risk runs the other way — a *digital d-pad with poor diagonal detection* being used
  for an 8-way game (`sf2`, `simpsons`, `tmnt` in this pass's sample, all `ways="8"`) is a much
  more plausible real friction point than a modern stick being "too capable" for a 4-way game.
- **`ways2`/`ways3` for `doublejoy`/`triplejoy`** describe each stick cluster's own directional
  restriction independently (§2.1) — e.g. `vindictr`'s `ways="vertical2" ways2="vertical2"` states
  both of its two stick clusters are restricted to vertical-only 2-way movement, a materially
  different cabinet fact from a generic 2-way (horizontal) restriction, and one a naive
  "ways=2 means left/right" assumption would get wrong.

**Verdict:** `ways` values are **cabinet control descriptions with a real, provable implication
for what the original game logic reads**, but they are **not, by themselves, evidence that a
specific class of modern hardware is required or insufficient** — a modern controller with strictly
more directional resolution than the cabinet's original hardware is never disadvantaged by that
fact alone.

## 5. Analog evidence — proposed categories, deliberately unmapped to hardware

`minimum`/`maximum`/`sensitivity`/`keydelta`/`reverse` (§2.1) describe an analog control's
calibration range and MAME's own keyboard-substitute tuning defaults. This document proposes four
conceptual categories for a future static model, and **explicitly does not map any of them to
specific modern controller hardware capabilities** — per the prior audit's Ready-to-Play boundary
reasoning (§9 below, §13/§21.5 of the prior audit), that mapping decision belongs to a future,
separate, more carefully curated phase, not this one:

- **`ANALOG_AXIS_REQUIRED`** — a bounded analog range read as a spatial axis (`stick`, and
  `paddle` when used as a rotational analog control rather than digital buttons).
- **`ANALOG_TRIGGER_OR_PEDAL`** — a bounded analog range read as a one-dimensional throttle/brake
  input (`pedal`).
- **`ABSOLUTE_POINTER`** — a bounded analog range read as an absolute screen/surface coordinate
  (`lightgun`, `positional`).
- **`RELATIVE_POINTER`** — an unbounded or wrap-around relative-motion analog read (`trackball`,
  `dial`, `mouse`) — grouped together here for evidence-tier purposes despite real UX differences
  the normalization step (§11) explicitly flags as a cost, not hidden.

## 6. Player count semantics — `players` is a supported count, never a required-controller count

**INFERENCE, grounded in both source reading (§2.1) and this pass's own real data (§16):**
`players` is computed as a plain running maximum of distinct player indices declared anywhere in
the machine's ports (`infoxml.cpp:1470-1471`) — it answers "how many simultaneous player slots does
this driver's input model support," never "how many controllers must be connected before the game
will start."

The real sample gathered in this pass makes the case concretely:

- `robotron` is `players="1"` — a single twin-stick control scheme, obviously not "requires
  exactly 1 and only 1 person," just a description of the game's player-count design.
- `simpsons` and `tmnt` are `players="4"` — both are well-documented as fully playable
  single-player (one person controls "player 1"'s joystick/buttons; the game does not require the
  other three player slots to be occupied to start or progress).
- `sf2` is `players="2"` — a 2-player fighting game that is trivially, commonly played entirely
  solo against the CPU using only "player 1"'s controls.

**Conclusion, stated plainly:** `players` is a **"designed for up to N simultaneous players"**
fact, never a **"requires N controllers to start"** fact. EmuWiz must never present `players="4"`
as "requires 4 controllers" — the correct, safe phrasing is closer to "supports up to 4 players"
(§18).

## 7. Button count semantics — a safe, purely descriptive claim

**INFERENCE:** `buttons` (and, where present and version-appropriate, `reqbuttons`, §2.1) is a
true, checkable fact about how many distinct in-game button actions the original machine's control
panel exposed for a player — nothing more. EmuWiz can safely state "this machine's original cabinet
used up to N action buttons" as a plain factual claim, because it is descriptive of the *game*, not
a claim about what modern hardware needs. Whether a specific gamepad (typically 4 face buttons +
2-4 shoulder/trigger inputs, commonly 8-12 total inputs on modern hardware) can comfortably map N
buttons is a UX/ergonomics question this document deliberately does not answer — that is a
different, future concern (mapping comfort, not requirement proof). Concretely: `sf2`'s
`buttons="6"` per player is a real, checkable fact about the original cabinet; whether 6 buttons
maps comfortably onto a modern pad's face+shoulder buttons (it generally does, since most modern
pads expose 6+ digital button inputs beyond the d-pad) is a UX judgment, never asserted as a hard
requirement by this document.

## 8. Special peripherals — conservative static evidence classes

Applying §3's three-tier grouping to the task brief's proposed states:

| Class | Meaning | Applied to (this pass's real examples) |
|---|---|---|
| `SPECIAL_PERIPHERAL_REQUIRED` | No meaningful gamepad substitute exists for authentic play | `lightgun` (`lethalen`, `cheyenne`), `mahjong` (`akiss`, `mjsiyoub`), `gambling` (`3cdpoker`), `keyboard`/`keypad` (`c64`, `pet2001`, `a2000`, `ibm5150`) |
| `SPECIAL_PERIPHERAL_LIKELY` | Commonly played, imperfectly but functionally, on an analog stick in practice; a real minority of titles genuinely degrade | `trackball` (`centiped`, `atarifb`), `dial` (`tempest`, `arkanoid`, `ssrj`), `paddle`/`pedal` (`outrun`, `wecleman`, `crgolf`), `mouse` (`a2000`, `ibm5150`), `positional` |
| `UNKNOWN` | No per-title curation exists yet distinguishing the "degrades badly" minority from the "fine on a pad" majority within `_LIKELY` | Every specific title inside the `_LIKELY` class, until a second curated layer exists (§9) |

**Rule, mirroring the prior audit's §10 rule:** `SPECIAL_PERIPHERAL_REQUIRED` may only be set from
raw MAME control-type presence for the tier-3 types in §3 (no meaningful substitute exists at
all, by the type's own nature); it must never be inferred for tier-2 types from raw MAME metadata
alone — those stay `_LIKELY` or `UNKNOWN` until a curated per-title source exists (§9).

## 9. Cabinet vs. modern input abstraction — the critical boundary

This is the section the task brief calls "critical," and the prior audit already reached the
governing conclusion this document applies rather than re-derives (prior audit §4, §12.3, §21.2):
**a cabinet's original controls are a fact about the cabinet, not a proof about what's required
for emulated, pad-based play today.**

Working through the precise boundary:

- **True, provable, static fact:** "MAME's driver metadata declares this machine's original
  cabinet used a `trackball` control." This is directly checkable from `-listxml` output, requires
  no judgment call, and is stable (barring a driver correction) across MAME versions.
- **Not provable by MAME metadata alone:** "the user must own a trackball to play this game
  acceptably." This is a claim about *today's emulated experience on typical modern hardware*,
  which depends on factors MAME's cabinet-hardware declaration says nothing about: how forgiving
  the specific game's trackball-reading code is to analog-stick-simulated input, whether the game
  has hard timing/precision requirements a stick cannot approximate, and community-accumulated
  playability experience — none of which is in `-listxml`.
- The gap between these two is exactly what a **second, curated evidence layer** would need to
  close, per-title — not something a raw schema read can ever resolve on its own, no matter how
  completely EmuWiz parses `<control>`.

**Concrete wording rules this document proposes for any future UI surfacing this evidence:**

1. Always phrase as a fact about the **original machine**: "the original cabinet used a
   trackball," never "you need a trackball."
2. Never phrase as a requirement on the **user's setup**: never "your controller must have X,"
   never "requires X" as a bare claim.
3. The only exception to rule 2 is a narrow, explicitly curated subset where a second evidence
   layer has separately and specifically established that no reasonable substitute exists for
   *this particular title* (not the type in general) — and even then, phrasing should name what
   was checked, not assert failure from absence (mirroring the prior audit's §18 UX discipline).
4. Never state a bare "N controllers required" from `players` (§6) or a bare "N buttons needed" as
   a requirement rather than a description (§7) — both are safe as descriptive facts, unsafe as
   requirement claims.

## 10. Proposed static model — conceptual types only

Illustrative, **non-normative** pseudocode; no such types exist in the repository.

```text
// DESIGN ONLY — illustrative shape, not Rust to be compiled or copied verbatim.

InputRequirementStrength {
    AuthoritativeMachineMetadata,  // directly read from this exact machine's <input>/<control>
    NormalizedRequirement,         // derived by applying §11's normalization vocabulary
    HeuristicMapping,              // pattern/genre-level guess — never used to assert a requirement
    Unknown,                       // default; no evidence gathered
}

ArcadeControlEvidence {
    raw_type: String,              // verbatim MAME `type`, e.g. "trackball", "doublejoy"
    player: Option<u8>,
    buttons: Option<u32>,
    req_buttons: Option<u32>,      // present only for MAME builds that still emit it (§2.1)
    ways: Option<String>,          // verbatim, e.g. "4", "vertical2" — never coerced to a number
    ways2: Option<String>,
    ways3: Option<String>,
    minimum: Option<i32>,
    maximum: Option<i32>,
    sensitivity: Option<i32>,
    keydelta: Option<i32>,
    reverse: bool,
}

ArcadeInputRequirement {
    normalized_family: NormalizedInputFamily,   // §11's small vocabulary
    strength: InputRequirementStrength,
    special_peripheral_class: SpecialPeripheralClass,  // §8's REQUIRED/LIKELY/UNKNOWN
    raw_evidence: Vec<ArcadeControlEvidence>,   // §12 — raw never discarded
}

ArcadeInputProfile {
    machine_name: String,           // MAME shortname, the identity this evidence is scoped to
    players_supported: u8,          // §6 — never "required"
    coins: Option<u32>,
    service: bool,
    tilt: bool,
    requirements: Vec<ArcadeInputRequirement>,
    provenance: ProvenanceRef,      // e.g. "MAME -listxml, machine <name>, artifact sha256 ..."
}
```

Entirely static — no physical-device state, no SDL/evdev reference, no "connected"/"detected"
concept anywhere in this model, consistent with the prior audit's `InputCapability`/
`InputDeviceEvidence` (runtime) being a deliberately separate future type this document does not
touch.

## 11. Normalization vocabulary

| Normalized family | Meaning | Real examples mapped |
|---|---|---|
| `DIGITAL_DIRECTIONS` | Restricted-direction digital stick | `joy`, `doublejoy`, `triplejoy` |
| `BUTTONS` | Pure digital button panel, no directional element | `only_buttons` |
| `ANALOG_AXIS` | Bounded analog spatial axis | `stick` |
| `RELATIVE_POINTER` | Unbounded/relative analog motion | `trackball`, `dial`, `mouse`, `paddle` (when used as a continuous rotary rather than digital) |
| `ABSOLUTE_POINTER` | Bounded analog absolute coordinate | `lightgun`, `positional` |
| `PEDAL` | One-dimensional analog throttle/brake | `pedal` |
| `DUAL_STICK` | Two independent digital direction clusters | `doublejoy` (a semantic refinement layered on top of `DIGITAL_DIRECTIONS` for twin-stick titles specifically, since `robotron`-class play is meaningfully different UX from a plain single stick) |
| `KEYBOARD` | Full or near-full alphanumeric keyboard | `keyboard` |
| `SPECIAL_PANEL` | Fixed, game-specific button scheme with no general convention | `keypad`, `mahjong`, `hanafuda`, `gambling` |
| `UNKNOWN` | A `type` string not yet mapped by this vocabulary | Any future/unrecognized MAME `type` value |

**Cost of normalization, stated explicitly per the task brief's instruction not to hide it:**
`paddle`, `dial`, and `trackball` are all bucketed into `RELATIVE_POINTER` here despite real UX
differences — a paddle is a bounded rotary knob with an end-stop, a dial/spinner is typically
unbounded (can spin indefinitely), and a trackball is a genuinely two-axis relative device, not a
single-axis one. Collapsing all three into one normalized family loses exactly the distinction
that matters for §8's `_LIKELY` vs `_REQUIRED` per-title judgment calls a future curated layer
would need to make. This is a real, acknowledged cost of normalization — not a reason to avoid
normalizing (a coarse family is still useful for broad filtering/search), but a reason §12's raw
retention is mandatory, not optional.

## 12. Raw evidence retention

**Recommendation, non-negotiable per the task brief's own framing:** retain every raw MAME
attribute (`type`, `ways`, `ways2`, `ways3`, `buttons`, `reqbuttons` where present, `minimum`,
`maximum`, `sensitivity`, `keydelta`, `reverse`, `player`) verbatim alongside the normalized
family from §11 — never replace raw with normalized-only. Justification, grounded in this pass's
own findings: `ways="vertical2"` (§4, §16) would be silently destroyed by any parser that coerces
`ways` to an integer; `reqbuttons` (§2.1) is meaningful exactly because it differs from `buttons`
in specific, real cases, and a normalized model that only kept `buttons` would erase that signal
entirely; and §11's own `RELATIVE_POINTER` bucket already demonstrates a normalized family alone
cannot answer per-title playability questions a future curated layer will need the raw `type`
string to even ask correctly.

## 13. DAT model boundary — explicit decision

The task brief's four options, with tradeoffs grounded in what this pass actually read:

- **(A) Generic DAT model (`dat/model.rs`'s `DatGameEntry`).** `DatGameEntry` is shared by every
  DAT ecosystem EmuWiz supports (No-Intro, Redump, MAME, Logiqx, ClrMamePro — confirmed by
  `DatEcosystem`'s use across `dat/parsers/`, read in this pass). Adding `<input>`/`<control>`
  fields here means every non-arcade DAT ecosystem's `DatGameEntry` instances carry always-empty
  input fields forever — the task brief's own "junk drawer" framing is correct, and this pass's own
  reading of `dat/model.rs`'s already-large field list (§1.1, 20+ fields) makes it concretely worse
  to extend further with arcade-only data. **Rejected.**
- **(B) MAME-specific metadata extension** (a new, MAME-listxml-only structured type, parsed by
  `dat/parsers/mame_listxml.rs` but stored separately from `DatGameEntry`, keyed by machine name).
  This keeps the generic DAT model clean while still landing the data at DAT-import time, in the
  same file already reading the raw XML stream (`dat/parsers/mame_listxml.rs`'s
  `handle_empty_like`, §1.2) — the natural place to capture `<control>` children the current code
  already walks past. **Strong candidate.**
- **(C) Arcade compatibility evidence model** (extending `arcade_mame_compatibility.rs`'s existing
  per-set projection). Rejected on the grounds read directly from that module in §1.4: its own
  module doc and type vocabulary (`MameSetCompatibilityState`, `MameMismatchReason`) are entirely
  about ROM/CHD dump completeness against a pinned installed build — a different question
  (can this exact dump run on this exact MAME?) from a fact about what the *machine* (any
  correctly-dumped copy, any MAME build) requires for play. Mixing the two would make an already
  single-purpose, well-scoped module's type vocabulary do double duty for an unrelated concern.
  **Rejected**, consistent with this module's own stated scope.
- **(D) Separate input metadata projection** (a wholly new module, e.g.
  `arcade_input_metadata.rs`, independent of both the DAT model and the compatibility module,
  populated from the same parsed `<input>`/`<control>` data as (B) but exposed as its own
  standalone evidence surface rather than folded into the MAME listxml parser's output type).

**Decision: (B), with the new structured data captured inside the existing MAME listxml parsing
path (`dat/parsers/mame_listxml.rs`) but stored in a dedicated type, not squeezed into
`DatGameEntry`.** Reasoning: (B) and (D) differ only in whether the new type lives adjacent to
where the XML is already being streamed and parsed, or is built as an entirely separate module;
this document prefers (B)'s tighter coupling because the parser already owns the exact
`Event::Start`/`Event::Empty` walk needed to read `<control>` children correctly (nested inside
`<input>`, itself nested inside `<machine>`) — building a second, independent XML walk elsewhere
(D) would duplicate that streaming/limits/warnings machinery (`DatLimits`, `ParseWarning`,
depth-limit enforcement, all already present in this exact file) for no benefit, and risks drifting
out of sync with the primary parser's own machine-identity resolution. The task brief's "should not
become a junk drawer" instinct about (A) is correct and independently confirmed by this pass's own
reading of `DatGameEntry`'s size; but that instinct does not, on inspection, extend to rejecting a
MAME-specific type that still lives in the MAME-specific parser file — it only rejects polluting
the *shared* model every other ecosystem's entries also carry.

## 14. Ready-to-Play boundary

Per this document's own non-goals (§0), no wiring is proposed. Consistent with, and directly
citing, the prior audit's own §13 A/B/C boundary answer (option (c), narrowly construed, with (b)
as the default day-to-day behavior — prior audit §21.5): **static MAME input evidence alone,
without runtime device evidence, can never justify more than an informational/coverage-note
presence.** A `players="4"`/`buttons="6"` fact by itself proves nothing about whether *this
session* can be played right now — the prior audit's axis framework (§11 there) already
establishes that a static requirement fact (axis A) combined with *nothing else* (no axis B/C/D
evidence at all) stays `Unknown`/informational, never `NeedsAttention`, never `Blocked`. This
document's own scope (static evidence only, §0) means it produces, at most, axis-A-only evidence
— exactly the case the prior audit's §13 already resolves to "informational only." The one
narrow case the prior audit allows `NeedsAttention` for (a proven-`_REQUIRED` peripheral class
with proven-absent runtime evidence, prior audit §13 example 2) explicitly requires *both* a
curated per-title layer *and* runtime evidence this document does not gather — so it remains
future work, not something this document's own scope produces on its own.

## 15. Moonlight/Sunshine boundary

Reaffirming the prior audit's finding (§15/§16 there) without re-deriving it: **server-side
virtual gamepad evidence is never proof of a Moonlight/Sunshine streaming client's actual
hardware.** This document adds one explicit clarification specific to its own static scope:
**static MAME requirement evidence and runtime host-device state are two entirely separate,
unrelated evidence dimensions that must never be merged into one signal in any future data
model.** Concretely: a future `InputReadiness` (or similarly named) projection must never let "MAME
`-listxml` proves this machine's cabinet used a lightgun" collapse into the same boolean or state
value as "a gamepad happens to be plugged into this server right now" — one is a permanent,
game-identity-scoped static fact; the other is an ephemeral, machine-scoped runtime fact that,
per the prior audit's §16 case study, may not even describe the right machine at all in a
remote-play topology. Keeping them as separate typed facts (as §10's model already does, by
carrying no runtime field whatsoever) is the concrete way this document proposes avoiding that
conflation.

## 16. Real-data case study

All rows are **DOCUMENTED FACT (local MAME 0.264)**, gathered directly via `mame -listxml
<shortname>` in this pass (bounded sample: the task brief's own seed list plus a handful of
targeted follow-ups for `keyboard`/`mouse`/`positional`/`only_buttons` coverage — no full-dataset
scan performed, per the task's own instruction).

| Game | Raw `<input>`/`<control>` | Control type(s) | Normalized family | Requirement strength | Peripheral class | Plain-language sentence (§18 rules) |
|---|---|---|---|---|---|---|
| `pacman` | `players="2" coins="2"`; `joy player="1" ways="4"`, `joy player="2" ways="4"` | `joy` | `DIGITAL_DIRECTIONS` | Authoritative | — | "The original cabinet used a 4-way joystick; supports up to 2 players." |
| `dkong` | `players="2" coins="1" service="yes"`; `joy buttons="1" ways="4"` ×2 | `joy` | `DIGITAL_DIRECTIONS`+`BUTTONS` | Authoritative | — | "The original cabinet used a 4-way joystick and 1 action button per player." |
| `sf2` | `players="2" coins="2" service="yes"`; `joy buttons="6" ways="8"` ×2 | `joy` | `DIGITAL_DIRECTIONS`+`BUTTONS` | Authoritative | — | "The original cabinet used an 8-way joystick and up to 6 action buttons per player." |
| `robotron` | `players="1" coins="3" service="yes" tilt="yes"`; `doublejoy ways="8" ways2="8"` | `doublejoy` | `DUAL_STICK` | Authoritative | LIKELY (two-stick control is unusual but reads naturally on a modern dual-stick pad) | "The original cabinet used two independent 8-way joysticks (twin-stick control)." |
| `vindictr` | `players="2" coins="3"`; `doublejoy buttons="4" ways="vertical2" ways2="vertical2"` ×2 | `doublejoy` | `DUAL_STICK` | Authoritative | LIKELY | "The original cabinet used two independent vertical-only 2-way joysticks (twin-stick control) and up to 4 buttons per player." |
| `centiped` | `players="1" coins="2" tilt="yes"`; `joy buttons="1" ways="8"`, `trackball minimum="0" maximum="255" sensitivity="50" keydelta="10" reverse="yes"` | `joy`, `trackball` | `DIGITAL_DIRECTIONS`+`RELATIVE_POINTER` | Authoritative | LIKELY | "The original cabinet used a trackball; EmuWiz has not confirmed whether this title plays acceptably on a standard controller." |
| `atarifb` | `players="2" coins="2" tilt="yes"`; `trackball buttons="1" minimum="0" maximum="255" sensitivity="100" keydelta="10" reverse="yes"` ×2 | `trackball` | `RELATIVE_POINTER` | Authoritative | LIKELY | "The original cabinet used a trackball per player." |
| `tempest` | `players="2" coins="3" tilt="yes"`; `dial buttons="2" minimum="0" maximum="240" sensitivity="100" keydelta="20"`, `dial buttons="2" minimum="0" maximum="15" sensitivity="100" keydelta="20"` | `dial` | `RELATIVE_POINTER` | Authoritative | LIKELY | "The original cabinet used a rotary spinner (dial) control." |
| `ssrj` | `players="1" coins="2" tilt="yes"`; `dial minimum="0" maximum="255" sensitivity="50" keydelta="4" reverse="yes"`, `pedal minimum="0" maximum="224" sensitivity="50" keydelta="32"` | `dial`, `pedal` | `RELATIVE_POINTER`+`PEDAL` | Authoritative | LIKELY | "The original cabinet used a rotary spinner and a foot pedal." |
| `outrun`* | `players="1" coins="2" service="yes"`; `paddle buttons="1" minimum="32" maximum="224" sensitivity="100" keydelta="4"`, `pedal minimum="0" maximum="255" sensitivity="100" keydelta="40"` | `paddle`, `pedal` | `RELATIVE_POINTER`+`PEDAL` | Authoritative | LIKELY | "The original cabinet used a steering wheel (modeled as an analog paddle) and a foot pedal." |
| `wecleman` | `players="1" coins="2" service="yes"`; `paddle buttons="2" minimum="0" maximum="255" sensitivity="50" keydelta="5"`, `pedal minimum="0" maximum="128" sensitivity="30" keydelta="10"` | `paddle`, `pedal` | `RELATIVE_POINTER`+`PEDAL` | Authoritative | LIKELY | "The original cabinet used a steering wheel (modeled as an analog paddle) and a foot pedal." |
| `arkanoid` | `players="2" coins="2" tilt="yes"`; `dial buttons="1" minimum="0" maximum="255" sensitivity="30" keydelta="15"` ×2 | `dial` | `RELATIVE_POINTER` | Authoritative | LIKELY | "The original cabinet used a rotary spinner (dial) per player." |
| `crgolf` | `players="2" coins="1"`; `joy buttons="6" ways="2"`, `stick minimum="0" maximum="255" sensitivity="70" keydelta="16" reverse="yes"` ×2 | `joy`, `stick` | `DIGITAL_DIRECTIONS`+`ANALOG_AXIS` | Authoritative | LIKELY (analog axis) | "The original cabinet used a 2-way joystick and an analog stick per player." |
| `sinistar` | `players="1" coins="3" service="yes" tilt="yes"`; `stick buttons="2" minimum="0" maximum="111" sensitivity="100" keydelta="10" reverse="yes"` | `stick` | `ANALOG_AXIS` | Authoritative | — (trivially fine) | "The original cabinet used an analog stick and up to 2 action buttons." |
| `lethalen`* | `players="2" coins="2" service="yes"`; `lightgun buttons="1" minimum="0" maximum="255" sensitivity="25" keydelta="15"` ×2 | `lightgun` | `ABSOLUTE_POINTER` | Authoritative | **REQUIRED** | "The original cabinet used a light gun." |
| `cheyenne` | `players="1" coins="2"`; `lightgun buttons="1" minimum="0" maximum="255" sensitivity="70" keydelta="10"` | `lightgun` | `ABSOLUTE_POINTER` | Authoritative | **REQUIRED** | "The original cabinet used a light gun." |
| `vectrex` | `players="2"`; `stick buttons="5" minimum="0" maximum="255" sensitivity="50" keydelta="30" reverse="yes"`, `lightgun minimum="0" maximum="255" sensitivity="35" keydelta="1" reverse="yes"`, `stick buttons="4" ...` | `stick`, `lightgun` | `ANALOG_AXIS`+`ABSOLUTE_POINTER` | Authoritative | **REQUIRED** for the lightgun-class control (the Vectrex's own 3-D Imager/light-pen accessory) | "The original hardware used an analog stick, and one player's controls include a light-pen-class pointer accessory." |
| `akiss` | `players="1" coins="1" service="yes"`; `mahjong buttons="19"` | `mahjong` | `SPECIAL_PANEL` | Authoritative | **REQUIRED** | "The original cabinet used a dedicated 19-button mahjong control panel." |
| `mjsiyoub` | `players="2" coins="3"`; `mahjong buttons="26"` ×2 | `mahjong` | `SPECIAL_PANEL` | Authoritative | **REQUIRED** | "The original cabinet used a dedicated 26-button mahjong control panel per player." |
| `3cdpoker`* | `players="1" coins="4"`; `gambling buttons="9"` | `gambling` | `SPECIAL_PANEL` | Authoritative | **REQUIRED** | "The original cabinet used a dedicated gambling control panel." |
| `quizshow` | `players="2" coins="2"`; `only_buttons buttons="5"`, `only_buttons buttons="4"` | `only_buttons` | `BUTTONS` | Authoritative | — (trivially fine) | "The original cabinet used a fixed button panel only, no joystick." |
| `simpsons` | `players="4" coins="4" service="yes"`; `joy buttons="2" ways="8"` ×4 | `joy` | `DIGITAL_DIRECTIONS`+`BUTTONS` | Authoritative | — | "The original cabinet used an 8-way joystick and up to 2 buttons per player; supports up to 4 players (fully playable solo)." |
| `tmnt` | `players="4" coins="4"`; `joy buttons="2" ways="8"` ×4 | `joy` | `DIGITAL_DIRECTIONS`+`BUTTONS` | Authoritative | — | "The original cabinet used an 8-way joystick and up to 2 buttons per player; supports up to 4 players (fully playable solo)." |
| `c64` | `players="1"`; `joy buttons="1" ways="8"`, `keyboard buttons="66"` | `joy`, `keyboard` | `DIGITAL_DIRECTIONS`+`KEYBOARD` | Authoritative | **REQUIRED** for keyboard-driven functionality | "This is a home-computer system; the original hardware used a keyboard alongside an 8-way joystick." |
| `pet2001` | `players="1"`; `keyboard buttons="74"` | `keyboard` | `KEYBOARD` | Authoritative | **REQUIRED** | "This is a home-computer system; the original hardware used a full keyboard, no joystick." |
| `a2000` | `players="2"`; `joy buttons="3" ways="8"`, `mouse minimum="0" maximum="255" sensitivity="100" keydelta="5"`, `keyboard buttons="94"` (×2 players for joy/mouse) | `joy`, `mouse`, `keyboard` | `DIGITAL_DIRECTIONS`+`RELATIVE_POINTER`+`KEYBOARD` | Authoritative | **REQUIRED** for keyboard/mouse-driven software | "This is a home-computer system; the original hardware used a keyboard, mouse, and joystick." |
| `ibm5150` | `players="1"`; `mouse buttons="3" minimum="0" maximum="61440" sensitivity="100"`, `keyboard buttons="121"` | `mouse`, `keyboard` | `RELATIVE_POINTER`+`KEYBOARD` | Authoritative | **REQUIRED** | "This is a home-computer (PC) system; the original hardware used a keyboard and mouse, no joystick." |

*`outrun`, `lethalen`, `3cdpoker` are the task-brief-supplied ground-truth examples, reproduced
here for the table's completeness; all others in this table were independently pulled from the
installed MAME 0.264 binary in this pass specifically to broaden §3's type coverage
(`only_buttons`, `keyboard`, `keypad`, `mouse`, a non-numeric `ways` value, and a second real
`trackball`/`mahjong`/`lightgun` example beyond the task brief's seeds).

**New control-type examples found in this pass beyond the task brief's seed list, as requested:**
`keyboard` (`c64`, `pet2001`, `a2000`, `ibm5150` — MAME's home-computer drivers, confirmed by
direct sample and by the `IPT_KEYBOARD` mapping in source), `mouse` (`a2000`, `ibm5150`),
`only_buttons` (`quizshow`), and a `keypad` type was confirmed present in source
(`IPT_KEYPAD`/`infoxml.cpp:1684-1689`) though no machine with `type="keypad"` in `<input>` (as
opposed to the `c64` floppy-drive submachine's separate device XML, which this pass's targeted
grep did not isolate) was captured directly in this bounded sample — flagged **UNCERTAIN, not
independently confirmed with a real top-level-machine `<control type="keypad">` example in this
pass**, though `IPT_KEYPAD`'s existence in current source is DOCUMENTED FACT. `positional` and
`hanafuda`/`gambling`'s exact `IPT_*` range boundaries were confirmed structurally in source
(§2.1, §3) but no real machine example was captured for `positional` specifically in this bounded
pass.

## 17. Performance

- **Parse once, at DAT-import time.** `dat/parsers/mame_listxml.rs` already streams the XML once
  per import (§1.2); extending its existing `Event::Start`/`Event::Empty` walk to also capture
  `<control>` children is strictly additive to work already being done, not a new pass over the
  file.
- **Cache compact static evidence** alongside the rest of the imported `ParsedDat`/`DatIndex`
  structures the existing importer already builds and returns (`identity_source/mame_listxml/
  import.rs:81-111`, `ImportedMameListxmlSource`) — no new, separately-invalidated cache.
- **No runtime MAME invocation per game.** Every fact in §16's table came from a single
  `-listxml` dump read once; nothing here requires launching MAME, or querying it, per title or
  per render.
- **No render-time XML parsing.** Once captured at import time into a compact typed structure
  (§10), a GUI page consuming it reads an already-parsed in-memory/cached value, never re-parses
  raw XML on paint.
- **No duplicate metadata stores.** §13's decision (option B) keeps this data in one place, owned
  by the MAME listxml import path, rather than a second copy maintained by some other subsystem.

## 18. UX

Plain-language example strings, extending the task brief's own examples and §16's table, all
following the §9 rule (state what the original machine used, never assert a requirement on the
user's setup):

- "The original cabinet supports up to 4 players. You can play solo using player 1's controls."
  (§6 — never "requires 4 controllers.")
- "The original cabinet used up to 6 action buttons per player." (§7 — a plain fact, not a
  compatibility claim.)
- "The original machine used a trackball. EmuWiz has not confirmed whether this title plays
  acceptably on a standard controller." (§9 — the prior audit's own phrasing, reused verbatim for
  consistency.)
- "The original machine used a light gun." (§8/§9 — stated as a bare cabinet fact for the
  `_REQUIRED` tier; even here, no "you need a light gun" language, consistent with §9 rule 3's
  narrow-curated-exception requirement not yet being met by raw MAME metadata alone.)
- "This is a home-computer system. The original hardware used a full keyboard." (§16's `pet2001`/
  `ibm5150` rows — a platform-convention fact, not a controller-compatibility claim.)

**Rule, restated for emphasis:** never say "you need X" from MAME metadata alone; only "the
original machine uses X."

## 19. Final decisions

### 19.1 Is MAME `<input>`/`<control>` strong enough to become authoritative static game input evidence?

**Yes — authoritative for exactly one thing: what the original cabinet's control panel physically
had, per §2's direct DTD/source reading.** It is **not** authoritative for, and must never be
presented as, a claim about what a modern gamepad or the user's own setup requires (§9). The
distinction is not a hedge — it is the entire finding of this document, grounded in reading the
generator source itself (§2.1): every attribute traces back to a driver author's declaration of
original hardware behavior, never to any judgment about emulated-play ergonomics.

### 19.2 Generic DAT vs. MAME-specific extension?

**MAME-specific extension, captured inside the existing MAME listxml parser file but stored in a
dedicated type, never folded into the shared `DatGameEntry`** (§13, option B). The generic model
already carries 20+ fields shared across every DAT ecosystem EmuWiz supports; adding
arcade-only input fields there would be exactly the "junk drawer" the task brief warned against,
confirmed by this pass's own direct reading of `dat/model.rs`.

### 19.3 Can player count equal required controller count?

**No.** `players` is a running maximum of distinct player-numbered input slots the driver
declares (§2.1, §6), not a "must connect N controllers" gate. `robotron` (`players="1"`,
obviously not exclusionary) and `simpsons`/`tmnt` (`players="4"`, both commonly and fully played
solo on "player 1"'s controls alone, §16) are the concrete evidence for this conclusion.

### 19.4 Can original cabinet controls equal required modern hardware?

**No, in general — yes only for a narrow, curated, explicitly-proven subset, never inferred
automatically from raw MAME metadata alone.** §3/§8's tier-3 types (`lightgun`, `keyboard`,
`keypad`, `mahjong`, `hanafuda`, `gambling`) are the strongest candidates for an eventual
per-title "yes, required" answer, with `lightgun`-class peripherals the clearest case (§3's
reasoning: fundamentally different interaction model, no directional-input analog exists). Even
for these, this document only proposes a static `_REQUIRED` classification of the *control type*,
not a live claim about the *user's* setup — that step still needs the curated second layer §9
describes.

### 19.5 What normalized families are safe?

The §11 vocabulary: `DIGITAL_DIRECTIONS`, `BUTTONS`, `ANALOG_AXIS`, `RELATIVE_POINTER`,
`ABSOLUTE_POINTER`, `PEDAL`, `DUAL_STICK`, `KEYBOARD`, `SPECIAL_PANEL`, `UNKNOWN` — with the
explicit, acknowledged cost that `RELATIVE_POINTER` flattens real UX differences between
`paddle`/`dial`/`trackball`/`mouse` (§11), which is exactly why §12's raw retention is mandatory.

### 19.6 What raw fields must always be retained?

All of them, verbatim, never coerced: `type`, `player`, `buttons`, `reqbuttons` (where present),
`ways`/`ways2`/`ways3` (kept as strings, not integers — `vindictr`'s `"vertical2"` value would be
destroyed by integer coercion, §4/§16), `minimum`, `maximum`, `sensitivity`, `keydelta`,
`reverse` (§12).

### 19.7 Should this feed Ready-to-Play directly?

**No, not in this phase.** Per §14, static-only evidence (no runtime axis at all) resolves to, at
most, an informational/coverage-note presence under the prior audit's own §13 boundary rules —
this document's scope does not produce anything stronger, and does not propose wiring even that
informational presence into `ready_to_play.rs` itself; that remains a distinct, later
implementation decision.

### 19.8 Smallest next implementation slice?

Extend `dat/parsers/mame_listxml.rs`'s existing `handle_empty_like` dispatcher to (a) stop writing
the misleading empty `mame.input` stub key identified in §1.2, and (b) capture `<input>`'s own
attributes plus each nested `<control>` element's full attribute set into a new, dedicated,
MAME-specific structure (§10's `ArcadeInputProfile`/`ArcadeControlEvidence` shape) stored
alongside — not inside — `DatGameEntry`, with zero normalization, zero Ready-to-Play wiring, and
zero UI surfacing in this first slice. This matches §20's MI0/MI1 roadmap phases below.

## 20. Roadmap

- **MI0 — Fix the stub, define the vocabulary.** Remove the misleading empty `mame.input` stub
  key (§1.2) in the same change that defines (but does not yet populate) the §10 typed vocabulary
  (`ArcadeControlEvidence`, `ArcadeInputRequirement`, `ArcadeInputProfile`,
  `InputRequirementStrength`) and the §11 normalized-family enum. No parsing of `<control>` yet.
- **MI1 — Capture raw `<input>`/`<control>` at MAME listxml import time.** Extend
  `dat/parsers/mame_listxml.rs`'s `handle_empty_like` to walk `<control>` children of `<input>`
  and populate MI0's types with every raw attribute (§12), stored as a MAME-specific structure per
  §13's option (B) decision — not inside `DatGameEntry`. This is the first phase producing any
  real, provenance-carrying evidence.
- **MI2 — Normalize.** Apply §11's mapping to produce `normalized_family` alongside the
  already-retained raw evidence (never replacing it), and apply §8's conservative
  `SPECIAL_PERIPHERAL_REQUIRED`/`_LIKELY`/`UNKNOWN` classification using only the tier boundaries
  §3/§8 establish from the control type alone — no per-title curation yet.
  `InputRequirementStrength` for everything in this phase stays `NormalizedRequirement`, never
  promoted to a stronger tier.
- **MI3 — Later bridge to a future `InputReadiness`.** Out of this document's scope to design in
  detail (per §0's non-goals and the prior audit's own I0-I5 roadmap, which this static evidence
  would feed as one of several inputs, specifically axis A of the prior audit's §11 six-axis
  model) — named here only to mark where this document's own roadmap terminates and the prior
  audit's broader roadmap picks the thread back up.

No reordering of the task brief's MI0-MI3 shape was found justified by this research; each phase's
prerequisite is exactly what the preceding section found missing, mirroring the prior audit's own
roadmap-ordering discipline (its §22 closing note).

## 21. Explicit DO-NOT-BUILD list

- No wiring into Ready-to-Play in this phase (§14, §19.7).
- No runtime SDL/evdev enumeration, no controller-presence checking of any kind — this document
  is static-only, full stop.
- No permanent polling daemon of any kind (inherited from the prior audit's §14/§23, restated
  here for completeness since this document is a narrower descendant of that scope).
- No claim that raw cabinet-original control types prove a modern requirement, for any type,
  without a second curated layer (§9) — this is the single most load-bearing rule in this
  document.
- No collapsing `players` into "controllers required" (§6, §19.3).
- No collapsing `buttons`/`reqbuttons` into a hard requirement claim rather than a descriptive
  fact (§7).
- No normalized-only storage that discards raw MAME attributes (§12) — raw retention is mandatory.
- No merging static MAME requirement evidence with runtime host-device state (SDL enumeration,
  Steam Input, Moonlight/Sunshine virtual-gamepad state) in any single evidence type or boolean
  (§15).
- No changes to `DatGameEntry` or any other shared, ecosystem-wide DAT model type to carry
  arcade-only input fields (§13's rejection of option A).
- No GUI wiring, no Attention-model wiring, no Doctor finding producer — this document proposes
  data capture and normalization design only.

## Closing footer

No code changes were made anywhere in this repository while producing this document. No
production Rust file, GUI page, launch planner, MAME importer, DAT model, controller-profile code,
SDL/evdev runtime code, or database code was modified. The only file created is this document, at
`docs/research/MAME_STATIC_INPUT_METADATA_AUDIT.md`. No other file in this shared worktree —
including any other contributor's in-progress, uncommitted changes reported by `git status` at the
start of this task — was read beyond what this document's own grounding required (the specific
files named in the task brief, plus `git show HEAD:...` for `arcade_mame_compatibility.rs` rather
than its dirty working-tree copy, exactly as instructed), or touched in any way. All MAME
invocations performed in this pass were read-only `mame -listxml <shortname>` calls against a
locally installed MAME 0.264 binary — no ROM files were read, no MAME process wrote anything, and
no bare/unfiltered `-listxml` dump was ever run.

**What remains genuinely unresolved / not independently verified in this pass:**

1. The exact upstream MAME version (later than 0.264, earlier than or equal to the current
   `master` snapshot read in §2.1) at which `reqbuttons` was removed from `-listxml` output — its
   *removal* is confirmed by direct source diffing against the 0.264 DTD, but the specific version
   boundary was not identified.
2. Whether `positional` and `keypad` (top-level, not sub-device) control types appear on any real
   machine in the currently installed MAME 0.264 build — both are confirmed to exist in MAME's own
   source (`IPT_POSITIONAL`/`IPT_POSITIONAL_V`, `IPT_KEYPAD`), but no real top-level `<input>`
   example was captured for either in this bounded, non-exhaustive sample.
3. The exact `IPT_GAMBLING_FIRST`/`IPT_GAMBLING_LAST` enum boundary in current MAME source was not
   directly re-confirmed by this pass's source read (inferred by analogy to the confirmed
   mahjong/hanafuda range-check pattern); the real `3cdpoker` example (task-brief-supplied) is
   independently solid evidence the `gambling` type itself is real, even though this specific
   boundary detail is UNCERTAIN.
4. Whether MAME's driver-declared `<input>`/`<control>` metadata for any given machine has ever
   been found incorrect, incomplete, or placeholder-quality by MAME's own contributor community —
   this document assumes driver-declared metadata is generally trustworthy (consistent with the
   prior audit's §4 point 3, which already flags this as a real, unresolved caveat) but did not
   independently audit MAME's own bug tracker for known input-metadata defects in this pass.
5. Every UNCERTAIN item already carried forward from the prior audit that this document's narrower
   scope did not re-examine (compat-database redistribution rights, LaunchBox/ES-DE schema
   specifics, per-emulator config formats, Steam Input hook-point ordering, Ryubing/Azahar
   documentation depth) — none of those are re-opened or re-resolved here; see the prior audit's
   own closing footer for that list.
