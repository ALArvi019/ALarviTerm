#!/usr/bin/env bash
# Genera un paquete .deb en target/.
#
#   scripts/package-deb.sh
#
# Compila en un directorio aparte y sin rutas de biblioteca propias, para que
# el binario del paquete dependa solo de las bibliotecas del sistema.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_id="io.github.alarvi019.ALarviTerm"
command -v dpkg-deb >/dev/null || { echo "falta dpkg-deb" >&2; exit 1; }

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
arch="$(dpkg --print-architecture)"
build="$root/target/deb"
stage="$build/stage"
package="$root/target/alarviterm_${version}_${arch}.deb"

echo "Compilando ALarviTerm $version para $arch..."
# Los binarios de Rust guardan rutas de los fuentes para sus mensajes de
# error. Se sustituyen para que el paquete no lleve la carpeta personal de
# quien lo compila.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/build --remap-path-prefix=$root=/build/alarviterm"
(cd "$root" && ALARVITERM_NO_RPATH=1 ./scripts/dev.sh cargo build --release --workspace --target-dir "$build")

rm -rf "$stage"
install -Dm755 "$build/release/alarviterm" "$stage/usr/bin/alarviterm"
install -Dm755 "$build/release/alarviterm-hook" "$stage/usr/bin/alarviterm-hook"
install -Dm644 "$root/data/$app_id.desktop" "$stage/usr/share/applications/$app_id.desktop"
for size in "$root"/data/icons/hicolor/*/; do
  install -Dm644 "$size/apps/$app_id.png" "$stage/usr/share/icons/hicolor/$(basename "$size")/apps/$app_id.png"
done
install -Dm644 "$root/README.md" "$stage/usr/share/doc/alarviterm/README.md"
strip "$stage/usr/bin/alarviterm" "$stage/usr/bin/alarviterm-hook" 2>/dev/null || true

if readelf -d "$stage/usr/bin/alarviterm" | grep -Eq 'RPATH|RUNPATH'; then
  echo "El binario lleva rutas de biblioteca propias y no debe empaquetarse así." >&2
  exit 1
fi
if grep -qaF "$HOME" "$stage/usr/bin/alarviterm" "$stage/usr/bin/alarviterm-hook"; then
  echo "El binario contiene la ruta $HOME y no debe empaquetarse así." >&2
  exit 1
fi

size="$(du -sk "$stage/usr" | cut -f1)"
install -d "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<CONTROL
Package: alarviterm
Version: $version
Section: utils
Priority: optional
Architecture: $arch
Installed-Size: $size
Depends: libc6, libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libvte-2.91-gtk4-0 (>= 0.76), libgtksourceview-5-0, libsqlite3-0, tmux (>= 3.2)
Recommends: git
Maintainer: ALArvi019 <5780489+ALArvi019@users.noreply.github.com>
Description: Centro de control para sesiones de Claude Code
 Aplicación de escritorio que gestiona muchas sesiones de Claude Code y de
 shell a la vez: terminales reales, paneles, grupos, estados y avisos.
 Las sesiones viven en un servidor tmux propio y sobreviven al cierre.
CONTROL

fakeroot_cmd=()
command -v fakeroot >/dev/null && fakeroot_cmd=(fakeroot)
"${fakeroot_cmd[@]}" dpkg-deb --build --root-owner-group "$stage" "$package" >/dev/null
echo "Paquete: $package"
dpkg-deb --info "$package" | sed -n '1,16p'
echo "Se instala con: sudo apt install $package"
