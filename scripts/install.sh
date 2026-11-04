#!/bin/sh
. "$(dirname -- "$0")/common.sh"

ask() {
    answer=''
    while [ -z "$answer" ]; do
        printf '%s' "$1: "
        IFS= read -r answer || fail 'Setup cancelled.'
    done
}

key() {
    od -An -tx1 -N32 /dev/urandom | tr -d ' \n'
}

create_local_env() {
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
}

create_server_env() {
    ask 'Domain, for example app.example.com'
    case $answer in
        *[!a-zA-Z0-9.-]*) fail 'Enter only the domain name, without https:// or a path.' ;;
    esac
    (
        umask 077
        cat > .env <<EOF
TURNESTA_DOMAIN=$answer
TURNESTA_IMAGE=${TURNESTA_IMAGE:-ghcr.io/timeofanarchy/turnesta:latest}

POSTGRES_PASSWORD=$(key)
TURNESTA_API_DB_PASSWORD=$(key)
TURNESTA_ADMIN_DB_PASSWORD=$(key)
TURNESTA_MIGRATOR_DB_PASSWORD=$(key)
TURNESTA_WORKER_DB_PASSWORD=$(key)
TURNESTA_DELIVERY_KEY=$(key)

# To send email, fill these in, uncomment COMPOSE_PROFILES and run install.sh again.
#COMPOSE_PROFILES=mail
TURNESTA_SMTP_HOST=
TURNESTA_SMTP_PORT=465
TURNESTA_SMTP_FROM=
TURNESTA_SMTP_USERNAME=
TURNESTA_SMTP_PASSWORD=

# Set an rclone remote to copy each backup off the server, for example s3:bucket/turnesta.
TURNESTA_BACKUP_REMOTE=
TURNESTA_BACKUP_KEEP=14
EOF
    )
}

schedule_backups() {
    if [ -w /etc/cron.d ]; then
        printf '30 3 * * * root %s/scripts/backup.sh >> /var/log/turnesta-backup.log 2>&1\n' "$PWD" > /etc/cron.d/turnesta
    else
        echo 'Add ./scripts/backup.sh to cron to back up the database every day.'
    fi
}

if [ "${1:-}" = --server ]; then
    [ ! -e .env ] || [ "$server" = true ] || fail 'This folder already has a local setup in .env.'
    server=true
fi

command -v docker >/dev/null 2>&1 || fail 'Docker is not installed: https://docs.docker.com/get-docker/'
docker compose version >/dev/null 2>&1 || fail 'Docker Compose is not installed.'
docker info >/dev/null 2>&1 || fail 'Docker is not running.'

if [ ! -e .env ]; then
    if [ "$server" = true ]; then
        create_server_env
    else
        create_local_env
    fi
fi

ask 'Organization name'
organization=$answer
ask 'Owner name'
owner=$answer
ask 'Owner email'
email=$answer

if [ "$server" = true ]; then
    echo 'Downloading and starting Turnesta.'
    compose up --pull always --wait
    schedule_backups
    origin="https://$(setting TURNESTA_DOMAIN)"
else
    echo 'Building and starting Turnesta. The first build takes several minutes.'
    compose up --build --wait
    origin=$(setting TURNESTA_PUBLIC_ORIGIN)
fi

password=$(od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
result=$(printf '%s' "$password" | compose run --rm -T admin auth bootstrap \
    --organization "$organization" --owner-name "$owner" --email "$email" --password-stdin)

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
