use std::io::Read;
use std::path::{Path, PathBuf};

use super::backend::{BackendSession, LaunchSpec, Liveness, SessionBackend};
use crate::claude::{build_argv, Resume};
use crate::model::{Group, NewSession, Project, Session, SessionKind};
use crate::monitor::{AgentState, StateStore, LAUNCH_ENV, STATE_DIR_ENV};
use crate::persistence::Database;
use crate::{Error, Result};

/// Variable con la que cada proceso sabe a qué sesión de ALarviTerm pertenece.
/// La monitorización por hooks la usa para atribuir los eventos.
pub const SESSION_ENV: &str = "ALARVITERM_SESSION_ID";

/// Sesión guardada junto a lo que el backend dice de ella ahora mismo.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub session: Session,
    pub liveness: Liveness,
    pub last_output_at: Option<i64>,
    /// Lo que Claude Code ha contado por sus hooks. Solo se rellena con el
    /// proceso vivo: un estado de un proceso que ya no existe no dice nada.
    pub agent: Option<AgentState>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub groups: Vec<Group>,
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionView>,
    /// Si el backend no respondió, el motivo. Las sesiones figuran entonces
    /// como [`Liveness::Missing`] aunque puedan seguir vivas.
    pub backend_error: Option<String>,
}

/// Une la base de datos con el backend. Toda la lógica de sesiones pasa por
/// aquí, la interfaz no habla con ninguno de los dos directamente.
pub struct SessionManager {
    db: Database,
    backend: Box<dyn SessionBackend>,
    claude: Option<PathBuf>,
    shell: String,
    monitor: Option<Monitor>,
}

/// Lo necesario para monitorizar sesiones de Claude Code con hooks.
#[derive(Debug, Clone)]
struct Monitor {
    store: StateStore,
    settings: PathBuf,
}

impl SessionManager {
    pub fn new(db: Database, backend: Box<dyn SessionBackend>, shell: String) -> Self {
        Self {
            db,
            backend,
            claude: None,
            shell,
            monitor: None,
        }
    }

    /// Activa la monitorización: las sesiones de Claude Code que se lancen a
    /// partir de ahora cargan `settings` y dejan su estado en `store`.
    pub fn enable_monitor(&mut self, store: StateStore, settings: PathBuf) {
        self.monitor = Some(Monitor { store, settings });
    }

    pub fn monitor_enabled(&self) -> bool {
        self.monitor.is_some()
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn db_mut(&mut self) -> &mut Database {
        &mut self.db
    }

    pub fn backend(&self) -> &dyn SessionBackend {
        self.backend.as_ref()
    }

    pub fn claude_executable(&self) -> Option<&Path> {
        self.claude.as_deref()
    }

    pub fn set_claude_executable(&mut self, path: Option<PathBuf>) {
        self.claude = path;
    }

    /// Shell con el que se lanzan las sesiones a partir de ahora.
    pub fn set_shell(&mut self, shell: String) {
        self.shell = shell;
    }

    pub fn shell(&self) -> &str {
        &self.shell
    }

    /// Estado completo para pintar la interfaz: una consulta a la base de
    /// datos y una única llamada al backend.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let groups = self.db.groups()?;
        let projects = self.db.projects()?;
        let sessions = self.db.sessions()?;
        let (live, backend_error) = match self.backend.list_sessions() {
            Ok(live) => (live, None),
            Err(e) => {
                log::error!("el backend no responde: {e}");
                (Vec::new(), Some(e.to_string()))
            }
        };
        let sessions = sessions
            .into_iter()
            .map(|session| {
                let found = live.iter().find(|l| l.backend_id == session.backend_id);
                let liveness = found.map(|l| l.liveness.clone()).unwrap_or(Liveness::Missing);
                let agent = match &self.monitor {
                    Some(monitor) if session.kind == SessionKind::Claude && liveness.is_alive() => {
                        monitor.store.read(&session.id)
                    }
                    _ => None,
                };
                SessionView {
                    liveness,
                    last_output_at: found.and_then(|l| l.last_output_at),
                    agent,
                    session,
                }
            })
            .collect();
        Ok(Snapshot {
            groups,
            projects,
            sessions,
            backend_error,
        })
    }

