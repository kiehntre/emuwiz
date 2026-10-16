# CHD hunk-size compatibility fixtures

Recovered byte-for-byte from historical `dcd7b1f5094c813a796adba958ccab8200a3bfb2`.
That candidate records these as MAME chdman 0.264 output for synthetic zero bytes,
using 512 KiB and 1 MiB hunks. They contain no ROM or user data.
`chd_hunk_size_limit.rs` reads each complete decoded hunk and checks its byte count
and all-zero contents. Header acceptance alone is not content verification.

- `hunk_1mib.chd`: 156 bytes, logical 8388608 bytes; SHA-256 `8869a9cb25fff2248ba3f99b2df6200babd2d8dcab82b83ddbc5cbec77414a01`.
- `hunk_512kib.chd`: 188 bytes, logical 8388608 bytes; SHA-256 `dffdf9d5d087d6bba1b70c3b880aba70db1fafd8f51ea9efe2bb18459728377e`.
