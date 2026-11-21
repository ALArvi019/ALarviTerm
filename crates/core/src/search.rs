//! Búsqueda aproximada para la paleta de comandos y filtro de sesiones para
//! la barra lateral.

use crate::sessions::SessionView;

/// Minúsculas y sin tildes, para comparar sin que importe cómo se escriba.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn is_boundary(previous: Option<char>) -> bool {
    previous.is_none_or(|c| !c.is_alphanumeric())
}

/// Puntúa una palabra de la consulta contra un texto ya plegado.
fn score_word(word: &[char], text: &[char]) -> Option<i64> {
    if word.is_empty() {
        return Some(0);
    }
    // Lo mejor es que aparezca tal cual, y más aún al principio de una palabra.
    let literal = (0..=text.len().saturating_sub(word.len()))
        .filter(|&start| text.len() >= word.len() && text[start..start + word.len()] == *word)
        .map(|start| {
            let boundary = is_boundary(start.checked_sub(1).map(|i| text[i]));
            100 + if boundary { 40 } else { 0 } + if start == 0 { 20 } else { 0 } - (start as i64).min(15)
        })
        .max();
    if literal.is_some() {
        return literal;
    }
    // Si no, que sus letras aparezcan en orden. Puntúan las seguidas y las
    // que caen en inicio de palabra.
    let mut score = 0i64;
    let mut position = 0usize;
    let mut last: Option<usize> = None;
    for &wanted in word {
        // La primera letra tiene que abrir una palabra. Sin eso casi todo
        // encaja con casi todo.
        let found = (position..text.len()).find(|&i| {
            text[i] == wanted && (last.is_some() || is_boundary(i.checked_sub(1).map(|p| text[p])))
        })?;
        score += 4;
        if last.is_some_and(|l| l + 1 == found) {
            score += 6;
        }
        if is_boundary(found.checked_sub(1).map(|i| text[i])) {
            score += 8;
        }
        last = Some(found);
        position = found + 1;
    }
    Some(score.min(90))
}

/// Puntuación de un texto frente a una consulta. `None` si no encaja. Cada
/// palabra de la consulta tiene que encajar por su cuenta.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = fold(text).chars().collect();
    let mut total = 0;
    for word in fold(query).split_whitespace() {
        let word: Vec<char> = word.chars().collect();
        total += score_word(&word, &text)?;
    }
    Some(total)
}

