# Batch 11 recovery notes

Index of what the Batch 11 low-collision recovery did and did not carry over from dormant worktrees (main `b66422c2`). Historical docs recovered here carry their own status headers.

## Deferred: Redump registry/coverage lockstep test

- **Source:** branch `integration/redump-coverage-reconciliation` (worktree `emuwiz-redump-coverage-reconciliation`), commit `eb1fd5ea` (`test(dat): lock Redump coverage to reviewed registry`).
- **Why deferred:** its destination, `crates/archivefs-core/src/dat/coverage_expectations.rs` (a test inside that file's `mod tests`), is currently modified by the active DAT D01 lane (`emuwiz-dat-d01-main-integration`). It was not edited here.
- **What it does:** replaces `all_current_redump_systems_expect_redump_authority` (a hard-coded platform list) with a test that pins the reviewed list of 17 Redump systems against `RedumpGameSystem::all()` in order, checks each system's `canonical_platform()` and canonical alias, and still asserts a Redump authority expectation per platform. Test-only; it changes no Redump behaviour.
- **Validated:** spliced into a scratch copy of current main's file, the test passes (10 of 10 `dat::coverage_expectations` tests). The scratch change was reverted; nothing was committed.
- **To apply:** after the D01 lane lands, replace that test in `coverage_expectations.rs` with the one below and add the two imports (`use crate::dat::updates::RedumpGameSystem;` and `use crate::game_identity::IdentityPlatform;`) to its `mod tests`. Re-run `cargo test -p archivefs-core --lib dat::coverage_expectations`.

```rust
    #[test]
    fn current_redump_registry_and_coverage_stay_in_lockstep() {
        let reviewed = [
            (
                RedumpGameSystem::PlayStation,
                "PSX",
                IdentityPlatform::PlayStation,
            ),
            (
                RedumpGameSystem::PlayStation2,
                "PS2",
                IdentityPlatform::PlayStation2,
            ),
            (
                RedumpGameSystem::PlayStation3,
                "PS3",
                IdentityPlatform::PlayStation3,
            ),
            (
                RedumpGameSystem::PlayStation4,
                "PS4",
                IdentityPlatform::PlayStation4,
            ),
            (RedumpGameSystem::Psp, "PSP", IdentityPlatform::Psp),
            (RedumpGameSystem::Saturn, "Saturn", IdentityPlatform::Saturn),
            (
                RedumpGameSystem::Dreamcast,
                "Dreamcast",
                IdentityPlatform::Dreamcast,
            ),
            (
                RedumpGameSystem::SegaCd,
                "Sega CD",
                IdentityPlatform::SegaCd,
            ),
            (
                RedumpGameSystem::GameCube,
                "GameCube",
                IdentityPlatform::GameCube,
            ),
            (RedumpGameSystem::Wii, "Wii", IdentityPlatform::Wii),
            (RedumpGameSystem::WiiU, "WiiU", IdentityPlatform::WiiU),
            (RedumpGameSystem::Xbox, "Xbox", IdentityPlatform::Xbox),
            (
                RedumpGameSystem::Xbox360,
                "Xbox360",
                IdentityPlatform::Xbox360,
            ),
            (RedumpGameSystem::ThreeDo, "3DO", IdentityPlatform::ThreeDo),
            (RedumpGameSystem::Pcfx, "PC-FX", IdentityPlatform::Pcfx),
            (
                RedumpGameSystem::PcEngineCd,
                "PC Engine CD",
                IdentityPlatform::PcEngineCd,
            ),
            (
                RedumpGameSystem::NeoGeoCd,
                "Neo Geo CD",
                IdentityPlatform::NeoGeoCd,
            ),
        ];
        assert_eq!(RedumpGameSystem::all().len(), reviewed.len());

        for (index, (system, platform, identity_platform)) in reviewed.iter().enumerate() {
            assert_eq!(RedumpGameSystem::all()[index], *system);
            assert_eq!(system.canonical_platform(), *identity_platform);
            assert_eq!(
                crate::canonical_platform_for_alias(platform),
                Some(*platform)
            );
            single_source(
                Some(*platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::Redump),
            );
        }
        assert!(matches!(
            expected_authoritative_coverage(Some("Philips CD-i")),
            PlatformCoverageExpectation::NoKnownAuthoritativeSource { .. }
        ));
    }
```

## Not recovered (superseded)

- **Immutable GUI artifact scripts** (`create-`, `verify-`, `test-immutable-gui-artifact.sh`, from `emuwiz-mr-wiz-guidance-v2`): superseded by the release tooling already on main (`scripts/build-release.sh`, `scripts/verify-release-artifact.sh`, `scripts/release/packaged_gui_smoke.py`, `scripts/release/run-rc-acceptance.py`). They also hard-coded a host-specific artifact root.
- **Installer acceptance test** (`test-installer-acceptance.sh`, same branch): its six ownership scenarios are covered by `tests/test_install.sh` on main (1,677 lines, including the installer ownership-safety assertions).

## Recovered QA helper

`scripts/qa/sunshine-acceptance-helper.sh` (from `emuwiz-sunshine-qa-helper`), adapted: run records go outside the repository by default and a repository run directory is refused; stale documentation references were replaced with files that exist; an offline self-test was added.
