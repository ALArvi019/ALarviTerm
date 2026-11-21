//! Traducción del estado real de una sesión a texto e indicador visual.
//!
//! Hay dos fuentes y ninguna es una suposición. El backend dice si el proceso
//! existe y con qué código terminó. Los hooks de Claude Code dicen qué hace:
//! pensar, ejecutar una herramienta, pedir permiso o esperar. Cuando no hay
//! eventos (un shell, o un Claude Code lanzado sin hooks) solo se muestra lo
//! que sabe el backend.

use alarviterm_core::model::SessionKind;
use alarviterm_core::monitor::{AgentState, Phase};
use alarviterm_core::sessions::{Liveness, SessionView};

pub struct StatusInfo {
    pub label: String,
    /// Clase CSS del indicador.
    pub class: &'static str,
    pub tooltip: String,
}

pub const STATUS_CLASSES: &[&str] = &[
    "st-alive",
    "st-working",
    "st-waiting",
    "st-attention",
    "st-done",
    "st-error",
    "st-missing",
];

pub fn describe(view: &SessionView) -> StatusInfo {
    let info = |label: &str, class, tooltip: &str| StatusInfo {
        label: label.to_string(),
        class,
        tooltip: tooltip.to_string(),
    };
    match &view.liveness {
        Liveness::Alive { .. } => match &view.agent {
            Some(agent) => describe_agent(agent),
            None if view.session.kind == SessionKind::Claude => info(
                "Activa",
                "st-alive",
                "El proceso está en ejecución. Claude Code no ha enviado ningún evento \
                 todavía: puede estar arrancando, en una pantalla previa (confianza de la \
                 carpeta, inicio de sesión) o haberse lanzado sin los hooks de alarviterm.",
            ),
            None => info("Activa", "st-alive", "El proceso está en ejecución."),
        },
        Liveness::Exited { code: Some(0) } | Liveness::Exited { code: None } => info(
            "Terminada",
            "st-done",
            "El proceso ha terminado. La salida sigue disponible y se puede reiniciar.",
        ),
        Liveness::Exited { code: Some(code) } => info(
            &format!("Error (código {code})"),
            "st-error",
            "El proceso ha terminado con un código de error.",
        ),
        Liveness::Missing => info(
            "Desconectada",
            "st-missing",
            "El backend no tiene esta sesión: se detuvo o el equipo se reinició. \
             Se puede relanzar.",
        ),
    }
}

fn describe_agent(agent: &AgentState) -> StatusInfo {
    let (label, class, what) = match agent.phase {
        Phase::Ready => (
            "Lista".to_string(),
            "st-waiting",
            "Claude Code ha arrancado y espera la primera instrucción.",
        ),
        Phase::Working => match &agent.tool {
            Some(tool) => (
                format!("Ejecutando {tool}"),
                "st-working",
                "Claude Code está ejecutando una herramienta.",
            ),
            None => (
                "Pensando".to_string(),
                "st-working",
                "Claude Code está procesando la petición.",
            ),
        },
        Phase::Permission => (
            match &agent.tool {
                Some(tool) => format!("Pide permiso: {tool}"),
                None => "Pide permiso".to_string(),
            },
            "st-attention",
            "Claude Code está detenido hasta que apruebes o respondas. Si ya lo has \
             hecho, el estado cambia cuando la herramienta termine: Claude Code no \
             avisa en el momento de conceder el permiso.",
        ),
        Phase::Waiting => (
            "Esperando respuesta".to_string(),
            "st-waiting",
            "Claude Code ha terminado su turno y espera tu siguiente mensaje.",
        ),
        Phase::Compacting => (
            "Compactando".to_string(),
            "st-working",
            "Claude Code está resumiendo la conversación para liberar contexto.",
        ),
        Phase::Ended => (
            "Conversación cerrada".to_string(),
            "st-done",
            "Claude Code ha cerrado la conversación.",
        ),
    };
    let mut tooltip = format!("{what}\nFuente: último evento de Claude Code.");
    if let Some(message) = &agent.message {
        if agent.phase == Phase::Permission {
            tooltip.push_str(&format!("\nAviso: {message}"));
        }
    }
    StatusInfo {
        label,
        class,
        tooltip,
    }
}

/// Texto para una notificación de escritorio cuando la sesión pasa a
/// necesitar al usuario, o `None` si el cambio no lo merece.
pub fn attention_message(before: Option<Phase>, agent: &AgentState) -> Option<String> {
    if before == Some(agent.phase) {
        return None;
    }
    match agent.phase {
        Phase::Permission => Some(match &agent.tool {
            Some(tool) => format!("Pide permiso para usar {tool}"),
            None => "Pide permiso para continuar".to_string(),
        }),
        // Solo avisa de un turno terminado, no de una sesión recién abierta.
        Phase::Waiting if matches!(before, Some(Phase::Working | Phase::Compacting | Phase::Permission)) => {
            Some("Ha terminado y espera tu respuesta".to_string())
        }
        _ => None,
    }
}

