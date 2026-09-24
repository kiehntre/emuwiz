# Final security and privacy hardening pass (v0.9.0 lineage)

- **Branch:** `fix/final-security-hardening`
- **Starting SHA:** `7069cdfc2b5a4da184e3419964701de591b15e60` (`chore(release): prepare v0.9.0`)
- **Resulting SHA:** the commit that adds this document; the code fix is
  `a799e33b` (`fix(romm): never route RomM requests through an environment proxy`).
  Run `git log --oneline 7069cdfc..fix/final-security-hardening` for the exact range.

Scope: a review of the release in the areas below, fixing only issues that
are real and reachable. MAME Arcade identity ingestion, MAME lifecycle/binding,
the DAT picker and GUI theme/polish were out of scope and were not touched.

## Vulnerability found and fixed

### RomM bearer token could be sent to an environment proxy

- **Where:** `crates/archivefs-core/src/identity_source/romm/client.rs`,
  `UreqTransport::new`.
- **What:** `ureq` 3 reads `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` (either
  case) by default (`Config::default()` calls `Proxy::try_from_env()`). It has
  no built-in exemption for loopback or private addresses; only `NO_PROXY`
  bypasses the proxy. Every other production transport in EmuWiz (emulator
  download, managed DAT, cheat sources, GameHacking, PCSX2 retrieval) already
  sets `.proxy(None)`. The RomM transport did not.
- **Effect:** if any proxy variable was set in the user's session, which is
  common on corporate desktops and with some VPN/privacy tools, every RomM
  request went to the proxy instead of the RomM address. That included the
  `Authorization: Bearer <token>` header, sent in cleartext whenever RomM is
  configured over HTTP (which the endpoint policy allows). This defeats the
  endpoint policy's main guarantee that the token only goes to an approved
  loopback/private-LAN address. LaunchBox artwork, fetched through the same
  transport, was also proxied.
- **Reachability:** real. It needs only a RomM source to be configured and a
  proxy variable in the environment. No attacker action on the EmuWiz host is
  needed; whoever runs or controls the proxy host gets the token.
- **Severity (plain language):** medium. A credential is disclosed to a third
  host under an ordinary, non-hostile configuration. The token is RomM's
  client token and, by the documented guidance, should be read-only scoped.
- **Fix:** `.proxy(None)` on the RomM agent, matching every other transport.
  `docs/security.md` now says the RomM client ignores proxy variables.
- **Proof:** the regression test failed before the fix with "the RomM
  transport connected to the environment proxy", and passes after it.

## Files changed

- `crates/archivefs-core/src/identity_source/romm/client.rs`: the fix.
- `crates/archivefs-core/src/identity_source/romm/tests.rs`: the regression test.
- `docs/security.md`: documents the proxy behaviour.
- `docs/qa/FINAL_SECURITY_PRIVACY_HARDENING.md`: this report.

## Tests added

- `identity_source::romm::tests::the_production_transport_never_routes_a_token_through_an_environment_proxy`
  re-runs the test binary as a child process with all six proxy variables
  pointed at a loopback "proxy" listener and `NO_PROXY` removed. It drives the
  real `UreqTransport` with a bearer token against a loopback "RomM" listener,
  then asserts three things: the proxy was never contacted, the request
  reached the RomM address, and the header was present there. Proxy variables
  are set only on the child, so the parent test process's environment is
  never changed.

## Areas reviewed with no issue found

1. **Secret/credential leakage.** `RommToken` redacts itself in `Debug`,
   `Display` and `Serialize`, and exposes the value only through
   `with_header_value`. Token files are loaded by
   `identity_source::settings::load_token_file`, which refuses symlinks,
   non-regular files, oversized files and any group/world permission bit.
   Transport errors are classified without echoing URLs or headers. The only
   `Authorization` header in the codebase is RomM's. The GitHub API, DAT,
   cheat and emulator-download fetches send no credentials. The
   RetroAchievements API key helper (`configured_api_key`) has no network
   caller. There is no environment dumping and no log file writer.
   `scripts/security-scan.sh` passes over 1,271 tracked files.
