#!/bin/sh
. "$(dirname -- "$0")/common.sh"

[ "$server" = true ] || fail 'update.sh works with a server setup.'

previous=$(docker inspect --format '{{join .RepoDigests " "}}' "$(compose images --quiet api)" | cut -d ' ' -f 1)

if [ $# -gt 0 ]; then
    export TURNESTA_IMAGE="$1"
fi
compose pull api
backup=$(./scripts/backup.sh)

if [ $# -gt 0 ]; then
    sed -i "s|^TURNESTA_IMAGE=.*|TURNESTA_IMAGE=$1|" .env
fi

if compose up --pull always --wait && compose run --rm inspector > /dev/null; then
    echo "Updated. Backup: $backup"
    echo "To return to the previous version: ./scripts/update.sh $previous"
else
    echo "Update failed. Backup: $backup" >&2
    echo "Previous version: $previous" >&2
    echo 'If the update changed the database, the previous version needs ./scripts/restore.sh with this backup.' >&2
    exit 1
fi
