# Cheat-format semantic evidence (V5)

**Decision status:** research complete; V6 adds a source-only DS AR parser and
classifier, but no encoder, installer, or GUI change.  The neutral IR remains deliberately limited
to proven direct writes.  In particular, a familiar brand name is never enough
to identify a cheat grammar.

## Evidence standard and repository baseline

The authoritative local implementation is
`crates/archivefs-core/src/patch_manager/cheat_ir.rs`.  At this revision it
has `Write8`, `Write16`, and `Write32`, parses the proven GameCube/Wii
Dolphin/Gecko and PS2 PNACH subsets, and intentionally reports DS Action
Replay, PS2 GameShark, PS2 CodeBreaker, and RetroArch expressions as opaque.
The V4 converter service (`convert_cheat_document`) provides preview-only
capability results; it does not install a converted file.

Evidence below is labelled as follows:

* **High** — current emulator source specifies the operation.
* **Medium** — an official emulator document establishes a format boundary but
  not a complete grammar.
* **Insufficient** — no version-specific, authoritative grammar was found;
  this is a refusal decision, not an invitation to infer one.

References are intentionally source or emulator documentation rather than
community conversion tables:

* [DeSmuME `CHEATS::ARparser`](https://github.com/TASEmulators/desmume/blob/master/desmume/src/cheatSystem.cpp)
  (the source itself points to GBATEK/Kodewerx as its specification inputs).
* [DeSmuME cheat documentation](https://wiki.desmume.org/index.php?title=Using_Cheats_in_DeSmuMe),
  which records Action Replay support after 0.9.2.
* [PCSX2 patch documentation](https://pcsx2.net/docs/advanced/writing-patches/),
  which defines PCSX2's supported PNACH workflow and cautions that fixed
  addresses are not universally appropriate.
* [RetroArch `cheat_manager.c`](https://github.com/libretro/RetroArch/blob/master/cheat_manager.c),
  the current writer/reader for `.cht` fields.

## Nintendo DS Action Replay

### Exact source-to-IR subset

DeSmuME's current `ARparser` interprets the high nibble of the first word.
It establishes this safe *source* subset:

| Opcode form | Meaning in the reference implementation | IR mapping | Conditions for exactness |
|---|---|---|---|
| `0XXXXXXX YYYYYYYY` | constant 32-bit write | `Write32 { address: XXXXXXX, value: YYYYYYYY }` | A standalone, unencrypted DS AR record containing only direct-write records; reject `00000000` because the reference treats it as a manual-hook special case. |
| `1XXXXXXX 0000YYYY` | constant 16-bit write | `Write16 { address: XXXXXXX, value: YYYY }` | Same standalone/direct-only requirement; reject non-zero unused high value bits rather than normalising them. |
| `2XXXXXXX 000000YY` | constant 8-bit write | `Write8 { address: XXXXXXX, value: YY }` | Same standalone/direct-only requirement; reject non-zero unused high value bits. |

`XXXXXXX` is the low 28 bits of the first word.  The reference's direct
writes add its mutable offset register, so the subset is exact only when no
earlier record can set or load that offset.  Requiring a complete record made
solely of the canonical `0`, `1`, and `2` forms proves that offset is its
initial zero value.  Alignment should also be checked for 16- and 32-bit
writes before an eventual parser accepts a record.

This is now implemented as a **High** confidence source-to-IR classifier in
`cheat_ir.rs`. It is **not** an approved target encoder: neither a melonDS nor
a DeSmuME native direct-write file/install contract is presently implemented or
reviewed in EmuWiz.

### DS families deliberately not reduced to writes

| Family / mask | Semantic effect | Stateful or control-flow? | V5 action |
|---|---|---:|---|
| `3`–`6` | 32-bit comparisons | yes | preserve unsupported |
| `7`–`A` | masked 16-bit comparisons | yes | preserve unsupported |
| `B` | loads the offset from memory | yes, pointer-like | preserve unsupported |
| `C0`, `D1`, `D2` | loop / next / full terminator | yes | preserve unsupported |
| `C4`, `C5`, `C6` | code rewriting, counter condition, offset store | yes | preserve unsupported |
| `D0` | conditional terminator | yes | preserve unsupported |
| `D3`, `DC` | set/add offset | yes | preserve unsupported |
| `D4`, `D5`, `D6`–`DB` | data register and incrementing writes/loads | yes | preserve unsupported |
| block/copy/fill and other control forms | multi-byte or execution-dependent effects | yes | preserve unsupported |
| master/init, button/joker, encrypted or obfuscated entries | device/version state is required | yes | preserve unsupported |

The current CheatBase integration retains Nintendo DS **Action Replay DS**
records (`cheatCode`) for browse-only use.  It does not give each record a
validated AR firmware/version or encryption-state marker, so its raw text
cannot be auto-promoted into the subset above without a parser that validates
the complete record.

### DS target-writer audit

| Target | Current EmuWiz state | Safe V5 direct writer? | Missing proof |
|---|---|---:|---|
| melonDS | local inventory/readiness only | No | reviewed native cheat-file schema, record association, and transactional installer |
| DeSmuME | adapter/launch support; no native cheat installer | No | reviewed `.dct`/native persistence contract and exact record ownership |
| RetroArch DS core | `.cht` install path retains native code bodies | No | core-specific handler/address/width context; see RetroArch section |

## PlayStation 2 GameShark

`GameShark` is a product brand spanning device generations.  The current
repository contains no PS2 GameShark parser, fixture corpus, device-version
field, decryptor, master-code model, or native writer.  PCSX2's supported
format is PNACH; its documentation does not define GameShark device-code
opcodes.  Legacy PCSX2 conversion guidance also treats CodeBreaker/GameShark
input as something that must first be converted to raw form, not as PNACH
syntax.

| PS2 family | Plaintext direct-write grammar proven here? | Encryption/master state | Parse to IR / encode preview |
|---|---:|---|---|
| GameShark / GameShark2-labelled input, version unspecified | No | unknown | Unsupported |
| GameShark device-entry/export format, version explicitly identified | No current authoritative fixture/source | potentially version-dependent | Unsupported pending evidence |
| decrypted/raw PS2 code supplied as PNACH | Yes, PNACH only | none in PNACH representation | Existing exact `byte`/`short`/`word` subset |

No claim is made that a first-nibble direct write is portable across PS2
GameShark versions.  Therefore master codes, joker/activator, condition,
pointer, slide/multi-write, copy/fill, and encrypted code families all remain
opaque.

## PlayStation 2 CodeBreaker

The same boundary applies to CodeBreaker.  Product/device versions and
readable-looking hexadecimal lines do not establish that the code is
plaintext, which cipher revision applies, or whether a master/init sequence
is needed.

| PS2 family | Plaintext direct-write grammar proven here? | Encryption/master state | Parse to IR / encode preview |
|---|---:|---|---|
| CodeBreaker-labelled input, version unspecified | No | unknown | Unsupported |
| CodeBreaker device-entry/export format, version explicitly identified | No current authoritative fixture/source | version-dependent | Unsupported pending evidence |
| decrypted/raw PS2 code supplied as PNACH | Yes, PNACH only | none in PNACH representation | Existing exact `byte`/`short`/`word` subset |

### PNACH bridge decision

| Path | Decision | Reason |
|---|---|---|
| PNACH → IR → GameShark | **Unsafe/Unsupported** | PNACH direct writes are known, but no versioned GameShark encoder or encryption/master-state contract is proven. |
| PNACH → IR → CodeBreaker | **Unsafe/Unsupported** | Same: a readable PS2 word is not evidence of a particular CodeBreaker device encoding. |
| GameShark/CodeBreaker → IR → PNACH | **Unsafe/Unsupported** unless the source is independently decrypted and identified as raw PNACH-equivalent | EmuWiz must not perform or guess decryption or discard device control codes. |

## RetroArch `.cht`

The earlier blanket conclusion that `.cht` contains only opaque expressions
was too broad, but it does not make generic conversion safe.  Current
RetroArch source persists both:

* opaque `cheatN_code` strings, which may use a core handler grammar; and
* structured manager fields such as `cheatN_handler`, `cheatN_cheat_type`,
  `cheatN_address`, `cheatN_value`, endian flag, and repeat fields.

Those fields describe RetroArch's running cheat-manager state, not a
platform-independent guarantee.  Handler, cheat type, memory region, endian,
repeat and core context determine what an address/value pair means.  EmuWiz's
current `cht_document` deliberately retains `code`, description, enable flag,
and opaque extra fields; it does not bind a `.cht` record to a verified core,
memory map, handler enum, or width semantics.

| `.cht` case | Safe V5 conversion? | Reason |
|---|---:|---|
| `cheatN_code` only | No | expression grammar is handler/core dependent. |
| address/value fields without verified handler, core, memory region, width and endian | No | the numeric tuple is semantically incomplete. |
| all structured fields plus a reviewed handler/core-specific contract | Potential future seam | requires a target-specific parser and fixtures; it is not a generic RetroArch mapping. |

The existing `.cht` installer is still useful: it performs a full-fidelity,
bounded native-file parse/render.  It is not an IR encoder and must not be
replaced by one.

## Consolidated capability matrix

| Platform | Format | Family | Parse to IR | Encode from IR | Exact direct-write subset | Master/encryption state? | Current EmuWiz support | Recommended next step |
|---|---|---|---|---|---|---|---|---|
| GameCube/Wii | Dolphin Action Replay | reviewed Dolphin lines | Yes | Yes | `00`/`02`/`04`, bounded address | rejected outside subset | direct-write preview/conversion | complete current boundary |
| GameCube/Wii | Gecko | reviewed Dolphin lines | Yes | Yes | `00`/`02`/`04`, bounded address | rejected outside subset | direct-write preview/conversion | complete current boundary |
| PS2 | PNACH | PCSX2 raw patch | Yes | Yes | `byte`/`short`/`word` | no device encryption | direct-write preview/conversion | complete current boundary |
| PS2 | GameShark | version unspecified | No | No | none proven | unknown/version-dependent | target shown unavailable | acquire lawful versioned fixtures and an authoritative grammar |
| PS2 | CodeBreaker | version unspecified | No | No | none proven | unknown/version-dependent | target shown unavailable | acquire lawful versioned fixtures and an authoritative grammar |
| Nintendo DS | Action Replay DS | DeSmuME ARparser-compatible canonical direct records | **Yes, V6** | No | canonical `0`/`1`/`2` complete direct-only records | no, only under direct-only constraint | CheatBase browse-only plus source-only IR classifier | separately audit a native target writer |
| RetroArch | `.cht` | generic/core-dependent | No generic parse | No generic encode | none globally | handler/core state required | bounded native install only | audit one named core/handler, not “RetroArch” generally |

## Ranked next implementation targets

1. **Nintendo DS Action Replay source-to-IR direct-only parser.** High
   semantic confidence, useful CheatBase coverage, small isolated parser;
   retain unsupported lines and do not add a writer.
2. **A named RetroArch core/handler structured format, if one has a reviewed
   native contract.** Potentially useful, but only after a per-core evidence
   pack establishes memory region, width, endian and handler semantics.
3. **PS2 GameShark or CodeBreaker only after versioned, lawful raw/decrypted
   fixtures and an authoritative grammar are obtained.** They are tied here
   because neither has enough evidence to rank independently.  This is not a
   request to add a decryptor.

## Non-negotiable implementation gates

Any later change must retain raw source and per-operation refusal reasons,
reject noncanonical value bits and unaligned DS writes, validate the whole DS
record before mapping any line, and add no native apply path until a target
writer is reviewed.  It must never guess encryption, master/init semantics,
or a cross-platform conversion.
