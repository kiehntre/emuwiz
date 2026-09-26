# Dreamcast DCP patch readiness

Status: read-only inspection and preview readiness only. EmuWiz does not apply
DCP packages, rebuild a GD-ROM, rewrite IP.BIN, mutate CDI/CHD/GDI sources, or
add an Apply button.

## Research findings

Universal Dreamcast Patcher documents DCP as its own patch format. The manual
form is a ZIP whose root contains changed or new filesystem files; an optional
`bootsector/IP.BIN` member is consumed as boot metadata rather than copied
into the ISO9660 filesystem. The automatic builder can use xdelta-style file
deltas. The package itself does not provide a cryptographic source identity;
the documented older implementation explicitly says file hashes are not used
for pre/post verification.

The current project documentation also states that DCP does not modify CDDA
tracks. Applying it still extracts and rebuilds the data track, so filesystem
extent ordering, padding, timestamps, IP.BIN, and output topology require
post-build verification. A title or product code alone is not a DCP target
binding.

Sources:

- [Universal Dreamcast Patcher repository](https://github.com/DerekPascarella/UniversalDreamcastPatcher)
- [documented DCP package layout and limitations](https://raw.githubusercontent.com/DerekPascarella/UniversalDreamcastPatcher/main/README.md)
- [current project/fork documentation](https://github.com/sega-dreamcast/universal-dreamcast-patcher)
- [independent DCP format reference](https://rom-weaver.com/docs/references)

The repository reports GPL-3.0 for the current Universal Dreamcast Patcher and
MIT/Apache/BSD licenses for several bundled third-party components. EmuWiz
does not vendor or execute that tool. Its implementation details are evidence
for the readiness boundary, not a new EmuWiz dependency.

## Package model

`archivefs_core::dreamcast_patch_readiness` inspects a DCP as a bounded ZIP:

- filesystem members are classified as replacements or recorded deltas;
- `bootsector/IP.BIN` is classified separately and never treated as a
  filesystem file;
- metadata/readme members are inert metadata;
- unsafe paths, symlinks at the package root, malformed ZIPs, excessive entry
  counts, and excessive expanded sizes fail closed; and
- every entry and the package receive hashes without extracting or applying
  anything.

The package records `source_hashes_embedded: false` for standard DCP. A trusted
catalogue/provider may supply a separate target claim containing an exact full
image hash, exact data-track hash, or a cryptographically tied product code.
The readiness evaluator uses that precedence and never promotes a filename,
title, region, or ordinary product-code hint to an exact match.

## Readiness and refusal policy

An exact external target binding produces `PossiblyReady`, not production
Apply-ready, when the package contains filesystem changes: the data track must
be rebuilt and its layout must be verified afterward. An IP.BIN member adds an
explicit IP.BIN impact and warns that region, VGA, boot filename, and related
fields need review. A package with no exact binding is `NotReady` even when its
name looks correct.

IPS/BPS/UPS/PPF/VCDIFF are classified through the existing bounded standalone
patch inspector, but are `Unsupported` for Dreamcast readiness because their
target semantics are not DCP filesystem/GD-ROM semantics. CDI is explicitly
unsupported for this workflow. CHD is not guessed equivalent to GDI; a CHD
source requires a complete, verified representation contract. Mixed-mode
sources retain audio as an independent invariant; DCP has no CDDA modification
operation, but an eventual rebuilt output must still prove audio hashes and
track topology unchanged.

EmuWiz therefore refuses:

- missing, weak, or conflicting target identity;
- malformed or unsafe packages;
- opaque standalone patch semantics;
- unproven filesystem/layout movement;
- unreviewed IP.BIN changes;
- CDI input/output claims; and
- any Apply or source mutation.

## Current EmuWiz gaps

Existing Dreamcast identity and IP.BIN inspection provide native product-code,
hardware, region, peripheral, VGA, and boot metadata evidence. GDI parsing
provides bounded data-track and all-track resolution; CDI and GD-ROM CHD
specialists provide identity/read-only routing. Existing standalone patch and
package inspectors provide bounded patch framing and ZIP safety.

The missing production primitives are a Dreamcast-aware DCP filesystem
extract/rebuild implementation, exact source-to-package binding metadata,
sector/extent/topology comparison after rebuild, deterministic output
verification, and a transaction/provenance-backed publisher. None is added by
this task.

## Tests

Synthetic tests cover exact target binding, no-hash DCP refusal, IP.BIN and
filesystem classification, source immutability, and traversal refusal. No
copyrighted disc, patch, or BIOS bytes are used.
