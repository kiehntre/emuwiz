#!/bin/sh
# Regression: tests/test_install.sh must not inherit configuration/data location
# overrides from the caller (a CI runner exporting XDG_CONFIG_HOME made the
# fresh-install assertions look for ~/.config/emuwiz/config.toml while the
# installer, correctly, wrote under the exported location).
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
out=$(EMUWIZ_INSTALL_TEST_ENV_PROBE=1 HOME="$scratch/home" \
    EMUWIZ_CONFIG_HOME="$scratch/explicit" XDG_CONFIG_HOME="$scratch/xdg" \
    XDG_DATA_HOME="$scratch/data" sh "$here/test_install.sh" </dev/null)
want='EMUWIZ_CONFIG_HOME=
XDG_CONFIG_HOME=
XDG_DATA_HOME='
if [ "$out" != "$want" ]; then
    printf 'FAIL: test_install.sh inherited location overrides:\n%s\n' "$out" >&2
    exit 1
fi
printf 'ok - test_install.sh scrubs EMUWIZ_CONFIG_HOME, XDG_CONFIG_HOME and XDG_DATA_HOME\n'