2. **Network/SSRF.** All seven `ureq` agents set `max_redirects(0)`.
   Emulator downloads follow redirects manually, and only to hosts on an
   allowlist. Managed DAT URLs are limited to the GitHub API, GitHub raw and
   Redump hosts over HTTPS. The PCSX2, GameHacking and cheat-source fetches
   are HTTPS-only with fixed hosts. The RomM endpoint policy
   (`identity_source/net_policy.rs`) rejects userinfo, limits connections to
   loopback/RFC 1918/ULA addresses, and reports redirects without following
   or resolving them. RomM artwork references are resolved against the RomM
   origin. LaunchBox CDN URLs require an exact host, HTTPS on port 443, no
   credentials and no IP literal. Every agent sets timeouts, and response
   bodies are read through `take(limit + 1)` with a check afterwards. The
   remaining DNS-rebinding gap is already documented in `docs/security.md`.
3. **Archive/file extraction.** Only cheat-source ZIP extraction writes
   members to disk (`cheat_sources::extract_zip_safely`). It rejects
   absolute paths, `..` and `.` components, backslashes, drive prefixes,
   NUL, duplicate and case-folding collisions, and symlink/special entry
   modes. It also enforces entry-count, path-length, component-count,
   per-file, total-expanded and compression-ratio limits. It writes with
   `create_new` + `O_NOFOLLOW` into a staging directory that is checked for
   symlinks. No-Intro pack import writes members under synthetic names
   (`N.dat`) and bounds them. The Dolphin catalogue and inspector paths parse
   in memory only. RAR/LHA/7z use a bounded, fd-pinned `7z x -so` stream and
   never extract to a directory. Tar is only listed, never unpacked.
4. **Path/filesystem boundaries.** Library View path components go through
   `sanitize_path_component_str/os` (a single `Normal` component). That
   includes RomM-supplied platform slugs. DAT-derived rename basenames block
   separators and NUL. RomM media mappings canonicalize and require the
   result to stay under the root, and refuse symlinks. Rename, repair and
   organisation all go through the journaled `renameat2(RENAME_NOREPLACE)`
   engine. The emulator AppImage install refuses symlinked or foreign
   destinations and uses `create_new` temporaries. The approval sidecar and
   texture-mod manifest writes use `create_new` temporaries.
5. **Database.** Every dynamic SQL string found interpolates only
   compile-time table names or `?` placeholders built in code. Values are
   always bound. Third-party SQLite databases (CheatBase, BSFree) are opened
   `SQLITE_OPEN_READ_ONLY` and query-only.
6. **Command execution.** Every launch uses `Command::new(program).args(..)`
   and no shell. The only `sh` invocations are in `#[cfg(test)]` code. The
   7-Zip helpers pass `--` before `/proc/self/fd/N` and the member path, with
   `-spd` (no wildcard expansion). Existing tests cover leading hyphens,
   spaces, Unicode and glob characters. `xdg-open` receives either absolute
   local paths or a URL checked against its host allowlist. Emulator
   launchers do not write generated config files containing ROM paths. ES-DE
   `gamelist.xml` output is XML-escaped.
7. **Privacy: features that make network requests, and what they send.**
   No telemetry exists and none was added.
   - Hasheous lookup (explicit "Check Hasheous" click): sends the SHA-1 of the
     selected file to `https://hasheous.org`.
   - RomM (configured by the user): sends the bearer token and paged API
     requests to the user's own RomM address only.
   - Emulator download (explicit): GitHub API/release requests. No local data.
   - Managed DAT update (explicit): GitHub/Redump fetches. No local data.
   - Cheat catalogues, GameHacking, PCSX2 patches (explicit): fixed-host
     fetches. GameHacking browser handoff strips the query and fragment.
   - **One automatic request:** after a Dolphin cheat catalogue has been
     installed, opening Cheats & Mods runs one "check for updates" per
     session, a GitHub commit lookup for `dolphin-emu/dolphin`. It sends no
     ROM names, paths, hashes or account data, only what any HTTPS request
     reveals (IP address, User-Agent). This is by design and documented in
     the code.
