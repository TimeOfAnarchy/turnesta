#!/bin/sh
# Linux/macOS counterpart of Initialize-Development.ps1: creates the ignored
# local .env with independent random keys, or completes an existing one.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
destination="$root/.env"
key() { od -An -tx1 -N32 /dev/urandom | tr -d ' \n'; }
hex64='[0-9a-fA-F]\{64\}'

if [ -e "$destination" ]; then
    if ! grep -q "^TURNESTA_DELIVERY_KEY=$hex64\$" "$destination"; then
        echo 'Existing .env is preserved. Add TURNESTA_DELIVERY_KEY with 32 random bytes encoded as 64 hex characters.' >&2
        exit 1
    fi
    additions=''
    if grep -q '^TURNESTA_MOCK_PAYMENT_KEY=' "$destination"; then
        if ! grep -q "^TURNESTA_MOCK_PAYMENT_KEY=$hex64\$" "$destination"; then
            echo 'Existing .env is preserved. TURNESTA_MOCK_PAYMENT_KEY must contain 32 random bytes encoded as 64 hex characters.' >&2
            exit 1
        fi
    else
        additions="${additions}TURNESTA_MOCK_PAYMENT_KEY=$(key)
"
    fi
    grep -q '^TURNESTA_TIPS_MODE=' "$destination" || additions="${additions}TURNESTA_TIPS_MODE=mock
"
    if [ -n "$additions" ]; then
        printf '\n%s' "$additions" >> "$destination"
        echo 'Preserved existing environment values and appended missing local mock configuration.'
    else
        echo 'Existing local .env is valid and was left unchanged.'
    fi
    exit 0
fi

template="$root/.env.example"
grep -q '^TURNESTA_DELIVERY_KEY=$' "$template" || { echo 'Expected an empty delivery key placeholder in .env.example.' >&2; exit 1; }
grep -q '^TURNESTA_MOCK_PAYMENT_KEY=$' "$template" || { echo 'Expected an empty mock payment key placeholder.' >&2; exit 1; }
umask 077
set -C
sed -e "s/^TURNESTA_DELIVERY_KEY=\$/TURNESTA_DELIVERY_KEY=$(key)/" \
    -e "s/^TURNESTA_MOCK_PAYMENT_KEY=\$/TURNESTA_MOCK_PAYMENT_KEY=$(key)/" "$template" > "$destination"
echo 'Created ignored local .env with independent delivery and mock payment keys. Keep secrets with database backups.'
