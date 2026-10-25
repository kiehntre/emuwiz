# Game patch format audit and isolated IPS hardening

Audited base: `54d0503b8fa2eaa48f3cf5a0ca71fe935a126384` (2026-09-30).
After `git fetch origin`, HEAD, local main and origin/main were equal in
`/home/davedap/emuwiz-main-release-fix`. Tracked main was clean; unrelated
untracked research files and Python caches were left alone. The candidate is
on `feature/game-patch-format-hardening` in
`/home/davedap/emuwiz-game-patch-hardening`.

Active worktrees were inspected for overlap. None of Cheat Core Batches 1–6
changes `standalone_patch.rs` relative to its own merge base, and the inspected
cheat/GUI/patch worktrees had no uncommitted changes in the standalone patch,
output-recovery or package-composition modules. Historical patch, Saturn and
Dreamcast branches remain separate. No branch was merged or cherry-picked.

## Chosen target and scope

CHOSEN PATCH TARGET: prevent unintended IPS output truncation and correct IPS
output-size evidence.

Both literal and RLE hunks previously called `out.resize(hunk_end, 0)` even
when the end was inside an existing ROM. A one-byte edit could therefore
discard the rest of the derivative. A later low-offset hunk also discarded
earlier high-offset changes. IPS inspection reported the highest hunk end as
an exact target size, masking the truncation or rejecting other valid outputs.
Even `PATCHEOF` claimed an exact zero-byte output, despite containing no size
instruction. The optional three-byte EOF size field was accepted but ignored
by inspection, disagreeing with application.

The repair stays in the existing format implementation:

- Ordinary hunks grow the output only when necessary. Untouched bytes and
  previous hunks remain present; growth gaps are zero-filled.
- Overlapping records retain file order: the later write wins only over its
  own range. No sorting or normalization changes patch semantics.
- No EOF size field means unknown exact target size, rather than the largest
  write offset or zero. The existing explicit EOF-size extension is decoded
  into `target_size` and remains the sole request to resize the final output.
  Its existing zero-filled expansion behavior is retained as well as shrink.
- IPS framing, signature, byte and record ceilings are checked before copying
  the base or applying records, including direct internal preparation calls.

This fixes ordinary valid patches that can produce corrupted derivatives and
keeps the repair isolated from the cheat campaign. The source file was already
preserved; this repair also preserves the unmodified derivative content. The
existing parser, identity models and publication pipeline are reused.

Allowed files are `standalone_patch.rs`,
`standalone_patch/ips_tests.rs`, and this document. Cheat models, routing,
reconciliation, provenance APIs, launch adapters and GUI remain untouched.

## Format support on the audited base

The implementations are in `crates/archivefs-core/src/standalone_patch.rs`.
File extensions alone were not counted as support. There is no public patch
encoder/writer for any of the formats below; test-only fixture builders and
an external xdelta encoding command are not product writers.

