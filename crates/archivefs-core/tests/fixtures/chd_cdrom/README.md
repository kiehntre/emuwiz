# CHD CD-ROM decoder fixtures (synthetic)

Generated with `chdman createcd -c <codec>` (MAME 0.264) from a 16-frame
synthetic MODE1_RAW image: sector `i` has MSF `00:02:i`, mode 1, and user bytes
`(i*31 + j*7) % 251`. No real ROM or disc content. Hunk size 19584 (8 frames),
2 hunks, 1 ECC flag byte per hunk.

- `cdzs.chd`, `cdlz.chd`, `cdzl.chd`: unmodified valid files.
- `cdzs-declared-length-overrun.chd`: `cdzs.chd` with bytes 230..232 (hunk 0's
  big-endian compressed-sector-stream length) set to `0xFFFF`. Header, map,
  metadata and file length are untouched, so it opens normally; the hunk
  CRC16 is only checked after decoding. Before the fix this panicked in
  `CdCodec::decompress` (`input[header_bytes..][..0xFFFF]`, cdrom.rs:203).
