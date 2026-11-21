# Hoja de ruta

Estado a 22 de noviembre de 2026.

## Fase 1. Investigación y arquitectura: hecha

* Stack elegido y justificado en `docs/ARCHITECTURE.md`: Rust, GTK4, libadwaita, VTE, tmux detrás de un trait, SQLite, sin Tokio.

## Fase 2. MVP funcional: hecha

Implementado y comprobado con la aplicación en marcha:

* Ventana principal con cabecera, barra lateral redimensionable y ocultable, área de terminal y footer.
* Terminales reales (VTE) conectados a procesos reales en tmux. Probado con zsh y con Claude Code 2.1.330.
* Crear sesiones de Claude Code o de shell con nombre, directorio, grupo, modelo, argumentos y variables de entorno.
* Grupos: crear, renombrar, eliminar, plegar. Mover sesiones entre grupos.
* Sesiones: renombrar, reiniciar, reiniciar con `--continue`, detener, eliminar.
* Cambio de sesión con ratón y teclado.
* Las sesiones sobreviven al cierre de la aplicación y se reconectan al abrirla.
* Estados reales: Activa, Terminada, Error con código, Desconectada.
* Persistencia en SQLite de grupos, sesiones, tamaño de ventana, barra lateral y última sesión.
* Detección de Claude Code (ruta, versión, indicio de credenciales) y de tmux.
* Tema oscuro.
* Pruebas unitarias y de integración contra un tmux real. Hoy son 95 en total.

Pendiente menor de esta fase:

* Instalar los paquetes de desarrollo del sistema para dejar de depender del sysroot local (ver README).

## Fase 3. Multipanel: hecha

Implementado y comprobado con la aplicación en marcha:

* Árbol de divisiones horizontales y verticales sin límite fijo, con separadores redimensionables.
* Pestañas por panel. Cerrar una pestaña suelta el terminal y deja el proceso vivo.
* Maximizar y restaurar un panel.
* Arrastrar pestañas y filas de la barra lateral a un panel. El centro añade una pestaña y los bordes dividen.
* Reordenar sesiones y cambiarlas de grupo arrastrando en la barra lateral. Subir y bajar grupos desde su menú.
* Disposiciones predefinidas (uno, dos columnas, dos filas, tres columnas, cuadrícula) y disposiciones guardadas con nombre.
* La disposición se guarda sola y se recupera al abrir, con sus proporciones.
* Navegación entre paneles con el teclado.
* El modelo de disposición vive en `crates/core/src/layout.rs`, sin widgets, con 16 pruebas propias.

Sin comprobar con la aplicación en marcha: mover un grupo arriba o abajo, y soltar una sesión sobre la cabecera de un grupo. Ambos casos están cubiertos por pruebas de la base de datos.

## Fase 4. Monitorización: hecha

Implementado y comprobado con la aplicación en marcha y un Claude Code 2.1.330 real:

* Binario `alarviterm-hook` y registro de hooks por sesión con `claude --settings`, sin tocar la configuración global.
* Estados Lista, Pensando, Ejecutando herramienta, Pide permiso, Esperando respuesta y Compactando a partir de eventos reales. Llegan a la interfaz al momento, por vigilancia del directorio de estado.
* Cada estado va ligado al lanzamiento del proceso que lo emitió. Al reiniciar, el evento de cierre del proceso que muere ya no se atribuye al nuevo.
* Reinicio con `claude --resume` y el identificador exacto de la conversación.
* Aviso inmediato de fin de proceso con hooks de tmux, también para sesiones sin terminal abierto.
* Notificación de escritorio cuando una sesión pide permiso o termina su turno con la ventana en segundo plano. Al pulsarla se abre esa sesión.
* Pie ampliado: rama de git, cambios pendientes, modelo del último turno, CPU y memoria del árbol de procesos, y recuento de sesiones que piden permiso.

Sin comprobar con la aplicación en marcha: Compactando, pulsar la notificación para abrir la sesión (se comprobó que se emite, no el clic), y el aviso de tmux cuando se cierra la última sesión del servidor, que en ese caso llega por el refresco periódico.

Pendiente de esta fase, pasa a la Fase 6: que el contenido del pie y las notificaciones se puedan configurar desde la ventana de ajustes.

## Fase 5. Paleta de comandos y atajos: hecha

Implementado y comprobado con la aplicación en marcha:

* Registro central de acciones en `crates/core/src/actions.rs`. De él salen los atajos, la paleta y el editor de atajos.
* Paleta de comandos con búsqueda aproximada sobre acciones, sesiones, grupos de destino, disposiciones y filtros.
* Editor de atajos: captura de la combinación, aviso de conflicto, varios atajos por acción, vuelta al valor de fábrica y guardado en la base de datos.
* Choques con el terminal resueltos: cada atajo declara si actúa con el foco en un terminal. Se comprobó que `Ctrl+G` asignado a Nuevo grupo llega al shell dentro del terminal y abre el diálogo fuera de él.
* Acciones nuevas de teclado: renombrar, reiniciar, detener y eliminar la sesión actual, e ir a la siguiente sesión que espera al usuario.
* Buscador en la barra lateral por nombre, carpeta o grupo, con filtro por estado.
* Los tooltips y el texto de la lista vacía citan el atajo vigente.

Sin comprobar con la aplicación en marcha: el filtro Te esperan y el salto a la siguiente sesión que espera, que necesitan un Claude Code en marcha y solo tienen pruebas, restablecer todos los atajos, y añadir un segundo atajo a una acción que ya tiene uno.

Limitación conocida: `Ctrl+Mayús+C` y `Ctrl+Mayús+V` los atiende el terminal y no pasan por el registro.

## Fase 6. Proyectos y ajustes: hecha

