#!/bin/sh
# shellcheck source=common.sh
. "$(dirname -- "$0")/common.sh"

keep=$(setting TURNESTA_BACKUP_KEEP)
remote=$(setting TURNESTA_BACKUP_REMOTE)
recipient=${TURNESTA_BACKUP_AGE_RECIPIENT:-$(setting TURNESTA_BACKUP_AGE_RECIPIENT)}
ping_url=${TURNESTA_BACKUP_PING_URL:-$(setting TURNESTA_BACKUP_PING_URL)}
work=''
dump_pid=''
verify_pid=''

monitor_ping() {
    case "$1" in http://*|https://*) ;; *) return 1 ;; esac
    case "$1" in *'"'*|*\\*|*'
'*) return 1 ;; esac
    printf 'url = "%s"\n' "$1" | curl -fsS -m 10 --config - > /dev/null 2>&1
}

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ -n "$dump_pid" ]; then kill "$dump_pid" 2>/dev/null || true; wait "$dump_pid" 2>/dev/null || true; fi
    if [ -n "$verify_pid" ]; then kill "$verify_pid" 2>/dev/null || true; wait "$verify_pid" 2>/dev/null || true; fi
    if [ -n "$work" ]; then
        rm -f "$work/dump.pipe" "$work/verify.pipe" "$work/tee-status" "$work/archive" || echo 'Could not remove temporary backup files.' >&2
        rmdir "$work" 2>/dev/null || true
    fi
    if [ "$status" -ne 0 ] && [ -n "$ping_url" ]; then monitor_ping "$ping_url/fail" || true; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

keep=${keep:-14}
case "$keep" in *[!0-9]*) fail 'TURNESTA_BACKUP_KEEP must be a positive integer.' ;; esac
[ "$keep" -gt 0 ] 2>/dev/null || fail 'TURNESTA_BACKUP_KEEP must be a positive integer.'
if [ -n "$recipient" ]; then
    command -v age > /dev/null 2>&1 || fail 'age is required when TURNESTA_BACKUP_AGE_RECIPIENT is set.'
else
    echo 'WARNING: this backup is not encrypted. Configure TURNESTA_BACKUP_AGE_RECIPIENT.' >&2
fi
if [ -n "$ping_url" ]; then command -v curl > /dev/null 2>&1 || fail 'curl is required when TURNESTA_BACKUP_PING_URL is set.'; fi

umask 077
acquire_backup_lock
latest=''
for candidate in backups/turnesta-*.dump backups/turnesta-*.dump.age; do
    [ -f "$candidate" ] || continue
    if [ -z "$latest" ] || [ "$candidate" -nt "$latest" ]; then latest=$candidate; fi
done
if [ -n "$latest" ]; then
    size=$(stat -c '%s' -- "$latest") || fail 'Could not determine the last backup size.'
    disk=$(df -Pk backups) || fail 'Could not determine available backup space.'
    available=$(printf '%s\n' "$disk" | awk 'END {print $4}')
    printf '%s\n' "$available" | grep -Eq '^[0-9]+$' || fail 'Could not determine available backup space.'
    awk -v free="$available" -v previous="$size" 'BEGIN {exit !(free * 1024 >= previous * 2)}' || fail 'Insufficient free space for the next backup.'
fi

work=$(mktemp -d backups/.turnesta-backup.XXXXXXXX)
file="backups/turnesta-$(date -u +%Y%m%dT%H%M%SZ)-$$.dump"
if [ -n "$recipient" ]; then
    file="$file.age"
    mkfifo "$work/dump.pipe" "$work/verify.pipe"
    compose exec -T db pg_dump --format=custom -U turnesta -d turnesta > "$work/dump.pipe" 9>&- &
    dump_pid=$!
    (
        if compose exec -T db pg_restore --list > /dev/null; then result=0; else result=$?; fi
        cat > /dev/null
        exit "$result"
    ) < "$work/verify.pipe" 9>&- &
    verify_pid=$!
    if (if tee "$work/verify.pipe" < "$work/dump.pipe"; then echo ok > "$work/tee-status"; fi) 9>&- |
        age -r "$recipient" 9>&- > "$work/archive"; then encrypted=true; else encrypted=false; fi
    if wait "$dump_pid"; then dumped=true; else dumped=false; fi
    dump_pid=''
    if wait "$verify_pid"; then verified=true; else verified=false; fi
    verify_pid=''
    [ "$encrypted" = true ] && [ "$dumped" = true ] && [ "$verified" = true ] && [ "$(cat "$work/tee-status" 2>/dev/null || true)" = ok ] || fail 'Encrypted backup or archive verification failed.'
else
    compose exec -T db pg_dump --format=custom -U turnesta -d turnesta 9>&- > "$work/archive"
    compose exec -T db pg_restore --list 9>&- < "$work/archive" > /dev/null
fi

ln "$work/archive" "$file" || fail 'Could not publish the backup without replacing an existing file.'
rm "$work/archive"
if [ -n "$remote" ]; then rclone copy "$file" "$remote" 9>&-; fi

{
    for candidate in backups/turnesta-*.dump backups/turnesta-*.dump.age; do
        [ -f "$candidate" ] || continue
        stat -c '%Y %n' -- "$candidate"
    done
} | sort -rn | awk -v keep="$keep" 'NR > keep {sub(/^[^ ]* /, ""); print}' | while IFS= read -r old; do
    rm -f -- "$old" || exit 1
done

if [ -n "$ping_url" ] && ! monitor_ping "$ping_url"; then fail 'Backup saved, but the monitoring success ping failed.'; fi
echo "$file"
