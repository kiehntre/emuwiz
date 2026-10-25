# N64 CIC real-retail fingerprint corpus

> **Research snapshot** — This manifest records research/reference data and is not current capability documentation. See the [README](../../README.md), [adapter support matrix](../ADAPTER_SUPPORT_MATRIX.md), and [roadmap](../../ROADMAP.md) for present guidance.

This manifest records legally distributable fingerprint metadata rather than
copyrighted ROM or IPL3 bytes. The CRC32 and MD5 values are published by the
CC0-licensed `n64checksum` research/reference implementation and agree with
the independently maintained `z64decompress` lookup table.

| IPL3/CIC family | IPL3 CRC32 | IPL3 MD5 | Byte-order evidence | Full-ROM CRC1/CRC2 |
| --- | --- | --- | --- | --- |
| CIC-NUS-6101 | `6170A4A1` | `900B4A5B68EDB71F4C7ED52ACD814FC5` | fingerprint metadata only | not validated: no full ROM included |
| CIC-NUS-6102 / 7101 | `90BB6CB5` | `E24DD796B2FA16511521139D28C8356B` | fingerprint metadata only | not validated: no full ROM included |
| CIC-NUS-6103 / 7103 | `0B050EE0` | `319038097346E12C26C3C21B56F86F23` | fingerprint metadata only | not validated: no full ROM included |
| CIC-NUS-6105 / 7105 | `98BC2C86` | `FF22A296E55D34AB0A077DC2BA5F5796` | fingerprint metadata only | not validated: no full ROM included |
| CIC-NUS-6106 / 7106 | `ACC8580A` | `6460387749AC0BD925AA5430BC7864FE` | fingerprint metadata only | not validated: no full ROM included |

The region-paired rows are intentionally not split: the published bootcode
fingerprint does not distinguish the paired PAL/NTSC CIC labels. No 6107,
6104, 5101, 8303, dynamic, or Xplorer64 mapping is added here.

The checked-in tests validate the published fingerprint table and all negative
cases. Byte-swapped normalization and CRC1/CRC2 computation remain covered by
the existing synthetic/algorithmic tests; this manifest does not claim a real
full-ROM CRC validation.

Sources:

- <https://github.com/Dragorn421/n64checksum/blob/main/README.md>
- <https://github.com/z64dev/z64decompress/blob/main/src/n64crc.c>
- <https://github.com/Decompollaborate/ipl3checksum>
