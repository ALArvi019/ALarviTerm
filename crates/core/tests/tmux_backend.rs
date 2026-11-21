//! Pruebas de integración contra un tmux real, en un servidor aislado.
//! Si tmux no está instalado se omiten con un aviso.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use alarviterm_core::sessions::tmux::{find_tmux, TmuxOptions};
use alarviterm_core::sessions::{LaunchSpec, Liveness, SessionBackend, TmuxBackend};

struct Fixture {
    backend: TmuxBackend,
    dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let Some(tmux) = find_tmux() else {
            eprintln!("tmux no está instalado, prueba omitida");
            return None;
        };
        let dir = std::env::temp_dir().join(format!("alarviterm-it-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("directorio temporal");
        let backend = TmuxBackend::new(
            tmux,
            format!("alarviterm-it-{tag}-{}", std::process::id()),
            dir.join("tmux.conf"),
        );
        backend.write_config(&TmuxOptions::default()).expect("configuración");
        Some(Self { backend, dir })
    }

    fn spec(&self, id: &str, argv: &[&str]) -> LaunchSpec {
        LaunchSpec {
            backend_id: id.to_string(),
            cwd: self.dir.clone(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: vec![("ALARVITERM_IT".into(), "valor con espacios".into())],
        }
    }

    fn wait_for(&self, what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "tiempo agotado esperando: {what}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.backend.kill_server();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn lifecycle_create_list_kill() {
    let Some(fx) = Fixture::new("life") else { return };
    assert!(fx.backend.check().expect("versión").starts_with("tmux"));
    assert!(fx.backend.list_sessions().expect("sin servidor").is_empty());

    fx.backend
        .create_session(&fx.spec("mt-a", &["sleep", "60"]))
        .expect("crea a");
    fx.backend
        .create_session(&fx.spec("mt-b", &["sleep", "60"]))
        .expect("crea b");
    assert!(
        fx.backend.create_session(&fx.spec("mt-a", &["sleep", "60"])).is_err(),
        "no debe duplicar una sesión existente"
    );

    let sessions = fx.backend.list_sessions().expect("lista");
    assert_eq!(sessions.len(), 2);
    let a = sessions.iter().find(|s| s.backend_id == "mt-a").expect("a");
    match &a.liveness {
        Liveness::Alive { pid, command } => {
            assert!(pid.is_some());
            assert_eq!(command.as_deref(), Some("sleep"));
        }
        other => panic!("se esperaba viva, es {other:?}"),
    }
    assert_eq!(
        a.cwd.as_ref().map(|p| p.canonicalize().expect("cwd")),
        Some(fx.dir.canonicalize().expect("dir"))
    );
    assert!(a.last_output_at.is_some());

    fx.backend.kill_session("mt-a").expect("mata");
    fx.backend.kill_session("mt-a").expect("matar dos veces no falla");
    assert_eq!(fx.backend.get_status("mt-a").expect("estado"), Liveness::Missing);
    assert!(fx.backend.get_status("mt-b").expect("estado").is_alive());
}

#[test]
fn exited_process_keeps_exit_code() {
    let Some(fx) = Fixture::new("exit") else { return };
    fx.backend
        .create_session(&fx.spec("mt-x", &["sh", "-c", "exit 3"]))
        .expect("crea");
    fx.wait_for("el proceso termina", || {
        fx.backend.get_status("mt-x").expect("estado") == Liveness::Exited { code: Some(3) }
    });
}

#[test]
fn input_and_environment_reach_the_process() {
    let Some(fx) = Fixture::new("io") else { return };
    let out = fx.dir.join("out.txt");
    let script = format!(
        "read line; printf '%s|%s' \"$line\" \"$ALARVITERM_IT\" > '{}'",
        out.display()
    );
    fx.backend
        .create_session(&fx.spec("mt-io", &["sh", "-c", &script]))
        .expect("crea");
    fx.backend.send_input("mt-io", "hola; $HOME 'x'\n").expect("envía");
    fx.wait_for("el proceso escribe el fichero", || out.exists());
    fx.wait_for("contenido completo", || {
        std::fs::read_to_string(&out).unwrap_or_default() == "hola; $HOME 'x'|valor con espacios"
    });
}

#[test]
fn default_shell_session_and_resize() {
    let Some(fx) = Fixture::new("shell") else { return };
    fx.backend.create_session(&fx.spec("mt-sh", &[])).expect("crea");
    assert!(fx.backend.get_status("mt-sh").expect("estado").is_alive());
    fx.backend.resize("mt-sh", 100, 30).expect("redimensiona");
    let attach = fx.backend.attach_command("mt-sh").expect("comando");
    assert!(attach.argv.iter().any(|a| a == "attach-session"));
    assert_eq!(attach.argv.last().map(String::as_str), Some("=mt-sh"));
}

#[test]
fn rejects_ids_that_tmux_would_misparse() {
    let Some(fx) = Fixture::new("ids") else { return };
    assert!(fx.backend.create_session(&fx.spec("a:b", &["sleep", "1"])).is_err());
    assert!(fx.backend.kill_session("a.b").is_err());
}

#[test]
fn process_exit_and_session_close_touch_the_notify_file() {
    let Some(fx) = Fixture::new("notify") else { return };
    let mark = fx.dir.join("con espacio").join(".backend");
    std::fs::create_dir_all(mark.parent().expect("padre")).expect("directorio");
    fx.backend
        .write_config(&TmuxOptions {
            notify_file: Some(mark.clone()),
            ..TmuxOptions::default()
        })
        .expect("configuración");

    fx.backend
        .create_session(&fx.spec("mt-corta", &["sh", "-c", "exit 3"]))
        .expect("crea");
    fx.wait_for("aviso de proceso terminado", || mark.exists());

    // Con la última sesión se va el servidor y ya no hay quien avise, así que
    // el cierre se comprueba con otra sesión todavía en marcha.
    fx.backend
        .create_session(&fx.spec("mt-larga", &["sleep", "60"]))
        .expect("crea");
    std::fs::remove_file(&mark).expect("borra el aviso");
    fx.backend.kill_session("mt-corta").expect("cierra");
    fx.wait_for("aviso de sesión cerrada", || mark.exists());
}
