# Retro tape and cassette identity research

**Status:** research only; no parser or registry changes are made by this
document.

**Scope:** Commodore TAP and T64, ZX Spectrum TAP and TZX, Amstrad CPC CDT,
and raw WAV/audio cassette captures. The closely related structured timing
formats PZX, CSW, and Acorn UEF are included where they change the design.

## Executive recommendation

EmuWiz should treat a tape file as a media object first and a game identity
only after a format-specific, bounded inspection. The file extension is not
enough: `.tap` is shared by incompatible Commodore and ZX representations,
and `.cdt` is a CPC convention for the same TZX container grammar used by
`.tzx`.

Recommended identity ladder:

1. **Media recognition:** identify a tape/container or sampled-audio object.
2. **Structural parsing:** validate framing, declared lengths, and supported
   version fields without playing or executing the tape.
3. **Platform evidence:** emit a family or platform candidate only where the
   format has real discriminating evidence; otherwise retain ambiguity.
4. **Logical members:** expose decoded block/member facts as provenance,
   never as a verified release name by themselves.
5. **DAT authority:** use an exact DAT/hash match for release identity. A
   whole-file hash is authoritative only when the DAT explicitly catalogues
   that representation.

The practical order is:

- **Implement first:** Commodore TAP container discrimination and T64
  bounded directory/member inspection.
- **Implement later:** ZX Spectrum TAP, then one shared TZX/CDT structural
  parser with no timing interpretation or control-flow execution.
- **Research more:** PZX, CSW, and UEF, unless a concrete catalogue or launch
  requirement makes one urgent.
- **Do not support as identity yet:** WAV/VOC/sample-audio decoding. EmuWiz
  should identify a WAV as audio media at most and should not guess a machine,
  program, or release from waveform heuristics.

This is consistent with the current code: `ContentKind::TapeImage` and the
`ContentEvidenceKind::TapeFormat` vocabulary already exist, while the tape
extensions currently have no production parser or `game_identity` evidence
arm. The current platform registry's TZX/CDT signature parity correctly keeps
the shared container ambiguous without stronger context.

## Format summary

| Format | Representation | Signature / bounded recognition | Embedded facts | Safe identity ceiling |
|---|---|---|---|---|
| Commodore TAP | Raw pulse-width stream in a small container | Strong `C64-TAPE-RAW` family signature; fixed header and declared data length can be checked with a short read | Version, machine-family byte, video standard, pulse widths; program headers/checksums are inside decoded tape data, not guaranteed | Strong Commodore tape-container evidence; machine byte is platform-family evidence; no release identity without decoded/catalogued bytes |
| Commodore T64 | Logical file/archive container, not a faithful tape waveform | Strong padded C64S/T64 description field plus version; header and directory are bounded | Entry type, C64 file type, PETSCII filename, start/end addresses, payload offset; no robust per-entry checksum | Strong C64-family container evidence; member facts are provenance; exact release remains hash/DAT based |
| ZX Spectrum TAP | Sequence of logical tape blocks | No magic; each block has a little-endian length; bounded only by a configured file limit and complete block walk | Standard blocks can contain flag, type, ten-character name, lengths, load/autostart parameters, and XOR checksum | Corroborated ZX-standard-block evidence at best; filename and header fields are labels/provenance; no platform proof from extension alone |
| ZX Spectrum TZX | Structured timing/block container preserving standard, turbo, and custom loaders | Strong `ZXTape!` + `0x1a` + version header; linear block-length walk is bounded | Block IDs, pauses, pulse timings, direct recordings, hardware/text/archive metadata, and sometimes embedded standard data headers | Strong TZX-container evidence; platform remains context/family evidence because the same grammar is used by CPC CDT and other extensions |
| Amstrad CPC CDT | TZX-compatible structured timing container using CPC conventions | Same strong `ZXTape!` header as TZX; `.cdt` is a naming convention, not a byte discriminator | Same block/timing fields; CPC loader data may contain names, lengths, addresses, and checksums according to its protocol | Strong structured-container evidence; CPC candidate requires trusted context, CPC-specific data interpretation, or DAT evidence |
| WAV/audio capture | Sampled waveform in RIFF/WAVE or raw PCM | RIFF/WAVE identifies audio only; audio parameters are bounded header facts | Sample rate, channels, sample format, duration; optional metadata is not tape identity | Audio-media recognition only; no identity or platform claim from waveform heuristics |

