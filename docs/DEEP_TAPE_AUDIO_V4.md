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
