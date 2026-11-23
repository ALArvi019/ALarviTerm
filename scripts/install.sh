#!/usr/bin/env bash
# Instala ALarviTerm para el usuario actual, sin permisos de administrador.
#
#   scripts/install.sh               instala en ~/.local
#   scripts/install.sh --uninstall   retira lo instalado
#   PREFIX=/otra/ruta scripts/install.sh
#
# Copia los binarios, la entrada de escritorio y el icono. No toca ninguna
# configuración ni los datos de ~/.local/share/alarviterm.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
prefix="${PREFIX:-$HOME/.local}"
app_id="io.github.alarvi019.ALarviTerm"

bin_dir="$prefix/bin"
lib_dir="$prefix/lib/alarviterm"
desktop="$prefix/share/applications/$app_id.desktop"
icons="$prefix/share/icons/hicolor"

refresh_caches() {
  command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
  command -v gtk4-update-icon-cache >/dev/null && gtk4-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin_dir/alarviterm" "$bin_dir/alarviterm-hook" "$desktop"
  rm -f "$icons"/*/apps/"$app_id".png "$icons/scalable/apps/$app_id.svg"
  rm -rf "$lib_dir"
  refresh_caches
  echo "ALarviTerm desinstalado de $prefix."
  echo "Tus sesiones y ajustes siguen en ~/.local/share/alarviterm. El README explica cómo borrarlos."
  exit 0
fi

command -v tmux >/dev/null || echo "Aviso: no se encuentra tmux. ALarviTerm lo necesita para mantener vivas las sesiones." >&2

echo "Compilando en modo release..."
(cd "$root" && ./scripts/dev.sh cargo build --release --workspace)

install -Dm755 "$root/target/release/alarviterm" "$bin_dir/alarviterm"
# El binario de hooks va junto al principal, que es donde este lo busca.
install -Dm755 "$root/target/release/alarviterm-hook" "$bin_dir/alarviterm-hook"
for size in "$root"/data/icons/hicolor/*/; do
  install -Dm644 "$size/apps/$app_id.png" "$icons/$(basename "$size")/apps/$app_id.png"
done
# El icono vectorial de versiones anteriores taparía al nuevo.
rm -f "$icons/scalable/apps/$app_id.svg"
install -d "$(dirname "$desktop")"
# Ruta absoluta: el lanzador del escritorio puede no tener ~/.local/bin en el PATH.
sed "s|^Exec=.*|Exec=$bin_dir/alarviterm|" "$root/data/$app_id.desktop" > "$desktop"

# Si el sistema no trae libvte para GTK4 se usa la copia local de desarrollo.
if ! ldconfig -p 2>/dev/null | grep -q 'libvte-2\.91-gtk4\.so\.0'; then
  runtime="$root/.sysroot/runtime"
  if compgen -G "$runtime/libvte-2.91-gtk4.so.0*" >/dev/null; then
    install -d "$lib_dir"
    # Sin sobrescribir en el sitio: una instancia abierta tiene la biblioteca
    # cargada y cambiarle el contenido la tumbaría.
    cp -a --remove-destination "$runtime"/libvte-2.91-gtk4.so.0* "$lib_dir/"
    echo "El sistema no tiene libvte para GTK4. Se ha copiado una a $lib_dir."
    echo "Lo limpio es instalarla: sudo apt install libvte-2.91-gtk4-0"
  else
    echo "Aviso: falta libvte-2.91-gtk4. Instálala con: sudo apt install libvte-2.91-gtk4-0" >&2
  fi
fi

# Lo mismo con GtkSourceView, el editor del control de código fuente: la
# biblioteca y sus lenguajes y esquemas de color.
if ! ldconfig -p 2>/dev/null | grep -q 'libgtksourceview-5\.so\.0'; then
  runtime="$root/.sysroot/runtime"
  data="$root/.sysroot/root/usr/share/gtksourceview-5"
  if compgen -G "$runtime/libgtksourceview-5.so.0*" >/dev/null && [ -d "$data" ]; then
    # Todo dentro de la carpeta de bibliotecas, que es solo de la aplicación.
    # En share/alarviterm están las sesiones del usuario y ahí no se toca.
    install -d "$lib_dir"
    cp -a --remove-destination "$runtime"/libgtksourceview-5.so.0* "$lib_dir/"
    rm -rf "$lib_dir/gtksourceview-5"
    cp -a "$data" "$lib_dir/gtksourceview-5"
    echo "El sistema no tiene GtkSourceView 5. Se ha copiado una a $lib_dir."
    echo "Lo limpio es instalarla: sudo apt install libgtksourceview-5-0"
  else
    echo "Aviso: falta libgtksourceview-5. Instálala con: sudo apt install libgtksourceview-5-0" >&2
  fi
fi

refresh_caches
echo "ALarviTerm instalado en $bin_dir/alarviterm."
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "Aviso: $bin_dir no está en tu PATH. La entrada del menú de aplicaciones funciona igual." ;;
esac
