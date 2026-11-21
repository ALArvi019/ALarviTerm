#!/usr/bin/env bash
# Mide el consumo de ALarviTerm con N sesiones de shell en un servidor X virtual.
#
#   scripts/measure.sh 5 10 20
#
# Usa una carpeta de datos, un socket de tmux y un identificador de aplicación
# propios, así que no toca las sesiones reales. Necesita Xvfb y xdotool, y el
# binario release ya compilado. Los terminales ocultos se desconectan a los
# pocos segundos (ALARVITERM_HIDDEN_TTL) para poder medir antes y después.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$root/target/release/alarviterm"
[ -x "$bin" ] || { echo "falta $bin, compila con: cargo build --release --workspace" >&2; exit 1; }
for tool in Xvfb xdotool tmux; do
  command -v "$tool" >/dev/null || { echo "falta $tool" >&2; exit 1; }
done

display=":97"
socket="alarviterm-measure"
ttl=6
work="$(mktemp -d)"
tmx() { tmux -L "$socket" "$@"; }
cleanup() {
  [ -n "${app:-}" ] && kill "$app" 2>/dev/null || true
  tmx kill-server 2>/dev/null || true
  [ -n "${xvfb:-}" ] && kill "$xvfb" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

Xvfb "$display" -screen 0 1400x900x24 >/dev/null 2>&1 &
xvfb=$!
sleep 1
export DISPLAY="$display" GSK_RENDERER=cairo
unset WAYLAND_DISPLAY

ticks="$(getconf CLK_TCK)"
# Tiempo de CPU acumulado de un proceso, en tics.
cpu_ticks() { awk '{print $14 + $15}' "/proc/$1/stat" 2>/dev/null || echo 0; }
# Memoria en MB: residente y proporcional (reparte lo compartido).
mem() {
  awk -v rss=0 -v pss=0 '/^Rss:/ {rss=$2} /^Pss:/ {pss=$2} END {printf "%.0f MB residentes, %.0f MB proporcionales", rss/1024, pss/1024}' \
    "/proc/$1/smaps_rollup" 2>/dev/null || echo "sin datos"
}
# Porcentaje de un núcleo que usa un proceso durante unos segundos.
cpu_percent() {
  local pid="$1" seconds="$2" before after
  before="$(cpu_ticks "$pid")"
  sleep "$seconds"
  after="$(cpu_ticks "$pid")"
  awk -v d="$((after - before))" -v t="$ticks" -v s="$seconds" 'BEGIN {printf "%.1f", d * 100 / t / s}'
}
report() {
  local label="$1" server clients
  server="$(tmx display-message -p '#{pid}' 2>/dev/null || true)"
  clients="$(tmx list-clients 2>/dev/null | wc -l)"
  echo "  $label"
  echo "    ALarviTerm:        $(mem "$app")"
  [ -n "$server" ] && echo "    servidor tmux:  $(mem "$server")"
  echo "    clientes tmux:  $clients"
  echo "    CPU de ALarviTerm: $(cpu_percent "$app" 10) % de un núcleo"
  [ -n "$server" ] && echo "    CPU de tmux:    $(cpu_percent "$server" 5) % de un núcleo"
}

for count in "$@"; do
  data="$work/data-$count"
  mkdir -p "$data"
  start="$(date +%s%N)"
  ALARVITERM_DATA_DIR="$data" ALARVITERM_TMUX_SOCKET="$socket" ALARVITERM_APP_ID="io.github.alarvi019.Measure" \
    ALARVITERM_HIDDEN_TTL="$ttl" "$bin" >/dev/null 2>&1 &
  app=$!
  until xdotool search --onlyvisible --pid "$app" >/dev/null 2>&1; do sleep 0.01; done
  shown="$(( ($(date +%s%N) - start) / 1000000 ))"
  window="$(xdotool search --onlyvisible --pid "$app" | head -1)"
  xdotool windowfocus "$window" 2>/dev/null || true
  sleep 0.5

  for _ in $(seq "$count"); do
    xdotool key ctrl+shift+t
    sleep 0.5
  done
  sleep 1
  created="$(tmx list-sessions 2>/dev/null | wc -l)"
  echo "== $created sesiones (ventana visible en $shown ms)"
  if [ "$created" -ne "$count" ]; then
    echo "   se pidieron $count: la medición no vale" >&2
  fi

  report "en reposo, recién creadas"
  sleep "$((ttl + 8))"
  report "en reposo, terminales ocultos ya desconectados"

  # Todas las sesiones escriben cinco líneas por segundo.
  for name in $(tmx list-sessions -F '#{session_name}'); do
    tmx send-keys -t "$name" 'while :; do date; sleep 0.2; done' Enter
  done
  sleep 2
  report "todas escribiendo, solo una a la vista"

  kill "$app" 2>/dev/null || true
  wait "$app" 2>/dev/null || true
  app=""
  tmx kill-server 2>/dev/null || true
  sleep 1
done
