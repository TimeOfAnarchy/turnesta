#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
mkdir -p "$work/scripts" "$work/deploy" "$work/bin"
cp "$root/scripts/common.sh" "$work/scripts/common.sh"
cp "${UPDATE_SCRIPT:-$root/scripts/update.sh}" "$work/scripts/update.sh"

old=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
new=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
old_ref=registry.example/app@$old
new_ref=registry.example/app@$new
export OLD_IMAGE=$old NEW_IMAGE=$new TEST_LOG="$work/docker.log"
export PATH="$work/bin:$PATH"
unset TURNESTA_IMAGE

cat > "$work/bin/docker" <<'DOCKER'
#!/bin/sh
set -eu
echo "$*" >> "$TEST_LOG"
case "$1" in
    compose)
        shift
        while [ "$#" -gt 0 ]; do
            case "$1" in
                version) exit 0 ;;
                ps) echo api-container; exit 0 ;;
                pull) [ "$2" = api ] && [ "$#" = 2 ]; exit "${TEST_PULL_EXIT:-0}" ;;
                stop) [ "$*" = 'stop api worker' ]; exit "${TEST_STOP_EXIT:-0}" ;;
                up) [ "$*" = 'up --pull never --wait' ] || [ "$*" = 'up --pull always --wait' ]; exit "${TEST_UP_EXIT:-0}" ;;
                run)
                    if [ "$*" = 'run --rm --no-deps inspector' ]; then exit "${TEST_INSPECT_EXIT:-0}"; fi
                    cat > /dev/null
                    echo '{"status":"created"}'
                    exit 0 ;;
            esac
            shift
        done
        exit 1 ;;
    inspect)
        if [ "$2" = --format ] && [ "$3" = '{{.State.Running}}' ]; then
            echo "${TEST_RUNNING:-true}"
        else
            echo "${TEST_CURRENT:-$OLD_IMAGE}"
        fi ;;
    info) exit 0 ;;
    tag) exit 0 ;;
    image)
        [ "$2" = inspect ]; shift 2
        format=''
        if [ "${1:-}" = --format ]; then format=$2; shift 2; fi
        image=$1
        case "$image" in
            turnesta-update-previous:*) exit 1 ;;
            *"$OLD_IMAGE"*|registry.example/app:one) id=$OLD_IMAGE ;;
            *"$NEW_IMAGE"*|registry.example/app:two) id=$NEW_IMAGE ;;
            *) exit 1 ;;
        esac
        case "$format" in
            *RepoDigests*) [ "${TEST_LOCAL:-false}" = true ] || echo "registry.example/app@$id" ;;
            *Id*) echo "$id" ;;
            *) echo '[]' ;;
        esac ;;
    buildx)
        [ "$2 $3" = 'imagetools inspect' ]
        [ "${TEST_BUILD_DOWN:-false}" = false ] || exit 1
        case "$4" in
            *missing*) exit 1 ;;
            ghcr.io/timeofanarchy/turnesta:latest)
                [ "${TEST_REGISTRY_DOWN:-false}" = false ] || exit 1
                echo "Digest: $NEW_IMAGE" ;;
            *two*|*"$NEW_IMAGE"*) echo "Digest: $NEW_IMAGE" ;;
            *one*|*"$OLD_IMAGE"*) echo "Digest: $OLD_IMAGE" ;;
            *) exit 1 ;;
        esac ;;
    *) exit 1 ;;
esac
DOCKER
chmod +x "$work/bin/docker" "$work/scripts/update.sh"
cat > "$work/scripts/backup.sh" <<'BACKUP'
#!/bin/sh
echo backup >> "$TEST_LOG"
echo backups/test.dump
BACKUP
chmod +x "$work/scripts/backup.sh"

cd "$work"
reset_env() {
    printf 'TURNESTA_DOMAIN=example.test\nTURNESTA_IMAGE=%s\nPRIVATE_VALUE=preserved\n' "$1" > .env
    : > "$TEST_LOG"
}

reset_env "$old_ref"
TEST_BUILD_DOWN=true ./scripts/update.sh > "$work/noop.out"
grep -q 'no update is needed' "$work/noop.out"
[ ! -e .env.before-update ]
if grep -q 'compose .* pull\|compose .* up\|^backup$' "$TEST_LOG"; then exit 1; fi

reset_env "$old_ref"
cp .env "$work/original"
if ./scripts/update.sh registry.example/app:missing > "$work/missing.out" 2>&1; then exit 1; fi
cmp .env "$work/original"
[ ! -e .env.before-update ]
if grep -q '^backup$\|compose .* up\|compose .* pull' "$TEST_LOG"; then exit 1; fi

