# ES-DE parity Batch 4 mapping plan

## 1. Current live parity counts

Authoritative repository state inspected at a356670154b243b64dfb9a39b8f4b1f3bf291dbd on
feature/archivefs-unified-platform.

The live registry has 76 canonical Platform::id values. The production
ES_DE_SYSTEM_MAP has 50 real mapping rows: 49 are complete and MegaDrive is the
one partial row because the code deliberately chose ES-DE megadrive over its
equally real regional alternatives genesis and megadrivejp. There are no
duplicate mapped platform IDs and no duplicate ES-DE targets.

| State | Count |
| --- | ---: |
| Canonical EmuWiz platforms | 76 |
| COMPLETE | 49 |
| PARTIAL (MegaDrive) | 1 |
| MISSING | 26 |

The live missing set is exactly the supplied set (only its display order differs):
3DO; Acorn Archimedes; Acorn Electron; Amstrad CPC; Apple II; Atari 8-bit; BBC
Micro; Commodore 128; FM Towns; Macintosh; NEC PC-8801; NEC PC-9801; NeoGeo64;
Neo Geo CD; NGage; PC; PC-98; PC Engine; PC Engine CD; PC-FX; Philips CD-i; PS4;
Sharp X68000; Switch; TurboGrafx-16; VIC-20.

## 2. Raw upstream ES-DE verification method

Every candidate below was checked in the current upstream Linux systems file,
not from search-result labels:

<https://gitlab.com/es-de/emulationstation-de/-/raw/master/resources/systems/linux/es_systems.xml>

For each EmuWiz ID, the raw XML was searched for the candidate name, then its
enclosing system was read to confirm the adjacent fullname and ROMPATH/name
context. Absence conclusions used exact raw searches for both the expected short
name and product full name. This matters for PC Engine: pce and pcecd occur in
emulator command names/options, but are not system name values in the current
file. The exact current systems are pcengine / NEC PC Engine and pcenginecd /
NEC PC Engine CD.

Repo verification was independently recomputed by comparing the authoritative
PLATFORMS IDs with ES_DE_SYSTEM_MAP; the map is exact-keyed and fail-closed.
es_de_system_for_platform does not resolve aliases itself.

## 3. Full remaining-platform verification matrix

Status is a Batch-4 decision, not a claim about whether the hardware exists.
DEFER can include a raw-verified, clean candidate intentionally sequenced after
this bounded 12-platform batch. “Plausible alternatives” lists real upstream
targets that must not be silently substituted.

| Canonical EmuWiz id / name | Exact raw ES-DE name / fullname | Plausible alternatives / ambiguity | Status |
| --- | --- | --- | --- |
| 3DO / 3DO Interactive Multiplayer | 3do / 3DO Interactive Multiplayer | None found. | VERIFIED |
| Acorn Archimedes / Acorn Archimedes | archimedes / Acorn Archimedes | None found. | VERIFIED |
| Acorn Electron / Acorn Electron | electron / Acorn Electron | Separate bbcmicro exists; the registry also keeps Electron distinct from BBC Micro. | VERIFIED |
| Amstrad CPC / Amstrad CPC | amstradcpc / Amstrad CPC | None found. | VERIFIED |
| Apple II / Apple II | apple2 / Apple II | apple2gs is a separate upstream model and must not be inferred from EmuWiz’s broader Apple II ID. | VERIFIED |
| Atari 8-bit / Atari 8-bit | atari800 / Atari 800 | Raw XML also has atarixe; EmuWiz’s ID covers the broader 8-bit family, not only one model. | AMBIGUOUS |
| BBC Micro / BBC Micro | bbcmicro / Acorn Computers BBC Micro | Separate electron exists and must remain distinct. | VERIFIED |
| Commodore 128 / Commodore 128 | No dedicated c128 or Commodore 128 system found. | c64 has an x128 command, but is explicitly Commodore 64, so it is not a dedicated target. | NO TARGET FOUND |
| FM Towns / Fujitsu FM Towns | fmtowns / Fujitsu FM Towns | None found. | VERIFIED |
| Macintosh / Apple Macintosh | macintosh / Apple Macintosh | None found. | VERIFIED |
| NEC PC-8801 / NEC PC-8801 | pc88 / NEC PC-8800 Series | The series/product spelling is a model-family label, but it is the direct upstream PC-88 target. | VERIFIED |
| NEC PC-9801 / NEC PC-9801 | pc98 / NEC PC-9800 Series | PC-98 is a separate canonical EmuWiz ID and EQUIVALENT_PLATFORM_IDS declares the pair equivalent; both would target pc98. | AMBIGUOUS |
| NeoGeo64 / Neo Geo 64 | No dedicated neogeo64 name or Neo Geo 64 fullname found. | neogeo is SNK Neo Geo; it is not Neo Geo 64. | NO TARGET FOUND |
| Neo Geo CD / Neo Geo CD | neogeocd / SNK Neo Geo CD | neogeocdjp has the same fullname and a Japan-specific target. Generic neogeocd is the direct non-region-labelled candidate. | DEFER |
| NGage / Nokia N-Gage | ngage / Nokia N-Gage | Raw XML also has broader symbian; N-Gage has a dedicated target. | VERIFIED |
| PC / PC | pc / IBM PC | lutris declares pc, pcwindows; EmuWiz PC aliases include Windows and PC games, while pc is DOS/IBM-PC-oriented. | AMBIGUOUS |
| PC-98 / NEC PC-98 | pc98 / NEC PC-9800 Series | Same many-to-one issue as NEC PC-9801; do not add both to pc98 under the present unique-target invariant. | AMBIGUOUS |
| PC Engine / PC Engine / TurboGrafx-16 | pcengine / NEC PC Engine | tg16 is separately present for TurboGrafx-16. pce is not an XML system id. | VERIFIED |
| PC Engine CD / PC Engine CD / TurboGrafx-CD | pcenginecd / NEC PC Engine CD | tg-cd is separately present for TurboGrafx-CD. pcecd is not an XML system id. | VERIFIED |
| PC-FX / PC-FX | pcfx / NEC PC-FX | Separate from PC Engine CD / TurboGrafx-CD. | DEFER |
| Philips CD-i / Philips CD-i | cdimono1 / Philips CD-i | The target’s machine-style ID is exact upstream evidence; it is not Daphne or another laserdisc system. | DEFER |
| PS4 / Sony PlayStation 4 | ps4 / Sony PlayStation 4 | None found. Mapping only; no emulator/provider/legal work follows. | DEFER |
| Sharp X68000 / Sharp X68000 | x68000 / Sharp X68000 | Separate x1 exists; do not conflate them. | DEFER |
| Switch / Nintendo Switch | switch / Nintendo Switch | None found. Mapping only; no emulator/provider/legal work follows. | DEFER |
| TurboGrafx-16 / TurboGrafx-16 | tg16 / NEC TurboGrafx-16 | pcengine is the regional PC Engine target. Its distinct upstream target avoids a duplicate target, but batching it with the equivalent canonical ID needs deliberate regional semantics. | DEFER |
| VIC-20 / Commodore VIC-20 | vic20 / Commodore VIC-20 | None found. | DEFER |

