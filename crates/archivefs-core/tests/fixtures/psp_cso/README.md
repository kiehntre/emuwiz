# PSP CSO reader fixture (ALCHEMY Q1)

`maxcso-1.13.0-synthetic.cso` was written by maxcso v1.13.0 (ISC licence; run once as a separate
program, not bundled) with `maxcso --block=2048 synthetic.iso`.

`synthetic.iso` is not committed: `psp_reversible_shrink::tests::synthetic_iso()` regenerates it
deterministically (96 sectors: constant, text and SHA-256-chain data). It contains no game data.

- synthetic.iso: 196,608 bytes, SHA-256 69150feecd8d0e65d7c884f8efa6d50d8f7e3cfb3dd1690f2fb6d903f6710801
- maxcso-1.13.0-synthetic.cso: 67868 bytes, SHA-256 8b40ecd9fde487c3f3b10c2400fb277039e6919f6805969a7e780549fb4b0532, header version 1, align 0

Purpose: an independent writer for EmuWiz's CSO reader, so EmuWiz's writer and reader cannot share a
private interpretation of the format.
