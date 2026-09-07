# Deep Tape Analysis V4: PCM/WAV evidence

V4 accepts bounded RIFF/WAVE PCM integer recordings (8-bit unsigned or 16-bit
little-endian, up to eight channels and 192 kHz). Stereo is safely downmixed,
DC offset is estimated and removed for quality/thresholding, and a hysteretic
adaptive edge detector emits bounded sample/time pulse edges. Results retain
only timing summaries and quality metrics, never PCM bytes.

V4a is generic pulse evidence for future cassette decoders. V4b adds a bounded
Spectrum ROM waveform demodulator: a calibrated pilot and sync pair gates
pulse-pair bit decoding, bytes are reconstructed MSB-first, and recovered
blocks retain sample/time provenance, checksum state, timing scale, and a
conservative confidence. Recovered bytes are handed to the existing TAP
interpreter rather than maintaining a second block decoder. Partial or noisy
blocks remain evidence and do not become fabricated data.

The decoder supports sample-rate-independent timing (nominal 2168 us pilot,
667/735 us sync, and 855/1710 us half-pulses), with modest jitter and speed
variation handled by pilot calibration. It remains read-only and bounded; raw
PCM is never retained in analysis results. Compressed audio, WAV formats other
than bounded PCM integer input, custom waveform loaders, emulator execution,
and tape extraction remain out of scope. Unsupported codecs and
malformed/truncated chunks fail closed.

## V5: generic custom/turbo waveform evidence

V5 adds `decode_custom_wav`, a conservative second pass over the existing edge
stream. It discovers bounded timing clusters, stable non-ROM pilot trains, and
one-to-three-pulse sync candidates, then attempts stage-local two-symbol data
recovery in paired-pulse or single-pulse mode. Timing centres, spread, stage
boundaries, ambiguity, bit-order evidence, and confidence are retained as
structured evidence. A small framing hint may distinguish MSB/LSB order;
otherwise the order remains ambiguous and bytes are not claimed.

This is generic custom decoding, not identification of a named commercial
loader. Standard Spectrum ROM decoding remains the V4b path. V5 does not
promise that every custom loader is decodable, does not fabricate TAP metadata,
and keeps uncertain timing as `UnknownCustom`/review evidence. No emulator is
executed, no DAT identity or rename is changed, and no raw PCM is persisted.

## V6: named Spectrum loader families

V6 adds one deliberately narrow named-family interpretation: **Alkatraz**.
It is emitted only when the semantic sequence has all of these independent
clues: a standard bootstrap, a non-header turbo block with a short 192–288
pulse pilot, a 10–14 second inter-stage gap, and a following custom-timed data
stage. This is high-confidence structural recognition. A single timing value,
partial recording, wrong gap, wrong stage order, or standard-header framing
stays in the generic classes.

The classifier uses normalized timing/stage facts, not titles, paths, raw
samples, or a game catalogue. Its fingerprint remains the existing normalized
loader fingerprint, so changing a filename cannot affect classification. TZX
directly supplies the full sequence. V5 WAV recovery intentionally does not
yet retain a whole-recording bootstrap/gap sequence, so it does not emit an
Alkatraz name from a partial waveform; that is a deliberate parity refusal,
not a TZX-only signature table.

Speedlock and Bleepload are intentionally deferred: reviewed references
describe their use and implementation families, but do not provide a stable,
non-game-specific timing-and-stage signature for this fail-closed registry.
Fixtures are synthetic timing structures only. No game title is inferred, no
emulator runs, no PCM is persisted, and no DAT or rename decision changes.

