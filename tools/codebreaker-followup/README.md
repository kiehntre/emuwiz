# CodeBreaker upstream follow-up harness

Test-only reproduction of the generator/checker in
[`CODEBREAKER_RS_C_DISCREPANCY.md`](../../docs/research/CODEBREAKER_RS_C_DISCREPANCY.md).
It never invokes EmuWiz, installs a trainer, changes the product dependency pin,
or writes to an application profile. Run it from a disposable directory outside
the repository. It needs GCC, Python 3 and the repository Rust toolchain.

Prepare these **reviewed, fixed source revisions**, retaining their licence files:

| Directory under the private root | Repository | Revision |
| --- | --- | --- |
| `fixed/` | https://github.com/mlafeldt/codebreaker-rs | `75471b6ed44f5692f7bba856e909699648b02aea` |
| `cb2util/` | https://github.com/mlafeldt/cb2util | v1.9, `5fed07bae1e39694e5d7890e41bd9239a5b77b79` |
| `libbig_int/` | https://github.com/mlafeldt/libbig_int | `fd8f3278ea03eb849420aa5eab540319ff9b3846` |

For the 2026-10-27 run, GitHub codeload archives used these URL suffixes and
SHA-256 values (archive bytes depend on the exact suffix):

| URL after `https://codeload.github.com/mlafeldt/` | SHA-256 |
| --- | --- |
| `codebreaker-rs/tar.gz/75471b6ed44f5692f7bba856e909699648b02aea` | `ad1aeb69fd9edf5cbb6774224befcb1bb45c27dd413a2a51d06b82977be8f1b1` |
| `cb2util/tar.gz/refs/tags/v1.9` | `f929488b275bd2698c6b026ecd9f1e8b6ef83cd280d9019e1d79dd59617a57e8` |
| `libbig_int/tar.gz/fd8f327` | `417ab6c6f0dcb71d58acb7da816702c90b9a9815cfd239e1e3efad8d43ba4849` |

Verify archives before extracting them into the named directories. Do not use
moving branches. The released baseline is Cargo registry `codebreaker =0.4.0`,
checksum `88e7e1d6cfc999dfad058f198b13fd539bfa0fefcf7589f976a0c4accf551d7e`.
The runner uses offline Cargo, with `bytemuck =1.25.1` matching EmuWiz's lockfile;
those two registry crates must already be cached. All generated files go under
the supplied private root, except the worktree-specific Cargo target managed
by `scripts/cargo-iso`.

```sh
python3 tools/codebreaker-followup/run.py /tmp/emuwiz-codebreaker-private
```

`run.py` builds both checkers, generates 4,000 streams per case, writes per-case
logs and `results.json`, and checks the two exact upstream golden vectors for
encryption, explicit decryption and auto-decryption. Explicit-decrypt random
streams are not auto-detection fixtures. See the research follow-up for the
observed results and limitations; `results-20261027.json` retains the counts.

Generator modes: 0 = reset/V1; 1 = common V7. Profiles:

| Profile | Shape |
| --- | --- |
| 0 | Original random stream, no key lines |
| 1 | Original single leading DE; random seed or `ZV=1` zero seed |
| 2 | Original arbitrary random DE/DF placement |
| 3 | Forced DE, DE, DE, write, followed by non-key random lines |
| 4 | Forced DE, DF, DF, write, followed by non-key random lines |
| 5 | Forced DE, DF, payload, DF, payload, write, then non-key random lines |

Metrics separate encryption agreement, Rust/C decryption agreement, and recovery
of generated plaintext. Adjacent DF sequences may violate the cipher's pending
payload semantics: matching C does not imply a successful round trip. The runner
requires fixed Rust/C encryption and decryption agreement, and requires any fixed
plaintext mismatch count to equal the C reference's own mismatch count. Baseline
disagreements are evidence and do not make the comparison runner fail.

Cipher attribution: Mathias Lafeldt, `codebreaker-rs` Copyright (c) Mathias Lafeldt
(MIT OR Apache-2.0); `cb2util` Copyright (C) 2006-2013 Mathias Lafeldt (MIT).
See [third-party notices](../../docs/THIRD_PARTY_NOTICES.md).
