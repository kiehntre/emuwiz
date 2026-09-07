# Atari 8-bit standard cassette waveform V16

V16 recognises the standard Atari 400/800 and XL/XE cassette record format
from PCM WAV. It is deliberately limited to the ROM/SIO standard and does not
identify or decode turbo loaders or named commercial formats.

## Evidence contract

The decoder uses the documented standard signal and record structure:

- 600-baud asynchronous FSK, 8N1, least-significant bit first;
- 3995 Hz space/zero and 5327 Hz mark/one;
- two `0x55` speed-marker bytes;
- control byte `0xFC` (full), `0xFA` (partial), or `0xFE` (EOF);
- 128 padded data bytes and one end-around-carry checksum.

For a partial record, the final data byte before the checksum is the number of
user bytes (1 through 127). An EOF record has zero-filled data. The recovery
model retains control type, payload length, checksum state, sequence and
sample/time bounds. It does not invent filenames, load addresses, or execution
addresses: standard cassette records do not carry those fields in this layer.

The waveform path calibrates around the nominal baud rate and accepts only a
bounded local drift window. A valid marker/control frame is required before
Atari evidence is emitted; generic FSK, or a carrier with no valid record
framing, remains unrecognised. Invalid checksums and truncated records are
reported as evidence with warnings and never upgraded to valid data.

## CAS relationship

The A8CAS container is a separate decoded-record representation. Its `data`
chunks carry standard SIO records and an inter-record-gap value; a `baud` chunk
can declare the rate. Its `fsk ` chunks carry raw non-standard pulse lengths,
while PWM/turbo chunks describe other timing families. V16 does not parse CAS
or interpret those custom chunks. A later bridge can feed standard `data`
chunks through the same record validator and retain the declared gap/baud as
provenance without treating raw `fsk ` or turbo chunks as standard Atari.

## Later custom stages

When a valid standard record anchor is followed by materially non-standard
pulse timing, V16 records a deferred custom-stage candidate. It does not label
the stage, decode it, or infer a loader name. That evidence is reserved for a
future bootstrap-gated custom/turbo lane.

## References

The implementation was checked against the Atari reference material and
emulator sources, including:

- [De Re Atari, Appendix C](https://www.atariarchives.org/dere/chaptC.php)
- [Altirra Hardware Reference Manual](https://www.atari800xl.eu/docs/reference/altirra-hardware-reference-manual.pdf)
- [A8CAS format](https://a8cas.sourceforge.net/format-cas.html)
- [Atari800 tape image source](https://sources.debian.org/src/atari800/4.1.0-3/src/img_tape.c)

No copyrighted tape image is used by the tests; fixtures synthesize short
standard records in memory.
