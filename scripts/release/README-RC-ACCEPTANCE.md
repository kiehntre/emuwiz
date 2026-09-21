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

The output contains `rc-evidence/summary.json`, `SUMMARY.md`, provenance,
binary hashes, SBOM, package/checksum data, smoke/synthetic/upgrade/recovery
reports, and bounded command logs. `0` means PASS, `1` means a gate failure,
and `3` means invalid invocation or inspection setup. The optional `--keep`
flag retains evidence when setup fails. No GUI stage is enabled by default;
the existing headless CLI smoke is the release gate.
