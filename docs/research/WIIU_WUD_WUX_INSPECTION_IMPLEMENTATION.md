# Wii U WUD/WUX inspection implementation

This feature adds bounded, read-only structural inspection for Wii U WUD and
WUX sources. It does not decrypt content, extract titles, convert formats,
rebuild images, or mutate source files.

## WUD

Raw WUD files are recognized conservatively. EmuWiz records the physical size,
the selected source, and any proven split-part information, but does not guess
at encrypted inner partitions or title identity. Container inspection does not
need keys; deeper content inspection remains explicitly unavailable without a
separate, supported key workflow.

Split WUD naming is used only to discover adjacent candidate parts. The
inspector reports missing and duplicate numbered parts and does not treat a
filename as a cryptographic identity.

## WUX

The inspector reads only the bounded WUX header and block table. It validates
the WUX magic, sector size, checked logical-size/block-count arithmetic, a
16 MiB maximum table allocation, table bounds, aligned data-array location,
and every referenced physical block range. It calculates logical size and
records the table and sector-array locations without decompressing blocks.

Truncated headers/tables, invalid magic, absurd counts, integer overflow, and
out-of-container block references are structural failures. No full multi-GB
image hash is calculated automatically.

WUA is reported as a separate unsupported representation; it is never
presented as WUD or WUX.

## Evidence and key state

The inspection report preserves structural provenance and typed key state. A
container-level report does not expose key bytes or promote filenames,
partition labels, or emulator boot success to verified identity. Exact hashes
remain an explicit, separately requested operation; structural evidence is
reported as structural evidence.

## GUI-v2

Selected Wii U WUD/WUX/WUA media now has a read-only “Wii U disc container”
section showing format, physical/logical size, structural status, split parts,
key state, readiness, WUX table facts, and typed issues. The section states
that source files remain unchanged and exposes no conversion or Apply button.

## Validation boundary

Synthetic fixtures cover valid WUD/WUX recognition, no source mutation,
truncated input, absurd logical sizes, out-of-range WUX references, and missing
split parts. The implementation intentionally does not claim title extraction,
decryption, WUD reconstruction, WUX decompression, WUA equivalence, or format
conversion.
