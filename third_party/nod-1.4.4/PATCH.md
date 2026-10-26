# Vendored nod 1.4.4: temporary zstd dependency security patch

Provenance: the crates.io `nod` 1.4.4 archive has SHA-256
`9fbe592a5e74ed882c6d7373b4cc6beca6e84cf6b7f9fe5cba5d600255d965b5`,
matching the original workspace lockfile. Upstream is
https://github.com/encounter/nod, commit
`df8ab228c8a8e8f906048daddc3ea07f43f18a5c`, crate path `nod`
(from the published `.cargo_vcs_info.json`). `LICENSE-APACHE` and
`LICENSE-MIT` are the upstream repository-root licences at that exact commit;
the published crate does not include them. The crate is MIT OR Apache-2.0.

Only one dependency requirement in the normalized Cargo.toml changes:

```diff
 [dependencies.zstd]
-version = "0.13"
+version = "0.14.3"
```

`README.md`, `Cargo.toml.orig` and all source except the mechanical warning
fixes below are byte-identical to the published package. No decoding, format,
error or feature algorithm changes.
Cargo's local cache marker and VCS metadata are omitted; provenance is recorded
above. The workspace lockfile governs resolution.

Why: `zstd` 0.13.3 has RUSTSEC-2026-0391; its `zstd-safe` 7.2.4 dependency
has RUSTSEC-2026-0398 and RUSTSEC-2026-0403. Patched versions are zstd
0.14.3 and zstd-safe 8.2.0. The workspace already resolves both patched versions
for ZIP fixtures; this patch lets nod use them too. The shared zstd-sys version
and native Zstandard library are unchanged. No advisory is ignored and no
archive format or security gate is disabled.

nod is used directly by archivefs-core for GameCube/Wii inspection and through
opticaldiscs for optical-disc access. nod's WIA/RVZ Zstandard reader is reachable
with `compress-zstd`. There is no patched stable nod release in the registry:
1.4.4 is the latest stable; 2.0 is prerelease and cannot satisfy opticaldiscs'
`nod ^1.4` requirement. A one-line dependency patch avoids an unrelated reader
migration and preserves the existing API and enabled features.

Wiring: workspace `[patch.crates-io] nod` and `exclude` cover both dependency
owners. Keep core's existing nod/opticaldiscs versions and feature selections.

Remove this directory, its patch entry and exclusion when a released nod
compatible with both owners accepts zstd >=0.14.3 / zstd-safe >=8.2.0.
Re-resolve only the affected chain, run the audit, synthetic RVZ/Zstandard and
archive/conversion/preservation regressions, and required CI. Do not extend
this patch with unrelated source edits.

## Rust 1.97 warning compatibility

Vendoring exposes compiler warnings that Cargo previously capped for the
registry dependency. Preserve the workspace warning ratchet without adding
baseline entries or suppressions:

- `src/disc/mod.rs`: spell the existing elided lifetimes as `'_` in `Cow`,
  `FileStream` and `Fst` return types.
- `src/disc/fst.rs`, `src/disc/gcn.rs`, `src/disc/wii.rs`: likewise spell the
  existing elided `FstIter` / `FileStream` return lifetimes explicitly.
- `src/io/gcz.rs`: qualify `WriteBuf::as_slice(self.block_buf.as_ref())` instead of
  the same trait method's `.as_slice()` call, avoiding a future std method
  collision. The buffer and selected method are unchanged.

These seven line edits affect five files, have no intended runtime effect and
are the only local source differences. They must be independently reviewed
along with the dependency change. No Clippy/security gate is weakened.
