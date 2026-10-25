# Dreamcast IP.BIN inspection and safe patch planning

Backend candidate based on main `fc8bda7be687e626d708a4633f73840bcbf15c32`.
No game image, retail bootstrap, or another project's implementation is bundled.
Tests construct synthetic metadata, protection text, and opaque payload bytes.

## Verified format and references

The fixed metadata layout was checked before implementation against
[Marcus Comstedt's IP0000.BIN specification](https://mc.pp.se/dc/ip0000.bin.html)
and the [KallistiOS makeip documentation](https://github.com/KallistiOS/KallistiOS/blob/master/utils/makeip/README.md).
[Comstedt's IP.BIN/bootstrap documentation](https://mc.pp.se/dc/ip.bin.html)
defines the complete boot area and its additional region enforcement.
[The IP.BIN Patcher README](https://github.com/DerekPascarella/Dreamcast-IP.BIN-Patcher)
corroborates the protection-text offset and width. Only documentation was used;
its implementation was not copied or adapted.
[Dreamcast Wiki](https://dreamcast.wiki/IP.BIN) provides corroborating header
offsets, but its overview has inconsistent size arithmetic. The inclusive
offset ranges in the primary documentation take precedence.

IP.BIN occupies sixteen 2048-byte sectors, exactly 32768 bytes. It is the
reserved boot area of the selected data track, rather than necessarily an
ISO9660 file. IP0000.BIN is a related metadata structure in the low-density
area; a standalone 256-byte metadata sample is **truncated for complete IP.BIN
tooling**, even though the existing metadata-only evidence API can inspect it.

| Byte range (end exclusive) | Width | Field and interpretation |
|---|---:|---|
| `0x0000..0x0010` | 16 | Hardware ID: exact `SEGA SEGAKATANA `, including one trailing space. |
| `0x0010..0x0020` | 16 | Maker ID: exact `SEGA ENTERPRISES`. This is distinct from the software company. |
| `0x0020..0x0030` | 16 | Device information: four ASCII hex CRC digits, space, six-byte media identifier, positive disc number/count, right padding. |
| `0x0030..0x0038` | 8 | Positional area symbols: `J`, `U`, `E` in slots 0, 1, 2; space disables a slot; remaining slots unassigned. |
| `0x0038..0x0040` | 8 | Peripherals: seven ASCII hex digits (28 bits), then a space. |
| `0x0040..0x004A` | 10 | Product number, e.g. a compact `T-...` identifier; exact padding participates in CRC. |
| `0x004A..0x0050` | 6 | Product version, `Vx.yyy`. |
| `0x0050..0x0060` | 16 | Release date, eight `YYYYMMDD` digits and padding; Gregorian calendar validated. |
| `0x0060..0x0070` | 16 | Boot filename; a bounded, printable, safe root filename. `1ST_READ.BIN` is conventional. |
| `0x0070..0x0080` | 16 | Software company/maker name. |
| `0x0080..0x0100` | 128 | Software title. |
| `0x0100..0x0300` | 512 | TOC/opaque bytes; preserved, not interpreted or repaired. |
| `0x0300..0x3700` | 13312 | Licence-screen code; preserved. No retail reference bytes bundled or verified. |
| `0x3700..0x3800` | 256 | Additional area-protection slots; existing text checked for newly enabled regions. |
| `0x3800..0x6000` | 10240 | Bootstrap 1; opaque, preserved. |
| `0x6000..0x8000` | 8192 | Bootstrap 2; opaque, preserved. |

The hardware signature is exactly the 16 bytes `SEGA SEGAKATANA `: one trailing
space. Maker ID is 16 bytes without trailing padding. ASCII and space padding
are documented for the metadata text fields. No cited specification permits
NUL padding in standard IP.BIN metadata. Observed trailing NULs are reported as
nonstandard and retained; embedded NUL/control bytes are malformed. Replacements
must be printable ASCII, fit the exact byte width, and use space padding.
Non-ASCII text, control characters and overlong replacements are refused;
nothing is silently truncated or encoded to expand a field.

## Discrepancies and conservative decisions

- Main's prefix evidence API also recognises `SEGA SEGAMARIO`. The verified
  standard layout only specifies SEGAKATANA. Complete tooling exposes MARIO and
  other Sega variants as unsupported and refuses edits; prefix identity policy
  remains unchanged.
- Some headers/repository fixtures use eight hexadecimal peripheral characters.
  The documentation specifies seven plus padding. Eight digits are inspected
  with warnings, with all high/reserved bits preserved; peripheral edits on that
  encoding are refused.
- Main's inspector previously assigned VGA to bit 1 and called bits above 20
  unknown. Both primary diagrams assign VGA to bit 4, with controller/optional
  peripheral declarations through bit 27. The shared prefix decoder is corrected.
- `A` and `K` are not documented region assignments. They and other alphabetic
  symbols remain typed unknown data, including their original slot and byte.
  A known letter in the wrong slot is also unknown. Nonalphabetic nonspace region
  bytes are malformed; they are still preserved for inspection.
- makeip describes region selection through header flags, while the bootstrap
  documentation also requires region text. This is consistent for makeip's
  template, which already contains every region's text. EmuWiz checks the second
  requirement and refuses newly enabling a region if its text is absent.

## Region and peripheral models

Regions retain eight typed slots: blank, known Japan/USA+Canada/Europe, or unknown
byte. An edit toggles one documented slot; it cannot overwrite an unknown symbol
or discard the five unassigned slots. Protection texts are 28-byte fields at
`0x3704`, `0x3724`, `0x3744`, after each slot's four-byte instruction prefix.
No protection text, branch instruction, or other bootstrap byte is edited.
Declarations do not prove television mode or runtime compatibility.

| Bit(s), least significant first | Documented declaration |
|---|---|
| 0 | Windows CE |
| 4 | VGA |
| 8, 9, 10, 11 | Other expansions, Puru Puru pack, microphone, memory card |
| 12 | Minimum Start/A/B/directions controller requirement |
| 13, 14, 15, 16, 17 | Minimum C/D/X/Y/Z button requirements |
| 18 | Minimum expanded directions requirement |
| 19, 20 | Minimum analog R/L trigger requirements |
| 21, 22 | Minimum horizontal/vertical analog requirements |
| 23, 24 | Minimum expanded horizontal/vertical analog requirements |
| 25, 26, 27 | Optional gun, keyboard, mouse |

Known mask is `0x0FFFFF11`. Bits 1–3 and 5–7 are reserved; any bits outside
the documented mask remain unknown. A malformed hexadecimal field has no decoded
capabilities. Edits toggle only explicitly selected documented bits and retain
reserved bits. These are header declarations, not tested hardware support.

## Integrity findings

The device field's first four ASCII hex digits encode a CRC on exactly the raw
16 bytes `0x40..0x50` (product number plus version, including padding).
The documented algorithm is CRC-16/IBM-3740, also called CCITT-FALSE:
polynomial `0x1021`, initial remainder `0xFFFF`, non-reflected input/output,
final XOR zero, 16-bit remainder. Storage is ASCII hexadecimal rather than a
binary integer with byte order. EmuWiz writes four uppercase digits.
Independent synthetic vectors: empty input → `FFFF`; `123456789` → `29B1`;
`T-1234M   V1.000` → `D937` (also checked using Python's `binascii.crc_hqx`).

Comstedt explicitly documents that a zero CRC placeholder can boot. Thus a
mismatch/placeholder is suspicious evidence, not proof that the bootstrap is
broken. Inspection reports matched, placeholder-zero, mismatch, or malformed.
Product/version byte edits recalculate the dependent CRC and expose that change
in preview. An explicit CRC-recalculation edit can repair a parseable mismatch.
Other edits and no-op previews leave a mismatching CRC unchanged.

There is no proven whole-header or whole-bootstrap CRC in these references.
The console's separate licence-code byte comparison is not a CRC repair API.
VMS save headers and system flash use related algorithms with different seeds
or inversion; their checksums are not applied to IP.BIN. EmuWiz verifies format,
the product CRC, requested fields, full output SHA-256 and byte preservation; it
does not certify licence code, executable scrambling, physical bootability, or
disc TOC integrity. The metadata-only legacy API retains its checksum-not-proven
status; the complete inspector supplies the explicit product-CRC finding.

## Preview, apply and preservation policy

Public backend entry point is `dreamcast_boot_evidence::ip_bin`. It reuses
`DreamcastIpBinInspection` and its individual raw-byte fields, preserving the
complete original buffer for rendering. Status distinguishes valid, truncated,
malformed, unsupported variant and suspicious-but-parseable. File inspection
uses the existing safe-read policy and reads at most 32769 bytes; oversized
inputs have no whole-file hash or editable plan. Extensions never establish
validity. Image mounting, extraction and whole-disc buffering are absent.

`preview_ip_bin_edits` is pure over a read-only file snapshot. Rows expose the
original/proposed value and raw bytes, exact affected range, validation result,
and whether the row changes integrity bytes. The immutable preview exposes
source path, file object identity, length, mtime, SHA-256, original parsed fields,
resulting inspection and expected complete output SHA-256. Refused edits return
an explicit validation error without writing. No edits, or an explicit semantic
no-op, render the original bytes exactly, including unusual padding.

Supported explicit edits: product number/version, valid release date, safe boot
filename, company, title, positional region toggles, documented peripheral
toggles, and product-CRC recalculation. Hardware/maker/device identifiers,
reserved symbols/bits, TOC, protection-text insertion, licence code, bootstrap,
logos, scrambling and arbitrary byte replacement are intentionally unsupported.
Malformed, incomplete or unsupported source variants cannot be edited.

Loose-file review binds the preview to an absent destination and its existing
parent directory. Apply rechecks source identity/size/SHA-256/parsed original,
stages a new copy in that parent, reparses and verifies exact expected fields
and bytes, rechecks source/parent, then uses the existing atomic no-replace
rename primitive and verifies the published output. Source is never opened for
writing. This small file path uses native temporary staging; durable tree
receipts are retained for extracted-tree work. A changed source returns
`STALE PLAN`; identical bytes under a replacement inode are also refused.
The existing pathname check/use limitations apply, as in DCP transactions.

## Relationship to DCP and future GUI projection

`dreamcast_dcp_apply::inspect_extracted_dreamcast_ip_bin` uses the existing
`bootsector/IP.BIN` convention. It lists root names without executing anything:
present, missing, case mismatch, ambiguity, unsafe member or not checked.
Missing targets never cause automatic header/file changes. A requested boot
filename edit in a tree requires an exact unambiguous regular target.

`review_extracted_dreamcast_ip_bin` extends the current DCP module and reuses
`optical_patch_tree::Contents` and `patch_output_recovery::tree::TreePatchPlan`.
It copies the complete reviewed tree, changes only IP.BIN, verifies every
member and reparses IP.BIN. `prepare` returns the same immutable tree receipt;
existing `publish`, `inspect` and `undo` handle no-clobber publication and
recovery. All source tree dependencies remain bound through publication.
No second image patcher, rebuild writer, migration or identity authority is added.
Existing DCP package replacement behavior remains unchanged.

A future GUI can project the typed inspection, raw/unknown slots and bits,
per-field edit refusals, checksum findings, boot-target status, changed byte
ranges, source evidence and expected hash. It should distinguish declarations
from tested compatibility, and invoke apply only with the reviewed immutable
preview. This candidate contains no GUI or CLI production changes.

## Validation

Synthetic unit tests cover structural detection/refusal, fixed widths/padding,
regions/peripherals, boot mappings, pure/no-op previews, exact round trips,
CRC vectors, stale files/trees, corrupted staged expectations, no-clobber,
symlinks, source immutability and the shared DCP publication/undo lifecycle.
Validation uses isolated `CARGO_TARGET_DIR=/tmp/emuwiz-ipbin-target`.
No real extracted Dreamcast trees were found for the optional inspection pass.

| Check | Result |
|---|---|
| Dreamcast focused (`--lib dreamcast`) | 135 passed, no failures |
| IP.BIN tests (included above) | 34 passed |
| DCP apply / readiness tests (included above) | 15 / 6 passed |
| Optical fingerprint focused | 8 passed |
| Game identity focused | 248 passed |
| Complete `archivefs-core --lib` | 10690 passed, 3 existing ignored, no failures |
| `cargo check --offline --locked --workspace` | Passed; same four GUI warnings as baseline |
| `cargo fmt --all -- --check` / `git diff --check` | Passed |
| Task scope / GUI boundary guards | Passed; exactly five allowed files |

Commands used `cargo test --offline --locked -p archivefs-core --lib` with
`dreamcast`, `optical_fingerprint`, `game_identity`, and no filter. The three
ignored library tests are existing manual real-pack verification, disposable
100k-catalogue performance measurement, and the 250000-entry scanner regression;
none belongs to this feature. The complete suite was run with the existing
localhost/fixture-process tests enabled. Initial and final collision audits
found no required-file changes in other worktrees. Main stayed at the starting
SHA, with a clean tracked tree matching `origin/main`; nothing was pushed.