/// Ordena candidatos por puntuación. A igualdad conserva el orden de entrada.
pub fn rank<T>(query: &str, items: Vec<T>, text: impl Fn(&T) -> String) -> Vec<T> {
    if query.trim().is_empty() {
        return items;
    }
    let mut scored: Vec<(i64, usize, T)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| fuzzy_score(query, &text(&item)).map(|score| (score, index, item)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Qué sesiones enseña la barra lateral según su estado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    /// Con el proceso vivo.
    Alive,
    /// Claude Code espera al usuario: pide permiso o terminó su turno.
    Attention,
    /// Con el proceso terminado o perdido.
    Stopped,
    /// Las archivadas, que ningún otro filtro enseña.
    Archived,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 5] = [
        StatusFilter::All,
        StatusFilter::Alive,
        StatusFilter::Attention,
        StatusFilter::Stopped,
        StatusFilter::Archived,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "Todas",
            StatusFilter::Alive => "Activas",
            StatusFilter::Attention => "Te esperan",
            StatusFilter::Stopped => "Paradas",
            StatusFilter::Archived => "Archivadas",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Alive => "alive",
            StatusFilter::Attention => "attention",
            StatusFilter::Stopped => "stopped",
            StatusFilter::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL.into_iter().find(|f| f.as_str() == value).unwrap_or_default()
    }

    pub fn accepts(self, view: &SessionView) -> bool {
        if view.session.archived != (self == StatusFilter::Archived) {
            return false;
        }
        match self {
            StatusFilter::All | StatusFilter::Archived => true,
            StatusFilter::Alive => view.liveness.is_alive(),
            StatusFilter::Attention => view.agent.as_ref().is_some_and(|a| a.phase.needs_user()),
            StatusFilter::Stopped => !view.liveness.is_alive(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionFilter {
    pub query: String,
    pub status: StatusFilter,
}

impl SessionFilter {
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || self.status != StatusFilter::All
    }

    /// El texto se busca en el nombre, la carpeta y los nombres del grupo y
    /// del proyecto. Aquí cada palabra tiene que aparecer literal: en una
    /// lista que se filtra mientras se escribe, la coincidencia aproximada
    /// enseña demasiado. Las archivadas solo salen con su propio filtro.
    pub fn accepts(&self, view: &SessionView, group_name: Option<&str>, project_name: Option<&str>) -> bool {
        if !self.status.accepts(view) {
            return false;
        }
        let haystack = fold(&format!(
            "{} {} {} {}",
            view.session.name,
            view.session.cwd.display(),
            group_name.unwrap_or(""),
            project_name.unwrap_or("")
        ));
        fold(&self.query).split_whitespace().all(|word| haystack.contains(word))
    }
}

/// Orden de las sesiones dentro de cada grupo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// El que el usuario fija arrastrando.
    #[default]
    Manual,
    /// Primero las que han tenido actividad más reciente.
    Activity,
    Name,
    /// Primero las más nuevas.
    Created,
}

impl SortOrder {
    pub const ALL: [SortOrder; 4] = [
        SortOrder::Manual,
        SortOrder::Activity,
        SortOrder::Name,
        SortOrder::Created,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Manual => "Orden manual",
            SortOrder::Activity => "Actividad reciente",
            SortOrder::Name => "Nombre",
            SortOrder::Created => "Fecha de creación",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Manual => "manual",
            SortOrder::Activity => "activity",
            SortOrder::Name => "name",
            SortOrder::Created => "created",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL.into_iter().find(|o| o.as_str() == value).unwrap_or_default()
    }
}

/// Último momento en que se supo algo de la sesión.
fn last_activity(view: &SessionView) -> i64 {
    let agent = view.agent.as_ref().map_or(0, |a| a.updated_at);
    agent
        .max(view.last_output_at.unwrap_or(0))
        .max(view.session.last_opened_at.unwrap_or(0))
        .max(view.session.created_at)
}

/// Ordena las sesiones. Es estable, así que a igualdad queda el orden manual.
pub fn sort_sessions(views: &mut [SessionView], order: SortOrder) {
    match order {
        SortOrder::Manual => {}
        SortOrder::Activity => views.sort_by_key(|v| std::cmp::Reverse(last_activity(v))),
        SortOrder::Name => views.sort_by_key(|v| fold(&v.session.name)),
        SortOrder::Created => views.sort_by_key(|v| std::cmp::Reverse(v.session.created_at)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Session, SessionKind};
    use crate::monitor::{AgentState, Phase};
    use crate::sessions::Liveness;

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("Sesión Ñandú"), "sesion nandu");
    }

    #[test]
    fn literal_matches_beat_scattered_letters() {
        let literal = fuzzy_score("panel", "Dividir el panel a la derecha").expect("encaja");
        let scattered = fuzzy_score("panel", "Pestaña anterior del lateral").expect("encaja");
        assert!(literal > scattered, "{literal} frente a {scattered}");
        assert_eq!(fuzzy_score("xyz", "Nueva sesión"), None);
        assert_eq!(fuzzy_score("", "Nueva sesión"), Some(0));
    }

    #[test]
    fn word_starts_and_accents_count() {
        let start = fuzzy_score("ses", "Nueva sesión").expect("encaja");
        let middle = fuzzy_score("ses", "Posesión").expect("encaja");
        assert!(start > middle);
        assert!(fuzzy_score("sesion", "Nueva sesión").is_some());
        assert!(fuzzy_score("nse", "Nueva sesión").is_some());
        // Cada palabra debe encajar.
        assert!(fuzzy_score("nueva grupo", "Nueva sesión").is_none());
        assert!(fuzzy_score("sesion nueva", "Nueva sesión").is_some());
    }

    #[test]
    fn ranking_orders_by_score_and_keeps_ties_stable() {
        let items = vec!["Posesión", "Nueva sesión", "Sesión siguiente", "Nuevo grupo"];
        let ranked = rank("ses", items.clone(), |s| s.to_string());
        assert_eq!(ranked, vec!["Sesión siguiente", "Nueva sesión", "Posesión"]);
        assert_eq!(rank("  ", items.clone(), |s| s.to_string()), items);
    }

    fn alive() -> Liveness {
        Liveness::Alive {
            pid: None,
            command: None,
        }
    }

    fn view(name: &str, cwd: &str, liveness: Liveness, phase: Option<Phase>) -> SessionView {
        SessionView {
            session: Session {
                id: name.to_string(),
                name: name.to_string(),
                group_id: None,
                kind: SessionKind::Claude,
                cwd: cwd.into(),
                model: None,
                extra_args: String::new(),
                env: Vec::new(),
                backend: "tmux".to_string(),
                backend_id: name.to_string(),
                position: 0,
                created_at: 0,
                last_opened_at: None,
                project_id: None,
                archived: false,
                profile: None,
            },
            liveness,
            last_output_at: None,
            agent: phase.map(|phase| AgentState {
                phase,
                tool: None,
                message: None,
                since: 0,
                updated_at: 0,
                conversation: None,
                transcript: None,
                permission_mode: None,
                model: None,
                launch: None,
            }),
        }
    }

    #[test]
    fn sessions_sort_by_activity_name_and_creation() {
        let mut a = view("beta", "/a", alive(), None);
        a.session.created_at = 10;
        a.last_output_at = Some(500);
        let mut b = view("Álamo", "/b", alive(), Some(Phase::Working));
        b.session.created_at = 30;
        if let Some(agent) = b.agent.as_mut() {
            agent.updated_at = 900;
        }
        let mut c = view("cedro", "/c", Liveness::Missing, None);
        c.session.created_at = 20;
        let names = |views: &[SessionView]| views.iter().map(|v| v.session.name.clone()).collect::<Vec<_>>();

        let mut views = vec![a, b, c];
        sort_sessions(&mut views, SortOrder::Manual);
        assert_eq!(names(&views), ["beta", "Álamo", "cedro"]);
        sort_sessions(&mut views, SortOrder::Name);
        assert_eq!(names(&views), ["Álamo", "beta", "cedro"], "sin importar tildes ni mayúsculas");
        sort_sessions(&mut views, SortOrder::Activity);
        assert_eq!(names(&views), ["Álamo", "beta", "cedro"]);
        sort_sessions(&mut views, SortOrder::Created);
        assert_eq!(names(&views), ["Álamo", "cedro", "beta"]);
        assert_eq!(SortOrder::parse("name"), SortOrder::Name);
        assert_eq!(SortOrder::parse("?"), SortOrder::Manual);
    }

    #[test]
    fn sessions_filter_by_text_and_status() {
        let api = view("API pagos", "/home/u/trabajo/pagos", alive(), Some(Phase::Permission));
        let web = view("Web", "/home/u/pers/tienda", alive(), Some(Phase::Working));
        let old = view("Migración", "/srv/datos", Liveness::Exited { code: Some(0) }, None);

        let mut filter = SessionFilter::default();
        assert!(!filter.is_active());
        assert!(filter.accepts(&api, None, None) && filter.accepts(&old, None, None));

        filter.query = "PAGOS api".into();
        assert!(filter.is_active());
        assert!(filter.accepts(&api, None, None));
        assert!(!filter.accepts(&web, None, None));
        filter.query = "tienda".into();
        assert!(filter.accepts(&web, None, None), "busca también en la carpeta");
        filter.query = "clientes".into();
        assert!(filter.accepts(&web, Some("Clientes"), None), "y en el nombre del grupo");
        filter.query = "migracion".into();
        assert!(filter.accepts(&old, None, None), "sin importar las tildes");

        filter.query.clear();
        filter.status = StatusFilter::Attention;
        assert!(filter.accepts(&api, None, None) && !filter.accepts(&web, None, None) && !filter.accepts(&old, None, None));
        filter.status = StatusFilter::Alive;
        assert!(filter.accepts(&web, None, None) && !filter.accepts(&old, None, None));
        filter.status = StatusFilter::Stopped;
        assert!(filter.accepts(&old, None, None) && !filter.accepts(&api, None, None));
        // Las archivadas solo aparecen con su filtro, y entonces solo ellas.
        let mut stored = view("Antigua", "/srv/datos", alive(), None);
        stored.session.archived = true;
        for status in [StatusFilter::All, StatusFilter::Alive, StatusFilter::Stopped] {
            filter.status = status;
            assert!(!filter.accepts(&stored, None, None));
        }
        filter.status = StatusFilter::Archived;
        assert!(filter.accepts(&stored, None, None) && !filter.accepts(&api, None, None));
        filter.query = "tienda".into();
        assert!(filter.accepts(&stored, None, Some("Tienda web")), "busca en el proyecto");

        assert_eq!(StatusFilter::parse("attention"), StatusFilter::Attention);
        assert_eq!(StatusFilter::parse("otra cosa"), StatusFilter::All);
    }
}