/// Tiempo transcurrido en lenguaje natural, con la precisión justa para una
/// vista que se refresca cada pocos segundos.
pub fn relative_time(now: i64, then: i64) -> String {
    let seconds = (now - then).max(0);
    match seconds {
        0..=59 => "hace menos de 1 min".to_string(),
        60..=3599 => format!("hace {} min", seconds / 60),
        3600..=86_399 => format!("hace {} h", seconds / 3600),
        _ => {
            let days = seconds / 86_400;
            if days == 1 {
                "hace 1 día".to_string()
            } else {
                format!("hace {days} días")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_buckets() {
        assert_eq!(relative_time(100, 100), "hace menos de 1 min");
        assert_eq!(relative_time(100, 200), "hace menos de 1 min");
        assert_eq!(relative_time(1000, 1000 - 60), "hace 1 min");
        assert_eq!(relative_time(10_000, 10_000 - 3599), "hace 59 min");
        assert_eq!(relative_time(10_000, 10_000 - 3600), "hace 1 h");
        assert_eq!(relative_time(200_000, 200_000 - 86_400), "hace 1 día");
        assert_eq!(relative_time(400_000, 400_000 - 3 * 86_400), "hace 3 días");
    }

    fn view(kind: SessionKind, liveness: Liveness, agent: Option<AgentState>) -> SessionView {
        SessionView {
            session: alarviterm_core::model::Session {
                id: "a".into(),
                name: "a".into(),
                group_id: None,
                kind,
                cwd: "/tmp".into(),
                model: None,
                extra_args: String::new(),
                env: Vec::new(),
                backend: "tmux".into(),
                backend_id: "mt-a".into(),
                position: 0,
                created_at: 0,
                last_opened_at: None,
                project_id: None,
                archived: false,
                profile: None,
            },
            liveness,
            last_output_at: None,
            agent,
        }
    }

    fn agent(phase: Phase, tool: Option<&str>) -> AgentState {
        AgentState {
            phase,
            tool: tool.map(str::to_string),
            message: None,
            since: 0,
            updated_at: 0,
            conversation: None,
            transcript: None,
            permission_mode: None,
            model: None,
            launch: None,
        }
    }

    const ALIVE: Liveness = Liveness::Alive {
        pid: None,
        command: None,
    };

    #[test]
    fn exit_codes_map_to_distinct_states() {
        let shell = |liveness| describe(&view(SessionKind::Shell, liveness, None));
        assert_eq!(shell(Liveness::Exited { code: Some(0) }).class, "st-done");
        assert_eq!(shell(Liveness::Exited { code: Some(2) }).class, "st-error");
        assert!(shell(Liveness::Exited { code: Some(2) }).label.contains('2'));
        assert_eq!(shell(Liveness::Missing).class, "st-missing");
        assert_eq!(shell(ALIVE).class, "st-alive");
    }

    #[test]
    fn hook_events_refine_a_live_claude_session() {
        let claude = |a| describe(&view(SessionKind::Claude, ALIVE, a));
        assert_eq!(claude(None).label, "Activa");
        assert_eq!(claude(Some(agent(Phase::Working, None))).label, "Pensando");
        let tool = claude(Some(agent(Phase::Working, Some("Bash"))));
        assert_eq!((tool.label.as_str(), tool.class), ("Ejecutando Bash", "st-working"));
        let ask = claude(Some(agent(Phase::Permission, Some("Write"))));
        assert_eq!((ask.label.as_str(), ask.class), ("Pide permiso: Write", "st-attention"));
        assert_eq!(claude(Some(agent(Phase::Waiting, None))).class, "st-waiting");
        for phase in [Phase::Ready, Phase::Working, Phase::Permission, Phase::Waiting, Phase::Compacting, Phase::Ended] {
            assert!(STATUS_CLASSES.contains(&claude(Some(agent(phase, None))).class));
        }
    }

    #[test]
    fn attention_only_on_transitions_that_need_the_user() {
        let ask = agent(Phase::Permission, Some("Bash"));
        assert!(attention_message(Some(Phase::Working), &ask).is_some());
        assert!(attention_message(Some(Phase::Permission), &ask).is_none());
        let waiting = agent(Phase::Waiting, None);
        assert!(attention_message(Some(Phase::Working), &waiting).is_some());
        assert!(attention_message(Some(Phase::Ready), &waiting).is_none());
        assert!(attention_message(None, &waiting).is_none());
        assert!(attention_message(Some(Phase::Waiting), &agent(Phase::Working, None)).is_none());
    }
}
