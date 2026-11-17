#!/bin/sh
# shellcheck source=common.sh
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
    [ "$port" -ge 1 ] 2>/dev/null && [ "$port" -le 65535 ] 2>/dev/null || fail 'The port must be between 1 and 65535.'
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
    printf '%s\n' "$answer" | awk '
        length($0) > 253 || NF < 2 { exit 1 }
        { for (i = 1; i <= NF; i++) if (length($i) < 1 || length($i) > 63 || $i ~ /^-/ || $i ~ /-$/) exit 1 }
    ' FS=. || fail 'Enter a fully qualified domain with valid labels, for example app.example.com.'
    image=${TURNESTA_IMAGE:-}
    if [ -z "$image" ]; then
        image=$(image_digest ghcr.io/timeofanarchy/turnesta:latest)
    fi
    (
        umask 077
        cat > .env <<EOF
TURNESTA_DOMAIN=$answer
TURNESTA_IMAGE=$image
# Set true only if the OpenAPI document should be public on this server.
TURNESTA_EXPOSE_OPENAPI=false

POSTGRES_PASSWORD=$(key)
TURNESTA_API_DB_PASSWORD=$(key)
TURNESTA_ADMIN_DB_PASSWORD=$(key)
TURNESTA_MIGRATOR_DB_PASSWORD=$(key)
TURNESTA_WORKER_DB_PASSWORD=$(key)
TURNESTA_DELIVERY_KEY=$(key)
TURNESTA_LOG_IP_SALT=$(key)

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

# Public age recipient for encrypted backups; keep the private identity off-server.
TURNESTA_BACKUP_AGE_RECIPIENT=
# Optional healthchecks-compatible URL for backup success/failure pings.
TURNESTA_BACKUP_PING_URL=
EOF
    )
}

schedule_backups() {
    case "$PWD" in *'%'*|*'"'*|*'$'*|*'`'*|*\\*|*'
'*) fail 'The installation path contains characters that cannot be used in cron.' ;; esac
    daily=$(printf '/bin/sh "%s/scripts/backup.sh" && /bin/sh "%s/scripts/purge.sh"' "$PWD" "$PWD")
    if [ -w /etc/cron.d ]; then
        printf '30 3 * * * root ( %s ) >> /var/log/turnesta-backup.log 2>&1\n' "$daily" > /etc/cron.d/turnesta
    else
        echo 'Add this line to your crontab to run daily backup and cleanup:'
        printf '30 3 * * * %s\n' "$daily"
    fi
    if [ -w /etc/logrotate.d ]; then
        if [ -e /etc/logrotate.d/turnesta ]; then
            echo 'Existing /etc/logrotate.d/turnesta retained; verify the backup log policy.'
        else
            (umask 022; set -C; cat > /etc/logrotate.d/turnesta <<'EOF'
/var/log/turnesta-backup.log {
    weekly
    rotate 8
    compress
    missingok
    notifempty
}
EOF
            )
        fi
    else
        echo 'Configure log rotation for /var/log/turnesta-backup.log.'
    fi
}

if [ "${1:-}" = --server ]; then
    [ ! -e .env ] || [ "$server" = true ] || fail 'This folder already has a local setup in .env.'
    server=true
fi

command -v docker >/dev/null 2>&1 || fail 'Docker is not installed: https://docs.docker.com/get-docker/'
docker compose version >/dev/null 2>&1 || fail 'Docker Compose is not installed.'
docker info >/dev/null 2>&1 || fail 'Docker is not running.'

if [ "$server" = true ]; then
    [ "$(uname -s)" = Linux ] || fail 'Server installation requires Linux. Use local installation to try Turnesta on this computer.'
    command -v flock >/dev/null 2>&1 || fail 'Install flock (util-linux) before server setup.'
    command -v curl >/dev/null 2>&1 || fail 'Install curl before server setup.'
    docker buildx version >/dev/null 2>&1 || fail 'Install the Docker Buildx plugin before server setup.'
fi

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
    mail_port=$(setting TURNESTA_MAILHOG_PORT)
    echo "Local email inbox: http://127.0.0.1:${mail_port:-8025}"
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
