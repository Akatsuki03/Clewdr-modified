#!/bin/sh
# Map Zeabur/generic "PORT" env var to CLEWDR_PORT if set
if [ -n "${PORT}" ]; then
  export CLEWDR_PORT="${PORT}"
fi

# Inform user how to set fixed passwords (passwords are read via CLEWDR_ prefix by figment)
if [ -z "${CLEWDR_PASSWORD}" ] && [ -z "${CLEWDR_ADMIN_PASSWORD}" ]; then
  echo "[clewdr] CLEWDR_PASSWORD and CLEWDR_ADMIN_PASSWORD are not set."
  echo "[clewdr] Random passwords will be generated and saved to /etc/clewdr/clewdr.toml."
  echo "[clewdr] Set these env vars in the Zeabur dashboard (or docker -e) to use fixed passwords."
fi

exec /usr/local/bin/clewdr \
  --config /etc/clewdr/clewdr.toml \
  --log-dir /etc/clewdr/log \
  "$@"