8. **Release/build output.** `scripts/build-release.sh` stages an explicit
   allowlist: two binaries, `install.sh`, README, CHANGELOG, LICENSE,
   `config.toml.example`, the desktop template and branding PNGs. No config,
   database, `.env`, QA screenshot or key can be included.
   `verify-release-artifact.sh` scans text and binary `strings` for
   maintainer and home paths, GitHub/AWS tokens and private keys. The
   shipped documents contain no `/home/` or host paths. There are no SBOM or
   provenance outputs on this lineage: the `feature/release-sbom` and
   `feature/release-packager-sbom` branches are not merged into
   `7069cdfc`. CI and release workflows use least-privilege `permissions:`
   and no `pull_request_target`.

## Release artifact inspection result

`scripts/build-release.sh --output-dir <scratch> --target-dir <cache>/release-packaging`
was run on this branch (a `--locked` release build). The built-in verifier
passed: "artifact structure, ownership, modes, checksum, privacy, and versions
verified" (CLI `emuwiz-cli 0.9.0`, GUI `emuwiz 0.9.0`). The tarball
`archivefs-v0.9.0-x86_64-linux.tar.gz` contains exactly these files:
`CHANGELOG.md`, `LICENSE`, `README.md`, `config.toml.example`, `install.sh`,
`emuwiz`, `emuwiz-cli`, the five `assets/branding/emuwiz-logo-*.png` files
and `assets/linux/io.github.kiehntre.emuwiz.desktop.in`. It contains no
`.env`, config, database, screenshot, key or token file, and the verifier's
binary `strings` scan found no home or maintainer path.

## Remaining accepted risks

- **Hasheous transport honours proxy variables.** Only a file hash is sent,
  over HTTPS to a fixed public host, so a user-configured proxy is an
  ordinary network choice there, not a credential leak. It was left
  unchanged.
- **DNS rebinding for RomM.** Already documented: the validated address is
  not pinned into the socket.
- **`RommToken::persist_to` / `RommToken::load_from`** are public but have no
  production caller (tests only). `load_from` skips the permission checks
  that `load_token_file` applies. They are not reachable today, so they were
  not changed. A future caller should use `load_token_file`.
- **GUI `/tmp/archivefs-rename-transactions` fallback**
  (`archivefs-gui/src/dat_sources_page.rs`) is unreachable. It only applies
  when `HOME` cannot be resolved, and the only caller returns early in that
  case, because the DAT registry path needs `HOME` too.
- **Release workflow interpolates `steps.vars.outputs.tag` into `run:`
  scripts.** The tag must already exist in the repository, and both trigger
  paths need write access, so this does not escalate privilege. It is
  recorded here and was not changed.
- **Same-user TOCTOU** between the token-file `symlink_metadata` check and
  the read, and the same-user races already listed as out of scope in
  `SECURITY.md`.

## Validation

- `cargo test -p archivefs-core --lib -- identity_source::romm:: identity_source::artwork`:
  181 passed.
- `cargo test -p archivefs-core --lib`: 9016 passed, 36 failed. The same 36
  tests fail at the base commit `7069cdfc` (9015 passed, 36 failed; the extra
  pass is the new test). They depend on the host (for example, PCSX2
  binding reports "2 viable executables", and some database alias
  assertions). The new test is not among them.
- `cargo check --workspace --all-targets`: clean.
- `cargo clippy -p archivefs-core --lib --tests -- -D warnings`: the changed
  code is clean. At the base commit, the pinned 1.97.1 toolchain already
  reports `needless_lifetimes` and `needless_borrow` in
  `src/evidence_resolution.rs` and `manual_is_multiple_of` in
  `examples/evidence_resolution_benchmark.rs`. These are not security issues
  and were left alone to stay in scope, but they will fail the CI clippy job
  as it stands.
- `cargo fmt --all --check`: clean. `git diff --check`: clean.
- `scripts/task-postcheck.sh`: not present in this repository.
