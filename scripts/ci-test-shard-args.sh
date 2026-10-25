# Sourced by the CI test steps: sets the bash arrays `packages` (from CI_TIER)
# and `selection` (from SHARD) from scripts/ci_test_shards.py, one argument
# per line, failing on any error. An empty selection stays an empty array.
packages_text=$(python3 -I scripts/ci_test_shards.py packages "$CI_TIER")
selection_text=$(python3 -I scripts/ci_test_shards.py selection "$SHARD")
packages=()
selection=()
[[ -z "$packages_text" ]] || mapfile -t packages <<< "$packages_text"
[[ -z "$selection_text" ]] || mapfile -t selection <<< "$selection_text"
((${#packages[@]} > 0)) || { echo "no cargo packages for tier $CI_TIER" >&2; exit 2; }
