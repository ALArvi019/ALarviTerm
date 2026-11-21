# Arquitectura de ALarviTerm

Documento de la Fase 1. Recoge qué se eligió, por qué, y qué se descartó.

## Resumen

ALarviTerm es una aplicación GTK4 nativa escrita en Rust. Cada sesión es un proceso real que vive dentro de un servidor tmux propio. La interfaz muestra cada sesión con un widget VTE, que es el mismo emulador de terminal que usan GNOME Terminal y Terminator. La aplicación no emula ni dibuja terminales por su cuenta.

```
┌───────────────────────── ALarviTerm (un proceso) ─────────────────────────┐
│  crates/app   GTK4 + libadwaita + VTE                                     │
│     ventana, barra lateral, diálogos, footer, widget de terminal          │
│                              │                                            │
│  crates/core  sin GTK                                                     │
│     SessionManager ── Database (SQLite)                                   │
│          │                                                                │
│     trait SessionBackend ── TmuxBackend                                   │
└──────────│────────────────────────────────────────────────────────────────┘
           │ tmux -L alarviterm (socket y configuración propios)
┌──────────▼───────────────┐
│ servidor tmux            │  sigue vivo aunque ALarviTerm se cierre
│  sesión mt-xxxx → claude │
│  sesión mt-yyyy → zsh    │
└──────────────────────────┘
```

## Decisiones

### Toolkit: GTK4 con libadwaita

Alternativas valoradas:

* **GTK4 + libadwaita + VTE** (elegida). Bibliotecas ya cargadas en cualquier escritorio Linux, por lo que el coste real de memoria es bajo. VTE es un emulador completo y maduro. Bindings de Rust oficiales y mantenidos (`gtk4`, `libadwaita`, `vte4`).
* **Tauri, Electron o cualquier WebView**. Descartadas: arrastran un motor web de cientos de MB y obligan a un terminal en JavaScript.
* **egui o iced con `alacritty_terminal`**. Descartadas: no hay widget de terminal maduro, habría que escribir el renderizado, la selección, el IME y el ratón. Justo lo que el proyecto pide no hacer.
* **Qt con QTermWidget**. Viable, pero los bindings de Rust para Qt son inmaduros.

libadwaita aporta valor concreto y se queda: gestor de tema (oscuro, claro, sistema), diálogos, filas de formulario para los ajustes y, más adelante, pestañas. En KDE funciona igual. El aspecto se ajusta con una hoja CSS propia.

### Terminal: VTE

VTE 0.76 cubre lo exigido: color verdadero, 256 colores, Unicode, ratón, aplicaciones a pantalla completa. El widget lanza en su PTY el comando que le entrega el backend. Para tmux, ese comando es `tmux attach`.

La capa de terminal está aislada en `crates/app/src/ui/terminal.rs`. Cambiar de emulador en el futuro afecta solo a ese fichero.

### Persistencia de procesos: tmux detrás de un trait

Un proceso lanzado directamente en el PTY del widget muere con la ventana. Para que las sesiones sobrevivan al cierre de la interfaz tiene que existir un proceso aparte que las sostenga. Opciones:

* **tmux** (elegida). Probado durante años, ya instalado, mantiene historial y permite reconectar.
* **Demonio propio** que guarde los PTY. Más control, pero es reescribir tmux y añadir un proceso que mantener.
* **`abduco` o `dtach`**. Más ligeros, pero no guardan el contenido de pantalla: al reconectar se ve un terminal vacío.

La interfaz no conoce tmux. Habla con `SessionBackend` (`crates/core/src/sessions/backend.rs`):

```rust
pub trait SessionBackend {
    fn name(&self) -> &'static str;
    fn check(&self) -> Result<String>;
    fn create_session(&self, spec: &LaunchSpec) -> Result<()>;
    fn attach_command(&self, backend_id: &str) -> Result<AttachCommand>;
    fn detach_session(&self, backend_id: &str) -> Result<()>;
    fn list_sessions(&self) -> Result<Vec<BackendSession>>;
    fn kill_session(&self, backend_id: &str) -> Result<()>;
    fn send_input(&self, backend_id: &str, text: &str) -> Result<()>;
    fn resize(&self, backend_id: &str, cols: u16, rows: u16) -> Result<()>;
    fn get_status(&self, backend_id: &str) -> Result<Liveness>;
}
```