## 4. Special-case findings

### PC Engine and PC Engine CD

| EmuWiz id | Verified ES-DE target | Verified fullname |
| --- | --- | --- |
| PC Engine | pcengine | NEC PC Engine |
| PC Engine CD | pcenginecd | NEC PC Engine CD |

Neither is pce/pcecd. Upstream gives cartridge and CD systems separate folders,
and the EmuWiz registry keeps them separate because their software is not
interchangeable. They are clean Batch 4 entries.

### TurboGrafx-16

ES-DE exposes a separately named regional system: tg16 / NEC TurboGrafx-16; it
also exposes tg-cd / NEC TurboGrafx-CD. The registry declares PC Engine and
TurboGrafx-16 equivalent hardware identifiers but preserves both stored
canonical IDs. Mapping the two cartridge IDs to their distinct regional upstream
folders would not violate the current unique-target invariant, but it makes a
regional publication choice visible. It is therefore deferred from this batch
rather than used to resolve MegaDrive’s unrelated regional policy.

### PC-98 and NEC PC-9801

Both canonical IDs are intentionally preserved in the registry, and
EQUIVALENT_PLATFORM_IDS relates them rather than silently rewriting stored data.
ES-DE offers one pc98 / NEC PC-9800 Series target. The current map’s test
invariant requires every ES-DE system target to be unique. Adding both would
violate it; selecting one would establish a many-to-one policy without a
dedicated preference/canonicalisation seam. Defer both.

### PC

EmuWiz’s PC is a broad folder-evidence identity with aliases including pcgames,
windows, and windowsgames. The raw pc target is specifically IBM PC and contains
DOSBox-era commands; lutris has a separate broad PC/Windows platform
declaration. Mapping PC to DOS, pc, or Lutris would make a frontend/policy
choice rather than preserve a demonstrated canonical meaning. Defer.

### NeoGeo64 and Neo Geo CD

No raw ES-DE Neo Geo 64 target was found; do not reuse arcade neogeo. Neo Geo CD
does have dedicated neogeocd and region-labelled neogeocdjp targets, distinct
from cartridge neogeo. The generic neogeocd candidate is verified but
intentionally sequenced after Batch 4 so its generic-versus-Japan handling is
explicitly reviewed in the next batch.

### Philips CD-i, PC-FX, FM Towns, NEC PC-8801, Sharp X68000, N-Gage,
Macintosh, Apple II, and Acorn/BBC

Each has a directly named raw system as recorded in the matrix. CD-i’s exact
system name is cdimono1; this is a dedicated Philips CD-i target, not a
laserdisc/Daphne substitution. PC-FX (pcfx) is distinct from PC Engine CD.
The Acorn family is also represented by distinct archimedes, electron, and
bbcmicro systems, matching the registry’s distinct canonical identities.

### MegaDrive

Remain outside Batch 4. The current map already records the megadrive selection
and the genuine competing upstream genesis / megadrivejp targets. This audit
found no smaller existing policy seam that would make that regional choice less
policy-bearing.

