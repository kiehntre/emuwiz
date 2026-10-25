# ROM patch representation guidance

In **Cheats & Mods → Mods & ROM Hacks → Standalone ROM patches**, choose a
local patch. The existing inspection worker reads the selected source and patch;
Patch Preview shows its source size, CRC32 and SHA-256 snapshot, patch input
requirements, representation explanation and cached DAT context. Confirm the
shown source, transformation and new output before **Create derived patched ROM**.
No inspection or conversion suggestion changes a ROM.

## Evidence and supported scope

- Existing IPS, BPS, UPS, VCDIFF and PPF inspectors remain unchanged. Detailed
  representation comparison needs an embedded source CRC32 (normally BPS/UPS).
  Source-hash-free formats remain Unknown in this preview unless an existing
  separate workflow supplies its required authority. A title is not authority.
- A matching CRC32/size is probable source compatibility, not cryptographic game
  identity, publisher authenticity or proof that every patch instruction is valid.
  The existing decoder and output checks still run before publication. Bad copy
  offsets can therefore refuse at Apply even when source checks pass.
- Reused diagnostics explain SNES 512-byte copier headers, NES iNES headers,
  N64 byte order and Genesis SMD layouts only when the comparison supports the
  suggested normalized view. SNES context never borrows an NES header rule.
  Unknown layouts, missing headers, unsupported reverse transforms and unmatched
  normalized content do not acquire guessed bytes or offsets.
- Normalized checksum agreement is advisory. It never authorizes a new Apply or
  conversion route. Detailed normalization is bounded to 64 MiB; larger files
  can retain the existing raw source checks without guessed normalization.
- The existing explicit SNES copier-header patch-input adjustment remains
  available when its existing size/CRC checks succeed. Preview names it and
  requires confirmation; only the derived patch input changes. This is separate
  from the conversion engine, which has no enabled cartridge-normalization route
  for these suggestions. Other normalization advice stays planning-only.

## DAT and stale evidence

The panel reuses the library's cached DAT summaries and provenance, not a new
catalogue store or a new audit. Missing, unknown and stale summaries stay distinct.
Historical results are retained. A normalized DAT match can explain layout without
changing the canonical DAT verdict or granting patch applicability. Available
hashes must agree on the same view; a CRC32 match cannot hide a stronger conflict.
Contradictory or unconfirmable current evidence prevents a new patch plan.

Changing the cached identity/DAT snapshot invalidates confirmation and the old
preview's Apply eligibility. Any already-running operation and its result remain
tracked. Recheck identity and select the patch again. Existing source and patch
SHA-256 revalidation at Apply, output no-overwrite, provenance, recovery and mod
Undo contracts remain in force. The guidance never grants extra Undo rights or
reclassifies a modified ROM as a trusted DAT original.
