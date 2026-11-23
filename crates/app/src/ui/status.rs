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
    "st-background",
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

/// Estado de un conjunto de sesiones en una sola señal, para la cabecera de
/// un grupo plegado. Manda lo que necesita al usuario: primero quien pide
/// permiso, después quien espera respuesta, después quien trabaja.
pub fn summarize<'a>(views: impl Iterator<Item = &'a SessionView>) -> Option<StatusInfo> {
    const ORDER: [&str; 8] = [
        "st-attention",
        "st-waiting",
        "st-working",
        "st-background",
        "st-alive",
        "st-error",
        "st-done",
        "st-missing",
    ];
    let mut counts = [0usize; 8];
    for view in views {
        let class = describe(view).class;
        if let Some(index) = ORDER.iter().position(|c| *c == class) {
            counts[index] += 1;
        }
    }
    let total: usize = counts.iter().sum();
    let first = counts.iter().position(|n| *n > 0)?;
    let n = counts[first];
    let one = n == 1;
    let label = match first {
        0 if one => "1 pide permiso".to_string(),
        0 => format!("{n} piden permiso"),
        1 if one => "1 te espera".to_string(),
        1 => format!("{n} te esperan"),
        2 if one => "1 trabajando".to_string(),
        2 => format!("{n} trabajando"),
        3 => format!("{n} en segundo plano"),
        4 if one => "1 activa".to_string(),
        4 => format!("{n} activas"),
        5 if one => "1 con error".to_string(),
        5 => format!("{n} con error"),
        _ => "paradas".to_string(),
    };
    let parts: Vec<String> = [
        (0, "piden permiso"),
        (1, "esperan tu respuesta"),
        (2, "trabajando"),
        (3, "con trabajo en segundo plano"),
        (4, "activas sin estado"),
        (5, "con error"),
        (6, "terminadas"),
        (7, "paradas"),
    ]
    .iter()
    .filter(|(index, _)| counts[*index] > 0)
    .map(|(index, what)| format!("{} {what}", counts[*index]))
    .collect();
    Some(StatusInfo {
        label,
        class: ORDER[first],
        tooltip: format!("{total} sesiones: {}", parts.join(", ")),
    })
}

fn describe_agent(agent: &AgentState) -> StatusInfo {
    // Con el turno terminado pero algo corriendo por su cuenta, la sesión no
    // espera al usuario: espera a que eso acabe.
    if agent.busy_in_background() {
        let label = match (agent.background_shell, agent.subagents) {
            (true, 0) => "Comando en segundo plano".to_string(),
            (false, 1) => "Subagente en segundo plano".to_string(),
            (false, n) => format!("{n} subagentes en segundo plano"),
            (true, _) => "Comando y subagentes en segundo plano".to_string(),
        };
        return StatusInfo {
            label,
            class: "st-background",
            tooltip: "Claude Code ha terminado su turno, pero dejó trabajo en marcha y seguirá cuando acabe. \
                      No espera nada de ti. Puedes escribirle igualmente."
                .to_string(),
        };
    }
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
        // Con un comando o un subagente todavía en marcha no espera a nadie.
        Phase::Waiting if agent.busy_in_background() => None,
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
            subagents: 0,
            background_shell: false,
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
    fn a_group_shows_first_whoever_needs_the_user() {
        let claude = |phase| view(SessionKind::Claude, ALIVE, Some(agent(phase, None)));
        let stopped = || view(SessionKind::Claude, Liveness::Missing, None);

        let all = [claude(Phase::Working), claude(Phase::Waiting), claude(Phase::Permission), stopped()];
        let top = summarize(all.iter()).expect("resumen");
        assert_eq!((top.class, top.label.as_str()), ("st-attention", "1 pide permiso"));
        assert!(top.tooltip.starts_with("4 sesiones"));

        let waiting = [claude(Phase::Working), claude(Phase::Waiting), claude(Phase::Waiting)];
        let top = summarize(waiting.iter()).expect("resumen");
        assert_eq!((top.class, top.label.as_str()), ("st-waiting", "2 te esperan"));

        // Quien dejó un comando o un subagente corriendo no espera al usuario.
        let background = |shell: bool, subagents: u32| {
            let mut state = agent(Phase::Waiting, None);
            state.background_shell = shell;
            state.subagents = subagents;
            view(SessionKind::Claude, ALIVE, Some(state))
        };
        let shell = describe(&background(true, 0));
        assert_eq!((shell.class, shell.label.as_str()), ("st-background", "Comando en segundo plano"));
        assert_eq!(describe(&background(false, 2)).label, "2 subagentes en segundo plano");
        let quiet = [background(true, 0), background(false, 1), stopped()];
        let top = summarize(quiet.iter()).expect("resumen");
        assert_eq!((top.class, top.label.as_str()), ("st-background", "2 en segundo plano"));
        let mixed = [background(true, 0), claude(Phase::Waiting)];
        assert_eq!(summarize(mixed.iter()).expect("resumen").label, "1 te espera");
        // Un subagente en marcha durante el turno no cambia nada: está trabajando.
        let mut working = agent(Phase::Working, None);
        working.subagents = 1;
        assert_eq!(describe(&view(SessionKind::Claude, ALIVE, Some(working))).class, "st-working");

        let busy = [claude(Phase::Working), stopped()];
        assert_eq!(summarize(busy.iter()).expect("resumen").label, "1 trabajando");
        let off = [stopped(), stopped()];
        let top = summarize(off.iter()).expect("resumen");
        assert_eq!((top.class, top.label.as_str()), ("st-missing", "paradas"));
        assert!(summarize(std::iter::empty()).is_none());
    }

    #[test]
    fn attention_only_on_transitions_that_need_the_user() {
        let ask = agent(Phase::Permission, Some("Bash"));
        assert!(attention_message(Some(Phase::Working), &ask).is_some());
        assert!(attention_message(Some(Phase::Permission), &ask).is_none());
        let waiting = agent(Phase::Waiting, None);
        assert!(attention_message(Some(Phase::Working), &waiting).is_some());
        assert!(attention_message(Some(Phase::Ready), &waiting).is_none());
        let mut background = agent(Phase::Waiting, None);
        background.background_shell = true;
        assert!(attention_message(Some(Phase::Working), &background).is_none());
        assert!(attention_message(None, &waiting).is_none());
        assert!(attention_message(Some(Phase::Waiting), &agent(Phase::Working, None)).is_none());
    }
}
