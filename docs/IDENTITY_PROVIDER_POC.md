# MAME / ScummVM provider proof of concept

Base: 429e5148. Worktree: emuwiz-identity-providers-poc. No production
catalogue migration or GUI service replacement is part of this task.

## Architecture audit (before implementation)

* `identity_source::model::IdentityProvider` already exists (currently RomM).
  Extend it with the two closed provider identities; do not invent a marketplace.
* `identity_source::mame_listxml` imports through the bounded shared parser;
  `ParsedDat` already preserves machine names, descriptions, clone/ROM parents,
  manufacturer/year, ROM size/CRC/SHA, BIOS/device declarations and raw metadata.
  The parser/importer currently drops the root build version: close this gap.
* Existing `DatIndex`, `audit_files`, archive hash readers and set/dependency
  assessments remain the authority for MAME byte/set evidence. CRC alone remains
  probable, not exact. A matched member is not proof of a complete runnable set.
* `scummvm_detection` already invokes an isolated local detector. Its launch-oriented
  deduplication deliberately merges variants of the same engine/game ID. Do not
  alter launch semantics or reuse that deduplication for provider variant evidence.
* Installed `/usr/games/scummvm` 2.8.0 exposes its official detection entries with
  `--dump-all-detection-entries`. It includes engine, game, language, platform,
  extra/variant, filenames, sizes and `md5-N` prefix signatures. These are NOT
  ordinary whole-file ROM hashes. Retain native records and unsupported signature
  forms explicitly. The local detector remains authoritative where dump semantics
  cannot establish a supported exact match. Unknown/fallback detection is not exact.
* Existing evidence-lineage types provide claim strength, provenance, representation
  and identity scope. Reuse these for discovery rather than inventing numeric certainty.
  Existing DAT verdicts provide exact/probable/ambiguous/no-match semantics; snapshot
  freshness is an additional presentation concern, not permission to promote evidence.
* Managed DAT snapshots already use explicit check/preview/apply and preserve previous
  state. Their descriptors are specifically DAT-based and cannot describe ScummVM
  native rules without lying. Provider snapshot storage will extend the existing
  catalogue database, not create a second database or replace the DAT registry.
  Store immutable normalized payloads with SHA-256, version, source/executable identity,
  import time and parser version. Compare the reviewed current snapshot at update time;
  retain prior snapshots for explicit rollback. No background refresh or verification cache.
* DATs & Verification and selected-game evidence are existing GUI integration points.
  A focused provider controller owns workers/state/rendering. GUI root gets module
  wiring only, not feature logic. Existing imports, matching and launch routes stay intact.

## Boundaries

Two local official-tool providers only. No downloads, third-party mirrors, source
media writes or automatic update. Bounded checks explicitly selected by the user.
MAME arcade first; no software-list expansion. ScummVM native signatures remain
separate from EmuWiz filename/header discovery, which cannot create official matches.
Conflicting discovery clues stay ambiguous; no useful clues stays unknown.
Verification against an older snapshot is labelled Needs re-check after an update.
Real QA uses scratch catalogue state, never the production database.

## Official references

* https://docs.mamedev.org/commandline/commandline-all.html (`-listxml`)
* https://docs.scummvm.org/en/latest/advanced_topics/command_line.html (`--detect`)
* https://github.com/scummvm/scummvm/blob/master/base/commandLine.cpp
* https://github.com/scummvm/scummvm/blob/master/engines/advancedDetector.cpp

Local executable help/output is the source of truth for installed-version capabilities;
current upstream source is reference, not proof that older dump formats preserve all flags.

## Real-library and RomM addendum

The deployed RomM audit is useful as an enrichment comparison, not as an identity
model. RomM does not run ScummVM detection, parse `.scummvm` launcher contents,
read `scummvm.ini`, or consult ScummVM detection tables. Its special case is an
exact filename/ID lookup against a bundled 428-entry ID-to-title table, followed
by ordinary metadata-provider search. Human-title filenames in the 185-game
library therefore usually bypass that table. Observed examples matched through
metadata search include Chewy, Blackwell Convergence, Clandestiny, and A Golden
Wake; Detective Gallo was unmatched. These results demonstrate useful metadata
coverage only. They do not establish ScummVM identity.

EmuWiz preserves the stronger boundary: a fuzzy or remote result can supply
presentation metadata and provenance, but its identity contribution is always
`NONE`. No ScreenScraper, IGDB, MobyGames, LaunchBox, or other remote provider is
implemented here.

## ScummVM result classes

The provider keeps these classes separate:

* `OfficialExact`: the pinned ScummVM runtime ID agrees with complete supported
  signature evidence from the pinned provider snapshot.
