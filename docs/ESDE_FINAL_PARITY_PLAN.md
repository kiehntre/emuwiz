# ES-DE final parity plan: Batch 5

## 1. Live parity state

Research point: e6ebe2297b01d2374cd40c220b72629efd2814f9 on
feature/archivefs-unified-platform.

The live platform registry has 76 canonical IDs. Comparing it with the
exact-keyed ES_DE_SYSTEM_MAP yields 61 direct COMPLETE rows and one deliberately
region-selected MegaDrive row. One compatibility table row is not a registry
platform and is excluded from parity.

| State | Count |
| --- | ---: |
| Canonical platforms | 76 |
| COMPLETE | 61 |
| PARTIAL | 1 (MegaDrive) |
| MISSING | 14 |

The exact live missing set is: Atari 8-bit, Commodore 128, NEC PC-9801, Neo Geo
CD, NeoGeo64, PC, PC-98, PC-FX, PS4, Philips CD-i, Sharp X68000, Switch,
TurboGrafx-16, and VIC-20. This confirms 61 / 1 / 14.

## 2. Upstream verification method

All target facts were read on 2026-09-05 from the current raw upstream Linux
definition, not from search summaries:

<https://gitlab.com/es-de/emulationstation-de/-/raw/master/resources/systems/linux/es_systems.xml>

For each candidate, the exact name was found and its enclosing system read to
verify the adjacent fullname and ROMPATH/name context. Absence conclusions
searched both likely IDs and product names. The exporter is exact-keyed and
fail-closed: aliases and EQUIVALENT_PLATFORM_IDS do not silently create ES-DE
mappings, and tests protect against duplicate ES-DE targets.

## 3. Final-gap evidence matrix

Raw context is the upstream line range read. Direct target evidence is not
permission to map where canonical semantics differ.

| Canonical platform | ES-DE name / fullname | Direct | Policy or alias conflict | Confidence / raw context | Classification |
| --- | --- | ---: | --- | --- | --- |
| Atari 8-bit | atari800 / Atari 800; also atarixe / Atari XE | Yes | Broad EmuWiz family vs two model-labelled folders | Exact targets, no safe choice; 273-280, 332-339 | POLICY_REQUIRED |
| Commodore 128 | No c128 or Commodore 128 system | No | c64 has a VICE x128 command but remains Commodore 64 | High absence evidence; c64 363-374 | NO_ESDE_TARGET |
| VIC-20 | vic20 / Commodore VIC-20 | Yes | None; distinct from C64/C128 | Exact; 2290-2297 | READY |
| NEC PC-9801 | pc98 / NEC PC-9800 Series | Yes | Equivalent retained PC-98 shares one target | Exact target, unsafe many-to-one; 1603-1610 | CANONICAL_CLEANUP_REQUIRED |
| NeoGeo64 | No neogeo64 or Neo Geo 64 system | No | neogeo is a distinct cartridge system | High absence evidence; neogeo 1442-1449 | NO_ESDE_TARGET |
| Neo Geo CD | neogeocd / SNK Neo Geo CD | Yes | Separate from cartridge neogeo; generic target beats regional neogeocdjp | Exact; 1452-1462, 1465-1475 | READY |
| Switch | switch / Nintendo Switch | Yes | None; mapping only | Exact; 2141-2148 | READY |
| PC | pc / IBM PC | Yes | EmuWiz aliases include Windows/PC games; upstream is IBM-PC/DOSBox-oriented | Exact target, insufficient semantic fit; 1577-1590 | DEFER |
| PC-98 | pc98 / NEC PC-9800 Series | Yes | Equivalent retained NEC PC-9801 shares one target | Exact target, unsafe many-to-one; 1603-1610 | CANONICAL_CLEANUP_REQUIRED |
| PC-FX | pcfx / NEC PC-FX | Yes | Separate from PC Engine CD | Exact; 1660-1667 | READY |
| Philips CD-i | cdimono1 / Philips CD-i | Yes | Machine-style ID; not Daphne/laserdisc | Exact; 377-385 | READY |
| PS4 | ps4 / Sony PlayStation 4 | Yes | None; mapping only | Exact; 1745-1755 | READY |
| Sharp X68000 | x68000 / Sharp X68000 | Yes | Separate from x1 | Exact; 2459-2467 | READY |
| TurboGrafx-16 | tg16 / NEC TurboGrafx-16 | Yes | Equivalent EmuWiz PC Engine has a separate regional ES-DE folder | Exact target, regional publication choice; 1628-1637, 2174-2187 | POLICY_REQUIRED |
| MegaDrive (PARTIAL) | genesis / Sega Genesis; megadrive / Sega Mega Drive; megadrivejp / Sega Mega Drive | Yes | One canonical identity, three regional folders | Exact; 934-948, 1181-1212 | POLICY_REQUIRED |

Final-gap totals including the MegaDrive partial are 7 READY, 3
POLICY_REQUIRED, 2 NO_ESDE_TARGET, 2 CANONICAL_CLEANUP_REQUIRED, and 1 DEFER.

## 4. Special-case conclusions

### Atari 8-bit

EmuWiz covers the broader 8-bit family while ES-DE exposes Atari 800 and Atari
XE folders. Choosing either is a frontend model preference, not a one-to-one
mapping. Keep it unexportable pending an explicit family-folder policy.

### Commodore 128 and NeoGeo64

Neither has a dedicated current target. Do not map C128 to c64 just because
that system offers VICE x128, and do not map NeoGeo64 to neogeo. Both would
collapse distinct libraries solely for coverage. They remain intentionally
unsupported unless upstream adds a target or a separately reviewed product
policy is approved.

### VIC-20 and Neo Geo CD

