#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
mkdir -p "$work/scripts" "$work/bin" "$work/deploy"
cp "$root/scripts/common.sh" "$root/scripts/backup.sh" "$root/scripts/restore.sh" "$work/scripts/"
export TEST_ROOT="$work" PATH="$work/bin:$PATH"
unset TURNESTA_BACKUP_AGE_RECIPIENT TURNESTA_BACKUP_AGE_IDENTITY TURNESTA_BACKUP_PING_URL

cat > "$work/bin/docker" <<'DOCKER'
#!/bin/sh
set -eu
echo "$*" >> "$TEST_ROOT/docker.log"
case " $* " in
    *' pg_dump '*)
        printf 'PGDMP-FIXTURE'
        if [ "${TEST_DUMP_DELAY:-0}" -gt 0 ]; then : > "$TEST_ROOT/dump-started"; sleep "$TEST_DUMP_DELAY"; fi
        [ "${TEST_DUMP_FAIL:-false}" = false ] || exit 7
        printf '%s\n' '-data' ;;
    *' pg_restore --list '*)
        content=$(cat)
        [ "$content" = 'PGDMP-FIXTURE-data' ] && [ "${TEST_VERIFY_FAIL:-false}" = false ] ;;
    *' pg_restore --no-password '*)
        content=$(cat)
        [ "$content" = 'PGDMP-FIXTURE-data' ] && [ "${TEST_RESTORE_FAIL:-false}" = false ] ;;
    *' psql '*|*' compose '*|compose\ *) exit 0 ;;
    *) exit 1 ;;
esac
DOCKER

cat > "$work/bin/age" <<'AGE'
#!/bin/sh
set -eu
case "$1" in
    -r)
        [ "${TEST_ENCRYPT_FAIL:-false}" = false ] || exit 1
        printf 'age-test\n'
        base64 ;;
    -d)
        [ "${TEST_DECRYPT_FAIL:-false}" = false ] || exit 1
        if [ "${TEST_DECRYPT_FAIL_SECOND:-false}" = true ]; then
            if [ -e "$TEST_ROOT/decrypted-once" ]; then printf 'partial'; exit 1; fi
            : > "$TEST_ROOT/decrypted-once"
        fi
        file=$4
        { IFS= read -r header; [ "$header" = age-test ]; base64 -d; } < "$file" ;;
    *) exit 1 ;;
esac
AGE

cat > "$work/bin/curl" <<'CURL'
#!/bin/sh
set -eu
config=$(cat)
printf '%s\n' "$config" >> "$TEST_ROOT/pings.log"
case "$config" in
    *'/fail"'*) exit 0 ;;
    *) [ "${TEST_PING_FAIL:-false}" = false ] ;;
esac
CURL

cat > "$work/bin/df" <<'DF'
#!/bin/sh
if [ "${TEST_LOW_SPACE:-false}" = true ]; then
    printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\ntest 100 100 0 100%% /\n'
else
    exec /bin/df "$@"
fi
DF
cat > "$work/bin/rclone" <<'RCLONE'
#!/bin/sh
[ "${TEST_REMOTE_FAIL:-false}" = false ]
RCLONE
chmod +x "$work/bin/docker" "$work/bin/age" "$work/bin/curl" "$work/bin/df" "$work/bin/rclone"

cd "$work"
cat > .env <<'ENV'
TURNESTA_DOMAIN=example.test
TURNESTA_BACKUP_KEEP=14
TURNESTA_BACKUP_REMOTE=
TURNESTA_BACKUP_AGE_RECIPIENT=age1synthetic
TURNESTA_BACKUP_PING_URL=https://monitor.example/check
ENV
: > "$work/identity"
archive=$(sh scripts/backup.sh)
case "$archive" in *.dump.age) ;; *) exit 1 ;; esac
grep -q '^age-test$' "$archive"
if grep -q 'PGDMP-FIXTURE' "$archive"; then exit 1; fi
[ "$(age -d -i "$work/identity" "$archive")" = 'PGDMP-FIXTURE-data' ]
grep -Fq 'url = "https://monitor.example/check"' "$work/pings.log"

before=$(find backups -maxdepth 1 -name '*.dump.age' | wc -l)
for mode in TEST_DUMP_FAIL TEST_VERIFY_FAIL TEST_ENCRYPT_FAIL TEST_LOW_SPACE; do
    if env "$mode=true" sh scripts/backup.sh > "$work/$mode.out" 2>&1; then exit 1; fi
    after=$(find backups -maxdepth 1 -name '*.dump.age' | wc -l)
    [ "$before" -eq "$after" ]
