# PS2 CodeBreaker → PCSX2 PNACH (preview only)

Module: `crates/archivefs-core/src/patch_manager/ps2_codebreaker.rs`
(`preview_ps2_codebreaker`). Date: 2026-10-25, starting `origin/main` `fdff2a86`.

This is a **preview**. It writes no file, enables no cheat and talks to no
provider. A `Translatable` result carries a `ManagedPnachCheat` and the exact
PNACH bytes that a fresh file would contain; installing it would still go
through the existing PCSX2 install plan, which has its own verified-CRC,
backup, journal and rollback rules. No GUI action is added in this change.

## Decisions implemented

| Area | Behaviour |
| --- | --- |
| Version | Chosen by the user: `V1ToV6`, `V7WithSetupLine`, `V7CommonKey`. Never guessed, never fallen back from one to another. |
| Input | At most 16 KiB and 512 lines, ASCII only, one `XXXXXXXX YYYYYYYY` pair per line. Titles, comments and mixed text are refused before any decryption. |
| Translated | Decrypted types `0`/`1`/`2` (8/16/32-bit constant write) in EE RAM (below 32 MiB), naturally aligned, value inside its width. Output is the existing IR encoder's `patch=1,EE,<addr>,<width>,<value>`, re-validated by `PnachPatchLine::parse`. |
| Excluded with acknowledgement | A positively identified type `9` hook (one line, 25-bit address): kept verbatim, left out of the PNACH, and only after the caller passes the exact hook line numbers. See "Master codes". |
| Refused | Increment/decrement, slide, copy, pointer, boolean, conditional (`C`, `D`, `E`), `F` and other control types, unknown types, a `9` with address bits above 25; out-of-RAM, unaligned or non-canonical writes; conflicting writes to one address; a cut-off multi-line code (refused at its head record); every key sequence that is not validated (below). |
| All-or-nothing | One refused record refuses the set. Keeping the writes behind a dropped conditional or master code would change behaviour. |
| Identity | Requires the selected game's verified PCSX2 CRC. CodeBreaker lines carry no game identity, so the association is the user's assertion, checked against the verified serial/CRC and any declared source game. Filename via the existing `pcsx2_pnach_filename`. |
| Preservation | Every record keeps the pasted line, its encrypted pair, its decrypted pair and a role or typed refusal. The input SHA-256 and decoder/crate versions are recorded. |

## Why `patch=1`

A CodeBreaker re-applies its codes continuously. The shared IR encoder already
emits PNACH place `1` for direct writes. PCSX2's own `extended` handler decodes
raw type `0`/`1`/`2` codes as writes to `addr & 0x0FFFFFFF`
(`pcsx2/Patch.cpp`, `handle_extended_t`), the same meaning used here.

## Master codes (type 9 hooks)

Question: can a master/enable code be left out when converting to PNACH?
Answer: **only a positively identified type `9` hook, and only with an
explicit acknowledgement.** Everything else stays a whole-list refusal.

| Code | What it is | Evidence | Decision |
| --- | --- | --- | --- |
| `9aaaaaaa vvvvvvvv` | Hook: "hard-codes a jal to the cheat engine at address @a if the 32-bit value at @a is equal to value @v"; "essential for most of the other types to work" | ps2rd `Documentation/code_types.txt` (the CodeBreaker-compatible type list by the author of the cipher crate). PCSX2's official patch documentation lists `9aaaaaaa` as "Not supported" among RAW codes. PCSX2 `Patch.cpp` `handle_extended_t` has no type-9 branch; PCSX2 applies `patch=` lines itself at place `1` ("on every vsync"), with no engine to hook. A community guide (SlyMods) states 9-type codes "are not needed for PCSX2" because cheats live in the host. | Exclude with acknowledgement |
| `Caaaaaaa vvvvvvvv` | "All following codes will be executed only if" the value matches: a gate over later codes | ps2rd docs; PCSX2 docs "Not supported" | **Refuse.** Dropping it would make gated writes unconditional. |
| `D`/`E` | Multi-line conditionals | ps2rd docs; PCSX2 implements them as `extended` | Refuse (not direct writes) |
| `F0100008 ...` and other `F` forms | CodeBreaker entry-point/master variants; not in the ps2rd list (`F - unused`) | Only forum posts; no primary specification found | **Refuse** as unknown |
| `3`-`7` | Increment, slide, copy, pointer, boolean | ps2rd docs | Refuse |

What the exclusion loses, which is why it needs acknowledgement: the hook's
equality test is a guard that the running game matches the list, and the device
writes from inside a hooked game function rather than once per vsync. After
exclusion the selected game's verified CRC is the only guard. The acknowledgement is the exact set of
hook line numbers: any other set (fewer, more, other lines) returns
`AcknowledgementRequired` and no PNACH text. A hook beside any refused record
(conditional, pointer, `C`, `F`...) still refuses the list, and acknowledging
never overrides a refusal. The hook stays in the records with its pasted line,
encrypted and decrypted pair.

