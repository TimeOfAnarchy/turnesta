#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
mkdir -p "$work/scripts" "$work/bin" "$work/backups"
cp "$root/scripts/common.sh" "$root/scripts/update.sh" "$root/scripts/backup.sh" "$root/scripts/restore.sh" "$work/scripts/"
chmod +x "$work/scripts/backup.sh"
REAL_FLOCK=$(command -v flock)
export TEST_ROOT="$work" REAL_FLOCK PATH="$work/bin:$PATH"
unset TURNESTA_IMAGE TURNESTA_BACKUP_AGE_RECIPIENT TURNESTA_BACKUP_PING_URL
old=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
a=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
b=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
pids=''
cleanup() {
    for pid in $pids; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

cat > "$work/bin/flock" <<'FLOCK'
#!/bin/sh
set -eu
if [ "${4:-}" = 7 ]; then
    : > "$TEST_ROOT/$CASE-$OP-lock-attempted"
    [ "${FAIL_LOCK:-false}" = false ] || exit 1
elif [ "${4:-}" = 9 ]; then
    : > "$TEST_ROOT/$CASE-$OP-backup-lock-attempted"
fi
exec "$REAL_FLOCK" "$@"
FLOCK
cat > "$work/bin/docker" <<'DOCKER'
#!/bin/sh
set -eu
mark="$TEST_ROOT/$CASE-$OP"
pause() {
    : > "$mark-entered"
    n=0
    while [ ! -e "$mark-release" ]; do
        n=$((n+1)); [ "$n" -lt 500 ] || exit 90
        sleep 0.02
    done
}
case "$1" in
    inspect)
        if [ "$3" = '{{.State.Running}}' ]; then echo true; else cat "$TEST_ROOT/running"; fi ;;
    image)
        case "$*" in *turnesta-update-previous*) exit 1 ;; esac
        if [ "${3:-}" = --format ]; then
            case "$4" in *RepoDigests*) exit 0 ;; esac
            printf '%s\n' "$5"
        fi ;;
    tag) exit 0 ;;
    compose)
        : > "$mark-docker"
        case " $* " in
            *' ps '*) sed -n 's/^TURNESTA_IMAGE=//p' "$TEST_ROOT/.env" > "$mark-read"; echo api-container ;;
            *' pg_dump '*)
                [ "${PAUSE_DUMP:-false}" = false ] || pause
                printf 'PGDMP-test' ;;
            *' pg_restore --list'*) [ "$(cat)" = PGDMP-test ] ;;
            *' pg_restore --no-password '*)
                [ "${PAUSE_RESTORE:-false}" = false ] || pause
                [ "$(cat)" = PGDMP-test ] ;;
            *' up --pull never --wait'*)
                printf '%s\n' "$TURNESTA_IMAGE" > "$TEST_ROOT/running"
                [ "${PAUSE_UP:-false}" = false ] || pause
                exit "${UP_EXIT:-0}" ;;
            *) exit 0 ;;
        esac ;;
    *) exit 1 ;;
esac
DOCKER
chmod +x "$work/bin/flock" "$work/bin/docker"
cd "$work"
printf PGDMP-test > backups/source.dump
reset_case() {
    CASE=$1; export CASE
    printf 'TURNESTA_DOMAIN=example.test\nTURNESTA_IMAGE=%s\nTURNESTA_BACKUP_KEEP=14\n' "$old" > .env
    printf '%s\n' "$old" > running
}
wait_for() {
    count=0
    while [ ! -e "$1" ]; do
        count=$((count+1))
        if [ "$count" -gt 250 ]; then echo "Timed out waiting for $1" >&2; exit 1; fi
        sleep 0.02
    done
}
assert_waiting() {
    # With the old script the contender reaches Docker instead of this lock.
    count=0
    while [ ! -e "$1-lock-attempted" ] && [ ! -e "$1-docker" ]; do
        count=$((count+1)); [ "$count" -lt 250 ] || exit 1
        sleep 0.02
    done
    [ -e "$1-lock-attempted" ] && [ ! -e "$1-docker" ] || {
        echo 'Contender entered maintenance before the previous operation completed.' >&2; exit 1;
    }
    if "$REAL_FLOCK" -n backups/.turnesta-maintenance.lock true; then
        echo 'The maintenance lock is not held.' >&2; exit 1
    fi
}

for outcome in failure success; do
    reset_case "$outcome"
    code=0; [ "$outcome" = success ] || code=42
    OP=A PAUSE_UP=true UP_EXIT=$code sh scripts/update.sh "$a" > "$CASE-A.log" 2>&1 &
    first=$!; pids="$pids $first"
    wait_for "$CASE-A-entered"
    OP=B sh scripts/update.sh "$b" > "$CASE-B.log" 2>&1 &
    second=$!; pids="$pids $second"
    assert_waiting "$CASE-B"
    : > "$CASE-A-release"
    if wait "$first"; then result=0; else result=$?; fi
    [ "$result" -eq "$code" ]
    wait "$second"
    expected=$old; [ "$outcome" = failure ] || expected=$a
    [ "$(cat "$CASE-B-read")" = "$expected" ]
    grep -q "^TURNESTA_IMAGE=$b$" .env
    [ "$(cat running)" = "$b" ]
done

reset_case update-restore
OP=A PAUSE_UP=true sh scripts/update.sh "$a" > "$CASE-A.log" 2>&1 &
first=$!; pids="$pids $first"
wait_for "$CASE-A-entered"
printf 'restore\n' | OP=R sh scripts/restore.sh backups/source.dump > "$CASE-R.log" 2>&1 &
second=$!; pids="$pids $second"
assert_waiting "$CASE-R"
: > "$CASE-A-release"
wait "$first"; wait "$second"

reset_case restore-update
printf 'restore\n' | OP=R PAUSE_RESTORE=true sh scripts/restore.sh backups/source.dump > "$CASE-R.log" 2>&1 &
first=$!; pids="$pids $first"
wait_for "$CASE-R-entered"
OP=B sh scripts/update.sh "$b" > "$CASE-B.log" 2>&1 &
second=$!; pids="$pids $second"
assert_waiting "$CASE-B"
: > "$CASE-R-release"
wait "$first"; wait "$second"

reset_case backup-update
OP=K PAUSE_DUMP=true sh scripts/backup.sh > "$CASE-K.log" 2>&1 &
first=$!; pids="$pids $first"
wait_for "$CASE-K-entered"
OP=B sh scripts/update.sh "$b" > "$CASE-B.log" 2>&1 &
second=$!; pids="$pids $second"
wait_for "$CASE-B-backup-lock-attempted"
: > "$CASE-K-release"
wait "$first"; wait "$second"
grep -q "^TURNESTA_IMAGE=$b$" .env

reset_case timeout
snapshots=$(find . -maxdepth 1 -name '.env.before-update*' | wc -l)
if OP=B FAIL_LOCK=true sh scripts/update.sh "$b" > "$CASE-B.log" 2>&1; then exit 1; fi
grep -q "^TURNESTA_IMAGE=$old$" .env
[ ! -e "$CASE-B-docker" ]
[ "$(find . -maxdepth 1 -name '.env.before-update*' | wc -l)" -eq "$snapshots" ]
grep -q 'maintenance lock' "$CASE-B.log"
echo 'Maintenance lock regression tests passed.'
