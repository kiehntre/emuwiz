# Release-candidate acceptance harness

`run-rc-acceptance.sh` composes the existing release packager/SBOM verifier,
release smoke harness, synthetic library lab, upgrade preflight, and pending
recovery inspector. It never uses normal EmuWiz state: HOME, every XDG root,
and EmuWiz overrides are created below the evidence output's private work
directory. Every subprocess has a bounded timeout and stage logs are retained.

```sh
scripts/release/run-rc-acceptance.sh \
  --source-tree "$PWD" \
  --gui-binary target/release/emuwiz \
  --cli-binary target/release/emuwiz-cli \
  --output /tmp/emuwiz-rc-acceptance
```

Add `--gui-smoke` to launch the GUI from the extracted, already-verified
release archive under a dedicated Xvfb display. The stage runs empty,
existing-profile, legacy-only, and both-root fixtures, retains per-case
stdout/stderr, an environment summary, a bounded `strace` write report, and a
first-frame capture when `xwd`/ImageMagick are installed. It never uses the
build-tree GUI binary. Each case also retains an environment summary and a
filesystem-write summary. `--require-gui-smoke` makes missing or unusable Xvfb,
or any GUI failure, a release failure; otherwise the stage reports `SKIPPED
(Xvfb unavailable)`.

The GUI helper can be tested independently:

```sh
python3 scripts/release/packaged_gui_smoke.py --self-test
```

The output contains `rc-evidence/summary.json`, `SUMMARY.md`, provenance,
binary hashes, SBOM, package/checksum data, smoke/synthetic/upgrade/recovery
reports, GUI smoke evidence when requested, and bounded command logs. `0` means
PASS, `1` means a gate failure, and `3` means invalid invocation or inspection
setup. The optional `--keep` flag retains evidence when setup fails. The GUI
stage is optional unless `--require-gui-smoke` is supplied.
