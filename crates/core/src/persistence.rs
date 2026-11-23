//! Persistencia en SQLite: grupos, sesiones y ajustes.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::model::{Group, NewProject, Project, Session, SessionKind};
use crate::{Error, Result};

/// Migraciones en orden. El índice más uno es la versión del esquema, que se
/// guarda en `PRAGMA user_version`. Nunca se edita una migración publicada, se
/// añade otra al final.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE groups (
        id         INTEGER PRIMARY KEY,
        name       TEXT    NOT NULL,
        position   INTEGER NOT NULL DEFAULT 0,
        collapsed  INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        id             TEXT    PRIMARY KEY,
        name           TEXT    NOT NULL,
        group_id       INTEGER REFERENCES groups(id) ON DELETE SET NULL,
        kind           TEXT    NOT NULL,
        cwd            TEXT    NOT NULL,
        model          TEXT,
        extra_args     TEXT    NOT NULL DEFAULT '',
        env            TEXT    NOT NULL DEFAULT '[]',
        backend        TEXT    NOT NULL,
        backend_id     TEXT    NOT NULL UNIQUE,
        position       INTEGER NOT NULL DEFAULT 0,
        created_at     INTEGER NOT NULL,
        last_opened_at INTEGER
    );
    CREATE INDEX sessions_group ON sessions(group_id);
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
", "
    CREATE TABLE layouts (
        name       TEXT    PRIMARY KEY,
        data       TEXT    NOT NULL,
        created_at INTEGER NOT NULL
    );
