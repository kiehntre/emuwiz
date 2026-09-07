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
