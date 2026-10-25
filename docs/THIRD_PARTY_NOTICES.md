# Third-party notices (binary distributions)

EmuWiz itself is MIT licensed (see `LICENSE`). Release binaries that statically
include the crates below must ship their licence text.

| Component | Version | Licence | Used for |
| --- | --- | --- | --- |
| `rars` | 0.10.2 (exact pin) | Apache-2.0 (`COPYING` in the crate) | In-process, read-only RAR 1.3-7 reading for CBR manuals (`manual_document::rar_pages`). Built with default features off: no encryption, writer, recovery or threading code is compiled into the product. Its only normal dependencies are `aho-corasick` (MIT OR Unlicense) and `memchr` (MIT OR Unlicense). |
| `codebreaker` | 0.4.0 (exact pin) | MIT OR Apache-2.0 (used under MIT) | Preview-only decryption of PS2 CodeBreaker code lines (`ps2_codebreaker`). `no_std`; its only dependency is `bytemuck`, already in the build. Copyright (c) Mathias Lafeldt. |

Apache-2.0 requires that the licence text accompany redistributed binaries and
that any upstream NOTICE file be carried along; `rars` 0.10.2 ships `COPYING`
and no separate NOTICE file.