The structural description used for Alkatraz is the documented loader sequence
in [Tape Decoding Using Taper](https://worldofspectrum.net/legacy-info/tape-decoding-using-taper/): standard BASIC, a short-pilot headerless turbo stage,
roughly twelve seconds of noise, then further custom loading. The matcher
requires the machine-readable subset of every one of those stages.

## V7: Commodore 64 cassette WAV decoding

V7 adds bounded decoding for the **standard C64 Datasette ROM protocol** only.
It uses the existing PCM downmix, hysteretic edges, and microsecond timestamps;
no second WAV parser or retained PCM representation is introduced. Each local
short-pulse leader calibrates the short/medium/long timing classes, then the
decoder requires an `L,M` byte marker, LSB-first `S,M`/`M,S` bit pairs, and an
odd parity bit. A valid stream must also carry the standard nine-byte countdown
(`$89..$81` or `$09..$01`) and one 192-byte buffer with its XOR check byte.

The decoder records source sample/time bounds, parity errors, checksum state,
and duplicate relationship. Standard duplicate copies are compared rather than
shown as separate logical files. Verified header/data pairs project the existing
`TapeEntry` fields: filename, BASIC-versus-code type, load address, and declared
length. Partial, malformed, checksum-invalid, Spectrum, and generic pulse
streams do not produce a logical entry.

This V1 intentionally defers TAP-payload decoding (the current TAP path exposes
container metadata only), VIC/PET/C16/Plus/4 timing variants, sequential-file
semantics, and all custom/turbo fastloaders. The implementation is based on
[Datassette Encoding](https://www.c64-wiki.com/wiki/Datassette_Encoding) and
[Simon’s Mostly Reliable Guide to the Commodore Tape Format](https://eden.mose.org.uk/download/Commodore%20Tape%20Format.pdf).
All fixtures are synthetic.

## V8: C64 custom/fastloader waveform evidence

V8 adds a deliberately generic interpretation for non-ROM pulse stages in a
recording that has already yielded at least one checksum-valid C64 standard-ROM
stream. This independent bootstrap requirement prevents a Spectrum recording,
random pulse train, or a lone unusual timing cluster from being labelled C64.
The existing bounded V5 stage scanner supplies stable pilots, one-to-three
pulse sync candidates, local timing clusters, pulse spread, and stage time
bounds. C64 ROM leaders are excluded from that custom-stage pass, so a normal
bootstrap followed by a custom payload can be represented as separate stages
and each custom stage calibrates its own timing scale.

When two stable timing families and V5's framing evidence make decoding
defensible, V8 retains generic single-pulse or paired-pulse bytes along with
the selected bit order, ambiguity count, confidence, and source-time bounds.
Ambiguous ordering or framing remains timing evidence without invented bytes.
Custom bytes are never parsed as a C64 ROM header: filename, type, and load
address remain absent unless V7 independently recovered a valid standard
header/data relationship. A damaged stage does not erase valid evidence from
another bounded stage.

The resulting class is only `GenericTurbo`, `CustomPulse`, `MultiStage`, or
`UnknownCustom`; no commercial fastloader name or game title is inferred. The
timing fingerprint is a short hash of normalized per-stage timing families,
pilot counts, and symbol modes. It excludes filenames, payload bytes, sample
rate, sample indexes, paths, and raw PCM, so equivalent 44.1/48 kHz captures
remain comparable while materially different timing changes the signature.

Current Commodore TAP support intentionally remains container-header-only and
does not expose tape intervals to this decoder. Consequently V8 does not claim
TAP custom-stage parity; that is deferred until the TAP reader can safely
provide bounded timing payloads to the same semantic stage decoder. Named C64
fastloaders (including Novaload, Cyberload, Freeload, Ocean, and US Gold
families) are also deferred: V8 has no documented, multi-clue, non-game-
specific signature plus near-miss corpus sufficient for fail-closed naming.
All fixtures remain synthetic.

## V9: standard Amstrad CPC cassette WAV decoding

V9 adds `decode_amstrad_cpc_wav` on the same bounded PCM edge stream. The
standard CPC cassette manager writes a record as a long leader of one bits
(normally 2048), a zero marker, a sync byte (`0x2c` for a header record or
`0x16` for a data record), then MSB-first bytes. Each bit is one low/high cycle;
the one period is twice the zero period. Records contain 256-byte segments and
the complemented CRC-16 (polynomial `x^15+x^12+x^5+1`, initial `0xffff`,
big-endian CRC bytes). Header fields are projected only from their documented
positions: 16-byte filename, block number, last/first flags, file type, data
length/location, logical length, and execution address.

The decoder calibrates the measured leader locally, accepts bounded timing
drift, retains source sample/time ranges, timing scale, checksum state,
warnings, and confidence, and refuses to identify CPC from timing alone. A
valid sync plus complete segment is required for a CPC block; bad CRC is
retained as an explicitly invalid block, while truncation, random pulses,
Spectrum ROM timing, C64 timing, and generic custom timing fail soft. Multiple
good blocks are retained independently. A non-standard gap/timing stage after
a valid standard record is exposed only as `custom_stage_candidate`; V10 may
use that CPC anchor for generic custom/turbo analysis, but V9 does not decode
or name it.

CDT/TZX standard blocks describe the same logical sync/data/CRC structure, so
their block identity and header metadata are comparable with V9 recovery. CDT
also carries container timing, pauses, and custom block encodings that are not
represented by this WAV evidence layer; byte-for-byte container parity is
therefore intentionally not claimed. The model follows the CPC firmware and
technical references: [CPC cassette data information](https://cpctech.cpcwiki.de/docs/sound.html),
[CPC firmware cassette manager](https://cpcrulez.fr/codingBOOK_soft968-CPC464-664-6128_firmware_008.htm),
and the [CDT/TZX format notes](https://www.cpctech.cpcwiki.de/docs/cdt.html).
Synthetic fixtures cover 22.05/44.1/48/96 kHz, drift, CRC failure, partial
data, false positives, metadata projection, and the deferred custom-stage
boundary. No named loader is guessed, no emulator is run, and no raw PCM is
stored.

## V10: CPC custom/turbo waveform recovery

V10 adds a generic, bootstrap-gated second pass for non-standard CPC waveform
stages. It runs only after V9 has recovered at least one checksum-valid standard
CPC block; a CPC-like leader, timing similarity, or an invalid/truncated header
cannot unlock this interpretation. Standard block metadata remains authoritative
and is retained alongside custom-stage evidence.

The scanner discovers bounded local timing families after the bootstrap,
recognises stable non-standard pilots, accepts one-to-three short sync pulses,
and cuts stages at long pauses. Each stage is calibrated independently, keeping
sample/time bounds, timing-family clusters, symbol mode, bit-order evidence,
ambiguity count, and confidence. Stable paired-pulse binary stages are reported
as `GenericTurbo`; a reliable custom pulse train without enough framing as
`CustomPulse`; materially different stages as `MultiStage`; and clear but
undecodable anchored evidence as `UnknownCustom`. Recovered bytes are retained
only when the existing generic V5 symbol model is sufficiently unambiguous.
MSB/LSB ambiguity is preserved rather than guessed, and a damaged stage does not
erase neighbouring recovered stages.

The CPC custom fingerprint hashes normalized timing-family ratios, pilot counts,
stage order, symbol mode, and bit-order evidence. It excludes sample rate,
absolute sample positions, filenames, and payload bytes, so equivalent 44.1 and
48 kHz captures (including small speed drift) remain comparable. No raw PCM is
stored. The V10 projection adds generic loader evidence without replacing V9
filename, addresses, block flags, or CRC state.

CDT/TZX parsing can provide logical CPC block identity and, for supported
container blocks, timing metadata, but the current parser does not expose every
custom pulse/direct-recording/generalized-data detail through the same bounded
interval API. V10 therefore compares standard block metadata only and does not
claim byte-identical custom-stage parity; that bridge is a later, separately
scoped task.

Named CPC loader recognition is deliberately deferred. The repository audit did
not identify a sufficiently reliable, non-game-specific combination of bootstrap,
sync, timing, and stage-order clues with a near-miss corpus for a fail-closed
family label. Future V11 research may evaluate families only with such evidence;
V10 emits no commercial loader names. Spectrum and C64 custom paths remain
independent, and no emulator execution, DAT change, extraction, rename, or
copyrighted fixture is involved.

## V13: MSX standard cassette WAV evidence

V13 adds a narrow MSX BIOS-cassette observer on the shared PCM edge stream.
Standard 1200-baud FSK uses approximately 1200 Hz for zero and 2400 Hz for
one, with an MSX header marker (`1f a6 de ba cc 13 7d 74`) required before a
record is surfaced. Generic KCS/BBC/Spectrum/C64/CPC-like tones therefore do
not become MSX merely by frequency coincidence. The result is bounded,
read-only, confidence-bearing metadata; MSX CAS is a separate decoded-byte
container and has no parser in the current repository. Turbo loaders and named
families remain deferred to V14.
## V11: BBC Micro standard cassette WAV evidence

V11 adds a conservative BBC Micro/Acorn standard-cassette projection on the
shared PCM edge stream. It recognises the documented 1200-baud CUTS/KCS-style
carrier (approximately 1200 Hz and 2400 Hz), 8N1 framing, and the `0x2a`
standard block synchronisation byte. Header metadata is decoded only when the
filename terminator, little-endian load/exec addresses, block number, length,
flags, and big-endian BBC CRC-16 are structurally present. Carrier similarity
alone is never promoted to BBC evidence; missing framing or an invalid header
CRC remains an explicit refusal/medium-confidence result.

The bounded result retains filename, addresses, block sequence metadata,
continuation/final state, checksum state, confidence, and audio sample/time
provenance. It does not retain PCM or payload bytes in the shared analysis
projection. Multiple blocks are recovered independently, so a valid standard
bootstrap followed by an unknown timing stage remains BBC-anchored without
claiming a custom BBC loader. Fastloaders and named loader families are
deliberately deferred.

UEF is a separate gzip-wrapped chunked container and is not treated as raw WAV;
the repository currently has no UEF parser. A future UEF bridge can compare
the same header fields and block CRCs, but V11 does not invent one or conflate
container metadata with waveform evidence. All fixtures are synthetic and the
decoder performs no extraction, launch, DAT, or rename work.

## V12: bootstrap-gated BBC custom stages

V12 reuses the generic bounded custom-stage analyser only after at least one
standard BBC block has a valid header CRC. Post-bootstrap timing families are
classified conservatively as `GenericTurbo`, `CustomPulse`, `MultiStage`, or
`UnknownCustom`, with normalized fingerprints and stage-local provenance. A
weak carrier match, invalid/truncated BBC header, Spectrum/C64/CPC waveform, or
random FSK cannot unlock BBC custom labeling. Named BBC loaders and fastloader
semantics remain deferred.
