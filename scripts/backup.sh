#!/bin/sh
. "$(dirname -- "$0")/common.sh"

keep=$(setting TURNESTA_BACKUP_KEEP)
remote=$(setting TURNESTA_BACKUP_REMOTE)
file="backups/turnesta-$(date -u +%Y%m%dT%H%M%SZ).dump"

umask 077
mkdir -p backups
trap 'rm -f "$file.partial"' EXIT

compose exec -T db pg_dump --format=custom -U turnesta -d turnesta > "$file.partial"
compose exec -T db pg_restore --list < "$file.partial" > /dev/null
mv "$file.partial" "$file"

ls -1t backups/turnesta-*.dump | tail -n +"$((${keep:-14} + 1))" | xargs -r rm -f

if [ -n "$remote" ]; then
    rclone copy "$file" "$remote"
fi

echo "$file"
