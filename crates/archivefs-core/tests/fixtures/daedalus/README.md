# Daedalus diagnostic fixtures

Real output, captured on 2026-11-13 and reduced only by replacing host-specific
paths, the CPU and GPU names and process ids.

`retroarch-1.21.0/`: RetroArch 1.21.0 (Ubuntu 24.04 package, built 2025-05-28) started
with `--verbose` against deliberately broken setups, in a private X display with
disposable settings and the bundled `2048` and `gambatte` cores:

| File | Setup | Exit |
|---|---|---|
| `vk-noicd.log` | `video_driver = "vulkan"`, no Vulkan driver in the environment | status 1 |
| `vk-noicd-quiet.stderr` | the same, without `--verbose` | status 1 |
| `vulkan-ok.log`, `gl-ok.log` | working Vulkan / OpenGL | status 0 |
| `core-ignored.log` | `-L` pointing at a path that does not exist | status 0 |
| `core-not-a-library.log` | `-L` pointing at a text file | status 1 |
| `content-missing.log` | game file does not exist | status 1 |
| `content-unreadable.log` | game file exists with mode 000 | status 1 |
| `content-garbage.log` | game file is random bytes | status 1 |
| `save-dir-read-only.log` | save folder not writable | status 0 |
| `silent-core-audio.log` | core that produces no sound | status 0 |
| `no-display.stderr` | `DISPLAY` unset | SIGABRT |

`host/`: the dynamic loader (glibc 2.39), Flatpak 1.14.6 and bubblewrap 0.9.0 refusing
to start a program.
