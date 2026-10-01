# Canonical CUE/CHD preservation guard

Decision: **C. BUILD A SMALL RECONCILED CANDIDATE**.

Main was verified at `ab4fa810505830fc1dbc87de705f23707a11fa90` in
`/home/davedap/emuwiz-main-release-fix`, branch `main`, local `origin/main`
parity and tracked-clean. Neither authoritative main nor either candidate
worktree is edited. The new branch is `integration/cue-chd-canonical`.

Compared A `f6981c8dae45f86ac3cd558cf3a4da7e44b06f78` and B
`c07f07c64327e5d3441d38efaa61548e9c036b68`. A is based directly on this main.
B's parent predates the installer promotion; its complete code patch applies
cleanly to current main. Both patches passed apply checks in the isolated
worktree. The installer delta is disjoint and retained unchanged.

## Semantic and safety comparison

| Behavior | A | B | Canonical decision |
| --- | --- | --- | --- |
| Canonical layout | Existing `CueLayout` | Same | No parallel model |
| Single MODE1/2048, frame-zero INDEX 01 | Admitted | Equivalent | Retain |
| INDEX 00, synthetic/combined gaps, POSTGAP, additional indexes, mixed/multiple tracks/components | Refused | Equivalent | Retain; inspection model unchanged |
| Unsupported directives / FILE interpretations | Strict original-text whitelist | Equivalent | Retain except explicitly benign annotations below |
| Generic CD preview | Shares strict admission, retains layout | Old broad preview can still claim lossless without these facts | Retain A's shared planner gate |
| Reading authoritative facts | Admission then parser reopens CUE; fingerprint reparses | Admitted text feeds canonical parser; admitted layout feeds fingerprint | Use B's small parser/fingerprint extraction |
| Source full-file identity | Full SHA-256, inode, size, precise mtime | Equivalent authority | Preserve; not metadata-only freshness |
| Same-size/same-mtime source edits | Full digest detects them | Same, with explicit regression | Retain B's test |
| Source revalidation phases | Planning, before execution, after converter | Also after fingerprinting and before publication; re-resolves mappings | Use B |
| Alias rebound during conversion | Post-converter old canonical BIN/CUE hashes alone can miss it | Fresh canonical mapping is compared after tool returns | Use B and its deterministic tool-shim test |
| Exact CHT2 metadata | One track, mode, frames, zero gaps, no subchannels; exact raw tokens | Equivalent | Retain one verifier and common tests |
| Decoded payload | Independent canonical program-sector fingerprint | Equivalent | Required alongside metadata/storage proof |
| CHD physical storage | No hunk-map bounds proof | Geometry, standalone/parent, stored-hunk bounds and reference checks | Use B; truncated zero payload is a real non-overlapping safety fix |
| Output identity/replacement | Full staged identity brackets metadata/payload and binds repair transaction | Equivalent binding, also brackets storage check | Retain B's integration; shared transaction still checks identity |
| Staging/finalization | Unique staging, cleanup guard, journaled publication | Same | No new transaction system |
| Quarantine nested/aliased references | Refused | Same protection, more directly from admitted original reference | Use B; permit harmless `./` components |
| Quarantine replacement authority | Recaptures identity when constructing move proposals | Carries reviewed plan identities, so replacements cannot acquire authority | Use B and its direct stale-proposal regression |
| CUE self-reference | No explicit rejection | Explicit refusal even if CUE is sector-aligned | Retain B |

Neither candidate strictly supersedes the other: B materially strengthens
execution; A alone prevents unsupported generic CD previews from claiming
lossless conversion. There is no need to stack both implementations. The
canonical tree uses B's execution path and one shared `optical_preservation`
module, with A's small planner wiring. Duplicate private/shared helpers are
not retained. No new formats, GUI work, or Dreamcast/Saturn executor wiring.

## Supported syntax

| Syntax | A | B | Canonical |
| --- | --- | --- | --- |
| Simple BINARY / MODE1/2048 / INDEX 01 at zero | Accept | Accept | Accept |
| REM COMMENT | Refuse | Refuse | Accept |
| REM GENRE | Refuse | Refuse | Accept |
| Quoted TITLE | Refuse | Refuse | Accept |
| PREGAP, POSTGAP (including zero) | Refuse | Refuse | Refuse |
| INDEX 00 or INDEX 02 | Refuse | Refuse | Refuse |
| FLAGS, CATALOG, unknown REM directives | Refuse | Refuse | Refuse |
| Lowercase directives, tabs, CRLF | Accept | Accept | Accept |
| Multiple tracks, multiple BINs, mixed audio/data | Refuse conversion | Refuse conversion | Refuse conversion |