| Format | Parser and preview | Apply | Source evidence enforced | Target validation | Bounds and fixtures |
|---|---|---|---|---|---|
| IPS | Signature, literal/RLE records, EOF and optional EOF size; typed inspection, compatibility and derivative plan | Built in; repaired here | No embedded checksum or source size. Reviewed base SHA-256 detects changes, but does not prove patch-to-game compatibility | No embedded target CRC. Published size/SHA-256 verified; explicit EOF size enforced | 24-bit offsets, 16-bit lengths, record cap and framing checks. Previous ordinary-output truncation fixed. Existing one-byte fixtures supplemented with independent literal expected bytes |
| IPS32 | No parser; `IPS32` is unknown/unsupported | No | None | None | No supported-format fixture or encoder |
| BPS | BPS1, checked biased varints, metadata, action count/length and patch CRC; inspection and plan | Built-in SourceRead, TargetRead, SourceCopy and overlapping TargetCopy | Embedded source size and CRC32 at plan/apply; reviewed SHA-256 freshness | Embedded target size/CRC32 and patch CRC32; publication SHA-256 | 10-step checked varints, metadata/record/output ceilings. Copy addresses are checked during apply, not fully during preview. Synthetic action, offset, malformed and copier-header fixtures |
| UPS | UPS1 sizes, XOR record framing and patch CRC; inspection and plan | Built-in forward XOR | Embedded source size/CRC32 and reviewed SHA-256 | Embedded target size/CRC32, patch CRC32 and publication SHA-256 | Varints/record count bounded; inspection does not enforce the declared-output ceiling. Apply caps target allocation. Synthetic UPS fixtures; reverse application absent |
| xdelta / VCDIFF / XDELTA3 | VCD magic, version and header-indicator checks only; not a full window decoder | Supervised external `xdelta3`; no independent XDELTA3 format or built-in encoder | No whole-source hash in the supported inspection model; external verified association/review needed, plus reviewed SHA-256 freshness | Decoder result and its checks when present, followed by computed SHA-256. No independent expected target hash in this inspection model | 60-second CPU/wall limits, 1 GiB process address space, bounded stdout; output file size/read bounds remain incomplete. Real xdelta3 synthetic round trip and malformed/header fixtures exist |
| PPF1 / PPF2 | Signature recognized as Unsupported | No | None enforced by a supported decoder | None | Signature/truncation fixtures only |
| PPF3 | Nominal PPF30 header inspection and derived plan, but current framing does not conform to the upstream PPF3 layout | Existing nominal record applier; cannot claim reliable standard PPF3 support | Reads a supposed size field from description/flag bytes; optional block-check payload is skipped, not compared | No embedded CRC enforced; generic publication SHA-256 only | Checked record offsets/lengths and synthetic fixture. Header positions, block checks, undo and file-ID handling need a dedicated repair with upstream-compatible fixtures |
| BSDIFF / BDF | No supported parser or preview | No | None | None | No decoder/decompression limits, writer or supported fixture |

CRC32 matching is format-level checksum evidence, not a cryptographic proof of
game identity. BPS/UPS mismatch paths refuse publication, but their current
error strings do not consistently display expected and actual values. There
is no filename-only exact association. Unsupported formats stay unsupported.

## Preservation, publication, provenance and undo

The audited standalone and composition paths do not open the original ROM for
writing. Built-in appliers copy source bytes, prepare a derivative, validate
available target size/checksum information and publish a new output. Plans
mark confirmation as required, reject source-equals-output, disable overwrite and
reject occupied destinations. The publisher uses new staging files and
no-clobber publication; source/patch hashes are checked for freshness.

`patch_output_recovery.rs` owns durable intent/checkpoint journals, temporary
and published output verification, explicit resume and rollback. Interrupted
operations can be rolled back by removing only owned, unchanged artifacts;
discovery itself does not mutate files. Successful standalone application
records base/patch/output SHA-256, paths, format, copier-header adjustment,
applier and time in a provenance sidecar. History does not offer a generic
completed-patch reverse operation. Deleting/regenerating a derivative leaves
the original available; this is distinct from a format-level inverse patch.

UPS reverse-source selection is not implemented. PPF undo bytes are skipped,
not used to restore an image. IPS has no intrinsic undo information. BPS and
xdelta have no reverse command in the EmuWiz API. Output bytes are deterministic
for the selected inputs and supported applier; timestamped sidecars, operation
IDs and journals are intentionally not byte-identical between publications.

## Packages and patch stacks

`patch_package_composition.rs` already represents an ordered chain of up to
32 patches. Multiple patches require a complete explicit order or metadata
order; filename order is not an inferred dependency order. Each patch is
re-inspected and checked against the immediately preceding derived source.
The base is copied into scratch, intermediate outputs stay in temporary
storage, and the last output is published as a new derivative. Package/base
digests detect stale inputs. Repeated composition can return AlreadyCreated
when output and provenance still agree.

Final composition provenance retains the base hash, package hash, ordered
patch paths/hashes/formats and final output hash. It does not retain every
intermediate source/output checksum as a permanent lineage graph: temporary
intermediates and their sidecars are removed. The composition sidecar replaces
the last standalone sidecar after publication; the whole chain does not have
one atomic recovery transaction. Disk-space estimation uses twice the base
size rather than a full expansion/chain estimate. These are deferred gaps,
not new stacking claims.

ZIP and folder packages feed the same standalone appliers. Package limits
declare 512 entries, 512 MiB expanded bytes and depth 16. Paths reject traversal,
absolute/drive paths, symlinks and special members. Archive members are
materialized to generated scratch names, not user-controlled destinations.
`archived_mod_package.rs` additionally performs list-first archive inspection;
ZIP patch bytes can be inspected in scratch, while external archive formats
are listed without pretending their patch payloads were decoded/applied.
Scripts remain inert. Limits based on declared member sizes do not substitute
for bounded decompression/read sinks.

