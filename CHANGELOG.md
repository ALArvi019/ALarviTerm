# Cambios

Todos los cambios relevantes de ALarviTerm se anotan aquí. El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y las versiones siguen [SemVer](https://semver.org/lang/es/).

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