Both are clean Batch 5 entries. vic20 remains distinct from C64/C128.
neogeocd is distinct from cartridge neogeo; its Japan-specific sibling does not
make the generic, non-region-labelled target ambiguous for EmuWiz's
non-region-specific Neo Geo CD identity.

### NEC PC-9801 and PC-98

The registry retains both historical IDs and explicitly relates them as
equivalent so stored data is never rewritten. ES-DE offers exactly one pc98
target. Two table rows would violate duplicate-target safety; choosing only one
would leave an equivalent identity without policy. Resolve equivalence at the
canonical seam first, then establish at most one physical target policy.

### PC

EmuWiz PC means broad folder evidence and admits pcgames, windows, and
windowsgames. ES-DE pc means IBM PC and carries DOSBox-style commands. Do not
map it to DOS, PC-98, Windows, or a generic desktop category without defining
the canonical scope.

### PC-FX, Philips CD-i, PS4, Sharp X68000, Switch

These are direct isolated rows. pcfx is not PC Engine CD; cdimono1 is the actual
Philips CD-i target, not a laserdisc substitute; x68000 is not x1. PS4 and
Switch mapping authorizes no emulator, provider, legal, or download work.

### TurboGrafx-16

Upstream provides pcengine and tg16 as regional folders, though both carry PC
Engine platform metadata. The EmuWiz canonical IDs are equivalent.
TurboGrafx-16 to tg16 is not a duplicate target but is still a visible regional
publication decision, so it requires policy.

## 5. MegaDrive policy and recommendation

The blocker remains exact: ES-DE has genesis, megadrive, and megadrivejp;
EmuWiz has one MegaDrive identity. Current code chooses megadrive
deterministically but correctly records the regional choice as PARTIAL.

| Option | Assessment |
| --- | --- |
| A. One global default | Smallest, deterministic, preserves one existing folder. |
| B. Verified game region | More faithful but needs trusted region facts and mixed-collection rules. |
| C. Persisted frontend preference | Explicit but needs settings, migration, and republishing semantics. |
| D. Canonical split | Risks stored-data migration and identity ambiguity. |
| E. Leave PARTIAL | Safest absent a product decision, but never resolves the known choice. |

Recommendation: formally adopt megadrive as the global default. It matches the
existing mapping, preserves existing users and folder paths, and keeps
mixed-region collections deterministic. It must be documented as a frontend
default, never inferred from aliases. A later user preference needs a separate
migration/republication design.

## 6. Batch 5: READY mappings only

Batch 5 should add exactly these seven one-to-one rows through the existing
table seam. It must not alter aliases, detection, launch commands, profiles, or
publication mechanics.

| Canonical EmuWiz ID | ES-DE name | ES-DE fullname |
| --- | --- | --- |
| VIC-20 | vic20 | Commodore VIC-20 |
| Neo Geo CD | neogeocd | SNK Neo Geo CD |
| Switch | switch | Nintendo Switch |
| PC-FX | pcfx | NEC PC-FX |
| Philips CD-i | cdimono1 | Philips CD-i |
| PS4 | ps4 | Sony PlayStation 4 |
| Sharp X68000 | x68000 | Sharp X68000 |

## 7. Expected parity and unresolved work

| State | Current | After Batch 5 |
| --- | ---: | ---: |
| COMPLETE | 61 | 68 |
| PARTIAL (MegaDrive) | 1 | 1 |
| MISSING | 14 | 7 |
| Canonical platforms | 76 | 76 |

The post-Batch-5 missing rows are Atari 8-bit, Commodore 128, NEC PC-9801,
NeoGeo64, PC, PC-98, and TurboGrafx-16. Including MegaDrive, eight canonical
rows remain unresolved in some form.

76/76 is not realistically achievable without canonical/platform-policy work.
The safe immediate ceiling is 68 COMPLETE plus the existing partial. Even after
regional, PC-scope, and PC-98-equivalence decisions, upstream lacks dedicated
Commodore 128 and Neo Geo 64 targets; a theoretical 74/76 ceiling requires
those policy decisions and does not justify substituting unrelated systems for
the two no-target rows.

## 8. Eventual implementation tests

No tests are added by this research task. Eventual Batch 5 implementation
should extend the existing table-driven tests with:

1. Exact canonical platform/name/fullname triples and ready entry-plan
   assertions for all seven rows.
2. One registry-alias test per selected row where applicable, proving aliases
   resolve to canonical rows without becoming map keys.
3. Distinct-neighbour protection: neogeocd versus neogeo, pcfx versus
   pcenginecd, cdimono1 versus laserdisc/Daphne, and x68000 versus x1.
4. Map-wide duplicate-platform and duplicate-target safety, including no second
   neogeo or pcenginecd target.
5. Unknown refusal: C128, NeoGeo64, Atari 8-bit, PC, PC-98/NEC PC-9801, and
   TurboGrafx-16 remain PlatformUnmapped until their decisions land.
6. Publication preview fixtures for each target, proving the exact
   roms/system/gamelist.xml destination before any write.
7. Existing recovery/idempotency transaction tests for apply/replan/rollback
   and unresolved-recovery refusal.
8. Later regional-policy tests for deterministic MegaDrive/TurboGrafx/Atari
   selection, mixed-region collections, and no silent republication; PC-98
   tests must normalize equivalence before sharing a physical target.

## 9. Definition of Done

Batch 5 is done only when the seven exact rows above are added through the
current table, table/entry/preview/recovery tests pass, no duplicate target is
introduced, and all unresolved rows still fail closed.

Final parity is done only after separately approved MegaDrive, Atari 8-bit,
TurboGrafx-16, PC-scope, and PC-98-equivalence policy; and only if upstream
adds dedicated targets or an explicit product policy is approved for Commodore
128 and NeoGeo64. A numerically complete table is never permission to merge
distinct libraries.
