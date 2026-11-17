#!/bin/sh
set -eu

# Use CURL_CA_BUNDLE for a stand with a local CA.
origin=${1:?usage: caddy-metrics.test.sh https://host[:port]}
origin=${origin%/}
get() { curl --silent --show-error --max-time 5 "$@"; }

# A healthy upstream proves that the proxy and TLS are working.
get --fail "$origin/health/ready" | grep -q '"status":"ready"'
for path in /metrics '/metrics?probe=1' /metrics/ /metrics/nested; do
    # Caddy's own 404 has an empty body and no API request ID.
    actual=$(get --write-out '%{http_code}:%header{x-request-id}' "$origin$path")
    if [ "$actual" != '404:' ]; then
        echo "Unexpected response to GET $path" >&2
        exit 1
    fi
done
status=$(get --head --output /dev/null --write-out '%{http_code}:%header{x-request-id}' "$origin/metrics")
[ "$status" = '404:' ]
