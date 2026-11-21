# Seguridad

## Avisar de un fallo

Si encuentras un fallo de seguridad, no abras una incidencia pública. Usa el aviso privado de GitHub, en la pestaña Security del repositorio, opción Report a vulnerability.

## Qué toca la aplicación

* Lanza los programas del usuario (`claude`, el shell y `tmux`) con su mismo entorno y sus mismos permisos. No eleva privilegios.
* Guarda sus datos en `~/.local/share/alarviterm`: la base de datos SQLite, la configuración de su servidor tmux y el estado de cada sesión.
* No lee ni guarda credenciales. Para saber si Claude Code tiene sesión iniciada solo comprueba que el fichero de credenciales existe, sin abrirlo.
* No modifica `~/.claude/` ni la configuración global de tmux. Los hooks se registran por sesión con `claude --settings`.
* No hace conexiones de red.

Las variables de entorno que se definan en una sesión o en un proyecto se guardan en claro en la base de datos local. No pongas secretos ahí si la carpeta de datos se comparte o se copia.
