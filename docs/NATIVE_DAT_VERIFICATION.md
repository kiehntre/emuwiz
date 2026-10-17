# Native Blue DAT verification

DAT Sources → **Verify library games…** → choose the system → choose an
installed catalogue → all library roots or a saved collection → **Preview
verification** → inspect evidence/scope → **Confirm verification** → View games
→ **Reload game list**. No external per-platform audit program is required.

The earlier engine already hashed content, inspected ZIP/7z members and supported
explicit library targets. The simple GUI did not expose the bounded preview or
confirmation, ignored unbound imported catalogue headers, used a folder fallback
when library scope was unavailable, and bundled verification with platform
enrichment. The CLI's old `dat audit` still compares supplied filename evidence;
it is not the content-verification entrypoint described here.

The new focused controller resolves the existing catalogue reference, freezes
its bytes in a private temporary directory, parses that snapshot, and requires
strong parsed platform agreement. An explicit incompatible source assignment is
refused. Preview reads library membership and file metadata, not ROM content.
It shows exact eligible paths, physical bytes and exclusions. Unknown systems,
other systems, duplicate paths, missing/unreadable files, symlinks, special files,
ancillary files and formats not represented in the selected catalogue are excluded.

Confirmation binds system, catalogue reference/source configuration, collection,
active database, exact DAT SHA-256 and file identities. The worker uses the existing
`run_dat_audit_with_targets` engine with an explicit `Catalogue` population and no
hash cache. There is no folder walk or platform-discovery step. Before saving it
rechecks DAT bytes and file identities; database membership is revalidated in the
same SQLite transaction as identity writes. New rows never silently join a run.
The existing partial-result protection preserves prior evidence. Historical facts
are not rewritten. New native evidence adds an optional catalogue SHA-256 inside
existing JSON facts; no schema migration is needed.

## Evidence and supported representations

Any canonical platform whose single-file installed DAT has strong compatible
platform evidence and ordinary ROM MD5/SHA-1 hashes can use this flow. This includes
No-Intro Game Boy (`.gb`) and Game Boy Color (`.gbc`), plus TOSEC ZX Spectrum
catalogues for TAP, TZX, Z80, SNA, TRD and SCL. The installed Spectrum inventory also
contains separate disk/media catalogues. Representation support comes from the
selected catalogue's actual ROM names and hashes, not a platform-wide whitelist.

A TAP catalogue hashes serialized TAP bytes; it does not normalize TZX tapes or
snapshots into TAP. A corresponding TZX/Z80/SNA/TRD/SCL catalogue is required for
those bytes. ZIP and 7z use the existing bounded decoded-member audit. Different
member representations cannot authorize a negative. Ambiguous or partial set-member
evidence also cannot authorize a definitive negative for the parent archive. A single exact member may
identify its parent only under the existing complete-pass, stable-container,
set and provenance rules. Exact outer-file catalogue matches remain valid. This
is content identity, not interpreted tape equivalence, complete preservation sets,
media validity or launch readiness.

When relevant catalogue entries lack comparable hashes, negatives for that format
are withheld even if other entries support exact positives. Probable or filename-only
evidence remains unverified in completion totals; existing detail views can describe
that weaker evidence more specifically.

CRC-only/name-only and SHA-256-only loose catalogues cannot provide definitive
verification through the current loose-file hashing engine and are refused.
Disk-only catalogues, aggregate DAT folders, unknown/ambiguous platform headers,
unsupported containers, encrypted/nested/unsupported-codec or incomplete archive
content remain unavailable/Not checked. Arcade retains the existing set-oriented
workflow rather than being redirected into this ROM flow. No new archive parser,
media conversion, DAT matching rules or CLI audit engine was introduced.

## Results and operation

Workers perform resolution, filesystem inspection, hashing and saving off the GUI
thread. Progress reports actual stages/files; no invented percentage or ETA is
shown. Hashing can be cancelled before saving. An already committing SQLite save
finishes atomically. Fatal validation/save errors are shown as failures; unchecked
content is a partial completion. Totals count selected library games once; archive
members do not inflate game totals. Errors count distinct failed paths.

Browse & Play prioritizes loaded DAT outcomes over older scan identification.
Game Details shows a separate DAT row directly from loaded evidence, even before
Review Identity has been opened. No usable evidence displays Not checked. Reload
game list refreshes these existing projections without restarting EmuWiz.

Synthetic fixtures/disposable databases cover scope, exact/negative/member
matches, corrupt and partial archives, missing files, stale confirmations, source
binding, cancellation, preservation, unrelated writes and native GUI controls.
Development does not perform any production audit or modify production ROMs.
