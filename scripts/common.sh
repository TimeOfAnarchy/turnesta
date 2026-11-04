set -eu
cd -- "$(dirname -- "$0")/.."

fail() {
    echo "$1" >&2
    exit 1
}

setting() {
    sed -n "s/^$1=//p" .env | tr -d '\r' | tail -n 1
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