## 5. Selected Batch 4 platforms and exact verified IDs

Batch 4 is deliberately limited to 12 direct, one-to-one mappings with no new
preference, canonicalisation, provider, or launch policy:

| Canonical EmuWiz id | Exact ES-DE name | Exact ES-DE fullname |
| --- | --- | --- |
| 3DO | 3do | 3DO Interactive Multiplayer |
| Acorn Archimedes | archimedes | Acorn Archimedes |
| Acorn Electron | electron | Acorn Electron |
| Amstrad CPC | amstradcpc | Amstrad CPC |
| Apple II | apple2 | Apple II |
| BBC Micro | bbcmicro | Acorn Computers BBC Micro |
| FM Towns | fmtowns | Fujitsu FM Towns |
| Macintosh | macintosh | Apple Macintosh |
| NEC PC-8801 | pc88 | NEC PC-8800 Series |
| NGage | ngage | Nokia N-Gage |
| PC Engine | pcengine | NEC PC Engine |
| PC Engine CD | pcenginecd | NEC PC Engine CD |

Implementation should add only these rows through the existing ES_DE_SYSTEM_MAP
table. It must not add aliases to that table, alter platform detection, choose
an emulator, write ES-DE configuration, or change publication policy.

## 6. Deferred and ambiguous platforms

| Classification | Platforms | Required decision/evidence before mapping |
| --- | --- | --- |
| DEFER — verified, sequenced after bounded Batch 4 | Neo Geo CD; PC-FX; Philips CD-i; PS4; Sharp X68000; Switch; TurboGrafx-16; VIC-20 | Add a subsequent clean batch; for Neo Geo CD and TurboGrafx-16 explicitly preserve/record the regional folder choice. |
| AMBIGUOUS — target/policy | Atari 8-bit | Decide whether the broad canonical family belongs in atari800, atarixe, or needs a family policy. |
| AMBIGUOUS — many-to-one policy | PC-98; NEC PC-9801 | Define an explicit policy for two retained canonical IDs and the one pc98 target, or keep one unexportable. |
| AMBIGUOUS — canonical/frontend scope | PC | Define PC/DOS/Windows/frontend semantics; do not use dos, pc, or lutris as an accidental default. |
| NO TARGET FOUND | Commodore 128; NeoGeo64 | Obtain a real upstream dedicated target or add a separately reviewed frontend policy; do not reuse c64 or neogeo. |

## 7. Eventual implementation test plan

No tests are added in this research task. The eventual implementation should
extend the existing es_de_export.rs Batch 1/3-style coverage with a single
BATCH_4 fixture containing the 12 exact triples above, then exercise these
requirements:

1. **Canonical mapping:** For every Batch 4 ID, assert exactly one map row and
   exact name/fullname values; build a resolved identity with a usable path and
   assert a ready entry lands in that ES-DE system.
2. **Alias resolution:** Use one current registry alias per selected platform:
   threedo, archie, elk, cpc, apple2, bbcb, towns, mac, pc88, nokiangage, pce,
   pcecd. Assert platform_for_alias returns the expected canonical ID and the
   same row as canonical lookup. This guards against confusing EmuWiz aliases
   pce/pcecd with upstream system names.
3. **Distinct-neighbour protection:** Assert Acorn Electron != BBC Micro; Apple
   II != Macintosh; PC Engine != PC Engine CD; PC Engine != TurboGrafx-16; PC
   Engine CD != PC-FX; and NEC PC-8801 != PC-98 target behaviour. The latter
   peers remain unmapped until a later policy is approved.
4. **Unknown refusal:** Preserve exact fail-closed handling for unknown,
   conflicting, and unmapped inputs; include NeoGeo64, Commodore 128, and a
   nonsense ID as still-unmapped examples.
5. **Duplicate protection:** Retain/extend map-wide tests rejecting duplicate
   platform IDs and duplicate ES-DE target names. This prevents accidental
   two-ID-to-pc98 publication.
6. **Publication preview path:** For each selected mapping, construct an
   existing EsDeProfile system-data fixture matching its exact target and assert
   plan_es_de_gamelist_publication produces the correct gamelist destination
   without applying it. Include pcengine versus pcenginecd as a non-collision
   regression.
7. **Recovery/idempotency regression:** Reuse existing ES-DE publication fixtures
   for the PC Engine pair: apply an approved publication, replan to unchanged,
   rollback restores prior bytes, and unresolved/corrupt recovery records still
   refuse a new publication. This validates the existing transaction seam.

## 8. Expected parity counts after Batch 4

If and only if the 12 selected rows are added:

| State | Current | After Batch 4 |
| --- | ---: | ---: |
| COMPLETE | 49 | 61 |
| PARTIAL | 1 | 1 |
| MISSING | 26 | 14 |
| Canonical platforms | 76 | 76 |

The 14 post-batch missing platforms are the eight deliberately sequenced
verified candidates, four ambiguous-policy platforms, and two no-target-found
platforms listed above. MegaDrive remains partial and is not counted as newly
complete.
