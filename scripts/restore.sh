#!/bin/sh
# shellcheck source=common.sh
. "$(dirname -- "$0")/common.sh"

[ "$server" = true ] || fail 'restore.sh works with a server setup.'
[ "$#" -ge 1 ] && [ "$#" -le 2 ] && [ -f "$1" ] || fail 'Usage: ./scripts/restore.sh backups/turnesta-<date>.dump[.age] [identity-file]'
acquire_maintenance_lock
archive=$1
identity=${2:-${TURNESTA_BACKUP_AGE_IDENTITY:-$(setting TURNESTA_BACKUP_AGE_IDENTITY)}}
encrypted=false
case "$archive" in
    *.dump.age)
        encrypted=true
        [ -n "$identity" ] && [ -f "$identity" ] && [ -r "$identity" ] || fail 'An age identity file is required to restore an encrypted backup.'
        command -v age > /dev/null 2>&1 || fail 'age is required to restore an encrypted backup.'
        ;;
    *) [ "$#" -eq 1 ] || fail 'An identity file is only used with .dump.age backups.' ;;
esac

umask 077
acquire_backup_lock
work=''
decrypt_pid=''
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ -n "$decrypt_pid" ]; then kill "$decrypt_pid" 2>/dev/null || true; wait "$decrypt_pid" 2>/dev/null || true; fi
    if [ -n "$work" ]; then
        rm -f "$work/archive.pipe" || echo 'Could not remove the temporary restore pipe.' >&2
        rmdir "$work" 2>/dev/null || true
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

if [ "$encrypted" = true ]; then
    exec 8< "$archive"
    age -d -i "$identity" /proc/self/fd/8 9>&- > /dev/null || fail 'The backup cannot be decrypted and authenticated with this identity.'
fi

sql() {
    compose exec -T db psql --no-psqlrc --quiet -v ON_ERROR_STOP=1 -U turnesta -d "$1" -c "$2" 9>&-
}

printf 'This replaces all current data with %s. Type "restore" to continue: ' "$archive"
IFS= read -r confirmation || fail 'Restore cancelled.'
[ "$confirmation" = restore ] || fail 'Restore cancelled.'

compose up --detach --wait db 9>&-
compose run --rm --no-deps db-bootstrap 9>&-

sql postgres 'DROP DATABASE IF EXISTS turnesta_restore'
sql postgres 'CREATE DATABASE turnesta_restore OWNER turnesta_migrator TEMPLATE template0'
sql turnesta_restore 'REVOKE ALL ON DATABASE turnesta_restore FROM PUBLIC'
sql turnesta_restore 'GRANT CONNECT ON DATABASE turnesta_restore TO turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker'
sql turnesta_restore 'GRANT CONNECT, CREATE ON DATABASE turnesta_restore TO turnesta_migrator'
if [ "$encrypted" = true ]; then
    work=$(mktemp -d backups/.turnesta-restore.XXXXXXXX)
    mkfifo "$work/archive.pipe"
    age -d -i "$identity" /proc/self/fd/8 > "$work/archive.pipe" 9>&- &
    decrypt_pid=$!
    if compose exec -T db pg_restore --no-password --exit-on-error --single-transaction -U turnesta -d turnesta_restore < "$work/archive.pipe" 9>&-; then restored=true; else restored=false; fi
    if wait "$decrypt_pid"; then decrypted=true; else decrypted=false; fi
    decrypt_pid=''
    [ "$restored" = true ] && [ "$decrypted" = true ] || fail 'Encrypted restore failed; the current database was not replaced.'
else
    compose exec -T db pg_restore --no-password --exit-on-error --single-transaction -U turnesta -d turnesta_restore < "$archive" 9>&-
fi

compose stop api worker caddy 9>&-
sql postgres 'DROP DATABASE IF EXISTS turnesta_previous'
sql postgres 'ALTER DATABASE turnesta RENAME TO turnesta_previous'
sql postgres 'ALTER DATABASE turnesta_restore RENAME TO turnesta'

compose up --wait 9>&-
compose run --rm inspector 9>&- > /dev/null

echo 'Restored. The replaced data stays in the database turnesta_previous until the next restore.'
