#!/bin/sh
export DSH_BIN="${DSH_BIN:-/app/bin/dsh-host}"
exec /app/bin/dsh-tui-bin "$@"