`attach_command` es la pieza que desacopla: devuelve el comando que un terminal debe ejecutar para ver la sesión. Otro backend devolvería otro comando y la interfaz no cambiaría.

Detalles del backend tmux:

* Servidor propio con `tmux -L alarviterm` y un fichero de configuración generado en el directorio de datos. No lee ni toca `~/.tmux.conf` ni las sesiones tmux del usuario.
* Sin tecla de prefijo y sin barra de estado. Ctrl+B llega intacto a Claude Code.
* `remain-on-exit on`: cuando el proceso termina, el panel se conserva con su salida y su código de salida. De ahí salen los estados Terminada y Error, que son datos y no heurísticas.
* Una sesión tmux por sesión de ALarviTerm, con un único panel. El identificador `mt-<id>` se guarda en SQLite y es único, así que no pueden crearse duplicados. Las sesiones vivas que no estén en la base de datos se adoptan al arrancar.

### Lanzamiento de Claude Code

Las sesiones de Claude Code se lanzan como `$SHELL -ic 'exec claude ...'`. Así heredan el mismo entorno que en una terminal normal (PATH, gestores de versiones, variables de `.zshrc`), también cuando ALarviTerm se abre desde el lanzador del escritorio. Las sesiones de shell usan el shell por defecto.

Cada proceso recibe `ALARVITERM_SESSION_ID`, `ALARVITERM_STATE_DIR` y `ALARVITERM_LAUNCH_ID`, con los que `alarviterm-hook` atribuye cada evento a su sesión y a su lanzamiento.

### Estados: solo lo comprobable

Hay dos fuentes. tmux dice si el proceso existe y con qué código salió: Activa, Terminada, Error y Desconectada. Los hooks de Claude Code dicen qué hace mientras vive: Lista, Pensando, Ejecutando herramienta, Pide permiso, Esperando respuesta y Compactando. Nada se deduce leyendo la pantalla. Una sesión sin eventos se queda en Activa.

### Monitorización con hooks

Validado contra Claude Code 2.1.330 antes de escribir el código: `--settings` acepta un fichero con hooks que se suman a los del usuario, y los eventos llegan con el entorno del proceso.

El recorrido de un evento:

