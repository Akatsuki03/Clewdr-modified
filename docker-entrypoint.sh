#!/bin/sh
# Map Zeabur/generic "PORT" env var to CLEWDR_PORT if set
if [ -n "${PORT}" ]; then
  export CLEWDR_PORT="${PORT}"
fi

exec /usr/local/bin/clewdr \
  --config /etc/clewdr/clewdr.toml \
  --log-dir /etc/clewdr/log \
  "$@"
