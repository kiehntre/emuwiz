# DOSBox sandbox on Ubuntu (AppArmor)

EmuWiz runs DOSBox only inside a [bubblewrap](https://github.com/containers/bubblewrap)
sandbox and refuses to start it when the sandbox cannot be created. There is no
unsandboxed fallback.

Ubuntu 24.04 and later set `kernel.apparmor_restrict_unprivileged_userns=1`. That
denies the unprivileged user namespaces bubblewrap needs, so on a stock install
EmuWiz refuses DOSBox with a message naming this setting.

## What EmuWiz does and does not do

EmuWiz never installs an AppArmor profile, never changes the sysctl, and never
falls back to running DOSBox unsandboxed. If the sandbox cannot be created, the
launch is refused before anything starts. The decision below is yours.

## Prefer a distribution profile where one exists

Newer Ubuntu releases ship a profile for bubblewrap (`/etc/apparmor.d/bwrap-userns-restrict`).
It allows the namespaces and also confines what runs inside them. If your system
has that file, use it and skip the next section. Ubuntu 24.04 (AppArmor
4.0.1-0ubuntu0.24.04.9) ships no bubblewrap profile, only the generic
`unprivileged_userns` profile, and that profile denies what bubblewrap needs.

## Ubuntu 24.04: a profile for /usr/bin/bwrap

Do **not** turn the restriction off (`sysctl ... =0`). That re-opens user namespaces
to every program. The profile in [`docs/apparmor/bwrap`](apparmor/bwrap) exempts a
single executable, `/usr/bin/bwrap`. It follows Ubuntu's documented pattern for
applications that need user namespaces.

**Read this before installing it. The exemption is not limited to EmuWiz.**

- It applies to `/usr/bin/bwrap` whoever runs it. **Any program or user on the
  machine** can then run bubblewrap and create user namespaces through it, as on
  Ubuntu before 24.04. Every other executable stays restricted.
- `flags=(unconfined)` means bubblewrap, and everything it starts, runs with **no
  AppArmor confinement**: exactly as if no profile existed and the restriction were
  off for that one program. The profile adds only the `userns` permission. It does
  not limit files, network or capabilities inside the namespace.
- The kernel's user-namespace code is the attack surface Ubuntu's restriction
  shrinks. This profile re-exposes it to anything that runs bubblewrap. It does
  not affect programs that do not.
- EmuWiz's own sandbox is unchanged by the profile. It still forbids nested user
  namespaces, unshares networking, and exposes only the game folder writable.

Install (as root, deliberately):

```sh
sudo install -m 0644 docs/apparmor/bwrap /etc/apparmor.d/bwrap
sudo apparmor_parser -r /etc/apparmor.d/bwrap
```

Check: `bwrap --ro-bind / / --unshare-all true && echo ok` prints `ok`, and
`cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns` still prints `1`.

Remove: `sudo apparmor_parser -R /etc/apparmor.d/bwrap && sudo rm /etc/apparmor.d/bwrap`.

Debian, Fedora and Arch do not restrict user namespaces this way. Bubblewrap
works there without any profile.

CI checks both cases with the restriction left on: stock AppArmor denies the
sandbox and EmuWiz fails closed with this hint; with only this profile loaded,
every sandbox test runs and none may skip.
