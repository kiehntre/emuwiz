# PS2 PSU Interoperability Validation

Date: 2026-09-15

This is a read-only validation record. No production Rust or GUI files were
changed, and no real memory card or personal save was opened.

## 1. Executive Summary

EmuWiz-generated PSU output was accepted by the independent `pypsu` reader,
listed with the expected directory and file metadata, and extracted with
byte-for-byte payload equality for all synthetic files. An equivalent PSU
created by `pypsu` had identical exposed metadata and payloads; its only byte
difference was that it left the PS2 attributes word zero. The result is
**A. INTEROPERABILITY VERIFIED** for the validated single-level PSU contract.

The independent negative tests also show that `pypsu` is permissive: it does
not reliably reject truncation, altered padding, or an inflated file length.
That is a limitation of the validation tool, not evidence against valid
EmuWiz output; EmuWiz's own writer remains create-only and deterministic.

## 2. EmuWiz PSU Implementation Under Test

The implementation under test is the existing `Ps2PsuExportPlan` /
`apply_ps2_psu_export` path in `crates/archivefs-core/src/memory_card_inventory.rs`.
It consumes an inspected PS2 save directory, writes 512-byte entries, emits
payloads in inventory order, pads each logical file to a 1024-byte boundary,
stages the result, verifies its size and SHA-256, and preserves the source
card hash. The source is read-only during export.

## 3. Independent References

