#!/bin/sh
# Map Zeabur/generic "PORT" env var to CLEWDR_PORT if set
if [ -n "${PORT}" ]; then
  export CLEWDR_PORT="${PORT}"
fi

# Safety net: if CLEWDR_PASSWORD or CLEWDR_ADMIN_PASSWORD are set to empty
# strings (e.g. inherited from a Dockerfile ENV default), unset them so that
# figment does NOT override the persisted TOML value with "".
# When they are unset, figment falls back to the value in clewdr.toml (the
# password generated on first start and saved to the volume).
# When the user sets a non-empty value in the Zeabur dashboard, the variable
# IS exported and figment picks it up — overriding the TOML correctly.
if [ -z "${CLEWDR_PASSWORD}" ]; then
  unset CLEWDR_PASSWORD
fi
if [ -z "${CLEWDR_ADMIN_PASSWORD}" ]; then
  unset CLEWDR_ADMIN_PASSWORD
fi

if [ -z "${CLEWDR_PASSWORD+x}" ] && [ -z "${CLEWDR_ADMIN_PASSWORD+x}" ]; then
  echo "[clewdr] CLEWDR_PASSWORD and CLEWDR_ADMIN_PASSWORD are not set."
  echo "[clewdr] The password saved in /etc/clewdr/clewdr.toml will be used (or"
  echo "[clewdr] a new random one generated if the volume is empty)."
  echo "[clewdr] Set these in the Zeabur dashboard to use a fixed password."
fi

exec /usr/local/bin/clewdr \
  --config /etc/clewdr/clewdr.toml \
  --log-dir /etc/clewdr/log \
  "$@"