## Platform-specific workflows

- RetroArch soft-patch sibling inventory/launch support covers IPS/BPS/UPS/
  xdelta; emulator-managed soft patching is separate from derivative creation.
  Inventory presence alone does not prove compatibility or format validity.
- The standalone SNES workflow permits an explicit 512-byte copier-header
  adjustment only with the platform rule and matching normalized source CRC.
  It records the adjustment and keeps the original headered source intact.
- HackHash provides a verified base/output-evidence workflow. It uses the same
  `prepare_standalone_patch_output` primitive, verifies provider output hashes,
  then hands publication to the existing transaction executor. It does not
  call the standalone publisher as a second writer.
- Saturn has disc manifests, typed target/readiness evidence and a comparator
  for already materialized cooked data tracks. Current main has no Saturn
  data-track rebuilder/applier. SSP semantics stay unknown; generic byte
  patches do not establish CUE/audio/System-ID preservation.
- Dreamcast DCP inspection is read-only. It bounds ZIP entry/path metadata and
  reports IP.BIN, topology and source-binding concerns; it neither extracts
  into a game nor rebuilds/applies a disc. A trusted exact target binding is
  needed before readiness. Standalone deltas are not DCP filesystem semantics.
- Dolphin, PCSX2, RPCS3, Xenia, Cemu and other runtime/emulator-native patch
  workflows live in patch_manager. They are not ROM delta encoders or generic
  optical-image patchers and were inspected only for the boundary, not changed.

## Remaining safety gaps

The constants are not all enforced before resource consumption: standalone
patch/base reads use `fs::read` before length checks, and the second patch read
is not bound to the exact bytes just inspected. Source hashing and recovery
also read complete files. xdelta staging can grow before the result-size
check, and its stage is read in full. Folder enumeration collects entries
before package-count checks; ZIP members use read_to_end after checking
declared sizes. These need bounded reads/decompression and snapshot handling
in a separate task. This IPS fix does not claim to solve those I/O races or
large-file resource guarantees.

The low-level apply API consumes a reviewed plan; enforcing the confirmation
flag is a caller responsibility. The generic standalone GUI only builds an
apply plan for a compatible checksum match. Verified provider/package binding
is separate from the reviewed source hash, which establishes freshness alone.

Further isolated candidates: standard PPF3 framing and source block-check
validation; BPS/UPS footer/operand and preview bounds; richer mismatch evidence;
UPS reverse mode; independently verified expected xdelta output; upstream
fixture coverage. IPS32/BSDIFF are unsupported and intentionally not added.
Broad lineage, recovery, shared identity/routing or provenance redesign should
wait until Cheat Core integration finishes. Existing cheat branches, GUI and
main are preserved; no push or promotion is part of this task.

## Primary implementation references

