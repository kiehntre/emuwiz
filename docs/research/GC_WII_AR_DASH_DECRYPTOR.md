# GameCube/Wii dash-format Action Replay interoperability

Research dates: 2026-10-02–03. Starting main and `origin/main`:
`006f391169b4c23484f26acca56a23234467883d`.
Branch: `feature/gc-wii-ar-decryptor`.
Worktree: `/home/davedap/emuwiz-gc-wii-ar-decryptor`.

## Decision before implementation

A narrow, independently written **GameCube** decoder is justified by the
public evidence below. Four independently published input/output sets match
both the pinned Dolphin decoder and standard DES through OpenSSL; encryption
in the reverse direction reconstructs every original encrypted line. A
further 512 deterministic random blocks agree between those implementations.
Only fixed-key, unexpanded verifier records are in scope. Wii-native dash
encryption has not been independently established and remains unsupported.

The decoder must remain **isolated** in this branch. The module registry is
dirty in other active worktrees. The user's collision instruction expressly
allows an isolated decoder with integration deferred. No registry workaround,
provider change, or automatic installation path is authorized by that exception.
BSFree encrypted records therefore remain browse-only in the compiled product.

## Algorithm and provenance

The behavioral reference is Dolphin
[`ARDecrypt.cpp` at `a475aba718746e7656e2e6b6e047dc98a4d05879`](https://github.com/dolphin-emu/dolphin/blob/a475aba718746e7656e2e6b6e047dc98a4d05879/Source/Core/Core/ARDecrypt.cpp).
Its SPDX license is GPL-2.0-or-later and it credits Parasyte's GCNcrypt.
SHA-256 of the retrieved file:
`69fe681cfa1833e15dbb75e3790be777066cc6c58ec873397b56e1897171f945`.
It establishes the alphabet, fixed key, word byte order, parity, and folded
checksum behavior. Its checksum-failure path emits words despite returning
failure; this adapter must reject that result completely.

The independently tested cipher mapping is standard DES decryption with
key `341C849EFDA4B67B`: reverse the four bytes of each input word, decrypt
the resulting eight-byte block, then reverse each output word's bytes.
The cipher will be written directly from
[NIST FIPS 46-3, algorithm and Appendix 1](https://csrc.nist.gov/files/pubs/fips/46-3/final/docs/fips46-3.pdf).
Its ordinary bit-selection permutations and S-box definitions suffice; no
GCNcrypt/Dolphin implementation or precombined lookup tables are needed.
The key is a published format parameter, not a recovered user secret.
The standard's numeric definitions are attributed to NIST, whose
[public-information policy](https://www.nist.gov/copyrights-disclaimers)
permits copying/distribution unless material is marked copyrighted. No
proprietary cipher tables are redistributed.

[Parasyte's GCNcrypt v1.1 README, publicly archived](https://www.neperos.com/article/rknu6h1193c27aac),
describes verifier metadata and expansions that can alter seeds and span
subsequent lines. Expanded and reserved verifier forms must be refused,
including the undocumented flag and nonzero unused bits. A master verifier
must remain visible to the downstream safety gate. Its internal game number
is not a Dolphin disc ID and cannot replace the existing identity checks.

No Datel source, executable, proprietary cheat database, or proprietary table
is included. Public GPL source is used only as an external behavioral oracle
in temporary research files. The implementation is independent; this is not
a claim of a separately staffed legal clean-room process or a legal opinion.
Any later decision to copy upstream source would require its license terms.

## Independent published vectors

The checksum nibble shown in each full verifier below was recovered from
the ciphertext. Older documentation clears it when printing the verifier.
The operational raw body excludes the verifier line.

| Source | Encrypted complete set | Full verifier | Operational raw body | Expected existing classification |
| --- | --- | --- | --- | --- |
| [EnHacklopedia GameCube example](https://doc.kodewerx.org/hacking_gcn.html) | `G12C-TMX0-WRT5C` / `G2ND-C1RJ-G4TZ1` | `21E22DC2 08000000` | `00690E90 000004FF` | ActionReplayNative: 8-bit write/fill |
| [GCNcrypt author's README example](https://www.neperos.com/article/rknu6h1193c27aac) | `XAUQ-995V-EMM2K` / `HHC0-6EH5-TQ6UD` | `704E01EF 08000000` | `021F11DA 00000001` | ActionReplayNative: 16-bit write |
| [First-person GCNcrypt decode report, Metagames](https://www.metagames-eu.com/forums/game-cube/cobra-2-1-et-cheat-codes-et-action-replay-aide-100400.html) | `GKMU-93RZ-82YV6` / `ZFPE-UYKV-PX95X` | `0776EB42 98000000` | `C435E298 0000FF01` | Unsupported: master opcode and master verifier |
| [Public PSO conversion example, Mogelpower](https://www.mogelpower.de/forum/thread.php?thread_id=67326) | `NT40-E3MT-TTTN4` / `T1MV-XZ0P-2YDR5` / `99DR-JVGX-Z6DAF` | `5F7E1000 88000000` | `057E6CF8 4BEB46D0` / `057E6CFC 000009C0` | Unsupported: master verifier, even though the body alone is native AR |

The two forum examples are interoperability observations, not authoritative
format specifications. They are independently checked against both engines.
OpenSSL 3.0.13's DES-ECB implementation supplied an independent inverse
direction; generated opcode-family vectors are additional tests, not new
independently published vectors. Development used only locally compiled
public Dolphin source and the installed OpenSSL executable, never Datel's
binary. Neither executable is a production decoding dependency.

## Wii verdict

[Datel's public GameCube product description](https://www.codejunkies.com/Products/SD-Media-Launcher__EF000195.aspx)
limits its Wii compatibility claim to **GameCube mode**. That does not prove
the encryption of cheats targeting native Wii games. No independently
verified native-Wii encrypted vector was found. Sharing a raw AR classifier
also does not prove cipher equivalence. The decoder API must be explicitly
GameCube-only; `bsfree_wii.rs` stays unchanged.

## Integration collision

The initial scan inspected 428 registered worktrees. These worktrees have
dirty `crates/archivefs-core/src/patch_manager/mod.rs`:

- `/home/davedap/archivefs`
- `/home/davedap/archivefs-fsuae-adapter`
- `/home/davedap/archivefs-vice-c64-adapter`
- `/home/davedap/emuwiz-082-cheat-reconciliation-plan-rehearsal`
- `/home/davedap/emuwiz-arcade-readiness`
- `/home/davedap/emuwiz-custom-dat-lifecycle`
- `/home/davedap/emuwiz-scummvm-dosbox-readiness`

`bsfree_gamecube.rs`, `bsfree_wii.rs`, their test directories, and the proposed
decoder paths were clear. Main and origin/main match the required SHA;
the authoritative repository's working checkout is an unrelated dirty
feature branch, which is preserved. The new feature worktree started clean.

A subsequent scan inspected 431 worktrees and found the same seven registry
collisions, with no other decoder/BSFree overlap. During validation another
lane advanced main and origin/main together to
`6c584b3152e0dd581138fd3ac9f394302b904eba` (GUI v2 Storage review). That commit
changes none of the task's integration paths. This branch retains its exact
requested parent, `006f391169b4c23484f26acca56a23234467883d`; this task did not
move main, promote or push.
The final pre-commit scan covered 432 worktrees and again found only those
same seven registry collisions.

## Required deferred integration

Once registry ownership is clear, one small GameCube integration commit must:

1. Register/export the decoder in `patch_manager/mod.rs`.
2. Invoke it only for a GameCube record whose device is explicitly Action
   Replay and whose entire nonempty body is strictly dash-encrypted. Reject
   mixed raw/encrypted records; never guess a platform or strip headings.
3. Preserve provider text, decoder version, raw decoded body, checksum evidence,
   verifier metadata, and the downstream classification in a distinct optional
   provenance field. Raw records must retain their current behavior and digest.
4. Force master-verifier records to `Unsupported`, then feed all other decoded
   bodies into the existing classifier. Master/zero/self-modifying opcode
   refusals remain authoritative. A successful decrypt is not an installation
   decision. Failure stays browse-only with its precise error.
5. Reuse the current GameCube identity gates and install/preview/journal/
   rollback path; introduce no alternate transaction machinery. Add integrated
   provider-device, provenance, classification and apply/rollback tests.

Wii integration, generic encrypted recovery, PS2, DS, universal conversion,
and GUI changes are outside that follow-up.

## Implemented isolated subset and bounds

`gamecube_wii_ar_decrypt.rs` exports only the GameCube decode operation,
version, bounds, error type and verified canonical-text result. DES primitives
and the key schedule remain private; encryption exists only in tests. There
are no new dependencies, file reads/writes, process launches or network calls
in the decoder. `action_replay.rs` and both BSFree bridges are unchanged.

| Input or work | Hard bound / behavior |
| --- | --- |
| Entire original input | 16,384 ASCII bytes, checked before allocations |
| Encrypted records | 256 nonempty lines, including exactly one initial verifier |
| Executable output | 255 pairs / 510 words; at most 4,589 canonical text bytes |
| Cipher work | At most 4,096 DES rounds per decode; no seed search or retries |
| Working storage | Two bounded vectors, a 128-byte key schedule, bounded raw text and exact original text; under 26 KiB of payload allocations |
| Syntax | Exactly `4-4-5` characters, canonical 32-symbol alphabet, ASCII case normalization; outer ASCII whitespace/blank lines accepted |
| Check mechanism | Every line's parity, then the complete set's folded CRC-16/KERMIT over little-endian words with verifier checksum bits cleared |
| Verifier | Expansion-disabled only; reserved flags/padding and region 3 refused; master flag retained explicitly |
| Partial failure | Error with reason; no raw body or fallback recovery returned |

The public spelling is base **32**, encoding 64 payload bits plus one parity
bit in 13 symbols. The old GameCube document's “base-31” label does not describe
the actual alphabet. Dolphin's ambiguous `I/L/O/S` aliases are deliberately
outside this strict subset.

The four-bit checksum is an error check, not authentication. Single-character
corruption can collide: `YAUQ-995V-EMM2K` in place of `XAUQ-995V-EMM2K`
passes Dolphin's folded checksum but yields an unsupported verifier, which
this decoder refuses. It is impossible to promise rejection of every corrupted
input that also represents another valid checked code. No such promise or
cryptographic trust is attached to `VerifiedGameCubeAr`.
An exhaustive check of the 403 one-symbol substitutions in the README's body
line found 208 parity failures, 181 checksum failures and **14 checksum
collisions**. Those collisions retain the original valid verifier and cannot
be rejected by verifier checks either. For example, `RHC0-6EH5-TQ6UD` decodes
to `3758E9EE 149E646D` with the same valid checksum. This limitation is inherent
in the specified format; installation safety must still come from the existing
classification, identity, review and transaction gates.

Exact original text, decoder version, complete verifier words, CRC-16,
line count, internal game/code numbers, region and master flag are returned
alongside the canonical body. These are evidence for a later BSFree provenance
field, not provider/device/identity authorization or downstream classification.
In particular, the PSO vector's otherwise native body must stay unsupported
because its verifier marks it as a master record.

## Validation

The standalone focused suite passes **20 tests**. It verifies the
four published sets, reverse encryption, independent OpenSSL opcode-family
fixtures, formatting, invalid alphabet/dashes/length, parity/checksum failures,
verifier restrictions, mixed input, exact provenance, and maximum bounds.
The independently written Rust cipher also agrees on all 512 deterministic
random blocks previously checked with OpenSSL and Dolphin.

```sh
rustc --edition 2024 --test \
  crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt.rs \
  -o /tmp/emuwiz-gcn-ar-tests
/tmp/emuwiz-gcn-ar-tests
```

`classifier_tests.rs` is a standalone integration rehearsal against the
unchanged, compiled core library. Its **8 additional tests** feed decoded text
into the actual BSFree classifiers, compare native/direct-write outputs and
digests, enforce the master-verifier gate in the test harness, retain provider
provenance, and show that both compiled bridges still refuse encrypted input.
Together with the focused suite this binary passes **28 tests**, not 48 unique
tests (the 20 focused tests are included again).

Reproduce the compiled-classifier check with an isolated Cargo target:

```sh
export CARGO_TARGET_DIR=/tmp/emuwiz-gcn-ar-validation
CARGO_BUILD_JOBS=4 cargo build --offline --locked -p archivefs-core --lib
core_rlibs=("$CARGO_TARGET_DIR"/debug/deps/libarchivefs_core-*.rlib)
rustc --edition 2024 --test \
  crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt/classifier_tests.rs \
  --extern "archivefs_core=${core_rlibs[0]}" \
  -L "dependency=$CARGO_TARGET_DIR/debug/deps" \
  -o /tmp/emuwiz-gcn-ar-classifier-tests
/tmp/emuwiz-gcn-ar-classifier-tests
```

The existing BSFree regression command passes **50 tests**: **33 GameCube**
and **17 Wii**, including existing raw AR classifications, encrypted refusals,
identity/review gates and the shared install/rollback round trips:

```sh
CARGO_BUILD_JOBS=4 cargo test --offline --locked -p archivefs-core --lib \
  patch_manager::bsfree_
```

`cargo fmt --all -- --check`, targeted `rustfmt --check` on all three added
Rust files, `git diff --check`, and the four-file task scope/GUI boundary guards
pass. Neither the registry nor an existing production file is changed.
`cargo check --offline --locked --workspace` also passes on the pinned branch
with four existing GUI warnings.

The full core library binary ran all **10,788 tests** with two worker threads:
**10,766 passed, 19 failed, 3 ignored** in 560.26 seconds. All 19 failures were
unchanged provider proxy/RomM tests denied permission to bind `127.0.0.1` mock
HTTP servers in the network sandbox. Targeted reruns with loopback access
passed every failed test (19 proxy tests, including one helper, plus one RomM
test). There are **no remaining failures**: 10,785 unique core tests passed
across these runs, with 3 ignored. The initial sandbox exit status was 101;
both targeted reruns exited 0. No production/provider fix was made or needed.

The full core run includes **1,972 passing patch-manager tests** (none ignored
or failing), including the existing shared transaction/rollback tests.
The standalone classifier rehearsal is additional
evidence for the deferred adapter, not a claim that production integration or
its future apply/rollback tests have already landed.

The original local BSFree source was opened read-only and its SHA-256 was
unchanged before/after validation:
`4cfee2640e5584adc52977bc56192f6b026814a8e3711687dd02519643631a06`
(`bsfree.4cfee26.db`, 296,218,624 bytes). It contains GameCube system rows
15/16/17 and no Wii system row. No source/provider files changed.

## Candidate scope and activation status

Exactly four files are added:

- `crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt.rs`
- `crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt/tests.rs`
- `crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt/classifier_tests.rs`
- `docs/research/GC_WII_AR_DASH_DECRYPTOR.md`

GameCube fixed-key decode: **verified, isolated implementation**.
Wii-native decode: **unproven, disabled**.
Production classification changes: **zero**.
Previously browse-only fixture/catalogue records unlocked in production: **0**,
because the registry/BSFree integration commit remains deferred by collisions.
The verified output can already be assessed by the existing classifier in the
test harness; operational activation requires the follow-up listed above.

Master verifiers/opcodes, zero/self-modifying commands, unsupported or reserved
verifier forms, noncanonical/mixed/malformed/check-failing/oversized inputs,
unverified Wii encryption and other platforms' proprietary encryption remain
outside the installable subset. The general proprietary recovery refusal is
unchanged. GUI, library/CLI roots, provider acquisition, transaction machinery
and all existing production files are unchanged. This branch is not promoted
or pushed; the candidate commit SHA is reported separately to avoid embedding
its own hash in its contents.
