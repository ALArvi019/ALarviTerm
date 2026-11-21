# Contribuir

Gracias por el interés. Esta guía resume cómo compilar, probar y proponer cambios.

## Preparar el entorno

Los requisitos y los paquetes de cada distribución están en el [README](README.md#requisitos). Con ellos instalados:

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets
```

Las pruebas de integración lanzan un tmux real en un socket temporal y lo cierran al terminar.

## Organización del código

* `crates/core`: lógica sin dependencias gráficas. Modelo, SQLite, backend de tmux, lanzador de Claude Code, acciones, ajustes y búsqueda. Lo que se pueda probar sin ventana va aquí.
* `crates/app`: la interfaz con GTK4, libadwaita y VTE.
* `crates/hook`: el binario `alarviterm-hook`, que recibe los hooks de Claude Code y escribe el estado de cada sesión.

Las decisiones de diseño están en [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) y el estado de cada parte en [ROADMAP.md](ROADMAP.md).

## Probar la interfaz sin tocar tus sesiones

Una segunda instancia aislada usa sus propios datos, su propio servidor tmux y su propio identificador:

```sh
ALARVITERM_DATA_DIR=/tmp/alarviterm-pruebas \
ALARVITERM_TMUX_SOCKET=alarviterm-pruebas \
ALARVITERM_APP_ID=io.github.alarvi019.Pruebas \
cargo run
```

Al terminar, `tmux -L alarviterm-pruebas kill-server` cierra sus sesiones.

## Reglas del proyecto

* La aplicación no modifica la configuración global del usuario: ni `~/.claude/`, ni `~/.tmux.conf`, ni ningún otro fichero ajeno.
* Los estados que se muestran salen de hechos comprobables, no de leer la pantalla. Una heurística se presenta como tal.
* Nada se da por terminado solo porque se vea bien. En el ROADMAP se separa lo comprobado con la aplicación en marcha de lo que no.
* Un cambio de comportamiento lleva su prueba en `crates/core` siempre que sea posible.
* `cargo clippy` sin avisos y `cargo test` en verde antes de proponer un cambio.

## Datos privados

No incluyas en commits, capturas ni registros nada que identifique una máquina o una persona: rutas de carpetas personales, nombres de equipo, tokens, direcciones de correo o contenido de sesiones reales. Las capturas se hacen con un usuario de demostración.

## Proponer cambios

Abre una incidencia para discutir los cambios grandes antes de escribirlos. Las propuestas pequeñas pueden ir directamente como pull request, con una descripción de qué cambia, por qué, y cómo se ha comprobado.
