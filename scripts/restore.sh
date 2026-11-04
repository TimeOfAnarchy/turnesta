#!/bin/sh
. "$(dirname -- "$0")/common.sh"

[ "$server" = true ] || fail 'restore.sh works with a server setup.'
[ $# -eq 1 ] && [ -f "$1" ] || fail 'Usage: ./scripts/restore.sh backups/turnesta-<date>.dump'

sql() {
    compose exec -T db psql --no-psqlrc --quiet -v ON_ERROR_STOP=1 -U turnesta -d "$1" -c "$2"
}

printf 'This replaces all current data with %s. Type "restore" to continue: ' "$1"
IFS= read -r confirmation || fail 'Restore cancelled.'
[ "$confirmation" = restore ] || fail 'Restore cancelled.'

compose up --detach --wait db
compose run --rm --no-deps db-bootstrap

sql postgres 'DROP DATABASE IF EXISTS turnesta_restore'
sql postgres 'CREATE DATABASE turnesta_restore OWNER turnesta_migrator TEMPLATE template0'
sql turnesta_restore 'REVOKE ALL ON DATABASE turnesta_restore FROM PUBLIC'
sql turnesta_restore 'GRANT CONNECT ON DATABASE turnesta_restore TO turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker'
sql turnesta_restore 'GRANT CONNECT, CREATE ON DATABASE turnesta_restore TO turnesta_migrator'
compose exec -T db pg_restore --no-password --exit-on-error --single-transaction -U turnesta -d turnesta_restore < "$1"

compose stop api worker caddy
sql postgres 'DROP DATABASE IF EXISTS turnesta_previous'
sql postgres 'ALTER DATABASE turnesta RENAME TO turnesta_previous'
sql postgres 'ALTER DATABASE turnesta_restore RENAME TO turnesta'

compose up --wait
compose run --rm inspector > /dev/null

echo 'Restored. The replaced data stays in the database turnesta_previous until the next restore.'