* `OfficialFallback`: the runtime reports an official ID, but exact exported
  signatures are not established. This includes fallback/fuzzy detector paths.
* `OfficialDetectionCoverageGap`: the runtime reports an ID that the exported
  snapshot does not cover. This is a source-coverage limitation, not a corrupt
  game and not proof of unsupported content.
* `EmuWizDerivedProbable`: local filename/resource evidence suggests an engine,
  without an official exact match.
* `Unknown`: no trustworthy classification.

The provider never promotes a printed game ID by itself. Absence of an “unknown
variant” warning is not exact evidence. A configured target ID from a `.scummvm`
file is stored separately from the official game ID and is local launcher
evidence only. A possible base ID derived from a target suffix is labelled a
heuristic and is never used as identity.

The normalized detection evidence retains engine, game ID, title, platform,
language, variant, flags/extra fields, filenames, declared sizes, signature
algorithm, signature byte-range key such as `md5-1048576`, signature value,
provider executable SHA-256, provider version, source snapshot hash, and native
detector output. Partial-file signatures remain partial-file signatures; they
are never represented as whole-file MD5s.

## Generated ID/title index

`id_title_index` generates a convenience mapping from the pinned official
ScummVM detection dump. It canonicalizes the engine-qualified ID as lowercase,
retains the original engine/game components, source version, platform, language,
and variant, and keeps coverage limited to the snapshot. It does not ship RomM's
428-entry fixture and does not infer suffix conversions such as
`monkey2-floppy -> monkey2`.

## Local title fallback and real-library QA

Presentation title fallback is: official detected title, official ID/title index,
cleaned local filename/folder title, then raw source name. Metadata failure never
removes a usable local title. The POC must exercise human-title cases, including:

| Local example | Official detection result | Class to report | Cleaned title / ID index | Enrichment |
|---|---|---|---|---|
| `Day Of The Tentacle (CD Dos)` | run detector and retain output | exact/fallback/coverage gap/unknown | local fallback if no official title | useful only if official identity is absent or incomplete |
| `Chewy - Esc from F5 (CD - DOS)` | run detector and retain output | same | local fallback | likely useful; observed ScreenScraper match is metadata only |
| `Blackwell Convergence (Windows)` | run detector and retain output | same | local fallback | likely useful; observed ScreenScraper match is metadata only |
| `Clandestiny (CD - Windows)` | run detector and retain output | same | local fallback | likely useful; observed IGDB/ScreenScraper matches are metadata only |
| `A Golden Wake (Windows)` | run detector and retain output | same | local fallback | likely useful; observed ScreenScraper match is metadata only |
| `Detective Gallo (Windows)` | run detector and retain output | same | local fallback | useful to attempt, but observed unmatched |

The POC report for each case must include runtime output, class, cleaned title,
ID/title index result, whether enrichment would help, and preserved evidence.
No source-game rename or filename normalization is performed.

## Engine coverage matrix

| Engine state | Official export available? | Runtime detect available? | Static detection table available? | Exact signatures accessible? | Fallback present? | Current POC support | Limitation |
|---|---|---|---|---|---|---|---|
| SCUMM | not guaranteed; current export path may be disabled | yes | yes in source | yes, often partial byte ranges | yes | bounded exported rows plus runtime confirmation | export coverage cannot stand for all native detector rules |
| Advanced Detector engines | varies by build/engine | yes where runtime supports it | engine-specific | varies; filenames, MD5 ranges, sizes, flags | yes | records only where dump exposes them | native rules can exceed dump schema |
| AGS/Wintermute/fan engines | varies | runtime-dependent | source tables may exist | engine-specific | possible | discovery fallback only when no exact table record | absence from export is a coverage gap, not unsupported |
| unknown/unofficial | no | may be no | no | no official signature | no | EmuWiz probable/unknown evidence | requires local/project/user metadata |

The provider therefore does not claim “ScummVM complete” from a successful
snapshot capture. It reports the snapshot's engine coverage and preserves the
runtime detector output separately.

## RomM fixture comparison boundary

The 428-entry RomM table is comparison/test material only. This branch does not
copy or ship it. A future comparison should report official-index overlap,
RomM-missing IDs, aliases/case differences, and engine discrepancies after
canonicalizing engine-qualified IDs. Any discrepancy affects enrichment or
coverage reporting; it cannot demote official detection truth.

The enforced pipeline is:

```text
local files
  -> official ScummVM detection
  -> identity, variant, and evidence
  -> official generated ID/title index
  -> cleaned local title fallback
  -> optional metadata enrichment
  -> title/year/description/artwork references
```

The lower layers never rewrite the identity result above them.