    pub fn create_session(&self, new: NewSession) -> Result<Session> {
        let name = new.name.trim().to_string();
        if name.is_empty() {
            return Err(Error::Invalid("la sesión necesita un nombre".into()));
        }
        if !new.cwd.is_dir() {
            return Err(Error::Invalid(format!(
                "el directorio {} no existe",
                new.cwd.display()
            )));
        }
        let id = random_id()?;
        let session = Session {
            backend_id: format!("mt-{id}"),
            id,
            name,
            group_id: new.group_id,
            kind: new.kind,
            cwd: new.cwd,
            model: new.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()),
            extra_args: new.extra_args.trim().to_string(),
            env: new.env,
            backend: self.backend.name().to_string(),
            position: self.db.next_session_position()?,
            created_at: crate::now_unix(),
            last_opened_at: None,
            project_id: new.project_id,
            archived: false,
            profile: None,
        };
        let spec = self.launch_spec(&session, &Resume::Fresh)?;
        self.backend.create_session(&spec)?;
        if let Err(e) = self.db.insert_session(&session) {
            // Sin registro en la base de datos el proceso quedaría huérfano.
            if let Err(kill) = self.backend.kill_session(&session.backend_id) {
                log::error!("no se pudo retirar la sesión {}: {kill}", session.backend_id);
            }
            return Err(e);
        }
        log::info!("sesión {} creada ({})", session.id, session.name);
        Ok(session)
    }

    /// Vuelve a arrancar el proceso de una sesión, esté viva, terminada o
    /// desaparecida del backend. Conserva identidad y configuración.
    ///
    /// Al pedir continuar, si los hooks dejaron anotada la conversación de
    /// esta sesión se reanuda esa en concreto. `--continue` elegiría la más
    /// reciente del directorio, que puede ser de otra sesión.
    pub fn relaunch(&self, id: &str, resume: Resume) -> Result<()> {
        let session = self.db.session(id)?;
        let store = self.monitor.as_ref().map(|m| &m.store);
        let resume = match resume {
            Resume::ContinueLatest => store
                .and_then(|s| s.read_any(id))
                .and_then(|state| state.conversation)
                .map(Resume::Conversation)
                .unwrap_or(Resume::ContinueLatest),
            other => other,
        };
        let spec = self.launch_spec(&session, &resume)?;
        self.backend.kill_session(&session.backend_id)?;
        self.backend.create_session(&spec)
    }

    /// Termina el proceso y conserva la sesión en la lista para relanzarla.
    pub fn stop(&self, id: &str) -> Result<()> {
        let session = self.db.session(id)?;
        self.backend.kill_session(&session.backend_id)
    }

    /// Termina el proceso y borra la sesión.
    pub fn delete(&self, id: &str) -> Result<()> {
        let session = self.db.session(id)?;
        self.backend.kill_session(&session.backend_id)?;
        if let Some(monitor) = &self.monitor {
            monitor.store.clear(id);
        }
        self.db.delete_session(id)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<()> {
        self.db.rename_session(id, name)
    }

    pub fn move_to_group(&self, id: &str, group_id: Option<i64>) -> Result<()> {
        self.db.move_session(id, group_id)
    }

    /// Archiva o recupera una sesión. Archivar no toca el proceso.
    pub fn set_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.db.set_session_archived(id, archived)
    }

    pub fn set_profile(&self, id: &str, profile: Option<&str>) -> Result<()> {
        self.db.set_session_profile(id, profile)
    }

    pub fn set_project(&self, id: &str, project_id: Option<i64>) -> Result<()> {
        self.db.set_session_project(id, project_id)
    }

    pub fn mark_opened(&self, id: &str) -> Result<()> {
        self.db.touch_session(id, crate::now_unix())
    }

    /// Da de alta las sesiones que viven en el backend pero no están en la
    /// base de datos (por ejemplo, tras perderla). Así nunca queda un proceso
    /// gestionado sin entrada visible ni se crean duplicados.
    pub fn adopt_orphans(&self) -> Result<usize> {
        let known = self.db.sessions()?;
        let orphans: Vec<BackendSession> = self
            .backend
            .list_sessions()?
            .into_iter()
            .filter(|live| known.iter().all(|s| s.backend_id != live.backend_id))
            .collect();
        for orphan in &orphans {
            let session = Session {
                id: random_id()?,
                name: format!("Recuperada {}", orphan.backend_id),
                group_id: None,
                // No hay forma fiable de saber qué se lanzó: se trata como shell.
                kind: SessionKind::Shell,
                cwd: orphan
                    .cwd
                    .clone()
                    .or_else(|| crate::paths::home_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("/")),
                model: None,
                extra_args: String::new(),
                env: Vec::new(),
                backend: self.backend.name().to_string(),
                backend_id: orphan.backend_id.clone(),
                position: self.db.next_session_position()?,
                created_at: crate::now_unix(),
                last_opened_at: None,
                project_id: None,
                archived: false,
                profile: None,
            };
            self.db.insert_session(&session)?;
            log::warn!("sesión huérfana {} adoptada", orphan.backend_id);
        }
        Ok(orphans.len())
    }

    fn launch_spec(&self, session: &Session, resume: &Resume) -> Result<LaunchSpec> {
        let settings = self.monitor.as_ref().map(|m| m.settings.as_path());
        // El proyecto aporta variables y comando de inicio. Se leen en cada
        // lanzamiento, así que editar el proyecto vale para el siguiente.
        let project = match session.project_id {
            Some(id) => self.db.projects()?.into_iter().find(|p| p.id == id),
            None => None,
        };
        let startup = project.as_ref().map(|p| p.startup.as_str());
        let argv = build_argv(
            session,
            self.claude.as_deref(),
            &self.shell,
            resume,
            settings,
            startup,
        )?;
        // Las variables de la sesión mandan sobre las del proyecto.
        let mut env: Vec<(String, String)> = project
            .map(|p| p.env)
            .unwrap_or_default()
            .into_iter()
            .filter(|(key, _)| session.env.iter().all(|(own, _)| own != key))
            .collect();
        env.extend(session.env.iter().cloned());
        env.retain(|(key, _)| key != SESSION_ENV && key != STATE_DIR_ENV && key != LAUNCH_ENV);
        env.push((SESSION_ENV.to_string(), session.id.clone()));
        if let Some(monitor) = &self.monitor {
            // Cada arranque lleva su marca: el proceso anterior aún puede
            // emitir eventos mientras muere y no deben atribuirse al nuevo.
            let launch = random_id()?;
            monitor.store.begin_launch(&session.id, &launch)?;
            env.push((LAUNCH_ENV.to_string(), launch));
            env.push((
                STATE_DIR_ENV.to_string(),
                monitor.store.dir().to_string_lossy().into_owned(),
            ));
        }
        Ok(LaunchSpec {
            backend_id: session.backend_id.clone(),
            cwd: session.cwd.clone(),
            argv,
            env,
        })
    }
}

