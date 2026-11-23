#!/usr/bin/env bash
# Ejecuta un comando (cargo, el binario...) usando el sysroot local de
# desarrollo si existe. Con los paquetes -dev instalados en el sistema no hace
# falta: basta con llamar a cargo directamente.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sys="$root/.sysroot/root"
if [ -d "$sys" ]; then
  lib="$sys/usr/lib/x86_64-linux-gnu"
  export PKG_CONFIG_PATH="$lib/pkgconfig:$sys/usr/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  # rpath solo hacia las bibliotecas que el sistema no trae (libvte gtk4 y
  # libgtksourceview), para
  # no contaminar el entorno de las sesiones con LD_LIBRARY_PATH.
  mkdir -p "$root/.sysroot/runtime"
  cp -an "$lib"/libvte-2.91-gtk4.so.0* "$root/.sysroot/runtime/" 2>/dev/null || true
  cp -an "$lib"/libgtksourceview-5.so.0* "$root/.sysroot/runtime/" 2>/dev/null || true
  # Lenguajes y esquemas de color del editor, que tampoco están en el sistema.
  if [ -d "$sys/usr/share/gtksourceview-5" ] && [ ! -d /usr/share/gtksourceview-5 ]; then
    export ALARVITERM_SOURCEVIEW_DATA="$sys/usr/share/gtksourceview-5"
  fi
  flags="-L native=$lib"
  # ALARVITERM_NO_RPATH=1 deja el binario sin rutas propias, como debe ir en un
  # paquete: en la máquina de destino libvte viene del sistema.
  if [ -z "${ALARVITERM_NO_RPATH:-}" ]; then
    # La primera ruta es donde scripts/install.sh deja la biblioteca cuando
    # el sistema no la tiene. La segunda sirve para ejecutar desde target/.
    flags="$flags -C link-arg=-Wl,-rpath,\$ORIGIN/../lib/alarviterm -C link-arg=-Wl,-rpath,$root/.sysroot/runtime"
  fi
  export RUSTFLAGS="${RUSTFLAGS:-} $flags"
fi
exec "$@"