done
grep -Fq 'url = "https://monitor.example/check/fail"' "$work/pings.log"
if TEST_PING_FAIL=true sh scripts/backup.sh > "$work/ping-failure.out" 2>&1; then exit 1; fi
grep -q 'monitoring success ping failed' "$work/ping-failure.out"
grep -Fq 'url = "https://monitor.example/check/fail"' "$work/pings.log"

printf 'TURNESTA_BACKUP_REMOTE=synthetic\n' >> .env
if TEST_REMOTE_FAIL=true sh scripts/backup.sh > "$work/remote-failure.out" 2>&1; then exit 1; fi
sed -i '/^TURNESTA_BACKUP_REMOTE=synthetic$/d' .env

TEST_DUMP_DELAY=4 sh scripts/backup.sh > "$work/first.out" 2> "$work/first.err" &
first=$!
count=0
while [ ! -e "$work/dump-started" ]; do
    kill -0 "$first" || exit 1
    count=$((count + 1)); [ "$count" -lt 100 ] || exit 1
    sleep .1
done
if find backups -type f -exec grep -l 'PGDMP-FIXTURE' {} + | grep -q .; then exit 1; fi
sh scripts/backup.sh > "$work/second.out" 2> "$work/second.err" &
second=$!
sleep 1
kill -0 "$second"
wait "$first"
wait "$second"

: > "$work/docker.log"
if sh scripts/restore.sh "$archive" > "$work/no-identity.out" 2>&1; then exit 1; fi
grep -q 'identity file is required' "$work/no-identity.out"
[ ! -s "$work/docker.log" ]
if TEST_DECRYPT_FAIL=true sh scripts/restore.sh "$archive" "$work/identity" > "$work/corrupt.out" 2>&1; then exit 1; fi
[ ! -s "$work/docker.log" ]
if printf 'restore\n' | TEST_DECRYPT_FAIL_SECOND=true sh scripts/restore.sh "$archive" "$work/identity" > "$work/restore-failure.out" 2>&1; then exit 1; fi
if grep -q 'ALTER DATABASE' "$work/docker.log"; then exit 1; fi

: > "$work/docker.log"
printf 'restore\n' | TURNESTA_BACKUP_AGE_IDENTITY="$work/identity" sh scripts/restore.sh "$archive" > "$work/restore-success.out"
grep -q 'ALTER DATABASE turnesta RENAME TO turnesta_previous' "$work/docker.log"
grep -q 'ALTER DATABASE turnesta_restore RENAME TO turnesta' "$work/docker.log"

mkdir -p "$work/plain/scripts"
cp "$work/scripts/common.sh" "$work/scripts/backup.sh" "$work/plain/scripts/"
printf 'TURNESTA_DOMAIN=example.test\nTURNESTA_BACKUP_KEEP=14\n' > "$work/plain/.env"
cd "$work/plain"
plain=$(sh scripts/backup.sh 2> "$work/plain.err")
case "$plain" in *.dump) ;; *) exit 1 ;; esac
grep -q 'not encrypted' "$work/plain.err"
grep -q 'PGDMP-FIXTURE-data' "$plain"

mkdir -p "$work/logrotate/etc/cron.d" "$work/logrotate/etc/logrotate.d"
awk '/^schedule_backups\(\)/ { capture=1 } /^if \[ "\$\{1:-\}" = --server \]/ { capture=0 } capture' "$root/scripts/install.sh" |
    sed "s|/etc/cron.d|$work/logrotate/etc/cron.d|g; s|/etc/logrotate.d|$work/logrotate/etc/logrotate.d|g" > "$work/logrotate/probe.sh"
printf '\nschedule_backups\n' >> "$work/logrotate/probe.sh"
sh "$work/logrotate/probe.sh" > /dev/null
for required in weekly 'rotate 8' compress missingok; do grep -q "$required" "$work/logrotate/etc/logrotate.d/turnesta"; done
printf 'existing policy\n' > "$work/logrotate/etc/logrotate.d/turnesta"
sh "$work/logrotate/probe.sh" > /dev/null
grep -qx 'existing policy' "$work/logrotate/etc/logrotate.d/turnesta"

echo 'Backup and restore regression tests passed.'