", "
    CREATE TABLE projects (
        id         INTEGER PRIMARY KEY,
        name       TEXT    NOT NULL,
        path       TEXT    NOT NULL,
        color      TEXT    NOT NULL DEFAULT '',
        icon       TEXT    NOT NULL DEFAULT '',
        env        TEXT    NOT NULL DEFAULT '[]',
        startup    TEXT    NOT NULL DEFAULT '',
        position   INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    ALTER TABLE sessions ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL;
    ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN profile TEXT;
", "
    ALTER TABLE projects ADD COLUMN account TEXT NOT NULL DEFAULT '';
", "
    ALTER TABLE projects ADD COLUMN image TEXT NOT NULL DEFAULT '';
"];

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let target = MIGRATIONS.len() as i64;
        if current > target {
            return Err(Error::Invalid(format!(
                "la base de datos es de una versión más reciente (esquema {current}, soportado {target})"
            )));
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
            tx.commit()?;
            log::info!("migración {} aplicada", index + 1);
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    // Grupos

    pub fn groups(&self) -> Result<Vec<Group>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, position, collapsed FROM groups ORDER BY position, id")?;
        let rows = stmt.query_map([], |row| {
            Ok(Group {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
                collapsed: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_group(&self, name: &str) -> Result<Group> {
        let name = clean_name(name)?;
        let position: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM groups", [], |r| {
                    r.get(0)
                })?;
        self.conn.execute(
            "INSERT INTO groups (name, position, created_at) VALUES (?1, ?2, ?3)",
            params![name, position, crate::now_unix()],
        )?;
        Ok(Group {
            id: self.conn.last_insert_rowid(),
            name,
            position,
            collapsed: false,
        })
    }

    pub fn rename_group(&self, id: i64, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        self.expect_one(
            self.conn
                .execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])?,
            || format!("grupo {id}"),
        )
    }

    pub fn set_group_collapsed(&self, id: i64, collapsed: bool) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE groups SET collapsed = ?1 WHERE id = ?2",
                params![collapsed, id],
            )?,
            || format!("grupo {id}"),
        )
    }

    /// Borra el grupo. Sus sesiones no se tocan: quedan sin grupo.
    pub fn delete_group(&self, id: i64) -> Result<()> {
        self.expect_one(
            self.conn
                .execute("DELETE FROM groups WHERE id = ?1", params![id])?,
            || format!("grupo {id}"),
        )
    }

    // Sesiones

    pub fn sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions ORDER BY position, created_at, id"
        ))?;
        let rows = stmt.query_map([], session_from_row)?;
        rows.map(|row| row.map_err(Error::from).and_then(|raw| raw.into_session()))
            .collect()
    }

    pub fn session(&self, id: &str) -> Result<Session> {
        self.conn
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1"),
                params![id],
                session_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("sesión {id}")))?
            .into_session()
    }

    pub fn insert_session(&self, session: &Session) -> Result<()> {
        let env = serde_json::to_string(&session.env)
            .map_err(|e| Error::Invalid(format!("variables de entorno no serializables: {e}")))?;
        self.conn.execute(
            "INSERT INTO sessions (id, name, group_id, kind, cwd, model, extra_args, env,
                                   backend, backend_id, position, created_at, last_opened_at,
                                   project_id, archived, profile)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                session.id,
                session.name,
                session.group_id,
                session.kind.as_str(),
                session.cwd.to_string_lossy(),
                session.model,
                session.extra_args,
                env,
                session.backend,
                session.backend_id,
                session.position,
                session.created_at,
                session.last_opened_at,
                session.project_id,
                session.archived,
                session.profile,
            ],
        )?;
        Ok(())
    }

    pub fn next_session_position(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM sessions",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn rename_session(&self, id: &str, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        self.expect_one(
            self.conn
                .execute("UPDATE sessions SET name = ?1 WHERE id = ?2", params![name, id])?,
            || format!("sesión {id}"),
        )
    }

    pub fn move_session(&self, id: &str, group_id: Option<i64>) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE sessions SET group_id = ?1 WHERE id = ?2",
                params![group_id, id],
            )?,
            || format!("sesión {id}"),
        )
    }

    pub fn touch_session(&self, id: &str, when: i64) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE sessions SET last_opened_at = ?1 WHERE id = ?2",
                params![when, id],
            )?,
            || format!("sesión {id}"),
        )
    }

    pub fn set_session_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE sessions SET archived = ?1 WHERE id = ?2",
                params![archived, id],
            )?,
            || format!("sesión {id}"),
        )
    }

    pub fn set_session_profile(&self, id: &str, profile: Option<&str>) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE sessions SET profile = ?1 WHERE id = ?2",
                params![profile, id],
            )?,
            || format!("sesión {id}"),
        )
    }

    /// Un perfil cambió de nombre: sus sesiones le siguen.
    pub fn rename_profile(&self, old: &str, new: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET profile = ?1 WHERE profile = ?2",
            params![new, old],
        )?;
        Ok(())
    }

    pub fn set_session_project(&self, id: &str, project_id: Option<i64>) -> Result<()> {
        self.expect_one(
            self.conn.execute(
                "UPDATE sessions SET project_id = ?1 WHERE id = ?2",
                params![project_id, id],
            )?,
            || format!("sesión {id}"),
        )
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.expect_one(
            self.conn
                .execute("DELETE FROM sessions WHERE id = ?1", params![id])?,
            || format!("sesión {id}"),
        )
    }

    /// Coloca la sesión en un grupo, justo antes de `before` o al final si
    /// no se indica. Renumera las posiciones para que el orden sea estable.
    pub fn place_session(&mut self, id: &str, group_id: Option<i64>, before: Option<&str>) -> Result<()> {
        let mut order: Vec<String> = self.sessions()?.into_iter().map(|s| s.id).collect();
        let Some(current) = order.iter().position(|s| s == id) else {
            return Err(Error::NotFound(format!("sesión {id}")));
        };
        order.remove(current);
        let at = before
            .and_then(|b| order.iter().position(|s| s == b))
            .unwrap_or(order.len());
        order.insert(at, id.to_string());

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET group_id = ?1 WHERE id = ?2",
            params![group_id, id],
        )?;
        for (index, session) in order.iter().enumerate() {
            tx.execute(
                "UPDATE sessions SET position = ?1 WHERE id = ?2",
                params![index as i64 + 1, session],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sube (`delta` negativo) o baja un grupo en la lista. En los extremos
    /// no hace nada.
    pub fn move_group(&mut self, id: i64, delta: i64) -> Result<()> {
        let mut order: Vec<i64> = self.groups()?.into_iter().map(|g| g.id).collect();
        let Some(current) = order.iter().position(|g| *g == id) else {
            return Err(Error::NotFound(format!("grupo {id}")));
        };
        let target = (current as i64 + delta).clamp(0, order.len() as i64 - 1) as usize;
        let moved = order.remove(current);
        order.insert(target, moved);
        let tx = self.conn.transaction()?;
        for (index, group) in order.iter().enumerate() {
            tx.execute(
                "UPDATE groups SET position = ?1 WHERE id = ?2",
                params![index as i64 + 1, group],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // Proyectos

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, path, color, icon, env, startup, position, account, image
             FROM projects ORDER BY position, id",
        )?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(2)?;
            let env: String = row.get(5)?;
            Ok((
                Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: PathBuf::from(path),
                    color: row.get(3)?,
                    icon: row.get(4)?,
                    env: Vec::new(),
                    startup: row.get(6)?,
                    position: row.get(7)?,
                    account: row.get(8)?,
                    image: row.get(9)?,
                },
                env,
            ))
        })?;
        rows.map(|row| {
            let (mut project, env) = row?;
            project.env = serde_json::from_str(&env).map_err(|e| {
                Error::Invalid(format!(
                    "variables de entorno corruptas en el proyecto {}: {e}",
                    project.name
                ))
            })?;
            Ok(project)
        })
        .collect()
    }

    pub fn create_project(&self, new: NewProject) -> Result<Project> {
        let new = new.validated()?;
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM projects",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO projects (name, path, color, icon, env, startup, position, created_at, account, image)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new.name,
                new.path.to_string_lossy(),
                new.color,
                new.icon,
                env_json(&new.env)?,
                new.startup,
                position,
                crate::now_unix(),
                new.account,
                new.image,
            ],
        )?;
        Ok(Project {
            id: self.conn.last_insert_rowid(),
            name: new.name,
            path: new.path,
            color: new.color,
            icon: new.icon,
            env: new.env,
            startup: new.startup,
            account: new.account,
            image: new.image,
            position,
        })
    }

    pub fn update_project(&self, id: i64, new: NewProject) -> Result<()> {
        let new = new.validated()?;
        self.expect_one(
            self.conn.execute(
                "UPDATE projects SET name = ?1, path = ?2, color = ?3, icon = ?4, env = ?5, startup = ?6,
                 account = ?8, image = ?9
                 WHERE id = ?7",
                params![
                    new.name,
                    new.path.to_string_lossy(),
                    new.color,
                    new.icon,
                    env_json(&new.env)?,
                    new.startup,
                    id,
                    new.account,
                    new.image,
                ],
            )?,
            || format!("proyecto {id}"),
        )
    }

    /// Borra el proyecto. Sus sesiones siguen existiendo, ya sin proyecto.
    pub fn delete_project(&self, id: i64) -> Result<()> {
        self.expect_one(
            self.conn
                .execute("DELETE FROM projects WHERE id = ?1", params![id])?,
            || format!("proyecto {id}"),
        )
    }

    // Disposiciones guardadas

    /// Nombres y contenido, por orden alfabético.
    pub fn layouts(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, data FROM layouts ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Guarda la disposición. Si el nombre existe, la sustituye.
    pub fn save_layout(&self, name: &str, data: &str) -> Result<()> {
        let name = clean_name(name)?;
        self.conn.execute(
            "INSERT INTO layouts (name, data, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET data = excluded.data",
            params![name, data, crate::now_unix()],
        )?;
        Ok(())
    }

    pub fn delete_layout(&self, name: &str) -> Result<()> {
        self.expect_one(
            self.conn
                .execute("DELETE FROM layouts WHERE name = ?1", params![name])?,
            || format!("disposición {name}"),
        )
    }

    // Ajustes

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn expect_one(&self, changed: usize, what: impl FnOnce() -> String) -> Result<()> {
        if changed == 0 {
            Err(Error::NotFound(what()))
        } else {
            Ok(())
        }
    }
}