- [Flips IPS implementation](https://github.com/Alcaro/Flips/blob/master/libips.cpp): ordinary output retains source length unless hunks grow it or an explicit truncation limits it; record order is preserved.
- [Rom Patcher JS IPS implementation](https://github.com/marcrobledo/RomPatcher.js/blob/master/rom-patcher-js/modules/RomPatcher.format.ips.js): explicit EOF-size extension can expand as well as truncate, matching the existing EmuWiz extension behavior retained here.
- [Upstream MakePPF3 source preserved in MultiPatch](https://github.com/Sappharad/MultiPatch/blob/master/ppfdev/makeppf3_linux.c): method/50-byte description and one-byte image-type, block-check, undo and reserved fields; no image-size field at EmuWiz's assumed offsets.

References were consulted for behavior/framing; no external implementation
code or game material was copied into the candidate.

## Focused validation

The original focused Cargo filter passed 22 tests before the repair. A
temporary harness extracted the original IPS parser/applier and reproduced
seven failing regressions; all eight isolated parser/applier cases passed
after the repair. That harness is not a second production implementation.

Final `cargo test --offline -p archivefs-core --lib standalone_patch --
--nocapture` compiled the affected core library and passed 33 tests, including
all 11 new IPS tests and the existing real xdelta3 round trip. The filter also
includes one standalone-history projection test.

The next Cargo invocation waited behind another worktree's build-directory
lock. Only that queued command was interrupted. A copy of the just-completed
test binary was saved to `/tmp/emuwiz-game-patch-hardening-core-tests`, its
11 IPS test names were confirmed with `--list`, and the directly affected
filters below ran from that immutable binary with the core crate as cwd.
This avoided rebuilding/interfering with the concurrent integration lane.

| Focused filter | Result |
|---|---:|
| `standalone_patch` (Cargo) | 33 passed |
| `patch_package_composition` | 7 passed |
| `patch_output_recovery` | 5 passed |
| `hackhash_apply` | 3 passed |
| `archived_mod_package` | 3 passed |
| `saturn_patch_readiness` | 9 passed |

All 60 final focused tests passed with no failures or ignored tests. New
coverage includes literal/RLE hunks, out-of-order overlap, growth, EOF-size
shrink/zero/expansion through publication, maximum offset/RLE fields, exactly
the million-record cap and refusal above it, malformed headers/hunks/EOF and
every truncated fixture prefix, weak source evidence, stale source/patch
hashes, unchanged source/patch bytes, deterministic output, output size/hash,
provenance and no-overwrite repeat application.

`cargo fmt --all -- --check`, `git diff --check`, the task scope guard and GUI
root boundary passed. A separate targeted Cargo check was not needed: the
focused Cargo test build compiled the changed crate. No full workspace/GUI
suite, release build or live GUI smoke was run. Only the three allowed files
are committed on the feature branch; main is not modified, pushed or promoted.

## Independent promotion review

The promotion review started from candidate
`b4c9237cd9bf7185ce55df943ccb15dd5c21f178`. A fresh fetch confirmed that
main and origin/main initially still equalled the audited base.
All 60 original focused tests and a targeted archivefs-core check passed in
the new isolated target directory `/tmp/emuwiz-ips-promotion-target-y4pPme`.
During validation main advanced to
`08e32a0c0c18840fced3246679349a422c4e6d04`, adding two Game Details GUI
commits with no patch-file overlap. Both patch commits were rebased onto that
main without conflicts, preserving its GUI changes. The rebase did not change
any core source, dependencies or repository instructions from the validated
correction.

The review found one consumer regression that those tests did not cover:
Saturn readiness compared optional source and target sizes directly. Correctly
reporting an ordinary IPS target size as unknown made `None == None` true,
which could incorrectly claim unchanged track topology and allow readiness
for a patch that grows the image. Both topology comparisons now require a
known source size as well as equal sizes. This narrowly adds
`crates/archivefs-core/src/saturn_patch_readiness.rs` to the candidate scope;
no Saturn models, rebuilders, GUI or launch adapters change.

One new public-readiness regression covers logical data tracks, component BIN
targets and full raw-image targets. Its valid IPS fixture writes beyond a
2048-byte source. Even with exact source-manifest evidence, unknown format
sizes retain TrackTopologyImpactUnknown and NotReady. The test also checks
repeatable results and unchanged source/patch bytes.

A temporary harness copied the actual IPS parser, applier and new tail
regression without editing the repository. The candidate passed; reverting
either the literal or RLE grow-only guard made that same test fail. The harness
was removed, and no mutation changes were left behind.

Before rebasing, the corrected candidate passed all six focused Cargo filters:
standalone_patch 33, patch_package_composition 7, patch_output_recovery 5,
hackhash_apply 3, archived_mod_package 3 and saturn_patch_readiness 10 (61 total,
no failures or ignored tests). The targeted archivefs-core check, workspace
format check and diff check also passed. No full workspace/GUI suite or live
GUI smoke was run.

After rebasing, all six focused Cargo filters passed again with the same
61-test total, followed by a successful targeted archivefs-core check.
`cargo fmt --all -- --check` and both working-tree/candidate diff checks
passed on the rebased tree. Validation stayed in the same isolated target;
concurrent worktrees and their build directories were not used or changed.

Main then advanced again to `973228c9fb3fbfd1d3dfb4de1121159ebe2aa9ea`
with the Browse & Play GUI commit and its research evidence. A second rebase
also completed without conflicts. The core tree still matched the validated
build (`1b2e597617013ff5c562d86d9d0d8cab8581835d`), and comparison excluding
GUI/docs showed no source, dependency or configuration changes. All 61 tests
were rerun successfully from that freshly built isolated test executable,
then the targeted Cargo check passed again on the latest base. Formatting
and diff checks passed again. No shared-cache binary was used.
