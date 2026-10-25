# Dragon 32/64 and Tandy CoCo cassette research (V16R)

## Decision

This is a **research-only** packet.  A future standard decoder is feasible,
but should be one shared 6809-family implementation with Dragon/CoCo metadata
profiles, not a new PCM pipeline and not a generic-FSK classifier.  No Dragon
or CoCo decoder is registered by V16R.

## Sources and confidence

The primary source for the ordinary CoCo record layout is Tandy's *Color
Computer 2 Service Manual*, section 4.11, “Cassette Tape Format Information”
(the archived [PDF](https://support.retrorewind.ca/_media/coco/color_computer_2_ntsc_service_manual_26-3134_26-3136_tandy_1_.pdf)).
It describes the 128-byte `$55` leader and checksum over block type, length,
and data.  The later CoCo 3 service manual independently retains the same
format.  XRoar's [manual](https://www.6809.org.uk/xroar/doc/xroar.shtml) is a
useful implementation-facing source: Dragon and emulated CoCo-family machines
use a single-cycle-per-bit cassette representation; its CAS is a direct bit
representation and CUE preserves silence/wavelength changes.  A contemporary
Dragon User description records `$55`, `$3c`, and approximately 1500 baud.

The exact ROM tables should be checked against a Dragon ROM/source listing
before production code is written.  In particular, this packet does **not**
promote a frequency pair to a Dragon/CoCo identity claim merely because it is
reported by secondary sources.

## Standard modulation and framing

Dragon 32/64 and the Tandy TRS-80 Color Computer use the same practical
cassette family: one complete tone cycle represents one bit and the cycle
wavelength selects the bit value.  The documented standard stream is about
1500 baud.  The proposed waveform decoder must calibrate the two local cycle
families from a leader, rather than use raw sample counts or a universal
frequency threshold.

Ordinary blocks are preceded by a leader of at least 128 `$55` bytes, followed
by sync `$3c`.  Bytes are serially framed by the ROM cassette routine; a future
implementation must verify the actual start/data/stop convention from the ROM
listing and reject a stream if framing cannot be established.  It must not
assume generic Kansas City framing simply because both are FSK-like.

The standard block body is:

```
leader ($55 repeated) | sync ($3c) | block type | length | data[length] | checksum
```

The checksum is the low eight bits of the sum of block type, length, and every
data byte.  Invalid checksums remain `Invalid` evidence; they never become a
strong platform anchor.  The service manual makes the checksum rule explicit.
No silent repair or duplicate-copy assumption is warranted.

## File and boot metadata

The standard name-file block contains the file name (eight bytes), file type,
ASCII/binary mode, a gap field, load address, and execution address.  A future
model may expose those fields **only** after it has decoded a valid standard
name block with a valid checksum.  Data blocks expose their block type, length,
payload, ordinal, checksum, and sample/time bounds.  EOF is represented by the
normal format's EOF/end block type, not guessed from a carrier gap.

The practical BASIC versus machine-code distinction belongs to the encoded file
type/ASCII fields, not to payload heuristics.  Boot media can be modelled as a
standard bootstrap/name/data sequence when ROM-documented fields prove that
meaning; V16R does not assert that arbitrary protected game loaders follow it.

## Dragon and CoCo relationship

The common 6809 cassette format is strong enough to recover a family-level
“Dragon/CoCo standard cassette” record.  It is not by itself a safe way to
separate Dragon 32/64 from CoCo 1/2/3: ordinary file blocks are intentionally
compatible.  Dragon-vs-CoCo platform selection should therefore remain
family-level unless a trusted container/profile, folder, DAT, or independently
verified machine-specific record proves more.  No material 32/64 or XL/XE-like
variant distinction was found for this ordinary cassette protocol.

## CAS and WAV containers

For this family `.cas` is **not** Atari CAS and must never be dispatched by
extension alone.  XRoar documents it as a compact direct bit representation;
the bytes can often be read in a hex editor when aligned.  Its optional CUE
extension records silences and per-bit wavelengths, which is useful for
fastloaders but is not a universal CAS standard.  WAV is sampled audio and
requires the normal bounded edge/cycle recovery.

The future bridge should first parse/directly validate ordinary CAS bits into
the same record decoder used after WAV byte recovery.  CUE should be retained
only as bounded timing/gap evidence.  Atari `FUJI` CAS and MSX CAS are unrelated
containers and must remain separate parsers.

## Proposed EmuWiz evidence gate

Strong Dragon/CoCo standard-cassette evidence requires all of:

1. a stable local two-cycle leader consistent with the recovered standard baud;
2. framed `$55` leader plus `$3c` sync at a credible boundary;
3. a bounded ordinary block type/length structure; and
4. a valid additive checksum.

A valid name block gives additional confidence and recoverable metadata.  A
checksum-invalid but otherwise plausible record is partial/weak evidence only.
A tone pair, a `$55`-like run, filename-looking bytes, or a single block with
no integrity proof remains `Unknown`/generic FSK.  This avoids accidental
classification of BBC/KCS, MSX, Atari, Spectrum, CPC, or random dual tones.

## Architecture fit

| Existing primitive | Fit |
| --- | --- |
| PCM downmix, hysteresis, bounded edge timestamps | Reuse directly |
| Local timing clusters and stage bounds | Reuse directly |
| Serial symbol/byte framing | Small profile-specific extension |
| Record recovery, checksum state, sample/time provenance | Reuse conventions directly |
| Partial recovery and warnings | Reuse directly |
| Normalized custom-stage fingerprints | Reuse after a valid standard anchor |

No second audio pipeline is justified.  A future `dragon_coco_tape.rs` should
be isolated until the active Atari/MSX registration seams are free.

## False-positive matrix

| Family | Why tone similarity is insufficient | Strong discriminator |
| --- | --- | --- |
| BBC / KCS | FSK and serial data can look similar | no `$55`/`$3c` + valid 6809 block checksum |
| MSX | FSK with a different marker/file layout | MSX marker/file semantics, not CoCo block body |
| Atari 8-bit | FSK-like boot/data records | Atari record controls/checksum differ |
| Spectrum / CPC | pulse-duration protocols, not this record layout | no valid cycle/framing/block gate |
| C64 | Datasette pulse pairs | no 6809 leader/sync/checksum structure |
| random dual tones | may form two clusters | fails framing and additive checksum |

## Future custom/turbo work

The safe V17/V18 hook is: checksum-valid ordinary Dragon/CoCo bootstrap record
followed by a later materially non-standard stage.  That stage can reuse the
existing generic `GenericTurbo`/`CustomPulse`/`MultiStage` evidence but must not
gain a Dragon/CoCo label without the anchor.  CAS+CUE is especially relevant to
that future work because it can retain wavelength changes and gaps.

Named fastloader recognition is deferred.  XRoar's CUE documentation proves
that altered wavelengths/fast loaders exist, but does not provide a
non-game-specific, multi-clue signature with a near-miss corpus.  No named
loader is recommended.

## Synthetic fixture plan and implementation order

Use synthetic leader/sync/block waveforms only: checksum-valid name/data/EOF,
checksum-invalid block, truncation, partial multi-block tape, modest and
excessive drift, and 22.05/44.1/48/96 kHz captures.  Negative fixtures must
include generic FSK, BBC, MSX, Atari, random tones, and a malformed
`$55`/`$3c`-looking sequence.  Confirm source-byte immutability and no raw PCM
retention.

Recommended order: (1) ROM-verify exact serial timing/framing and block-type
constants; (2) isolated CAS bit/record parser; (3) synthetic waveform decoder;
(4) only then register a family-level TapeAnalysis variant; (5) later evaluate
bootstrap-gated generic custom stages.  Do not attach release identity, run an
emulator, rename, or extract content in any phase.
