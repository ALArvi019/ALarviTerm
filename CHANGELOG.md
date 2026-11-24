# Cambios

Todos los cambios relevantes de ALarviTerm se anotan aquí. El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y las versiones siguen [SemVer](https://semver.org/lang/es/).

## Sin publicar

### Añadido

* Continuar una conversación de Claude Code de cualquier carpeta como sesión suelta, sin proyecto, con `Ctrl+Mayús+H`.
* Atajo `Ctrl+Mayús+O` para abrir un terminal en un panel nuevo al lado, en la carpeta de la sesión actual.
* Botón en la pestaña de código para plegar todo o desplegar de una vez todos los repositorios con cambios.
* Eliminar un árbol de trabajo de git desde la pestaña de código, con doble confirmación.
* Límites de uso de la cuenta en el pie: lo usado de las ventanas de 5 horas y de 7 días y el tiempo hasta su reinicio.
* Medidores en el pie con barras finas: CPU, memoria y red del equipo, con un panel ampliado al pasar el ratón que incluye la sesión activa y la propia aplicación.
* Panel de estadísticas de uso de Claude Code, a partir de sus transcripciones locales.
* Añadir a un proyecto conversaciones de Claude Code que ya existen, reanudándolas con su historial.
* Argumentos propuestos al crear una sesión, en Ajustes.
* Cuenta de Claude por proyecto: límites de uso separados por cuenta y filtro por cuenta en las estadísticas.
* Encender y apagar de una vez las sesiones de un grupo o de un proyecto.
* Menú contextual en las pestañas para cerrar esta, las demás o las de un lado, y reordenado arrastrando.
* Programas que más CPU y memoria consumen en el panel del equipo, que queda fijo con un clic.
* Icono nuevo de la aplicación.
* Columna derecha con las listas que los complementos aportan para la sesión a la vista, y ejemplo `examples/plugins/pendientes`.
* Imagen propia para cada proyecto, en lugar del icono de texto.
* Un grupo plegado enseña el estado de sus sesiones: primero quien pide permiso o espera respuesta, después quien trabaja.
* Modo de permisos de la sesión a la vista en el pie.
* Pestaña de código en la columna izquierda: repositorios y árboles de trabajo de las carpetas abiertas, árbol de ficheros cambiados, comparación lado a lado con la última confirmación en el sitio de los terminales y editor con guardado.
* Estado propio para las sesiones que terminaron su turno con un comando o un subagente en segundo plano, que ya no figuran como si esperasen respuesta.
* Tema de Claude Code propio para las sesiones de ALarviTerm.
* Registro en fichero, con la pila de llamadas de un fallo, y aviso al abrir tras un cierre inesperado.
* Contexto usado por la sesión activa en el pie, y opción de no mostrar la línea de estado propia dentro de las sesiones.
* Complementos: programas externos que añaden a cada sesión una etiqueta, un cronómetro y acciones. Contrato en `docs/PLUGINS.md` y ejemplo en `examples/plugins/cronometro`.

### Cambiado

* En la comparación de código las dos mitades se desplazan a la par en los dos sentidos, y una barra de marcas señala y lleva a cada bloque de cambios.
* El árbol de código lleva iconos por tipo de fichero, fondo verde o rojo para lo nuevo y lo borrado, filas más juntas y flechas más grandes.
* Las recargas por eventos de las sesiones se espacian y git y el consumo de la sesión se consultan como mucho cada cuatro segundos. Con cinco sesiones emitiendo cien eventos por segundo la aplicación pasa del 10 % al 2,6 % de un núcleo.

### Corregido

* Un menú contextual abierto sobre una fila o una pestaña que se redibujaba podía cerrar la aplicación.
* Los colores de Claude Code se veían reducidos a 256 dentro de la aplicación. Ahora las sesiones reciben color de 24 bits.
* Cierre inesperado al retirar un terminal mientras su proceso terminaba.
* El instalador ya no sobrescribe en el sitio la biblioteca de terminal de una instancia abierta.

## [0.1.0] 2026-11-21

Primera versión.

### Añadido

* Sesiones de Claude Code y de shell en terminales VTE reales, mantenidas vivas por un servidor tmux propio.
* Barra lateral con grupos, búsqueda, filtros por estado, orden configurable y archivado.
* Paneles divisibles con pestañas, arrastre entre paneles y disposiciones guardadas.
* Estado de cada sesión de Claude Code a partir de sus hooks, sin leer la pantalla y sin tocar la configuración global.
* Notificaciones de escritorio cuando una sesión pide permiso o termina su turno.
* Paleta de comandos y atajos de teclado configurables.
* Proyectos con carpeta, color, icono, variables de entorno y comando de inicio.
* Ajustes: tema oscuro, claro o del sistema, perfiles de terminal, historial, ejecutables, pie y notificaciones.
* Instalación en la carpeta del usuario con `scripts/install.sh` y paquete `.deb` con `scripts/package-deb.sh`.
