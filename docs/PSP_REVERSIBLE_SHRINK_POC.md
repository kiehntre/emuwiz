# PSP reversible shrink POC

This proof converts a PSP ISO to standard CSO v1 and reports `ReversibleVerified` only after all of these steps succeed:

1. The ISO is opened read-only, checked for a non-zero 2048-byte-aligned size, and SHA-256 hashed.
2. The ISO is streamed into independently zlib-compressed CSO v1 blocks.
3. The generated CSO header, index table, offsets, compressed blocks, and decoded block sizes are checked.
4. The CSO is restored into a temporary ISO in the output directory.
5. The restored ISO is hashed and compared byte-for-byte with the original SHA-256.

The converter is in-process and uses a fixed argument-free codec boundary. It records `CSO v1`, a 2048-byte block size, zlib compression level 6, the POC tool/version marker, source and output paths, source/restored hashes, and the existing PSP identity/boot-evidence integration point. It does not trim languages, rewrite the source, replace an existing output, or delete either file. A collision is refused. A partial or failed output is never reported as trusted and is left for explicit user cleanup.

The current GUI surface is intentionally deferred from this isolated core POC because there is no existing non-conflicting converter panel to extend while the provider and managed-snapshot work proceeds. The intended panel contract is `Converter > Reversible Shrink > PSP ISO -> CSO`, showing original size, compressed size, saved space, CSO readability, restored-hash equality, and `REVERSIBLE VERIFIED`, with `Keep both`, `Keep compressed copy`, and `Cancel`. `Delete Original` is deliberately absent. The core result is sufficient for that panel to remain a presentation layer over a verified operation receipt.

The PSP boot evidence module remains the identity integration point. The shrink operation preserves source hash and conservative PSP ISO evidence in provenance; it does not infer a game identity from compression or permit metadata to replace identity evidence.
