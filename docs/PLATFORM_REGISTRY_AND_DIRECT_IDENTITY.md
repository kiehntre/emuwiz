# Platform registry and direct identity

## CURRENT BEHAVIOR

Platform classification and game identity are layered:

1. media recognition identifies a plausible container/format;
2. structural parsing extracts format-specific evidence;
3. platform evidence selects or narrows an IdentityPlatform;
4. verified identity records facts proven by bounded inspection;
5. DAT/hash matching supplies exact release authority where available;
6. persistence stores evidence and freshness, not unconditional trust;
7. launch/library/cheat projections consume the result.

The canonical platform registry and current aliases are the live registry, not
an exhaustive list in this document. Folder and filename evidence can classify
a platform or produce a candidate, but weak evidence must not become verified
game identity.

## Direct identity examples

- PS2 ISO/CHD routes through PS2-specific structural readers where the format
  is supported; serial and executable CRC are separate verified facts.
- NGP/NGPC header evidence can choose between closely related platforms.
- Dreamcast CHD uses specialist routing and format evidence rather than a
  generic extension claim.
- Atari and C64/tape formats retain ambiguity when media bytes do not prove a
  single platform.
- Virtual Boy media can be recognized without claiming a verified game.
- Archive members and equivalent loose files use the same evidence rules;
  an outer archive name does not outrank the member's evidence.

Other current observers cover many cartridge, disc, tape, and executable
formats. Consult the current registry and support docs rather than copying a
platform inventory here.

## Identity safety

Platform compatibility is not identity. A launch row, emulator preference, or
folder alias cannot supply a missing game ID. Conflicting, stale, incomplete,
or ambiguous evidence remains visibly unresolved and downstream consumers
fail closed.

## HISTORICAL DESIGN CONTEXT

Earlier platform tables and direct-identity notes sometimes listed exact
counts or treated extensions as authority. Those are historical snapshots and
were intentionally replaced by the layered registry/evidence model.