The format documentation behind this table is the [VICE emulator file-format
reference](https://vice-emu.sourceforge.io/manual/vice.pdf), the [ZX TAP
format reference](https://worldofspectrum.net/zx-modules/fileformats/tapformat.html),
the [TZX specification](https://www.alessandrogrussu.it/tapir/tzxform120.html),
the [TZX/CDT reference](https://www.cpcwiki.eu/index.php?title=Format:CDT_tape_image_file_format),
and Microsoft's [RIFF/WAVE overview](https://learn.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff-).

## Format-by-format findings

### Commodore TAP

The Commodore TAP format is a pulse representation, not a logical directory
or block archive. In the VICE layout, the header begins with the ASCII
`C64-TAPE-RAW` signature and includes a TAP version, computer-platform byte,
video-standard byte, reserved data, and a little-endian data-size field. TAP
v0 stores pulse lengths in one-byte units with an overflow representation;
v1 adds extended pulse lengths after a zero marker; v2 changes the pulse
interpretation for C16 half-waves. Those version rules must not be collapsed
into one parser mode.

Recognition is unusually practical: a bounded read can check the signature,
version, supported machine byte, and declared data length against the actual
file length. This proves the object is a plausible Commodore raw-tape image.
It does not prove that the pulse stream decodes successfully, contains a
complete program, or was made for one exact software release.

The machine and video fields are useful structural/platform evidence, but
they are self-declared container metadata rather than an independent release
authority. A decoded Commodore ROM-loader block may contain a filename and
the tape protocol may carry an XOR checksum. Turbo loaders and copy
protection use other pulse patterns, so EmuWiz must not require ROM-loader
blocks or interpret pulses in the first parser.

The format itself has no cryptographic checksum over the pulse stream. A TAP
whole-file SHA-256 is an excellent fixity identifier for that exact TAP
artifact, but is not a logical-program identifier. Different PAL/NTSC
recording parameters, pulse encodings, loader choices, or preservation edits
can represent related logical content with different bytes. Without a
reviewed Commodore pulse decoder, a normalized logical fingerprint is not
safe to derive.

### Commodore T64

T64 is a logical container of C64-style files, not a pulse-accurate tape
image. The documented layout has a fixed header, a directory of fixed-size
records, and payloads elsewhere in the file. The header carries a padded C64S
tape-image description, version, entry counts, and a user description. A
directory record carries entry type, C64 file type, start/end addresses,
payload offset, and a 16-byte C64 filename.

The header description is a strong C64/T64 container signature when accepted
variants are validated. It does not preserve tape timings, pilot tones,
gaps, or custom loaders. VICE explicitly describes T64 as poorly suited to
actual tape emulation and notes that it is mainly a file container.

A bounded T64 parser should validate the header, cap the declared entry
count, skip free entries, check each payload range against the file, reject
integer overflow and overlapping/ambiguous ranges, and retain member index
and provenance. Address ranges are useful metadata and consistency checks,
not cryptographic identity. The filename is a label; PETSCII decoding must
not be treated as a verified title.

There is normally no per-entry checksum in the T64 directory format. Hashing
each bounded payload gives useful exact-byte identity for the contained file;
hashing the whole T64 identifies the particular container arrangement. A
canonical member fingerprint can be derived as an explicit tuple such as
`entry type + C64 file type + load/end addresses + payload bytes`, but it
must remain a candidate unless a DAT defines that representation. Do not
silently extract or turn a T64 member into a separate launchable game during
identity inspection.

### ZX Spectrum TAP

ZX TAP is a simple sequence of blocks. Each block begins with a little-endian
16-bit length followed by the bytes stored in the tape block. It has no magic
header, so `.tap` alone cannot distinguish it from Commodore TAP. The block
length walk is bounded and deterministic, but a small file can be arbitrary
bytes that happen to satisfy one or more length fields.

Standard ROM-style header blocks are 19 bytes and include a flag, type,
ten-character filename, data length, and parameters such as BASIC autostart
line or machine-code load address. Standard data blocks use a flag and an
XOR checksum. The reference also documents fragmented blocks shorter than
two bytes; these should be classified rather than incorrectly treated as a
normal complete header/data pair.

The XOR checksum is useful for detecting many accidental errors and for
supporting a structural parse, but it is only eight bits and is neither
collision-resistant nor authenticity-protecting. A valid checksum does not
prove a commercial title. Header names, type bytes, load addresses, and
autostart values are protocol metadata or labels. They can support a
corroborated “standard Spectrum tape block” observation, not exact release
identity.

A complete logical block sequence is a reasonable normalized candidate for
standard TAP: preserve block order, flags, bytes, and whether each checksum
validated. Keep fragmented/custom blocks distinct. Do not strip all headers
or concatenate payloads without preserving boundaries; doing so can merge
different tapes into the same fingerprint. A DAT/hash match remains the
release authority.

### ZX Spectrum TZX

TZX begins with `ZXTape!`, the text-file marker `0x1a`, and major/minor
version bytes. It then contains ID-tagged blocks for standard and turbo data,
pure tones, pulse sequences, pure data, direct recordings, generalized data,
pauses, grouping, selection, text/archive information, hardware information,
and control-flow-like tape operations.

The header and each declared block length can be validated with bounded
linear reads. The format intentionally preserves timing and custom-loader
structure, so the presence of a program name or a standard data block is not
the whole identity of the tape. Hardware-information blocks and `.tzx`
convention are useful hints, but shared TZX usage prevents the signature from
being a unique platform authority.

The embedded standard data blocks may have the ZX XOR checksum described
above. TZX itself does not provide a universal cryptographic checksum for the
whole logical tape. Timing, pauses, custom pulses, direct recordings, and
control-flow metadata can all differ while retaining the same intended
software. Conversely, those differences may be preservation-significant for
a protected or turbo-loaded title.

The safe first parser is therefore a framing/inventory parser. It should
record block IDs, declared sizes, version, bounded metadata, and the presence
of timing/control-flow/expansion blocks. It should **not** follow jumps,
loops, calls, or selections, and should not expand CSW/RLE or simulate pulses.
The format is a tape program for an emulator, but it must be data to an
identity parser.

### Amstrad CPC CDT

CDT uses the TZX structure and the same `ZXTape!` header. The `.cdt`
extension distinguishes a CPC-oriented file by convention; the inner bytes
are not a separate magic format. CPC block timing and loader conventions are
represented through the shared TZX block types and data parameters.

This is why EmuWiz must not make a `ZXTape!` hit uniquely mean “ZX
Spectrum”. The current registry already treats the shared signature as
corroborated container evidence for both CPC and ZX contexts, allowing a
folder/DAT/other structural fact to settle the platform. A CDT parser should
be one shared TZX framing engine with platform-specific interpretation kept
in separate evidence consumers.

CPC data headers may expose names, lengths, load/exec information, and
protocol checksums when the relevant loader convention is understood. These
are medium-strength structural facts and labels, not a universal release ID.
Custom CPC loaders make a generic “decode every program” promise unsafe.

### Raw WAV/audio cassette captures

WAV is a sampled-audio container. RIFF/WAVE parsing can establish a strong
fact about the outer object and can safely read `fmt`/`data` chunk metadata
with checked chunk lengths. It does not establish that the sound is a tape,
which computer recorded it, or which software it contains. Optional INFO,
BEXT, or application metadata is provenance/hint data unless independently
trusted.

Unlike a digital TAP/TZX/T64 artifact, two recordings of the same physical
tape normally have different sample values because of recorder frequency
response, speed drift, wow/flutter, channel balance, phase, amplitude,
background noise, edits, and sample-rate/bit-depth choices. Ordinary SHA-256
is therefore useful for exact-file fixity and useless for cross-recording
logical identity.

Resampling and amplitude normalization can improve decoder input but do not
make a stable identity transform: timing drift and noise remain, and
different decoders make different edge decisions. Pulse/edge extraction can
produce a stable logical sequence **after** a machine-specific decoder has
successfully identified the encoding and recovered blocks. That sequence is
decoder-dependent, can lose meaningful custom-loader information, and must
be treated as a candidate unless a preservation DAT defines it.

Machine-specific decoders are required because Commodore, Spectrum, CPC,
Atari, MSX, and other computers use different modulation, pilot, sync,
threshold, and checksum rules. A generic waveform classifier would have a
large false-positive surface: speech, music, modem sounds, noise, and a
different computer's tape can all contain plausible pulse runs. Processing
cost is at least linear in the samples and may require filtering, adaptive
thresholding, resampling, and retrying decoder parameters. Long audio files
also create a straightforward CPU and memory denial-of-service risk.

**WAV verdict:** do not support WAV identity in EmuWiz core now. At most,
recognize RIFF/WAVE as audio media, report its bounded technical metadata,
and offer no platform/release claim. If a user later needs this workflow,
make it an explicit external conversion process that produces a structured
TZX/PZX/CAS/TAP artifact. EmuWiz can then inspect the resulting digital
format under normal bounded rules.

## Trustworthiness of identity signals

| Signal | Classification | What it can support | What it must not do |
|---|---|---|---|
| Exact cryptographic hash of the delivered file | Strong for that exact representation | Exact DAT entry when the DAT hashes that same representation | Prove equivalence to another container or recording |
| Valid fixed magic and version | Strong structural evidence | “This is plausibly a T64 / Commodore TAP / TZX container” | Prove title, release, or unique platform where the format is shared |
| Valid bounded block/chunk framing | Corroborated structural evidence | “The file contains a coherent ZX/TZX/CPC-style block stream” | Prove the blocks are executable, complete, or a known game |
| Valid embedded protocol checksum | Medium corroboration | Detect many accidental block errors; support a decoded block fact | Authenticity, cryptographic equality, or malware/content safety |
| Machine/hardware metadata in the container | Medium platform hint | Narrow the candidate family when consistent with other evidence | Override contradictory bytes, folder assignment, or DAT evidence |
| Program/filename/block name | Weak label/provenance | Display and candidate matching | Verified game identity or automatic rename |
| Load/exec address and lengths | Medium structural metadata | Explain a member and validate internal consistency | Unique release identity |
| Timing/pulse parameters | Strong preservation metadata when parsed | Explain loader/tape representation; compare exact artifacts | Universal logical identity after arbitrary normalization |
| RIFF/WAVE audio header | Strong audio-container evidence | Recognize audio and describe sample format | Platform, tape, title, or release identity |
| WAV-derived decoded blocks | Decoder-dependent candidate | Candidate logical content after an explicit, successful decoder | Generic cross-machine authority |

The existing EmuWiz vocabulary maps naturally to this model: format facts
belong in `ContentEvidence` with a confidence about the fact itself;
platform resolution must separately consider whether that fact is generic,
family-scoped, or platform-specific; verified release facts belong only to a
trusted exact match. “Strong container evidence” must never be converted
implicitly into “strong game identity”.

## Bounded parsing and corruption policy

All tape readers should use the existing `safe_read` and bounded-reader style,
checked arithmetic, and explicit refusal states. A parser should return a
structured observation or a refusal; it must not truncate input until it
looks valid.

### Common limits

These are recommended initial policy limits, not schema requirements. They
should be turned into named technical constants only after corpus sampling:

| Limit | Initial recommendation | Reason |
|---|---:|---|
| Total tape bytes | 8 MiB tape-specific ceiling, below the existing 64 MiB identity read ceiling | Ordinary digital tape images are small; prevents accidental giant reads |
| TZX/CDT/PZX/UEF/CAS blocks or chunks | 65,536 | Stops count-driven allocation and pathological walks |
| Single declared block/chunk | 8 MiB | Allows large direct-recording/data blocks without unbounded allocation |
| Metadata text | 4 KiB per text/archive/hardware description | Prevents hostile display allocations |
| T64 directory entries | 64 by default; also bound by remaining bytes | Matches common layout while retaining a hard walk bound |
| TZX control-flow execution | Zero | Count/validate control-flow records; never follow them |
| Pulse expansion | No expansion in Phase 1 | Avoid CSW/RLE bombs and waveform-scale work |
| WAV samples | Explicit byte and duration cap; stream where possible | CPU cost is proportional to samples and DSP retries |

### Format-specific checks

- **Commodore TAP:** verify signature variant, supported version, machine and
  video field ranges, declared payload length, and v1/v2 overflow semantics.
  Do not interpret a zero byte using the wrong version's rule.
- **T64:** verify the fixed header, counts, entry types, payload offsets,
  payload lengths, range overflow, and overlaps. Free/reserved entries are
  counted but not treated as games. Unknown entry types should be retained as
  unsupported metadata, not guessed as normal files.
- **ZX TAP:** require every declared block to fit before reading it. Permit
  documented fragmented blocks but mark them as such. A bad XOR checksum is a
  warning/refusal for a “validated standard block” claim, not proof that the
  whole file is useless or proof of a different platform.
- **TZX/CDT:** validate version compatibility, every block ID's fixed or
  declared size, unknown length-prefixed extensions, and metadata length.
  Record jumps/loops/calls/selects without resolving them. Refuse malformed
  framing, but do not run a tape control-flow graph.
- **WAV:** validate RIFF size bounds, chunk padding, `fmt` structure, sample
  format, channel count, and `data` range. Do not allocate from an unchecked
  sample count or infer tape identity from a short prefix.

Malformed/truncated input should produce a specific structural finding such
as “truncated block declaration” or “declared payload exceeds file”, with the
recognized media family retained when safe. It should never fall back to a
filename-derived verified title.

## Related formats that affect the architecture

### PZX

PZX is a structured ZX-class tape format with a `PZXT` signature and
length-prefixed chunks such as pulse, data, pause, browse, and stop records.
Its flat chunk model is simpler to validate than TZX because it has no TZX
jump/loop/call control-flow records. It is still timing-oriented, not a
release catalogue. If needed, it is a good later sibling parser after the
shared TZX/CDT work.

### CSW

CSW represents compressed pulse streams. Container recognition can be
bounded, but RLE/Z-RLE expansion can be much larger than the stored file.
Phase 1 should record the container and compression metadata only. If pulse
counts become useful later, expansion must be streamed under a global output
cap and never allocated from an unchecked declared size.

### Acorn UEF

UEF is a gzip-wrapped chunked tape format used by BBC Micro and Acorn Electron
workflows. It materially reinforces two rules: decompression needs an output
limit, and a valid shared tape container gives family evidence rather than a
unique machine identity. It is a separate later parser, not a reason to put
gzip or generic tape execution into the first implementation.

Other cassette ecosystems such as Atari CAS, MSX CAS, and machine-specific
UEF/CAS variants reinforce that extensions are collision-prone and that a
format detector must be separate from the canonical platform registry.

## DAT ecosystem implications

### TOSEC

TOSEC-style DATs commonly catalogue exact delivered files using the
ClrMamePro/Logiqx hash vocabulary and use structured names/categories to
describe system, media, region, language, dump status, and variants. The
name and category are useful source metadata; the file hash is the exact
artifact authority. A TOSEC entry for a `.tap`, `.tzx`, `.cdt`, or `.t64` can
therefore verify that particular file representation, but does not imply
that a differently timed TZX, another T64 container, or a WAV recording is
the same hash identity.

This is an evidence-based engineering conclusion from TOSEC's naming/DAT
model, not a claim that every historical or future TOSEC branch contains all
of these tape formats. Treat the current DAT snapshot and source family as
the authority.

### No-Intro

No-Intro is primarily ROM/digital-content oriented. Its documented file
convention includes size, CRC32, MD5, SHA-1, and SHA-256 fields, along with
optional native filename, serial, version, and status metadata. Those fields
are exact-file evidence when present. They do not define a universal
normalized tape representation, and the availability of richer fields varies
by DAT export. No-Intro should not be assumed to catalogue raw WAV captures
or to provide a tape-normalization authority.

See the [No-Intro file convention](https://wiki.no-intro.org/index.php?title=File_Convention)
and [No-Intro naming convention](https://wiki.no-intro.org/index.php?title=Naming_Convention)
for the distinction between exact file fields and descriptive archive
metadata.

### MAME software lists

MAME software lists are structured per-system/per-list catalogues. They can
describe cassette media as software parts and use CRC/SHA-1 values for exact
media images where the list supplies them. MAME's own documentation explicitly
warns that inherently analog media, such as home-computer software on audio
tape cassettes, are problematic to dump identically. That is direct support
for keeping WAV/audio logical identity outside EmuWiz's generic identity
claims.

MAME's short names and parent/clone relations are authority within the MAME
software-list namespace, not a universal title identity for every `.tap` or
`.tzx`. An EmuWiz MAME match must retain the list name, software name, part,
hash, and source provenance rather than flattening it into an unqualified
game title.

See [MAME software-list guidelines](https://docs.mamedev.org/contributing/softlist.html)
and [MAME asset lookup documentation](https://docs.mamedev.org/usingmame/assetsearch.html).

### Preservation-specific and generic DATs

Preservation collections may publish exact file hashes, logical member hashes,
track hashes, or format-specific checksums. There is no universal convention
that makes a DAT's `sha1` mean “decoded tape program” rather than “delivered
file”. The safe runtime rule is representation equality:

- compare a whole-file hash only to a DAT entry explicitly about that whole
  file representation;
- compare a T64 member hash only to an entry explicitly about that member;
- compare a normalized block/pulse hash only when the DAT defines the exact
  normalization and version;
- otherwise retain the hash as local fixity and report “not matched” rather
  than inventing equivalence.

This is especially important for WAV: exact recordings can be catalogued for
preservation, but a recording hash is not a cross-recording program identity.

## Proposed EmuWiz architecture

The current repository already provides the right seams:

| Existing concept | Tape extension point | Responsibility |
|---|---|---|
| `media_registry` | Keep media recognition separate from platform assignment | Recognize registered tape extensions as media; do not make the extension a platform authority |
| `ingestion::content_registry` / `ContentKind::TapeImage` | Add format-specific observations behind the existing content category | Tell the catalogue “this is tape media” even when identity is unresolved |
| bounded readers / `safe_read` | New pure tape observers | Validate fixed headers and linear framing under explicit limits |
| `ContentEvidenceKind::TapeFormat` and `MediaClass::Tape` | Emit `TAP`, `T64`, `TZX`, `CDT`, or audio facts | Describe the object and confidence; never resolve a game by itself |
| `game_identity` / `IdentityEvidence` | Add only format facts that have a reviewed evidence contract | Preserve status (`Verified`, `Candidate`, `Ambiguous`, `Invalid`, etc.), value, diagnostic, and provenance |
| `content_evidence_scope` | Classify tape facts as generic, family, or platform-specific | Prevent a strong container fact from becoming a platform-specific fusion leg |
| platform registry / fusion | Consume parser evidence with existing conflict rules | Resolve C64/ZX/CPC only when the evidence actually discriminates; preserve ambiguity otherwise |
| DAT parsers/audit | Compare exact representations and store source lineage | Supply exact release authority and set/audit verdicts where a DAT supports the representation |
| archive-member evidence | Run bounded tape observers on eligible members | Preserve member path/index provenance; never extract or create a new launch item as a side effect |
| database persistence | Reuse existing identity/evidence persistence | Persist observations and freshness; no tape-specific migration is justified by this research |
| launch planner | Future platform-specific launch rows/adapters | Consume already-resolved identity/readiness; a tape parser must not create launch authority |

The minimal future result shape should distinguish:

- `media`: recognized audio/tape/container format;
- `structure`: valid/truncated/invalid plus bounded counts;
- `platform_evidence`: a candidate and its scope;
- `members`: block/member metadata and per-member provenance;
- `release_match`: DAT/hash verdict, initially absent unless exact;
- `warnings`: checksum, custom-loader, unsupported-block, or ambiguity notes.

Do not create a second tape identity system. The parser should be a pure
observer; the existing identity orchestrator and DAT lane should remain the
decision/persistence boundaries.

## Recommended implementation order

### Implement first

1. **Commodore TAP header observer.** Small, high-value, and the decisive
   discriminator against ZX TAP for the shared `.tap` extension. Emit strong
   Commodore-TAP container evidence and a separately scoped machine-family
   hint. Do not decode pulses.
2. **T64 bounded container/member observer.** It has a clear signature and
   fixed directory records. Expose member names/addresses as provenance and
   exact payload hashes as local facts. Do not extract or launch members.

### Implement later

3. **ZX TAP block observer.** Validate complete/fragmented block framing and
   standard XOR checksums under a file cap. Emit corroborated block evidence,
   not unique platform identity from the extension.
4. **Shared TZX/CDT framing observer.** Parse block IDs and lengths linearly,
   keep CPC/ZX interpretation separate, and inventory control-flow and
   compressed-pulse blocks without following or expanding them.

### Research more

5. **PZX** after the TZX/CDT consumer need is clear.
6. **CSW** as container-only first; expansion only with a concrete need and
   measured global cap.
7. **UEF** after confirming the platform rows and gzip policy needed by the
   current registry.
8. **Protocol-specific logical fingerprints** only after real DAT examples
   define what normalization means for each format.

### Do not support as identity yet

9. **WAV/VOC/sample audio.** Recognize only the outer audio media if needed;
   no heuristic tape/platform/title identity in core.
10. **Pulse decoding for Commodore TAP/TZX/CDT.** Preserve and inventory
    pulse/timing structures first; decoding belongs to a separately reviewed
    machine-specific effort.

## Suggested Phase 1 implementation

Phase 1 should be a read-only structural/evidence slice:

1. Add a bounded Commodore TAP observer for the fixed header and declared
   payload length.
2. Add a bounded T64 observer with entry/range validation and member
   provenance, without extraction.
3. Add parser tests for valid headers, unsupported versions, bad lengths,
   overflow/overlap, free/unknown entries, and adversarial counts.
4. Emit `ContentEvidence` facts and evidence lineage while keeping platform
   resolution conservative.
5. Make `.tap` collision behavior explicit: Commodore magic can distinguish
   the Commodore representation; a non-Commodore `.tap` still needs ZX block
   evidence or trusted external context.
6. Keep exact release matching on the existing DAT/hash path. No rename,
   automatic title assignment, launch support, WAV decoder, or database
   migration belongs in Phase 1.

The Phase 1 acceptance statement should be: “EmuWiz can say what kind of
tape/container this is, show bounded structural facts, and explain when the
platform or release remains unknown.” It should not say “EmuWiz identifies
every tape game.”

## Security and resource limits

The parser must defend against both malformed framing and semantic resource
amplification:

- checked offsets, lengths, additions, and multiplications;
- total input and per-block caps;
- bounded entry/chunk counts;
- no recursion based on tape data;
- no TZX jump/loop/call/select execution;
- no CSW/RLE or gzip expansion in the first slice;
- no audio DSP or decoder retry loops in core;
- bounded metadata retention and safe text decoding;
- no external process, emulator, shell, network, extraction, or filesystem
  writes during identity inspection;
- preserve the original bytes and report partial/unsupported data explicitly.

The outer identity read limit already present in EmuWiz is an upper safety
boundary, not permission to consume the whole file for every format. Tape
parsers should use a smaller format policy where practical and expose a
named `ResourceLimitReached`/equivalent outcome.

## Explicit non-goals

- No WAV-to-TAP/TZX decoder or waveform classifier.
- No automatic extraction of T64 files into the library.
- No BASIC tokenization or program execution.
- No emulator control-flow simulation for TZX.
- No pulse normalization presented as exact logical identity.
- No title/filename-only verified identity.
- No automatic renaming from embedded tape labels.
- No DAT schema or database migration from this research.
- No claim that a valid container is a launchable game.
- No attempt to make every historic/custom/turbo loader semantically
  understood.

## Open questions

1. Which canonical platform IDs should represent the machine byte values in
   Commodore TAP, especially C16/Plus-4 and VIC-20, in the current registry?
2. Should T64 members be first-class child observations or remain a compact
   list attached to one library item?
3. Which TZX/CDT hardware-info records are trusted enough to narrow the
   platform, and how should contradictory `.cdt`/`.tzx` convention be shown?
4. Does the target DAT corpus publish whole tape files, logical blocks,
   decoded PRG payloads, or several representations?
5. For standard ZX/CPC blocks, should normalized fingerprints preserve pauses,
   block boundaries, flags, and checksum bytes exactly?
6. Are there real, legally usable fixtures for each supported loader class,
   including malformed and hostile inputs?
7. Which tape platforms have a supported emulator/profile and safe launch
   command after identity exists? This must be answered by
   [the current launch-support documentation](../LAUNCH_SUPPORT.md), not by a
   parser.

## Current support boundary

At this research base, `.tap`, `.tzx`, and `.cdt` are recognized as generic
tape media through the existing registries. The current `game_identity` code
does not have a tape parser arm, and no T64/TAP/TZX/CDT logical identity is
produced by production tape code. Existing whole-file DAT machinery can still
hash a discovered file when the normal catalogue path accepts it; that is
exact-file verification, not decoded tape identity.

The existing [tape format support audit](TAPE_FORMAT_SUPPORT_AUDIT.md) is a
historical repository snapshot and useful design context, but this document's
recommendations are the current research conclusion. The [platform registry
and direct identity guide](../PLATFORM_REGISTRY_AND_DIRECT_IDENTITY.md),
[domain model](../domain-model.md), and [architecture overview](../architecture.md)
remain the authoritative EmuWiz layering references.

## Sources and confidence notes

**High confidence:** fixed headers, field layouts, and checksum descriptions
quoted from the VICE manual, World of Spectrum format references, the TZX/CDT
specification, and Microsoft RIFF documentation linked above.

**High confidence from current repository inspection:** EmuWiz has generic
tape media registration and tape evidence vocabulary, but no production tape
parser or tape-specific `game_identity` observer at this base.

**Medium confidence / implementation inference:** the recommended normalized
fingerprint shapes, caps, and implementation order are engineering proposals.
They require real DAT samples and representative fixtures before becoming
code or release policy. A DAT's hash field must always be interpreted in the
source representation it actually documents.
