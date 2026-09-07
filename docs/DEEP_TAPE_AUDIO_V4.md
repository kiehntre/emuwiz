# Deep Tape Analysis V4: PCM/WAV evidence

V4 accepts bounded RIFF/WAVE PCM integer recordings (8-bit unsigned or 16-bit
little-endian, up to eight channels and 192 kHz). Stereo is safely downmixed,
DC offset is estimated and removed for quality/thresholding, and a hysteretic
adaptive edge detector emits bounded sample/time pulse edges. Results retain
only timing summaries and quality metrics, never PCM bytes.

This is generic pulse evidence for future cassette decoders. It does not yet
demodulate WAV into Spectrum blocks, decode compressed audio, emulate loaders,
or interpret Commodore/Atari/MSX cassette signals. Unsupported codecs and
malformed/truncated chunks fail closed.
