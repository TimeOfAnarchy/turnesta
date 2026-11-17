#!/bin/sh
# shellcheck source=common.sh
. "$(dirname -- "$0")/common.sh"

# Do not start or migrate services as a side effect of scheduled cleanup.
if [ "$server" = true ]; then service=inspector; else service=migrator; fi
compose --profile tools run --rm --no-deps -T "$service" ops purge "$@"
