set -eu
cd -- "$(dirname -- "$0")/.."

fail() {
    echo "$1" >&2
    exit 1
}

setting() {
    sed -n "s/^$1=//p" .env | tr -d '\r' | tail -n 1
}

image_digest() {
    reference=$1
    case "$reference" in ''|*[!A-Za-z0-9._:/@-]*) fail 'Invalid image reference.' ;; esac
    inspected=$(docker buildx imagetools inspect "$reference") || fail "Cannot resolve image: $reference"
    digest=$(printf '%s\n' "$inspected" | awk '$1 == "Digest:" { print $2; exit }')
    printf '%s\n' "$digest" | grep -Eq '^sha256:[0-9a-f]{64}$' || fail "Cannot resolve image digest: $reference"
    repository=${reference%@*}
    case "${repository##*/}" in *:*) repository=${repository%:*} ;; esac
    printf '%s@%s\n' "$repository" "$digest"
}

acquire_backup_lock() {
    command -v flock > /dev/null 2>&1 || fail 'flock is required for backup and restore.'
    mkdir -p backups
    exec 9>>backups/.turnesta-backup.lock
    flock -x -w 600 9 || fail 'Could not acquire the backup and restore lock within 10 minutes.'
}

compose() {
    if [ "$server" = true ]; then
        docker compose --env-file .env --file deploy/compose.prod.yaml "$@"
    else
        docker compose --env-file .env "$@"
    fi
}

server=false
if [ -e .env ] && grep -q '^TURNESTA_DOMAIN=' .env; then
    server=true
fi