1. Al arrancar, la aplicación escribe `claude-hooks.json` en su directorio de datos. Registra `alarviterm-hook` para `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `Notification`, `Stop`, `PreCompact` y `SessionEnd`.
2. Cada sesión de Claude Code se lanza con `--settings` apuntando a ese fichero. Si el usuario puso su propio `--settings` en los argumentos, se respeta el suyo y la sesión no se monitoriza.
3. Claude Code ejecuta `alarviterm-hook` con el evento en JSON por la entrada estándar. El binario aplica una máquina de estados pura (`monitor::apply_event`) y escribe el resultado en `state/<sesión>.json` con escritura atómica. No imprime nada y siempre sale con 0, para no inyectar texto en el contexto de Claude ni bloquear herramientas.
4. La aplicación vigila el directorio con `GFileMonitor` (inotify), agrupa las ráfagas durante 80 ms y refresca.

Decisiones:

* **Ficheros en lugar de un socket**. El estado sobrevive al cierre de la aplicación, así que al volver a abrirla se ve lo último que pasó. No hay servidor que mantener ni conexiones que reintentar.
* **Binario aparte y sin GTK**. Se ejecuta en cada llamada a herramienta, tiene que arrancar en milisegundos. Mide 450 kB y tarda unos 2 ms.
* **Marca de lanzamiento**. Al reiniciar una sesión, el Claude Code que muere emite `SessionEnd` cuando el nuevo ya ha arrancado. Cada arranque recibe un `ALARVITERM_LAUNCH_ID` aleatorio que queda anotado en `state/<sesión>.launch`, y solo se muestra el estado cuyo lanzamiento coincide. El fallo se encontró probando con un Claude Code real.
* **El estado solo cuenta con el proceso vivo**. Si tmux dice que el proceso no existe, manda tmux.
* **Conversación exacta**. El identificador que llega en los eventos se guarda y el reinicio usa `--resume <id>`. `--continue` elegiría la conversación más reciente del directorio, que puede ser de otra sesión.
* **Fin de proceso sin sondeo**. tmux toca `state/.backend` en sus hooks `pane-died` y `session-closed`, y la misma vigilancia lo recoge.

Límites conocidos: Claude Code no emite ningún evento al conceder un permiso ni al interrumpir un turno con Esc. En el primer caso el estado se corrige cuando la herramienta termina, en el segundo con su aviso de inactividad, alrededor de un minuto después.

### Datos del pie

La rama de git y el consumo se consultan para la sesión activa en el pool de hilos de GLib (`gio::spawn_blocking`), nunca en el hilo de la interfaz. Git se consulta con `GIT_OPTIONAL_LOCKS=0` para no estorbar al git del usuario. El consumo suma el árbol de procesos que cuelga del proceso de la sesión leyendo `/proc` una vez. El modelo sale de la transcripción al terminar cada turno, leyendo solo el final del fichero.

### Paneles: un modelo y una función que lo dibuja

La disposición es un árbol binario (`crates/core/src/layout.rs`). Las hojas son paneles con pestañas y los nodos internos son divisiones con una orientación y una proporción. Todas las operaciones (dividir, cerrar, mover una sesión, buscar el panel vecino, guardar en JSON) son funciones puras con sus pruebas.

La interfaz no guarda estado propio sobre los paneles. `MainWindow::render` recibe el árbol y deja los widgets como corresponde: un `GtkPaned` por división y una vista por panel. Las vistas y los terminales se reutilizan entre llamadas, así que dividir o mover no reinicia nada. El árbol de widgets solo se reconstruye cuando cambia la forma.

Consecuencias prácticas:

* Una sesión se muestra en un único panel. Un widget de terminal solo puede tener un padre, y dos clientes tmux sobre la misma sesión se pelearían por el tamaño.
* Solo tienen terminal las sesiones con pestaña abierta. Cerrar la pestaña libera el widget y su cliente tmux, y el proceso sigue en el servidor.
* Las pestañas inactivas crean su terminal la primera vez que se activan.

El arrastre usa un tipo propio (`SessionRef`) y no texto, para que el terminal no lo interprete como texto que pegar.

### Acciones, atajos y paleta

Todo lo que se puede hacer desde el teclado está declarado una vez, en `core::actions::ACTIONS`: identificador, título, familia y atajos de fábrica. El identificador es el nombre de la acción de ventana de GTK que la ejecuta. De esa tabla salen tres cosas, y por eso no pueden desincronizarse: el mapa de teclas, las entradas de la paleta y las filas del editor de atajos.

El mapa de teclas (`Keymap`) une los atajos de fábrica con los cambios del usuario, que se guardan como JSON en el ajuste `keys.overrides` y solo contienen las diferencias. Una combinación pertenece a una sola acción. Asignarla a otra se la quita a la anterior. Un JSON ilegible se ignora y quedan los atajos de fábrica.

No se usan los aceleradores de GTK. Un controlador de teclado en fase de captura sobre la ventana traduce cada pulsación a una combinación del núcleo y consulta el mapa. Es la única forma de decidir antes que el terminal. Cada atajo lleva un dato, si actúa con el foco en un terminal. Así `Ctrl+N` puede crear una sesión desde la barra lateral y seguir llegando a Claude Code dentro del terminal. La función `safe_in_terminal` propone ese dato al capturar un atajo nuevo: Ctrl+Mayús, Súper, Ctrl+Tab, Ctrl+número y Ctrl+RePág o AvPág no los usa casi ningún programa de terminal. El usuario puede contradecirla.

La paleta ordena con `core::search::rank`. Cada palabra de la consulta tiene que encajar, literal o como letras en orden que empiezan en inicio de palabra. La barra lateral usa en cambio coincidencia literal, porque una lista que se filtra al escribir no debe enseñar parecidos lejanos.

### Ajustes, perfiles y proyectos

Los ajustes son una estructura con tipos, `core::settings::Settings`, que se guarda como un único JSON en la tabla de ajustes. Se lee con tolerancia: lo que falta o no se entiende toma su valor de fábrica, y `normalize` deja siempre un estado coherente, con al menos un perfil de terminal y un perfil predeterminado que existe. La interfaz no valida nada por su cuenta. Cada cambio en la ventana de ajustes modifica la estructura, la guarda y llama a una sola función, `apply_settings`, que lo lleva al tema, al pie, al gestor de sesiones, a la configuración de tmux y a los terminales abiertos.

El tema cambia recargando la hoja de estilos con otro bloque de colores con nombre. Las reglas son las mismas para claro y oscuro. Los colores de cada terminal salen de su perfil, y una clase CSS por esquema pinta el margen del terminal del mismo fondo.

Un proyecto vive en su propia tabla y las sesiones lo referencian. Sus variables y su comando de inicio no se copian a la sesión: se leen en cada lanzamiento. Así editar un proyecto vale para el siguiente arranque de todas sus sesiones. El comando de inicio se antepone dentro del mismo `$SHELL -ic`, unido con `&&`, de modo que un fallo impide el arranque y se ve.

Archivar es una marca en la sesión. El filtro de la barra lateral se consulta siempre, también sin nada elegido, y es él quien oculta las archivadas.

### Persistencia de datos: SQLite

Un único fichero `alarviterm.db` con grupos, sesiones, disposiciones guardadas y ajustes. Migraciones numeradas con `PRAGMA user_version`. Se enlaza con el SQLite del sistema.

### Concurrencia: sin Tokio

No hace falta. Las órdenes a tmux tardan pocos milisegundos y se ejecutan en el hilo principal. Lo lento (`claude --version`, git, la lectura de `/proc`) va al pool de hilos de GLib. Los cambios de estado llegan por inotify. Queda un refresco de respaldo cada 10 segundos, una sola ejecución de tmux y solo con la ventana activa.

### Estructura del código

Tres crates en un workspace:

* `crates/core`: modelo, SQLite, backends, detección y lanzamiento de Claude Code, monitorización y datos del sistema. Sin dependencias gráficas, se prueba en cualquier máquina.
* `crates/app`: la interfaz GTK.
* `crates/hook`: el binario `alarviterm-hook`, una capa mínima sobre `core::monitor`.

Respecto a la propuesta inicial se han fusionado módulos que hoy serían de un solo fichero pequeño. Se separarán cuando crezcan.

### Terminales ocultos

Un terminal solo existe para las pestañas que se han mostrado. Cada uno mantiene un cliente del backend, y el backend le sigue enviando la salida aunque nadie la mire. Por eso un terminal que lleva cinco minutos sin ser la pestaña activa de ningún panel se suelta. El proceso de la sesión no se entera, y al volver a su pestaña se crea un terminal nuevo que se conecta y recibe la pantalla entera.

### Distribución

Dos caminos, los dos con scripts del repositorio. La instalación en `~/.local` no necesita administrador. El paquete `.deb` es el camino limpio en Ubuntu y Debian, porque declara las bibliotecas y tmux como dependencias.

El binario busca `libvte-2.91-gtk4` primero en `../lib/alarviterm` respecto a sí mismo. Es donde el script de instalación deja una copia cuando el sistema no la tiene. El binario del paquete se compila aparte, sin rutas de biblioteca propias y con las rutas de los fuentes sustituidas, y el script se niega a empaquetar si encuentra alguna.

Flatpak no encaja bien. ALarviTerm existe para lanzar el `claude`, el shell y el `tmux` del usuario, con su entorno y sus ficheros. Dentro del aislamiento de Flatpak habría que saltárselo para todo con `flatpak-spawn --host`, y lo que quedaría dentro sería solo la ventana. AppImage es viable pero obliga a empaquetar GTK 4, libadwaita y VTE, decenas de megabytes para una aplicación de uno y medio, y no resuelve la dependencia de tmux. Si hiciera falta salir de Debian y Ubuntu, AppImage sería la opción.

## Limitaciones conocidas del diseño

* **Selección de texto**. Con el ratón activo en tmux, arrastrar selecciona dentro de tmux. Al soltar, tmux entrega el texto a su `copy-command`, que lo escribe en un fichero del directorio de estado. La aplicación ya vigila ese directorio, lleva el texto al portapapeles y borra el fichero. VTE no atiende la secuencia OSC 52, por eso no se usa. Para la selección nativa del terminal hay que mantener Mayús al arrastrar.
* **Historial**. Lo guarda tmux. Se recorre con la rueda del ratón, no con una barra de desplazamiento.
* **Mayús+Intro**. VTE 0.76 no distingue esa tecla, así que ALarviTerm la traduce a la secuencia que Claude Code entiende como salto de línea.
* **Reinicio del equipo**. tmux no sobrevive. Las sesiones aparecen como Desconectadas y se pueden relanzar, las de Claude Code también con `--continue`.
