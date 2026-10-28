# CodeBreaker upstream fix follow-up — 2026-10-27

Decision: **the fix is validated against the pinned C reference; retain the
production `=0.4.0` pin until a fixed release is reviewed and explicitly
approved.** No dependency is promoted by this change.

## Exact upstream review and release evidence

Reviewed Mathias Lafeldt's commit
[`75471b6ed44f5692f7bba856e909699648b02aea`](https://github.com/mlafeldt/codebreaker-rs/commit/75471b6ed44f5692f7bba856e909699648b02aea),
2026-10-26 13:21:29 UTC, “Restore double rekey on beefcodes in V7 mode”.
The commit says CodeBreaker v9.2 applies the beefcode twice, matching cb2util.
This resolves the uncertainty in our original discrepancy note. It remains
the author's device-behaviour evidence; we did not test physical hardware.

The explicit encrypt/decrypt wrappers now call `enter_v7` after either cipher
branch, and V7 auto-decrypt calls it after detecting a decrypted beefcode.
`Cb7` already applies the first re-key; the wrapper restores the second.
The changed common-key vector now contains `02210430 184C16E8`, exactly our
smallest C reproduction. A setup/DF/payload regression vector covers explicit
and auto-decryption. Comparing all four Rust source files with registry 0.4.0
shows `cb1.rs`, `cb7.rs` and `rc4.rs` byte-identical; `lib.rs` contains this
restoration and the regression-vector changes.

The [public Cargo registry index](https://github.com/rust-lang/crates.io-index/blob/master/co/de/codebreaker)
queried for this run lists **0.4.0 as the latest published version**, not yanked,
published `2026-07-08T13:13:11Z`. Its checksum is
`88e7e1d6cfc999dfad058f198b13fd539bfa0fefcf7589f976a0c4accf551d7e`;
the installed crate's `.cargo_vcs_info.json` identifies
`503d36e31ef6dbb48c1b9ab24117d2d3caee49d1`.
It predates the fix and demonstrably retains the bug. No newer registry release
was present. The fixed commit's manifest still says `0.4.0`; that does **not**
make the released archive fixed. The crates.io metadata endpoint returned HTTP
403, so release status was checked through the public registry index instead.

## Executed differential evidence

Reconstructed the existing C/Rust harness from the original research note,
then added independent Rust encryption comparison and separate Rust/C decryption
and C/plaintext round-trip counters. The historical combined mismatch counter
remains available. Original random profiles were retained, with forced repeated
DE and DF cases added. Each case has 4,000 streams; zero and nonzero seeds are
covered. Baseline and candidate consume the same generated C ciphertext.

Reference sources: cb2util v1.9
`5fed07bae1e39694e5d7890e41bd9239a5b77b79`, libbig_int
`fd8f3278ea03eb849420aa5eab540319ff9b3846`. Candidate is the exact reviewed
commit above, not a branch. Both checkers use bytemuck `=1.25.1`, matching the
product lockfile. All Cargo commands ran through `scripts/cargo-iso`.

The table counts **Rust/C decryption differences**. Baseline encryption
differences have the same counts. Fixed encryption differences are zero in
every row.

| Initial mode / profile | Lines | Released 0.4.0 differences | Fixed differences | C's own plaintext round-trip failures |
| --- | ---: | ---: | ---: | ---: |
| Reset / no key | 132,023 | 0 | 0 | 0 |
| Common V7 / no key | 132,023 | 0 | 0 | 0 |
| Reset / single leading DE, random seed | 133,491 | 0 | 0 | 0 |
| Reset / single leading DE, zero seed | 134,552 | 0 | 0 | 0 |
| Reset / random DE and DF | 135,015 | 89,296 | 0 | 26,736 |
| Common V7 / random DE and DF | 135,015 | 110,257 | 0 | 26,736 |
| Reset / DE, DE, DE | 132,023 | 124,023 | 0 | 0 |
| Common V7 / DE, DE, DE | 132,023 | 128,023 | 0 | 0 |
| Reset / DE, DF, DF | 132,023 | 124,023 | 0 | 120,023 |
| Common V7 / DE, DF, DF | 132,023 | 128,023 | 0 | 120,023 |
| Reset / DE, DF, payload, DF, payload | 136,294 | 128,294 | 0 | 0 |
| Common V7 / DE, DF, payload, DF, payload | 136,294 | 132,294 | 0 | 0 |
| Common V7 / leading DE, random seed | 133,491 | 129,491 | 0 | 0 |
| Common V7 / leading DE, zero seed | 134,552 | 130,552 | 0 | 0 |

Total per implementation: **56,000 streams, 1,870,842 lines**.
Exact per-case metrics are retained in
[`results-20261027.json`](../../tools/codebreaker-followup/results-20261027.json).
Reproduction sources and archive hashes are in
[`tools/codebreaker-followup`](../../tools/codebreaker-followup/README.md).
The two exact upstream golden vectors additionally check encryption, explicit
decryption and auto-decryption (30 assertions).

The upstream package's own unit-test command could not run offline because
`doc-comment` was absent from the local registry cache. The self-contained
golden-vector and differential checks require only the cached runtime crates;
do not describe these as the complete upstream test suite.

## Remaining compatibility boundaries

The historical statement that C always round-trips the generated plaintext was
too broad. Arbitrary DF placement, including adjacent DF lines, can fail in C
itself. In cb2util `cb7_encrypt_code`, beefcode handling precedes pending-DF
payload handling; `cb7_decrypt_code` handles a pending DF payload first. A DF
followed immediately by another key line therefore cannot be treated as an
ordinary round-trip fixture. The fixed Rust crate reproduces C's decryption
output, including those failures. Payload-separated repeated DF cases recover
the plaintext in this run. No new Rust/C discrepancy remains in this matrix;
agreement is not proof of validity for every device stream.

EmuWiz still refuses common-key re-keying, mid-list DE and DF sequences. Its
scripted-cipher refusal tests make that policy independent of post-key cipher
output. This follow-up changes no production Rust source or tests, root
`Cargo.toml`, core `Cargo.toml`, or product `Cargo.lock`; the existing preview
remains memory-only. Product tests were not rebuilt for this documentation and
external-harness change.

PNACH support remains restricted to validated direct writes, with existing
verified identity, alignment, RAM, width and all-or-nothing checks. The explicit
type-9 acknowledgement rule remains; unsupported conditionals, pointers,
slides, master-code variants and other semantics remain refused. No installer,
GUI, trainer activation or application profile was exercised or changed.

## Smallest safe update path

1. Wait for a published crate containing the reviewed fix. Verify its registry
   checksum, VCS revision and full delta from 0.4.0; rerun this harness against
   the actual published archive. Do not substitute moving `main`.
2. With explicit promotion approval, update only the exact dependency pin,
   lockfile entry, decoder-version/provenance strings and affected notices.
   Keep the current re-key refusals and PNACH boundaries.
3. Run the existing focused `ps2_codebreaker` tests, including scripted refusal
   cases, against that candidate. Any expansion of accepted key sequences is
   separate work requiring validated stream semantics and policy review.

An exact Git revision could be reviewed as a separate dependency proposal if
needed, but is not warranted to broaden a preview that already refuses the
affected shapes. It has not been introduced here.

Attribution is recorded in [third-party notices](../THIRD_PARTY_NOTICES.md):
Mathias Lafeldt, Copyright (c) Mathias Lafeldt for codebreaker-rs
(MIT OR Apache-2.0, used under MIT); Copyright (C) 2006-2013 Mathias Lafeldt
for the MIT cb2util reference, with both repository links.
