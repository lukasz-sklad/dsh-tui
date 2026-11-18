#!/bin/sh
set -- \
  sh -c '
    for p in "$HOME/.local/bin" "$HOME/.nvm/current/bin" "$HOME"/.nvm/versions/node/*/bin; do
      [ -x "$p/dsh" ] && PATH="$p:$PATH"
    done
    export PATH
    exec dsh "$@"
  ' sh "$@"
[ -n "${DEEPSEEK_API_KEY:-}" ] && set -- "--env=DEEPSEEK_API_KEY=$DEEPSEEK_API_KEY" "$@"
[ -n "${DSH_HOME:-}" ] && set -- "--env=DSH_HOME=$DSH_HOME" "$@"
[ -n "${DSH_PERMISSION_MODE:-}" ] && set -- "--env=DSH_PERMISSION_MODE=$DSH_PERMISSION_MODE" "$@"
exec flatpak-spawn --host "$@"
