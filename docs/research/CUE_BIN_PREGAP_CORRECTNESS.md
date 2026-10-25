# CUE/BIN INDEX 00 and PREGAP correctness

## Reference semantics

- CUE MSF values are `MM:SS:FF`, with 75 frames per second. The frame value
  must be below 75 and the seconds value below 60.
- `INDEX 00` is an actual, file-relative source position for the track
  pregap. It can contain audio or data and must not be replaced by silence.
- `INDEX 01` is the file-relative start of the track program data. Identity
  extraction starts here.
- `PREGAP` declares synthetic pregap sectors that are not stored in the
  referenced file. It therefore does not shift the file byte offset of
  `INDEX 01`.
- `POSTGAP` is synthetic trailing gap metadata and is not included in a
  referenced file's byte range.
- For a track in a shared file, its data range ends at the next track's
  `INDEX 00` when present, otherwise the next `INDEX 01`; for the final track
  it ends at the referenced file's checked length.

References:

- libyal's [CUE sheet format documentation](https://github.com/libyal/libodraw/blob/main/documentation/CUE%20sheet%20format.asciidoc)
- GNU ccd2cue's [CUE sheet format appendix](https://www.gnu.org/software/ccd2cue/manual/html_node/CUE-sheet-format.html)
- libcdio's [pregap discussion](https://fossies.org/linux/libcdio/doc/libcdio.texi)

## EmuWiz invariants

The parser keeps checked frame offsets and distinguishes `CuePregap::InFile`
from `CuePregap::Synthetic`. All source ranges are checked with overflowing
arithmetic before a read. Raw and cooked logical-media adapters expose only a
bounded sector range, so CUE/BIN identity hashing cannot include an INDEX 00
pregap or audio track by accident. CUE/BIN and source files are read-only.

Unsupported or malformed layouts fail closed, including missing INDEX 01,
decreasing INDEX 00/01, out-of-file positions, partial sectors, unsafe file
references, and unsupported MODE2 identity extraction.
