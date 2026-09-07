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