Implementado y comprobado con la aplicación en marcha:

* Proyectos con carpeta, color, icono, variables de entorno y comando de inicio. Se comprobó que un terminal de proyecto arranca en su carpeta, con sus variables y con el comando de inicio ya ejecutado.
* Ventana de ajustes con aplicación inmediata: tema oscuro, claro o del sistema, orden de la lista, contenido del pie, notificaciones, perfiles de terminal, historial y ejecutables.
* Perfiles de terminal con fuente, colores, cursor y parpadeo, asignables por sesión. Se comprobó el cambio en un terminal abierto.
* El límite de historial llega al servidor tmux en marcha, comprobado con `show-options`.
* Archivar y desarchivar sesiones, con filtro propio en la barra lateral.
* Orden de las sesiones por actividad, nombre o fecha.
* La base de datos migra sola al esquema 3 sin perder sesiones.

Sin comprobar con la aplicación en marcha: el tema del sistema al cambiar el escritorio de claro a oscuro, editar y borrar un proyecto, el interruptor de cada dato del pie y de cada notificación, la ruta propia del ejecutable de Claude Code y el cambio de shell. Todo ello tiene pruebas del núcleo salvo la reacción de la interfaz.

Limitaciones conocidas:

* Los colores de las etiquetas de proyecto están pensados para el tema oscuro y en el claro contrastan menos.
* Con un orden que no sea el manual, arrastrar una sesión la cambia de grupo pero no de posición.

## Fase 7. Optimización: hecha

Implementado y comprobado con la aplicación en marcha:

* Medición con 5, 10 y 20 sesiones, repetible con `scripts/measure.sh`. Los resultados están en el README.
* Los terminales que llevan cinco minutos ocultos se desconectan y se vuelven a conectar al mostrarse. Se comprobó que los clientes tmux bajan de uno por sesión a uno, y que al volver a la pestaña el terminal se redibuja.
* La selección con el ratón dentro de tmux llega al portapapeles del sistema, sin depender de `xclip` ni de `wl-copy`. Se comprobó copiando en tmux y pegando con `Ctrl+Mayús+V`.

Sin hacer: medir con aceleración gráfica real y con sesiones de Claude Code trabajando. La memoria del proceso no baja al desconectar terminales, solo desaparecen sus clientes tmux.

## Fase 8. Distribución: hecha

Implementado y comprobado:

* Icono y entrada de escritorio en `data/`, validada con `desktop-file-validate`.
* `scripts/install.sh` instala en `~/.local` sin administrador y `--uninstall` lo retira. Se comprobó instalando en una carpeta de pruebas: el binario instalado arranca, encuentra `alarviterm-hook` a su lado y carga su copia de libvte.
* `scripts/package-deb.sh` genera `target/alarviterm_0.1.0_amd64.deb`, de 590 kB, con dependencias declaradas y sin rutas de la máquina de compilación.

Sin comprobar: instalar el `.deb`, porque requiere administrador. Se revisó su contenido y su fichero de control con `dpkg-deb`. Tampoco se ha instalado en el `~/.local` real, eso queda a decisión del usuario.

Flatpak y AppImage, valorados y descartados por ahora. La explicación está en `docs/ARCHITECTURE.md`.

## Fase 9. Límites de la cuenta y complementos: hecha

Implementado y comprobado con la aplicación en marcha:

* Límites de uso en el pie, con lo que queda de las ventanas de 5 horas y de 7 días y el tiempo hasta el reinicio. Se comprobó con una sesión real de Claude Code lanzada con la línea de estado encadenada: la línea de estado del usuario se siguió viendo igual y los límites reales quedaron guardados. El pie se comprobó con un fichero de límites escrito a mano: los medidores con barra, sus colores según lo que queda, la actualización al cambiar el fichero y el panel de detalle.
* Consumo de la sesión activa con una gráfica diminuta de CPU. Se comprobó con un proceso que alterna carga y reposo.
* Consumo de la propia aplicación en el pie. Se comprobó que la memoria que muestra coincide con la proporcional que da el sistema para el proceso.
* Complementos externos con el contrato de `docs/PLUGINS.md`. Se comprobó con el de ejemplo: botón en la fila, acción, aviso, cronómetro que avanza, cambio del botón tras la acción y acción en la paleta.

* Panel de estadísticas de uso. Se comprobó con transcripciones de demostración, incluido el cambio de periodo, y con las transcripciones reales de un equipo con unos 8 GB en dos meses: 10 segundos la primera lectura en modo release y sin relectura después.

Sin comprobar con la aplicación en marcha:

* El panel de estadísticas en el tema claro.
* El pie con los límites llegando desde una sesión de Claude Code abierta dentro de ALarviTerm. Cada mitad se probó por separado.
* El interruptor de Ajustes que apaga los límites.
* Las acciones de complementos en el menú contextual, la apertura de una dirección web y el aviso de un complemento que falla. La lectura defensiva de las respuestas y el tiempo máximo tienen pruebas en el núcleo.

Limitaciones:

* Los complementos se leen al arrancar. Para añadir o quitar uno hay que reiniciar la aplicación.
* La línea de estado de los ajustes de un proyecto no se encadena, solo la de los ajustes personales.

## Ideas para después

* Estadísticas: coste estimado, filtro por carpeta al pulsar una fila y agrupar las subcarpetas de un mismo proyecto.
* Recargar los complementos sin reiniciar y una página de ajustes que los liste.

* Colores de las etiquetas de proyecto adaptados al tema claro.
* Reordenar arrastrando con cualquier orden de lista.
* Medición con aceleración gráfica real y con sesiones de Claude Code trabajando.
* Publicar el paquete en un repositorio `apt` propio para recibir actualizaciones.