The primary behavioral reference was the public-domain
[`ps2dev/mymc` implementation](https://github.com/ps2dev/mymc), especially
[`ps2save.py`](https://github.com/ps2dev/mymc/blob/master/ps2save.py)'s
`load_ems`, `save_ems`, and `detect_file_type` paths. It defines the three
initial directory records, file-length reads, 1024-byte payload alignment,
and the structural no-magic PSU detection rule.

The independent runtime reader/writer was
[`McCaulay/pypsu`](https://github.com/McCaulay/pypsu), version 0.1.2, installed
only in `/tmp/emuwiz-psu-validation-venv`. Its `psu.PSU.load`, `list`,
`export`, and `save` paths were used.

Additional layout references were the
[PS2 Developer Wiki PSU description](https://www.psdevwiki.com/ps2/index.php?section=1&title=PSU),
the [PS2 Save Tools EMS PSU description](https://www.ps2savetools.com/documents/ps2-save-game-format-for-ems-adapter-psu/),
and the [PCSX2 memory-card filesystem reference](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/Reference/PS2-MemoryCardFileSystem.htm).

## 4. Validation Environment

- Host: `saltbox26`
- Worktree: `/home/davedap/emuwiz-main-release-fix`
- Starting HEAD: `7921a0e0c48b98a4282af30403ca63843174a03e`
- Temporary artifacts: `/tmp/emuwiz-psu-validation/`
- Independent tool: `pypsu 0.1.2` in `/tmp/emuwiz-psu-validation-venv`
- `mymc`: not installed; no system-wide installation was attempted
- PCSX2, `mymcplus`, and other card-import tools: unavailable

The generated source card was synthetic, 8,388,608 bytes, and was used only
through EmuWiz's existing inspection/export path.

## 5. Synthetic Fixtures

The backend generated these artifacts from synthetic card evidence:

| Fixture | Contents | PSU bytes | SHA-256 |
|---|---|---:|---|
| `simple.psu` | `icon.sys` (100 bytes) | 3,072 | `88799c046bfc8fe5872c6962590c9393da89a7be9336129b64ac215c6cb98b7f` |
| `multi.psu` | `icon.sys`, `fragmented.bin` (1,500), `zero.bin` (0), 31-byte-name file (1) | 7,680 | `815d1a6b6c8c652f67d96301d6750d1e8291e18a0f49f1af90c8396aa49a808e` |

The fragmented source chain was `[5, 7]`; its payload was reconstructed in
FAT order as 1,024 bytes of `0x5a` followed by 476 bytes of `0x6b`. The source
also populated validated timestamps, modes, and an attributes field in the
synthetic evidence.

Expected payload hashes:

- `icon.sys`: `7117f9a540bb641872d362d2cf67481fc022a6d63283b88593e87b38e374d384`
- `fragmented.bin`: `221d111790a77d7e850df91b70b88be99b850981aa9128939a5e41719cb227d1`
- `zero.bin`: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`
- 31-byte-name file: `cbe5cfdf7c2118a9c3d78ef1d684f3afa089201352886449a06a6511cfef74a7`

## 6. Container Layout Validation

The output was independently inspected at byte level. Each directory entry is
512 bytes and uses the expected little-endian fields:

| Offset | Size | Observed field |
|---:|---:|---|
| `0x00` | 2 | mode: `0x8427` directory / `0x8497` file |
| `0x04` | 4 | logical length |
| `0x08` | 8 | created timestamp raw bytes |
| `0x18` | 8 | modified timestamp raw bytes |
| `0x20` | 4 | attributes |
| `0x40` | 448 | NUL-terminated name field |

The stream order was root directory, `.`, `..`, then files. Root counts were
`3` for one file and `6` for four files, i.e. `2 + file_count`.

`pypsu` independently reported the same names, types, lengths, timestamps,
and modes. Its `Header` model stops at offset `0x20` and treats the remaining
32 bytes before the name as opaque padding, so its API does not expose the
PS2 attributes word independently. The EmuWiz attribute placement was still
verified directly against the documented/source layout.

## 7. File Payload Round Trip

`pypsu.PSU.load` accepted both files. Its `list` output reported every file,
and `PSU.export` reconstructed all payloads. Every output length and SHA-256
matched the synthetic source values exactly, including:

- 1,500-byte fragmented payload with 548 bytes of zero alignment padding;
- zero-length file with no fabricated payload;
- one-byte payload with 1,023 bytes of zero alignment padding;
- the 31-byte filename without truncation.

No spare/ECC bytes were present in PSU output; those belong only to the source
memory-card physical representation.

## 8. Timestamp / Mode / Attribute Validation

Both EmuWiz and `pypsu` decoded the timestamps as `2026-06-24 12:44:38` for
creation and `2026-06-24 12:44:39` for modification. The raw bytes were
`00 26 2c 0c 18 06 ea 07` and `00 27 2c 0c 18 06 ea 07`, respectively, matching
the six-field PS2 time structure used by the writer.

Modes were independently observed as `0x8427` for directories and `0x8497`
for regular files. `pypsu` does not surface the attributes word; EmuWiz wrote
the existing validated attributes at offset `0x20` without changing the
source evidence.

## 9. Padding / Alignment Validation

Payload starts and padding were independently measured:

- `icon.sys`: payload at offset 2,048, 100 bytes, 924 zero bytes;
- `fragmented.bin`: payload at offset 3,584, 1,500 bytes, 548 zero bytes;
- `zero.bin`: no payload or padding;
- one-byte file: 1 byte followed by 1,023 zero bytes.

Every next entry begins on the next 1024-byte logical payload boundary. The
final container sizes are therefore `4 * 512 + 1024 = 3,072` and
`7 * 512 + 1024 + 2048 + 0 + 1024 = 7,680`.

## 10. Independent Tool Results

`pypsu 0.1.2` accepted and listed both generated files, exposed the expected
metadata, and extracted all payloads successfully. It is a useful independent
behavioral reader, but not a strict corruption validator.

The source-level `mymc` contract independently agrees with the accepted
layout and payload/padding behavior. A runnable mymc validation was not
possible because only Python 3.12 was available and mymc's checked source is
Python-2-era code.

## 11. Cross-Writer Comparison

`pypsu` created an equivalent four-file PSU using the same directory name,
timestamps, modes, names, sizes, and payload bytes. Its output was:

- size: 7,680 bytes;
- SHA-256: `44bada2f24cb149f5127d7eacbb4cba4133ba67445b0d6afb2a21be12ddc8e64`;
- byte-for-byte equal to EmuWiz `multi.psu`: **NO**, because the seven
  attributes words at entry-relative offset `0x20` were `0x20` in EmuWiz and
  `0` in `pypsu`;
- all seven exposed entry tuples and all four extracted payloads: **equal**.

This is an allowed implementation variation: `pypsu`'s `Header` model does
not expose the PS2 attributes word and treats that region as opaque entry
padding. The difference is not a payload, length, name, timestamp, mode, or
alignment incompatibility.

## 12. Throwaway Memory Card Test

Not performed. No PCSX2/mymcplus card-import executable was available, and
the independent runtime reader did not provide card-image import. No real or
personal card was touched.

## 13. Negative Tests

Throwaway damaged copies were tested with `pypsu`:

- truncated header/entry: partially listed rather than strictly rejected;
- inflated file length: listed with the inflated declared size;
- altered padding: accepted;
- truncated payload: partially listed.

These outcomes demonstrate permissive behavior in `pypsu`'s parser. They do
not invalidate the positive interoperability result, and no production code
was changed in response. EmuWiz's own create-only writer validates the plan,
stages output, verifies length/hash, and never treats an external parser's
permissiveness as a safety guarantee.

## 14. Compatibility Decision

**A. INTEROPERABILITY VERIFIED**

For the tested contract—single-level PSU saves with 512-byte entries,
little-endian metadata, direct regular files, zero-length files, 1024-byte
zero padding, and deterministic ordering—EmuWiz output is accepted and
round-tripped by an independent runtime library. It is semantically equal to
an independent writer, with only the independently opaque attributes word
varying when populated by EmuWiz.

## 15. Any Proven Gaps

No proven EmuWiz PSU incompatibility was found.

Remaining validation limits are:

- no runnable mymc import/export binary;
- no disposable PCSX2/mymcplus card-image round trip;
- independent `pypsu` does not strictly reject malformed/truncated input;
- no non-UTF-8 filename fixture was used because the independent writer/API
  is UTF-8 oriented;
- nested-directory PSU behavior remains outside EmuWiz's writer contract.

## 16. Recommended Next Step

Keep the current PSU writer unchanged for the validated scope. A future
optional validation slice may add a disposable mymc/MyMC++ or PCSX2 card-image
round trip, but it should remain separate from production writer changes and
must use synthetic or explicitly throwaway card data only.