The annotation refusals are avoidable usability regressions in **both** input
candidates. Only COMMENT/GENRE REM annotations and well-formed quoted TITLE
are exempted. They do not change track/index/sector facts. The original CUE
remains byte-bound and retained (or explicitly quarantined intact); this does
not certify that CHD embeds CD-text. A changed annotation after preview is
still a stale source. All other unknown declarations fail closed.

## Test quality and reconciliation

B has 15 test functions not named in A; A has 3 not named in B, explaining the
net additional 12, rather than 12 wholly independent new guarantees. Common
candidate tests already cover gap distinctions, raw/unsupported modes, metadata
mismatches and genuine simple chdman output. B's unique coverage includes:

- Resolve/hash the admitted text/layout without reopening CUE.
- Unusual numbering and repeated FILE blocks; explicit CUE self-reference.
- Nested-reference quarantine refusal (overlaps A's quarantine tests).
- Complete versus explicit sparse hunks; truncated hunks whose decoded zeros
  still match; geometry and parent dependencies; recorded undecodable small CHD.
- Real compressed/uncompressed partial tracks and multiple hunks.
- Same-size/restored-mtime content edits, inode replacement and modified plans.
- Alias remapping before/after conversion; source mutation during converter run.
- Invalid output bytes or symlinks; quarantine proposals retain old authority.
- A separate unsupported-declaration-before-tool test, redundant with the
  common refusal helper; omitted from the canonical tree.

A's distinctive stale-layout matrix is retained: pregap/index changes, sector
mode, reordered tracks, component substitution/deletion, and changed plan facts
must refuse before converter/staging. B's nested-reference test is retained;
A's separate nested/alias tests are consolidated into one contract test covering
aliases and harmless dot prefixes. Every existing refusal-helper case now also
checks generic planner refusal, without duplicating its test cases.

New annotation tests check canonical layout equality and shared preview
admission, malformed/unrecognized directive refusal, actual chdman output
metadata/storage/payload verification, source retention, and stale annotation
refusal. Counts alone were not used to choose the implementation.

## Evidence limits

The existing decoder can return zeros for physically truncated uncompressed
hunks. B's synthetic regression proves that both the metadata verifier and
payload fingerprint used by A accept that fixture; the added map/bounds check
refuses it. Explicit sparse zero hunks remain supported.

Recorded chdman 0.264 evidence includes real compressed multi-hunk output and
uncompressed partial tracks, decoded payload comparison and exact metadata.
One valid compressed single-hunk recording cannot be read by chd-rs 0.3.4;
both old payload verification and the canonical contract refuse it. There is
no unsafe compatibility fallback, decoder replacement, or arbitrary round-trip
claim. Other converter versions must satisfy the same actual-output checks.

Source checks are phase-boundary freshness checks, not an atomic filesystem
snapshot. Publication and optional quarantine remain separate transactions;
a late source/quarantine failure can leave verified output published while
preserving the changed source. Broader audio/session/subchannel/index support,
optical duplicate-quarantine equivalence and disc-rebuild backends remain
separate work.

## Final validation

All Cargo validation used offline locked dependencies and isolated
`CARGO_TARGET_DIR=/tmp/emuwiz-cue-chd-review-target` (reusing its dependency
cache), two build jobs, disabled incremental compilation and zero dev/test
debug info. Execution fixtures were synthetic and temporary. No real media
collection was converted or patched.

| Check | Result |
| --- | --- |
| Full `archivefs-core --lib`, eight test threads | **10,084 passed, 0 failed, 3 existing ignored** |
| Shared preservation / executable conversion | 33 / 17 passed |
| CUE / disk format | 18 / 107 passed |
| CHD (`chd_`) / Redump filters | 174 / 126 passed |
| Optical fingerprint / raw optical | 7 / 29 passed |
| Conversion planner | 8 passed |
| Dreamcast / Saturn filters | 86 / 42 passed |
| `cargo check --offline --locked --workspace` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check`, scope and GUI boundary guards | Passed |

Focused filters overlap; do not sum them with the full-core result. The three
ignored tests are existing explicit performance/large-directory/manual-real-
Downloads tests. Five existing GUI unused/dead-code warnings remain. Evidence
logs are `/tmp/emuwiz-cue-canonical-*.log`; structured focused results are in
`/tmp/emuwiz-cue-canonical-focused.json`.

The final result is one current-main implementation, not two stacked commits.
Main modified: **NO**. Pushed: **NO**. Promotion is left to the user.