const SESSION_COLUMNS: &str = "id, name, group_id, kind, cwd, model, extra_args, env, backend, \
                               backend_id, position, created_at, last_opened_at, project_id, \
                               archived, profile";

fn env_json(env: &[(String, String)]) -> Result<String> {
    serde_json::to_string(env)
        .map_err(|e| Error::Invalid(format!("variables de entorno no serializables: {e}")))
}

/// Fila tal cual sale de SQLite, antes de validar `kind` y `env`.
struct RawSession {
    session: Session,
    kind: String,
    env: String,
}

impl RawSession {
    fn into_session(mut self) -> Result<Session> {
        self.session.kind = SessionKind::parse(&self.kind)?;
        self.session.env = serde_json::from_str(&self.env).map_err(|e| {
            Error::Invalid(format!(
                "variables de entorno corruptas en la sesión {}: {e}",
                self.session.id
            ))
        })?;
        Ok(self.session)
    }
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<RawSession> {
    let cwd: String = row.get(4)?;
    Ok(RawSession {
        kind: row.get(3)?,
        env: row.get(7)?,
        session: Session {
            id: row.get(0)?,
            name: row.get(1)?,
            group_id: row.get(2)?,
            kind: SessionKind::Shell,
            cwd: PathBuf::from(cwd),
            model: row.get(5)?,
            extra_args: row.get(6)?,
            env: Vec::new(),
            backend: row.get(8)?,
            backend_id: row.get(9)?,
            position: row.get(10)?,
            created_at: row.get(11)?,
            last_opened_at: row.get(12)?,
            project_id: row.get(13)?,
            archived: row.get(14)?,
            profile: row.get(15)?,
        },
    })
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("el nombre no puede estar vacío".into()));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, group_id: Option<i64>) -> Session {
        Session {
            id: id.to_string(),
            name: format!("sesión {id}"),
            group_id,
            kind: SessionKind::Claude,
            cwd: PathBuf::from("/tmp"),
            model: Some("opus".into()),
            extra_args: "--verbose".into(),
            env: vec![("A".into(), "1".into())],
            backend: "tmux".into(),
            backend_id: format!("mt-{id}"),
            position: 1,
            created_at: 100,
            last_opened_at: None,
            project_id: None,
            archived: false,
            profile: None,
        }
    }

    #[test]
    fn migrations_reach_latest_version_and_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("alarviterm-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("directorio temporal");
        let path = dir.join("t.db");
        {
            let db = Database::open(&path).expect("abre");
            assert_eq!(db.schema_version().expect("versión"), MIGRATIONS.len() as i64);
            db.create_group("uno").expect("grupo");
        }
        let db = Database::open(&path).expect("reabre");
        assert_eq!(db.groups().expect("grupos").len(), 1);
        std::fs::remove_dir_all(&dir).expect("limpieza");
    }

    #[test]
    fn old_databases_gain_projects_without_losing_sessions() {
        // Una base de datos con el esquema 2 y una sesión ya guardada.
        let conn = Connection::open_in_memory().expect("abre");
        conn.execute_batch(MIGRATIONS[0]).expect("esquema 1");
        conn.execute_batch(MIGRATIONS[1]).expect("esquema 2");
        conn.execute_batch(
            "INSERT INTO sessions (id, name, kind, cwd, backend, backend_id, created_at)
             VALUES ('vieja', 'Vieja', 'shell', '/tmp', 'tmux', 'mt-vieja', 1);
             PRAGMA user_version = 2;",
        )
        .expect("datos");
        let db = Database::init(conn).expect("migra");
        assert_eq!(db.schema_version().expect("versión"), MIGRATIONS.len() as i64);
        let old = db.session("vieja").expect("sigue ahí");
        assert_eq!((old.project_id, old.archived, old.profile), (None, false, None));
    }

    #[test]
    fn project_crud_and_session_links() {
        let db = Database::open_in_memory().expect("abre");
        let new = NewProject {
            name: "  Tienda ".into(),
            path: PathBuf::from("/tmp"),
            color: "blue".into(),
            icon: "🛒 de más".into(),
            env: vec![("NODE_ENV".into(), "dev".into())],
            startup: " nvm use 20 ".into(),
            account: "  Mi   empresa ".into(),
            image: String::new(),
        };
        let project = db.create_project(new.clone()).expect("crea");
        assert_eq!(project.name, "Tienda");
        assert_eq!(project.icon.chars().count(), 2);
        assert_eq!(project.startup, "nvm use 20");
        assert_eq!(db.projects().expect("lista"), vec![project.clone()]);

        assert_eq!(db.projects().expect("lista")[0].account, "Mi empresa");
        assert!(db.create_project(NewProject { name: " ".into(), ..new.clone() }).is_err());
        assert!(db.create_project(NewProject { image: "/no/existe.png".into(), ..new.clone() }).is_err());
        let own = std::env::current_exe().expect("ejecutable").to_string_lossy().into_owned();
        let pictured = db.create_project(NewProject { image: format!(" {own} "), ..new.clone() }).expect("crea");
        assert_eq!(pictured.image, own);
        db.delete_project(pictured.id).expect("borra");
        assert!(db
            .create_project(NewProject { path: "/no/existe".into(), ..new.clone() })
            .is_err());
        let odd = db
            .create_project(NewProject { color: "fucsia".into(), ..new.clone() })
            .expect("crea");
        assert_eq!(odd.color, "", "un color desconocido se descarta");

        db.update_project(project.id, NewProject { name: "Tienda web".into(), color: "red".into(), ..new })
            .expect("actualiza");
        let updated = db.projects().expect("lista").remove(0);
        assert_eq!((updated.name.as_str(), updated.color.as_str()), ("Tienda web", "red"));

        let mut session = sample("a", None);
        session.project_id = Some(project.id);
        session.profile = Some("Grande".into());
        db.insert_session(&session).expect("inserta");
        assert_eq!(db.session("a").expect("lee"), session);
        db.set_session_archived("a", true).expect("archiva");
        db.rename_profile("Grande", "Enorme").expect("renombra");
        assert_eq!(db.session("a").expect("lee").profile.as_deref(), Some("Enorme"));
        db.set_session_profile("a", None).expect("perfil");
        let stored = db.session("a").expect("lee");
        assert!(stored.archived && stored.profile.is_none());

        // Borrar el proyecto no borra la sesión.
        db.delete_project(project.id).expect("borra");
        assert_eq!(db.session("a").expect("lee").project_id, None);
        db.set_session_project("a", Some(odd.id)).expect("reasigna");
        assert_eq!(db.session("a").expect("lee").project_id, Some(odd.id));
        assert!(matches!(db.delete_project(999), Err(Error::NotFound(_))));
    }

    #[test]
    fn group_crud() {
        let db = Database::open_in_memory().expect("abre");
        let a = db.create_group("  ACME ").expect("crea");
        let b = db.create_group("Personal").expect("crea");
        assert_eq!(a.name, "ACME");
        assert!(b.position > a.position);
        db.rename_group(a.id, "Acme").expect("renombra");
        db.set_group_collapsed(b.id, true).expect("contrae");
        let groups = db.groups().expect("lista");
        assert_eq!(groups[0].name, "Acme");
        assert!(groups[1].collapsed);
        assert!(db.create_group("   ").is_err());
        assert!(matches!(db.rename_group(999, "x"), Err(Error::NotFound(_))));
    }

    #[test]
    fn session_crud_and_group_deletion_keeps_sessions() {
        let db = Database::open_in_memory().expect("abre");
        let group = db.create_group("g").expect("grupo");
        let session = sample("abc", Some(group.id));
        db.insert_session(&session).expect("inserta");
        assert_eq!(db.session("abc").expect("lee"), session);

        db.rename_session("abc", "nuevo").expect("renombra");
        db.touch_session("abc", 555).expect("marca");
        let read = db.session("abc").expect("lee");
        assert_eq!(read.name, "nuevo");
        assert_eq!(read.last_opened_at, Some(555));

        db.delete_group(group.id).expect("borra grupo");
        assert_eq!(db.session("abc").expect("lee").group_id, None);

        db.delete_session("abc").expect("borra");
        assert!(matches!(db.session("abc"), Err(Error::NotFound(_))));
        assert!(db.sessions().expect("lista").is_empty());
    }

    #[test]
    fn backend_id_is_unique() {
        let db = Database::open_in_memory().expect("abre");
        db.insert_session(&sample("a", None)).expect("inserta");
        let mut dup = sample("b", None);
        dup.backend_id = "mt-a".into();
        assert!(db.insert_session(&dup).is_err());
    }

    #[test]
    fn place_session_reorders_and_regroups() {
        let mut db = Database::open_in_memory().expect("abre");
        let group = db.create_group("g").expect("grupo");
        for id in ["a", "b", "c"] {
            let mut session = sample(id, None);
            session.position = db.next_session_position().expect("posición");
            db.insert_session(&session).expect("inserta");
        }
        let order = |db: &Database| -> Vec<String> {
            db.sessions().expect("lista").into_iter().map(|s| s.id).collect()
        };
        db.place_session("c", None, Some("a")).expect("coloca");
        assert_eq!(order(&db), vec!["c", "a", "b"]);
        db.place_session("c", Some(group.id), None).expect("coloca");
        assert_eq!(order(&db), vec!["a", "b", "c"]);
        assert_eq!(db.session("c").expect("lee").group_id, Some(group.id));
        // Un destino desconocido equivale a «al final».
        db.place_session("a", None, Some("no existe")).expect("coloca");
        assert_eq!(order(&db), vec!["b", "c", "a"]);
        assert!(matches!(db.place_session("zz", None, None), Err(Error::NotFound(_))));
    }

    #[test]
    fn move_group_swaps_and_stops_at_the_ends() {
        let mut db = Database::open_in_memory().expect("abre");
        let ids: Vec<i64> = ["a", "b", "c"]
            .iter()
            .map(|n| db.create_group(n).expect("grupo").id)
            .collect();
        let order = |db: &Database| -> Vec<i64> {
            db.groups().expect("lista").into_iter().map(|g| g.id).collect()
        };
        db.move_group(ids[2], -1).expect("sube");
        assert_eq!(order(&db), vec![ids[0], ids[2], ids[1]]);
        db.move_group(ids[0], -1).expect("ya es el primero");
        assert_eq!(order(&db), vec![ids[0], ids[2], ids[1]]);
        db.move_group(ids[0], 1).expect("baja");
        assert_eq!(order(&db), vec![ids[2], ids[0], ids[1]]);
    }

    #[test]
    fn layouts_are_saved_replaced_and_deleted() {
        let db = Database::open_in_memory().expect("abre");
        db.save_layout("Trabajo", "{1}").expect("guarda");
        db.save_layout("casa", "{2}").expect("guarda");
        db.save_layout("Trabajo", "{3}").expect("sustituye");
        assert_eq!(
            db.layouts().expect("lista"),
            vec![("casa".to_string(), "{2}".to_string()), ("Trabajo".to_string(), "{3}".to_string())]
        );
        db.delete_layout("casa").expect("borra");
        assert!(matches!(db.delete_layout("casa"), Err(Error::NotFound(_))));
        assert!(db.save_layout("  ", "{}").is_err());
    }

    #[test]
    fn settings_upsert() {
        let db = Database::open_in_memory().expect("abre");
        assert_eq!(db.setting("k").expect("lee"), None);
        db.set_setting("k", "1").expect("guarda");
        db.set_setting("k", "2").expect("guarda");
        assert_eq!(db.setting("k").expect("lee").as_deref(), Some("2"));
    }
}
