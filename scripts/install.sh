#!/bin/sh
set -eu
cd -- "$(dirname -- "$0")/.."

fail() {
    echo "$1" >&2
    exit 1
}

ask() {
    answer=''
    while [ -z "$answer" ]; do
        printf '%s' "$1: "
        IFS= read -r answer || fail 'Setup cancelled.'
    done
}

setting() {
    sed -n "s/^$1=//p" .env | tr -d '\r' | tail -n 1
}

compose() {
    docker compose --env-file .env "$@"
}

command -v docker >/dev/null 2>&1 || fail 'Docker is not installed: https://docs.docker.com/get-docker/'
docker compose version >/dev/null 2>&1 || fail 'Docker Compose is not installed.'
docker info >/dev/null 2>&1 || fail 'Docker is not running.'

if [ ! -e .env ]; then
    printf 'Port for Turnesta [8080]: '
    IFS= read -r port || fail 'Setup cancelled.'
    port=${port:-8080}
    case $port in
        *[!0-9]*) fail 'The port must be a number.' ;;
    esac
    ./scripts/init-env.sh
    sed -i.bak \
        -e "s/^TURNESTA_API_PORT=.*/TURNESTA_API_PORT=$port/" \
        -e "s|^TURNESTA_PUBLIC_ORIGIN=.*|TURNESTA_PUBLIC_ORIGIN=http://127.0.0.1:$port|" .env
    rm -f .env.bak
fi

ask 'Organization name'
organization=$answer
ask 'Owner name'
owner=$answer
ask 'Owner email'
email=$answer

echo 'Building and starting Turnesta. The first build takes several minutes.'
compose up --build --wait

password=$(od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
result=$(printf '%s' "$password" | compose run --rm -T admin auth bootstrap \
    --organization "$organization" --owner-name "$owner" --email "$email" --password-stdin)

origin=$(setting TURNESTA_PUBLIC_ORIGIN)
echo
echo "Turnesta is running: $origin/login"
case $result in
    *'"exists"'*)
        echo 'An organization already exists, so no account was created.'
        ;;
    *)
        echo "Email: $email"
        echo "Temporary password: $password"
        echo 'You will choose your own password on first sign-in.'
        ;;
esac
