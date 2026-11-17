#!/bin/sh
# shellcheck source=common.sh
. "$(dirname -- "$0")/common.sh"

[ "$server" = true ] || fail 'update.sh works with a server setup.'
[ "$#" -le 1 ] || fail 'Usage: ./scripts/update.sh [image]'
acquire_maintenance_lock
[ "$(grep -c '^TURNESTA_IMAGE=' .env || true)" -eq 1 ] || fail 'Expected exactly one TURNESTA_IMAGE in .env.'

configured=$(setting TURNESTA_IMAGE)
requested=${1:-$configured}
container=$(compose ps --all --quiet api)
[ -n "$container" ] && [ "$(printf '%s\n' "$container" | wc -l)" -eq 1 ] || fail 'Expected one API container.'
previous_id=$(docker inspect --format '{{.Image}}' "$container")
running=$(docker inspect --format '{{.State.Running}}' "$container")
docker image inspect "$previous_id" > /dev/null || fail 'The API container image is not available locally for rollback.'

case "$requested" in
    *@sha256:*|sha256:*)
        requested_id=$(docker image inspect --format '{{.Id}}' "$requested" 2>/dev/null || true)
        if [ "$requested_id" = "$previous_id" ] && [ "$configured" = "$requested" ] && [ "$running" = true ]; then
            echo 'The requested image is already running; no update is needed.'
            exit 0
        fi
        ;;
esac

previous=$previous_id
case "$configured" in
    *@sha256:*)
        configured_id=$(docker image inspect --format '{{.Id}}' "$configured" 2>/dev/null || true)
        if [ "$configured_id" = "$previous_id" ]; then previous=$configured; fi
        ;;
esac
if [ "$previous" = "$previous_id" ]; then
    digests=$(docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$previous_id")
    for candidate in $digests; do
        if docker buildx imagetools inspect "$candidate" > /dev/null 2>&1; then previous=$candidate; break; fi
    done
fi

case "$requested" in
    sha256:*)
        printf '%s\n' "$requested" | grep -Eq '^sha256:[0-9a-f]{64}$' || fail 'Invalid local image ID.'
        resolved=$requested
        docker image inspect "$resolved" > /dev/null || fail 'The requested image ID is not available locally.'
        local_image=true
        ;;
    *) resolved=$(image_digest "$requested"); local_image=false ;;
esac
resolved_id=$(docker image inspect --format '{{.Id}}' "$resolved" 2>/dev/null || true)
if [ "$resolved_id" = "$previous_id" ] && [ "$configured" = "$resolved" ] && [ "$running" = true ]; then
    echo 'The requested image is already running; no update is needed.'
    exit 0
fi

snapshot=.env.before-update
if [ -e "$snapshot" ]; then snapshot=".env.before-update.$(date -u +%Y%m%dT%H%M%SZ).$$"; fi
(umask 077; set -C; cat .env > "$snapshot") || fail 'Could not save the environment snapshot.'

retained="turnesta-update-previous:${previous_id#sha256:}"
retained_id=$(docker image inspect --format '{{.Id}}' "$retained" 2>/dev/null || true)
[ -z "$retained_id" ] || [ "$retained_id" = "$previous_id" ] || fail 'The previous-image retention tag is already in use.'
if [ -z "$retained_id" ]; then docker tag "$previous_id" "$retained"; fi

export TURNESTA_IMAGE="$resolved"
if [ "$local_image" = false ]; then compose pull api; fi
backup=$(./scripts/backup.sh)
changed=true
restore_environment() {
    if [ "$changed" = true ]; then
        if cat "$snapshot" > .env; then
            echo "Update failed; .env restored from $snapshot." >&2
        else
            echo "Update failed; restore .env from $snapshot manually." >&2
        fi
        echo "Backup: $backup" >&2
        echo "To return to the previous image: ./scripts/update.sh '$previous'" >&2
        echo 'Containers may already have changed. If the schema changed, restore the backup before using the previous image.' >&2
    fi
}
trap restore_environment EXIT
trap 'exit 1' HUP INT TERM
sed -i "s|^TURNESTA_IMAGE=.*|TURNESTA_IMAGE=$resolved|" .env
compose up --pull never --wait
compose run --rm --no-deps inspector > /dev/null
changed=false
trap - EXIT HUP INT TERM
echo "Updated. Backup: $backup"
echo "Environment snapshot: $snapshot"
echo "To return to the previous image: ./scripts/update.sh '$previous'"
