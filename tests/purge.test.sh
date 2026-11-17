#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
mkdir -p "$scratch/scripts" "$scratch/bin"
cp "$root/scripts/common.sh" "$root/scripts/purge.sh" "$scratch/scripts/"
cat > "$scratch/bin/docker" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" > "$PURGE_TEST_TRACE"
EOF
chmod +x "$scratch/bin/docker"
export PURGE_TEST_TRACE="$scratch/trace"
PATH="$scratch/bin:$PATH"
export PATH

: > "$scratch/.env"
sh "$scratch/scripts/purge.sh" --dry-run
grep -F -- '--profile tools run --rm --no-deps -T migrator ops purge --dry-run' "$PURGE_TEST_TRACE" > /dev/null

printf '%s\n' 'TURNESTA_DOMAIN=example.test' > "$scratch/.env"
sh "$scratch/scripts/purge.sh" --dry-run
grep -F -- '--file deploy/compose.prod.yaml --profile tools run --rm --no-deps -T inspector ops purge --dry-run' "$PURGE_TEST_TRACE" > /dev/null