/// Identificador corto y aleatorio (48 bits en hexadecimal).
fn random_id() -> Result<String> {
    let mut bytes = [0u8; 6];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::sessions::AttachCommand;

    /// Backend en memoria para probar el gestor sin procesos reales.
    #[derive(Default)]
    struct FakeState {
        sessions: Vec<(LaunchSpec, Liveness)>,
        fail_list: bool,
    }

    #[derive(Clone, Default)]
    struct FakeBackend(Rc<RefCell<FakeState>>);

    impl SessionBackend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn check(&self) -> Result<String> {
            Ok("fake 1".into())
        }
        fn create_session(&self, spec: &LaunchSpec) -> Result<()> {
            let mut state = self.0.borrow_mut();
            if state.sessions.iter().any(|(s, _)| s.backend_id == spec.backend_id) {
                return Err(Error::Backend {
                    backend: "fake",
                    message: "duplicada".into(),
                });
            }
            state.sessions.push((
                spec.clone(),
                Liveness::Alive {
                    pid: Some(1),
                    command: None,
                },
            ));
            Ok(())
        }
        fn attach_command(&self, backend_id: &str) -> Result<AttachCommand> {
            Ok(AttachCommand {
                argv: vec!["attach".into(), backend_id.into()],
                env: Vec::new(),
            })
        }
        fn detach_session(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn list_sessions(&self) -> Result<Vec<BackendSession>> {
            let state = self.0.borrow();
            if state.fail_list {
                return Err(Error::BackendUnavailable("caído".into()));
            }
            Ok(state
                .sessions
                .iter()
                .map(|(spec, liveness)| BackendSession {
                    backend_id: spec.backend_id.clone(),
                    liveness: liveness.clone(),
                    last_output_at: Some(42),
                    cwd: Some(spec.cwd.clone()),
                })
                .collect())
        }
        fn kill_session(&self, backend_id: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .sessions
                .retain(|(s, _)| s.backend_id != backend_id);
            Ok(())
        }
        fn send_input(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        fn get_status(&self, backend_id: &str) -> Result<Liveness> {
            Ok(self
                .list_sessions()?
                .into_iter()
                .find(|s| s.backend_id == backend_id)
                .map(|s| s.liveness)
                .unwrap_or(Liveness::Missing))
        }
    }

    fn manager() -> (SessionManager, FakeBackend) {
        let backend = FakeBackend::default();
        let mut manager = SessionManager::new(
            Database::open_in_memory().expect("base de datos"),
            Box::new(backend.clone()),
            "/bin/sh".into(),
        );
        manager.set_claude_executable(Some(PathBuf::from("/usr/bin/claude")));
        (manager, backend)
    }

    fn new_session(kind: SessionKind) -> NewSession {
        NewSession {
            name: " Backend API ".into(),
            group_id: None,
            kind,
            cwd: std::env::temp_dir(),
            model: Some(" opus ".into()),
            extra_args: String::new(),
            env: vec![("FOO".into(), "bar".into())],
            project_id: None,
        }
    }

    #[test]
    fn project_sessions_inherit_env_and_startup() {
        use crate::model::NewProject;
        let (manager, backend) = manager();
        let project = manager
            .db()
            .create_project(NewProject {
                name: "Tienda".into(),
                path: std::env::temp_dir(),
                env: vec![("FOO".into(), "del proyecto".into()), ("NODE_ENV".into(), "dev".into())],
                startup: "nvm use 20".into(),
                ..NewProject::default()
            })
            .expect("proyecto");
        let mut new = new_session(SessionKind::Claude);
        new.project_id = Some(project.id);
        let session = manager.create_session(new).expect("crea");
        assert_eq!(session.project_id, Some(project.id));
        {
            let state = backend.0.borrow();
            let (spec, _) = &state.sessions[0];
            assert_eq!(spec.argv[2], "nvm use 20 && exec /usr/bin/claude --model opus");
            assert!(spec.env.contains(&("NODE_ENV".into(), "dev".into())));
            // La variable de la sesión gana a la del proyecto.
            assert!(spec.env.contains(&("FOO".into(), "bar".into())));
            assert!(!spec.env.iter().any(|(_, v)| v == "del proyecto"));
        }
        assert_eq!(manager.snapshot().expect("estado").projects.len(), 1);

        // Sin proyecto, el relanzamiento vuelve al comando normal.
        manager.set_project(&session.id, None).expect("quita");
        manager.relaunch(&session.id, Resume::Fresh).expect("relanza");
        let state = backend.0.borrow();
        assert_eq!(state.sessions[0].0.argv[2], "exec /usr/bin/claude --model opus");

        drop(state);
        manager.set_archived(&session.id, true).expect("archiva");
        assert!(manager.snapshot().expect("estado").sessions[0].session.archived);
    }

    #[test]
    fn create_launches_in_backend_and_persists() {
        let (manager, backend) = manager();
        let session = manager.create_session(new_session(SessionKind::Claude)).expect("crea");
        assert_eq!(session.name, "Backend API");
        assert_eq!(session.model.as_deref(), Some("opus"));
        assert_eq!(session.backend, "fake");

        let state = backend.0.borrow();
        let (spec, _) = &state.sessions[0];
        assert_eq!(spec.backend_id, session.backend_id);
        assert_eq!(spec.argv[2], "exec /usr/bin/claude --model opus");
        assert!(spec.env.contains(&("FOO".into(), "bar".into())));
        assert!(spec.env.contains(&(SESSION_ENV.into(), session.id.clone())));
        drop(state);

        let snapshot = manager.snapshot().expect("estado");
        assert_eq!(snapshot.sessions.len(), 1);
        assert!(snapshot.sessions[0].liveness.is_alive());
        assert_eq!(snapshot.sessions[0].last_output_at, Some(42));
    }

    #[test]
    fn invalid_input_creates_nothing() {
        let (manager, backend) = manager();
        let mut unnamed = new_session(SessionKind::Shell);
        unnamed.name = "  ".into();
        assert!(manager.create_session(unnamed).is_err());
        let mut nowhere = new_session(SessionKind::Shell);
        nowhere.cwd = PathBuf::from("/no/existe");
        assert!(manager.create_session(nowhere).is_err());
        let mut bad_args = new_session(SessionKind::Claude);
        bad_args.extra_args = "'sin cerrar".into();
        assert!(manager.create_session(bad_args).is_err());
        assert!(backend.0.borrow().sessions.is_empty());
        assert!(manager.snapshot().expect("estado").sessions.is_empty());
    }

    #[test]
    fn sessions_missing_from_backend_are_reported_not_dropped() {
        let (manager, backend) = manager();
        let session = manager.create_session(new_session(SessionKind::Shell)).expect("crea");
        // Simula un reinicio del equipo: el backend lo ha perdido todo.
        backend.0.borrow_mut().sessions.clear();
        let snapshot = manager.snapshot().expect("estado");
        assert_eq!(snapshot.sessions[0].liveness, Liveness::Missing);

        manager.relaunch(&session.id, Resume::Fresh).expect("relanza");
        assert!(manager.snapshot().expect("estado").sessions[0].liveness.is_alive());
        assert_eq!(backend.0.borrow().sessions.len(), 1, "no debe duplicar");
    }

    #[test]
    fn relaunch_with_continue_adds_flag_and_reuses_identity() {
        let (manager, backend) = manager();
        let session = manager.create_session(new_session(SessionKind::Claude)).expect("crea");
        manager.relaunch(&session.id, Resume::ContinueLatest).expect("relanza");
        let state = backend.0.borrow();
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].0.backend_id, session.backend_id);
        assert!(state.sessions[0].0.argv[2].contains("--continue"));
    }

    #[test]
    fn monitored_sessions_load_hooks_and_resume_their_own_conversation() {
        use crate::monitor::{apply_event, Phase};

        let (mut manager, backend) = manager();
        let dir = std::env::temp_dir().join(format!("alarviterm-manager-{}", std::process::id()));
        let store = StateStore::new(&dir);
        store.ensure().expect("directorio");
        manager.enable_monitor(store.clone(), PathBuf::from("/datos/hooks.json"));

        let session = manager.create_session(new_session(SessionKind::Claude)).expect("crea");
        {
            let state = backend.0.borrow();
            let spec = &state.sessions[0].0;
            assert!(spec.argv[2].ends_with("--settings /datos/hooks.json"));
            assert!(spec
                .env
                .contains(&(STATE_DIR_ENV.into(), dir.to_string_lossy().into_owned())));
        }
        assert_eq!(manager.snapshot().expect("estado").sessions[0].agent, None);

        // Lo que escribiría alarviterm-hook al recibir un evento.
        let launch_of = |backend: &FakeBackend| {
            let state = backend.0.borrow();
            let env = &state.sessions[0].0.env;
            env.iter().find(|(k, _)| k == LAUNCH_ENV).map(|(_, v)| v.clone())
        };
        let event = serde_json::json!({"hook_event_name": "Stop", "session_id": "conv-9"});
        let mut state = apply_event(None, &event, 10).expect("estado");
        state.launch = launch_of(&backend);
        assert!(state.launch.is_some());
        store.write(&session.id, &state).expect("escritura");
        let agent = manager.snapshot().expect("estado").sessions[0].agent.clone().expect("agente");
        assert_eq!(agent.phase, Phase::Waiting);

        manager.relaunch(&session.id, Resume::ContinueLatest).expect("relanza");
        assert!(backend.0.borrow().sessions[0].0.argv[2].contains("--resume conv-9"));
        // El estado del proceso anterior no se arrastra al nuevo, ni siquiera
        // si aquel escribe su evento de cierre después del relanzamiento.
        assert_eq!(manager.snapshot().expect("estado").sessions[0].agent, None);
        store.write(&session.id, &state).expect("escritura");
        assert_eq!(manager.snapshot().expect("estado").sessions[0].agent, None);
        assert_ne!(launch_of(&backend), state.launch);

        // Con el proceso parado tampoco se muestra un estado, aunque sea suyo.
        state.launch = launch_of(&backend);
        store.write(&session.id, &state).expect("escritura");
        assert!(manager.snapshot().expect("estado").sessions[0].agent.is_some());
        manager.stop(&session.id).expect("detiene");
        assert_eq!(manager.snapshot().expect("estado").sessions[0].agent, None);
        std::fs::remove_dir_all(&dir).expect("limpieza");
    }

    #[test]
    fn stop_keeps_record_and_delete_removes_it() {
        let (manager, backend) = manager();
        let session = manager.create_session(new_session(SessionKind::Shell)).expect("crea");
        manager.stop(&session.id).expect("detiene");
        assert!(backend.0.borrow().sessions.is_empty());
        assert_eq!(manager.snapshot().expect("estado").sessions.len(), 1);
        manager.delete(&session.id).expect("borra");
        assert!(manager.snapshot().expect("estado").sessions.is_empty());
    }

    #[test]
    fn backend_failure_is_surfaced_in_snapshot() {
        let (manager, backend) = manager();
        manager.create_session(new_session(SessionKind::Shell)).expect("crea");
        backend.0.borrow_mut().fail_list = true;
        let snapshot = manager.snapshot().expect("estado");
        assert!(snapshot.backend_error.is_some());
        assert_eq!(snapshot.sessions[0].liveness, Liveness::Missing);
    }

    #[test]
    fn orphans_are_adopted_once() {
        let (manager, backend) = manager();
        backend
            .create_session(&LaunchSpec {
                backend_id: "mt-huerfana".into(),
                cwd: PathBuf::from("/tmp"),
                argv: Vec::new(),
                env: Vec::new(),
            })
            .expect("crea");
        assert_eq!(manager.adopt_orphans().expect("adopta"), 1);
        assert_eq!(manager.adopt_orphans().expect("adopta"), 0);
        let snapshot = manager.snapshot().expect("estado");
        assert_eq!(snapshot.sessions[0].session.backend_id, "mt-huerfana");
        assert_eq!(snapshot.sessions[0].session.kind, SessionKind::Shell);
    }

    #[test]
    fn random_ids_are_distinct_hex() {
        let a = random_id().expect("id");
        let b = random_id().expect("id");
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