Vectors: the hook and write examples are the ps2rd-documented raw codes
(`902D51F8 0C0B95F6`, `002BAA31 00000063`, `107657B2 0000FFFF`,
`20417A64 42C80000`) encrypted with cb2util v1.9 for V1, V7 setup-line and V7
common key, plus conditional, pointer, `C`, `F`, bit-25 and hook-only cases.

Limits of the evidence: no complete real-world list with a known-good PCSX2
conversion was found (the GameHacking.org list pages were not retrievable), so
equivalence rests on the documented mechanism of the hook, PCSX2's source and
documentation, and the cipher validation, not on list-level vectors. The
address field of types `0`-`9` is 25 bits (`ps2rd`); this module additionally
requires the bits above it to be clear.

## Fidelity of the direct-write mapping

Checked against PCSX2 `pcsx2/Patch.cpp` / `Patch.h` (master, 2026-10-25):

| Property | CodeBreaker raw code | PNACH line emitted | PCSX2 behaviour |
| --- | --- | --- | --- |
| Width | command nibble `0`/`1`/`2` = 8/16/32-bit | `byte`/`short`/`word` | `ApplyPatch` calls `IdempotentWrite8/16/32`; `extended` does the same widths |
| Address | low 28 bits | 8 uppercase hex digits, required `< 0x02000000` | `patch=` writes at the literal address; `extended` masks with `0x0FFFFFFF`; EE RAM is 32 MiB |
| Alignment | device writes at the address | required naturally aligned (2/4 bytes) | not enforced by PCSX2, refused here to avoid ambiguous hardware behaviour |
| Value | `vv` / `vvvv` / `vvvvvvvv` | fixed-width uppercase hex (2/4/8 digits), no bits above the width | hex, no prefix, truncated to the type |
| Timing | re-applied continuously | place `1` = "continuously (on every vsync)" | `PPT_CONTINUOUSLY` |
| CPU | EE | `EE` | `CPU_EE` |

`every_accepted_direct_write_matches_the_pcsx2_effect_exactly` sweeps the three
commands over boundary addresses (including `0x01FFFFFC..F`, `0x02000000`,
`0x0FFFFFFC`) and boundary values, models both sides independently, and checks
the exact text for every accepted write and the refusal kind for every other.

## Dependency review: `codebreaker` 0.4.0

| Item | Finding |
| --- | --- |
| Licence | `MIT OR Apache-2.0` (crate metadata and both licence files in the repository). EmuWiz is MIT. |
| Provenance | Single author (Mathias Lafeldt). The README says it is a port of his 2006 C library `cb2util` (MIT, `cb2_crypto.c`), which was reverse-engineered from the CodeBreaker's MIPS R5900 code. |
| Dependencies | One: `bytemuck` 1.x (already `1.25.1` in `Cargo.lock`). `Cargo.lock` gains only the `codebreaker` entry. |
| Rust | Edition 2024, `no_std`, little-endian targets only. Builds on the repository toolchain (1.97). The crate declares no `rust-version`. |
| Pinning | Exact `=0.4.0`; checksum recorded in `Cargo.lock`. |
| Gates | `scripts/release/test_generate_sbom.py` passes; `THIRD_PARTY_NOTICES.md` updated. `cargo audit` runs in CI. |

## Independent validation

Vectors were generated with the original C implementation (`cb2util` v1.9,
commit `5fed07b`, MIT; `libbig_int` `fd8f327` for its RSA step) by encrypting
random plaintext streams, then decrypted with the crate and with the C code.
Three-way agreement (plaintext, C, crate) was required per line.

| Shape (streams of 4-63 lines) | Lines | Mismatches |
| --- | --- | --- |
| V1-V6, no key lines | 132,023 | **0** |
| V7 common key, no key lines | 132,023 | **0** |
| Leading `BEEFC0DE` setup line (random seed) then V7 | 133,491 | **0** |
| Common key, then a `BEEFC0DE` line | 4,000 streams | every stream disagrees after the key line |
| Streams with a second `BEEFC0DE` or any `BEEFC0DF` | mixed | disagreements in most streams |

Only the first three shapes are accepted. See
[`CODEBREAKER_RS_C_DISCREPANCY.md`](CODEBREAKER_RS_C_DISCREPANCY.md) for the
reproducible examples and the upstream investigation.

**Limit of the evidence.** The C library and the crate share an author and a
reverse-engineering effort, and no cipher vectors published by anyone else
were found. "Agrees with the original implementation" is the strongest
available proof, not independent proof that the device cipher is reproduced.
Wrong-version input is usually caught because it decrypts to non-direct-write,
out-of-RAM, unaligned or non-canonical records, but a single 32-bit record can
look plausible (about one in 500 for a random line), so decrypted values are
always shown for review.

## Not in scope

PS2 GameShark/Action Replay v1-v2, ARMAX, GBA and Dreamcast decryptors;
CodeBreaker types other than direct writes; any GUI, apply or file-writing
path; CB v8+ file formats (`cb2util` code saves); changes to the upstream
crate.