./scripts/update.sh registry.example/app:two > "$work/success.out"
grep -q "^TURNESTA_IMAGE=$new_ref$" .env
cmp .env.before-update "$work/original"
grep -q 'compose .* pull api' "$TEST_LOG"
grep -q 'compose .* up --pull never --wait' "$TEST_LOG"
grep -q '^backup$' "$TEST_LOG"
awk '/^backup$/ {backup=NR} /compose .* stop api worker$/ {stop=NR} /compose .* up --pull never --wait$/ {up=NR} END {exit !(backup > 0 && backup < stop && stop < up)}' "$TEST_LOG"
grep -Fq "./scripts/update.sh '$old_ref'" "$work/success.out"

reset_env "$old_ref"
if TEST_INSPECT_EXIT=42 ./scripts/update.sh registry.example/app:two > "$work/failure.out" 2>&1; then exit 1; fi
cmp .env "$work/original"
cmp .env.before-update "$work/original"
grep -q 'env restored' "$work/failure.out" || grep -q '\.env restored' "$work/failure.out"
grep -Fq "./scripts/update.sh '$old_ref'" "$work/failure.out"

reset_env "$old_ref"
if TEST_UP_EXIT=9 ./scripts/update.sh registry.example/app:two > "$work/up-failure.out" 2>&1; then exit 1; fi
cmp .env "$work/original"
grep -Fq "./scripts/update.sh '$old_ref'" "$work/up-failure.out"

reset_env "$old_ref"
if TEST_STOP_EXIT=8 ./scripts/update.sh registry.example/app:two > "$work/stop-failure.out" 2>&1; then exit 1; fi
cmp .env "$work/original"
if grep -q 'compose .* up' "$TEST_LOG"; then exit 1; fi

reset_env "$old_ref"
TEST_RUNNING=false ./scripts/update.sh "$old_ref" > "$work/stopped.out"
grep -q 'compose .* up --pull never --wait' "$TEST_LOG"

reset_env "$old_ref"
if TEST_PULL_EXIT=1 ./scripts/update.sh registry.example/app:two > "$work/pull-failure.out" 2>&1; then exit 1; fi
cmp .env "$work/original"
if grep -q '^backup$\|compose .* up' "$TEST_LOG"; then exit 1; fi

reset_env registry.example/app:one
TEST_LOCAL=true ./scripts/update.sh registry.example/app:two > "$work/local.out"
grep -Fq "./scripts/update.sh '$old'" "$work/local.out"
reset_env "$new_ref"
TEST_LOCAL=true TEST_CURRENT="$new" ./scripts/update.sh "$old" > "$work/local-rollback.out"
grep -q "^TURNESTA_IMAGE=$old$" .env
if grep -q 'compose .* pull' "$TEST_LOG"; then exit 1; fi

mkdir -p "$work/install/scripts" "$work/install/deploy"
cp "$root/scripts/common.sh" "$work/install/scripts/"
awk '/^schedule_backups\(\)/ {exit} {print}' "$root/scripts/install.sh" > "$work/install/scripts/install-probe.sh"
printf '\ncreate_server_env\n' >> "$work/install/scripts/install-probe.sh"
cd "$work/install"
printf 'example.test\n' | sh scripts/install-probe.sh > "$work/install.out"
grep -q "^TURNESTA_IMAGE=ghcr.io/timeofanarchy/turnesta@$new$" .env

mkdir -p "$work/install-failure/scripts" "$work/install-failure/deploy"
cp "$work/install/scripts/common.sh" "$work/install/scripts/install-probe.sh" "$work/install-failure/scripts/"
cd "$work/install-failure"
if printf 'example.test\n' | TEST_REGISTRY_DOWN=true sh scripts/install-probe.sh > "$work/install-failure.out" 2>&1; then exit 1; fi
[ ! -e .env ]

for domain in example..test -example.test example-.test example.test. localhost; do
    if printf '%s\n' "$domain" | sh scripts/install-probe.sh > "$work/invalid-domain.out" 2>&1; then exit 1; fi
    [ ! -e .env ]
done

awk '/^schedule_backups\(\)/ {exit} {print}' "$root/scripts/install.sh" > scripts/local-probe.sh
printf '\ncreate_local_env\n' >> scripts/local-probe.sh
for port in 0 65536 999999999999999999999 abc; do
    if printf '%s\n' "$port" | sh scripts/local-probe.sh > "$work/invalid-port.out" 2>&1; then exit 1; fi
    [ ! -e .env ]
done

echo 'Deployment update regression tests passed.'
