# DOSBox sandbox on Ubuntu (AppArmor)

EmuWiz runs DOSBox only inside a [bubblewrap](https://github.com/containers/bubblewrap)
sandbox (`/usr/bin/bwrap`). It refuses to start DOSBox when the sandbox cannot be
created. There is no unsandboxed fallback.

Ubuntu 24.04 and later set `kernel.apparmor_restrict_unprivileged_userns=1`. A
program that is not covered by an AppArmor profile, and that creates a user
namespace, is placed in the `unprivileged_userns` profile, which denies all
capabilities. Bubblewrap needs them to build its sandbox, so EmuWiz refuses DOSBox
with a message naming this setting. Whether a given machine is affected depends
on its configuration: check with
`bwrap --ro-bind / / --unshare-all true && echo ok`.

## What EmuWiz does and does not do

EmuWiz never installs or enables an AppArmor profile, never changes the sysctl,
and never runs DOSBox without the sandbox. The choice below is yours.

## Do not turn the restriction off

`sysctl kernel.apparmor_restrict_unprivileged_userns=0` re-opens user namespaces
to every program on the machine.

## Ubuntu's own profile: `bwrap-userns-restrict`

Ubuntu ships a profile for bubblewrap in the `apparmor-profiles` package as an
*extra* profile. It is not enabled by default:

```sh
sudo apt install apparmor-profiles
sudo install -m 0644 /usr/share/apparmor/extra-profiles/bwrap-userns-restrict /etc/apparmor.d/
sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict
```

Check: `bwrap --ro-bind / / --unshare-all cat /proc/self/attr/current` prints
`bwrap//&unpriv_bwrap (enforce)`, and the sysctl above still reads `1`.

Remove: `sudo apparmor_parser -R /etc/apparmor.d/bwrap-userns-restrict && sudo rm /etc/apparmor.d/bwrap-userns-restrict`.

**Read this before enabling it. It is not limited to EmuWiz.**

- It attaches to `/usr/bin/bwrap` whoever runs it. **Every program and user on the
  machine that runs bubblewrap** (Flatpak, GNOME thumbnailers and others, not only
  EmuWiz) is affected. Programs not started through bubblewrap keep the restriction.
- Bubblewrap itself gets user namespaces, mounts and **all capabilities** inside
  its namespace. That is what it needs to build a sandbox.
- **Everything bubblewrap starts runs confined** (`unpriv_bwrap`, stacked): all
  capabilities are denied, also inside any user namespace it creates (the same
  limit the restriction imposes); files, network and D-Bus are allowed. The confinement removes capabilities; it is not a file or network
  sandbox. EmuWiz's own bubblewrap arguments provide that, unchanged: only the
  game folder is writable, there is no network, and nested user namespaces are
  forbidden.
- Ubuntu tried enabling this profile by default in 2024 and reverted it because
  some Flatpak apps could no longer save files (LP: #2072811). Enabling it may
  affect Flatpak apps on your machine.
- It re-exposes the kernel's user-namespace code (the attack surface the
  restriction shrinks) to anything that runs bubblewrap. Any local user can call
  bubblewrap with arguments of their choosing, and bubblewrap holds full
  capabilities in its own namespace, so its mount operations (including
  `--overlay`) become reachable to every local user.

Debian, Fedora and Arch do not restrict user namespaces this way, and bubblewrap
works there without any profile.

## What CI checks

The `DOSBox sandbox enforcement` job runs on Ubuntu 24.04 and on the newest Ubuntu
runner (26.04, AppArmor 5, when this was written, whose stock setup lets bubblewrap
through). The restriction is never turned off:

1. It records what the runner's stock AppArmor does with bwrap. Where it denies
   bwrap (24.04), EmuWiz's real probe and launch must be refused there, nothing
   may start, and the message must name the AppArmor setting.
2. It runs EmuWiz's real probe and launch under an AppArmor denial
   (`aa-exec -p unprivileged_userns`). The sandbox must be refused for a permission
   reason, nothing may start, and the message must name the AppArmor setting.
   This proves EmuWiz fails closed on an AppArmor denial. It is not an exact copy of
   a stock machine: there bwrap is refused later (when it needs capabilities in the
   new namespace) than under `aa-exec`, where it may be refused earlier.
3. Where stock AppArmor lets plain bwrap through, it runs every DOSBox sandbox
   test in that stock setup.
4. It enables `bwrap-userns-restrict` (installed from the extra profiles where the
   distribution does not already ship it), checks that bubblewrap's children
   really run as `bwrap//&unpriv_bwrap`, and runs every DOSBox sandbox test again.
   None may skip in either run.
