//! Official Bastion terminal UI for the webhook + SSE API — the same surface the
//! mobile companion app pairs with (`/auth/exchange`, `/webhook`, `/events`).
//!
//! Local startup discovers or starts the runtime and consumes the owner-scoped
//! bootstrap token automatically. Pairing codes are reserved for remote devices.
//!
//! Known gap (2026-07-02): `/events` today only broadcasts `mesh_sync` messages
//! (`src/mesh/p2p.rs`) — there is no per-turn tool-call/progress event yet, so a
//! turn is a blocking `POST /webhook` with an animated spinner, not a live trace
//! of tool calls. The SSE panel is wired up so it starts working the day that lands.

mod companion;
mod visual;

use anyhow::{bail, Context, Result};
use companion::{CareAction, CareCue, CompanionState};
use crossterm::event::{
    Event as CEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use url::{Host, Url};
use visual::{Appearance, Identity, VisualMode};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Serialize, Deserialize, Clone)]
struct Session {
    jwt: String,
    owner_id: String,
    device_name: String,
}

#[derive(Deserialize)]
struct WebhookOut {
    reply: String,
}

fn token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("bastion")
        .join("session.json")
}

fn load_session() -> Option<Session> {
    let raw = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_session(session: &Session) -> Result<()> {
    let path = token_path();
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir).context("creating ~/.config/bastion")?;
    }
    write_private_file(&path, serde_json::to_string_pretty(session)?.as_bytes())
        .context("saving session")
}

/// Creates `dir` (if missing) and locks it down to owner-only (0700). The
/// session file inside carries a 90-day bearer JWT — this must not be
/// group/world-readable on a shared or corporate machine.
#[cfg(unix)]
fn ensure_private_dir(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_dir(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(Into::into)
}

/// Writes `contents` to `path` created with mode 0600 from the start — no
/// window where the credential file is readable with the default umask.
#[cfg(unix)]
fn write_private_file(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    std::fs::write(path, contents).map_err(Into::into)
}

fn clear_session() {
    let _ = std::fs::remove_file(token_path());
}

fn is_local_url(base_url: &str) -> bool {
    Url::parse(base_url)
        .ok()
        .is_some_and(|url| match url.host() {
            Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        })
}

fn local_bootstrap_token(base_url: &str) -> Option<String> {
    is_local_url(base_url)
        .then(|| std::env::var("BASTION_BOOTSTRAP_TOKEN").ok())
        .flatten()
        .filter(|token| !token.trim().is_empty())
}

fn token_session(token: &str, owner: &str) -> Session {
    Session {
        jwt: token.to_owned(),
        owner_id: owner.to_owned(),
        device_name: "terminal".to_string(),
    }
}

fn find_compose_dir(start: &Path) -> Option<PathBuf> {
    const COMPOSE_FILES: &[&str] = &[
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ];

    start.ancestors().find_map(|dir| {
        COMPOSE_FILES
            .iter()
            .any(|name| dir.join(name).is_file())
            .then(|| dir.to_path_buf())
    })
}

async fn runtime_ready(client: &Client, base_url: &str) -> bool {
    client
        .get(format!("{}/readyz", base_url.trim_end_matches('/')))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn start_compose(compose_dir: &Path) -> Result<bool> {
    println!("◈ Runtime local ausente; iniciando o Bastion com Docker Compose…");
    match Command::new("docker")
        .args(["compose", "up", "-d"])
        .current_dir(compose_dir)
        .status()
    {
        Ok(status) if status.success() => Ok(true),
        Ok(status) => bail!("Docker Compose terminou com {status}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("não foi possível executar docker compose"),
    }
}

fn ensure_compose_port(compose_dir: &Path) -> Result<()> {
    let output = Command::new("docker")
        .args(["compose", "port", "core", "8080"])
        .current_dir(compose_dir)
        .output()
        .context("verificando a porta publicada pelo Docker Compose")?;
    let published = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && !published.trim().is_empty() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr);
    bail!(
        "o container core iniciou sem publicar a porta HTTP; verifique se BASTION_PUBLISH_HOST e BASTION_HTTP_PORT estão livres ({})",
        detail.trim()
    )
}

fn start_native_daemon() -> Result<()> {
    println!("◈ Iniciando daemon Bastion local em background…");
    let executable = std::env::current_exe().context("localizando o executável do Bastion")?;
    let mut child = Command::new(executable)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("iniciando daemon Bastion local")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

async fn ensure_runtime(client: &Client, base_url: &str, auto_start: bool) -> Result<()> {
    if runtime_ready(client, base_url).await {
        return Ok(());
    }

    if !is_local_url(base_url) {
        bail!("runtime remoto indisponível ou ainda não pronto em {base_url}");
    }
    if !auto_start {
        bail!(
            "runtime local indisponível em {base_url}; inicie `bastion daemon` ou remova --no-auto-start"
        );
    }

    let compose_dir = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_compose_dir(&cwd));
    let compose_started = match compose_dir {
        Some(dir) => {
            let started = start_compose(&dir)?;
            if started {
                ensure_compose_port(&dir)?;
            }
            started
        }
        None => false,
    };
    if !compose_started {
        start_native_daemon()?;
    }

    let started_at = Instant::now();
    while started_at.elapsed() < STARTUP_TIMEOUT {
        if runtime_ready(client, base_url).await {
            println!("◈ Runtime Bastion pronto.");
            return Ok(());
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
    }

    bail!(
        "o runtime foi iniciado, mas não ficou pronto em {}s; verifique `docker compose logs core` ou `.bastion/bastion.log`",
        STARTUP_TIMEOUT.as_secs()
    )
}

/// Plain-terminal pairing prompt — runs BEFORE raw mode / the alternate screen
/// are entered (and again, suspended back to plain mode, if a session expires
/// mid-run), so it stays simple stdin/stdout instead of an in-TUI text field.
async fn pair(client: &Client, base_url: &str) -> Result<Session> {
    println!("◈ Nenhuma sessão Bastion pareada encontrada.");
    println!("Em um canal já autorizado, digite: /connect-app terminal");
    println!("Depois cole o código impresso (formato BAST-XXXX-XXXX).");
    print!("código> ");
    io::stdout().flush()?;
    let mut otc = String::new();
    io::stdin().read_line(&mut otc)?;
    let otc = otc.trim();
    if otc.is_empty() {
        bail!("pareamento cancelado — nenhum código informado");
    }

    let resp = client
        .post(format!("{base_url}/auth/exchange"))
        .json(&json!({ "otc": otc }))
        .send()
        .await
        .context(
            "não foi possível conectar ao daemon — BASTION_WEBHOOK_ADDR está setado e o daemon rodando?",
        )?;

    if !resp.status().is_success() {
        bail!(
            "pareamento falhou: HTTP {} (código expirado ou desconhecido?)",
            resp.status()
        );
    }

    let session: Session = resp
        .json()
        .await
        .context("resposta inesperada de /auth/exchange")?;
    save_session(&session)?;
    println!(
        "Pareado como '{}' (dispositivo '{}'). ◈\n",
        session.owner_id, session.device_name
    );
    Ok(session)
}

/// One transcript entry.
enum Line {
    You(String),
    Bastion(String),
    Event(String),
    System(String),
}

/// Result of a completed turn, delivered back into the event loop.
enum TurnOutcome {
    Reply(String),
    Error(String),
    Unauthorized,
}

/// Everything the render loop reacts to — key presses, spinner ticks, SSE
/// events, and completed turns all funnel through one channel so there is a
/// single point of truth for screen state (no interleaved prints, no races).
enum AppMsg {
    Key(KeyEvent),
    Tick,
    SseEvent(String),
    Turn(TurnOutcome),
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Known Bastion slash commands, mirrored from `src/agent/command.rs` for
/// autocomplete. `remote` matches the allowlist in `main.rs`'s inbound_rx arm
/// (WEB-CMD-01) — keep both lists in sync if a command's exposure changes.
struct CommandInfo {
    name: &'static str,
    usage: &'static str,
    desc: &'static str,
    remote: bool,
}

const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/pet",
        usage: "/pet <ação>",
        desc: "cuida e configura o companion — digite espaço para ver as ações",
        remote: false,
    },
    CommandInfo {
        name: "/theme",
        usage: "/theme <nome|#RRGGBB>",
        desc: "troca as cores da TUI na hora — digite espaço para ver os temas",
        remote: false,
    },
    CommandInfo {
        name: "/help",
        usage: "/help",
        desc: "mostra os comandos",
        remote: true,
    },
    CommandInfo {
        name: "/contest",
        usage: "/contest <id>",
        desc: "revoga uma crença por ID",
        remote: true,
    },
    CommandInfo {
        name: "/model",
        usage: "/model <nome>",
        desc: "troca o modelo — console-only, afeta o daemon inteiro",
        remote: false,
    },
    CommandInfo {
        name: "/as",
        usage: "/as <persona>",
        desc: "força persona no próximo turno — console-only",
        remote: false,
    },
    CommandInfo {
        name: "/cabinet",
        usage: "/cabinet [personas..]",
        desc: "convoca o Cabinet — console-only",
        remote: false,
    },
    CommandInfo {
        name: "/logs",
        usage: "/logs",
        desc: "erros recentes do daemon — console-only",
        remote: false,
    },
    CommandInfo {
        name: "/stop",
        usage: "/stop",
        desc: "desliga o daemon — console-only",
        remote: false,
    },
    CommandInfo {
        name: "/connect-app",
        usage: "/connect-app <device>",
        desc: "pareia um novo dispositivo — console-only",
        remote: false,
    },
];

/// Ações locais do `/pet`, expandidas no menu assim que o usuário digita o
/// espaço — cada entrada é o comando completo para Tab/Enter completarem
/// direto, sem argumento a adivinhar (exceto `use`, que pede um caminho).
const PET_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/pet stats",
        usage: "/pet stats",
        desc: "nível, XP e necessidades do companion",
        remote: false,
    },
    CommandInfo {
        name: "/pet game on",
        usage: "/pet game on",
        desc: "liga o game mode — XP por turno concluído",
        remote: false,
    },
    CommandInfo {
        name: "/pet game off",
        usage: "/pet game off",
        desc: "desliga o game mode",
        remote: false,
    },
    CommandInfo {
        name: "/pet water",
        usage: "/pet water",
        desc: "hidrata o companion (beba água você também)",
        remote: false,
    },
    CommandInfo {
        name: "/pet feed",
        usage: "/pet feed",
        desc: "alimenta o companion",
        remote: false,
    },
    CommandInfo {
        name: "/pet play",
        usage: "/pet play",
        desc: "registra uma pausa curta — alongar, caminhar, respirar",
        remote: false,
    },
    CommandInfo {
        name: "/pet sleep",
        usage: "/pet sleep",
        desc: "descansa o companion e zera as necessidades",
        remote: false,
    },
    CommandInfo {
        name: "/pet use",
        usage: "/pet use <pet.toml|builtin>",
        desc: "troca o pet pack ativo",
        remote: false,
    },
];

/// Temas nomeados do `/theme`, expandidos no menu após o espaço — mesma
/// mecânica do `/pet`. Cores hex (`/theme #RRGGBB`) são digitadas direto.
const THEME_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/theme rgb",
        usage: "/theme rgb",
        desc: "ciclo de cores por estado — o padrão vivo",
        remote: false,
    },
    CommandInfo {
        name: "/theme cyan",
        usage: "/theme cyan",
        desc: "monocromático ciano",
        remote: false,
    },
    CommandInfo {
        name: "/theme blue",
        usage: "/theme blue",
        desc: "monocromático azul",
        remote: false,
    },
    CommandInfo {
        name: "/theme magenta",
        usage: "/theme magenta",
        desc: "monocromático magenta",
        remote: false,
    },
    CommandInfo {
        name: "/theme amber",
        usage: "/theme amber",
        desc: "monocromático âmbar",
        remote: false,
    },
    CommandInfo {
        name: "/theme green",
        usage: "/theme green",
        desc: "monocromático verde — modo matrix",
        remote: false,
    },
    CommandInfo {
        name: "/theme mono",
        usage: "/theme mono",
        desc: "sem cor de destaque",
        remote: false,
    },
];

/// Matches while typing the command token; a space normally closes the menu
/// (the user is typing arguments), except after `/pet ` and `/theme `, where
/// the menu switches to the subcommand list so as opções ficam visíveis.
fn command_matches(input: &str) -> Vec<&'static CommandInfo> {
    if input.is_empty() || !input.starts_with('/') {
        return vec![];
    }
    if input.starts_with("/pet ") {
        return PET_COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(input))
            .collect();
    }
    if input.starts_with("/theme ") {
        return THEME_COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(input))
            .collect();
    }
    if input.contains(' ') {
        return vec![];
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(input))
        .collect()
}

/// `/theme` é resolvido inteiro na TUI: aplica na hora e persiste em
/// `~/.config/bastion/tui.json` (bastion.toml continua valendo como base).
fn theme_command(app: &mut App, input: &str) -> Option<String> {
    let mut parts = input.split_whitespace();
    if parts.next()? != "/theme" {
        return None;
    }
    let response = match parts.next() {
        None => format!(
            "{}\nUso: /theme <{}|#RRGGBB>",
            app.appearance.theme_status(),
            visual::THEME_NAMES.join("|")
        ),
        Some(value) if value.starts_with('#') => match app.appearance.apply_accent(value) {
            Ok(()) => format!("Accent {value} aplicado e salvo."),
            Err(error) => format!("{error}"),
        },
        Some(name) => match app.appearance.apply_theme(name) {
            Ok(()) => format!("Tema {name} aplicado e salvo."),
            Err(error) => format!("{error}"),
        },
    };
    Some(response)
}

fn companion_command(app: &mut App, input: &str) -> Option<(String, VisualMode)> {
    let mut parts = input.split_whitespace();
    if parts.next()? != "/pet" {
        return None;
    }
    let command = parts.next().unwrap_or("stats");
    let mut mode = VisualMode::Success;
    let response = match command {
        "stats" => format!(
            "Companion: {}\nNecessidades: {}\nGame mode: {}",
            app.companion.status(),
            app.companion.needs_status(),
            if app.companion.game_enabled {
                "ligado"
            } else {
                "desligado"
            }
        ),
        "game" => match parts.next() {
            Some("on") => {
                app.companion.game_enabled = true;
                save_companion(
                    app,
                    "Game mode ligado. O progresso recompensa turnos concluídos, nunca volume de tokens.",
                )
            }
            Some("off") => {
                app.companion.game_enabled = false;
                save_companion(
                    app,
                    "Game mode desligado. O companion visual continua ativo.",
                )
            }
            _ => "Uso: /pet game <on|off>".into(),
        },
        "water" | "drink" => {
            app.companion.care(CareAction::Water);
            save_companion(
                app,
                "Keeper hidratado. Bom momento para você beber água também.",
            )
        }
        "feed" => {
            app.companion.care(CareAction::Feed);
            save_companion(
                app,
                "Keeper alimentado. O cuidado do companion fica separado do seu progresso.",
            )
        }
        "play" => {
            app.companion.care(CareAction::Play);
            save_companion(
                app,
                "Pausa para brincar concluída. Alongue, caminhe, respire ou escolha qualquer reset curto que funcione para você.",
            )
        }
        "sleep" | "rest" => {
            app.companion.care(CareAction::Sleep);
            mode = VisualMode::Sleep;
            save_companion(
                app,
                "Companion descansando. Seu progresso está seguro; considere encerrar sua sessão longa também.",
            )
        }
        "use" => match parts.next() {
            Some("builtin") => {
                app.appearance.pet = None;
                app.companion.pet_path = None;
                save_companion(app, "Usando a família nativa de companions do Bastion.")
            }
            Some(path) => {
                let path = PathBuf::from(path);
                match app.appearance.use_pet(&path) {
                    Ok(()) => {
                        app.companion.pet_path = Some(path);
                        save_companion(app, "Pet pack customizado carregado.")
                    }
                    Err(error) => format!("Não foi possível carregar o pet pack: {error}"),
                }
            }
            None => "Uso: /pet use <pet.toml|builtin>".into(),
        },
        _ => {
            mode = VisualMode::Unknown;
            "Uso: /pet <stats|game on|off|feed|water|play|sleep|use>".into()
        }
    };
    Some((response, mode))
}

/// Quanto tempo o rosto de um estado local fica na tela antes de voltar ao
/// guard — descanso e dúvida merecem uma pausa maior que um sucesso.
fn settle_after(mode: VisualMode) -> Duration {
    match mode {
        VisualMode::Sleep => Duration::from_millis(4000),
        VisualMode::Unknown => Duration::from_millis(2200),
        _ => Duration::from_millis(1400),
    }
}

/// Comando de barra que não existe nem local nem no daemon (COMMANDS espelha
/// `src/agent/command.rs`): responde na hora com o Keeper em dúvida, sem
/// gastar um turno do runtime.
fn unknown_command(text: &str) -> Option<String> {
    if !text.starts_with('/') {
        return None;
    }
    let first = text.split_whitespace().next()?;
    if COMMANDS.iter().any(|c| c.name == first) {
        return None;
    }
    Some(format!(
        "Comando desconhecido: {first}. Digite / para ver o menu ou /help para a lista completa."
    ))
}

fn save_companion(app: &App, success: &str) -> String {
    match app.companion.save() {
        Ok(()) => success.to_string(),
        Err(error) => format!("A mudança vale nesta sessão, mas não pôde ser salva: {error}"),
    }
}

fn care_cue(cue: CareCue) -> &'static str {
    match cue {
        CareCue::Water => "Keeper está com sede — /pet water (e pegue água para você também).",
        CareCue::Feed => "Keeper está com fome — /pet feed para alimentá-lo.",
        CareCue::Play => "Keeper quer um reset curto — /pet play quando você escolher pausar.",
        CareCue::Sleep => "Esta sessão ativa está longa — /pet sleep quando for hora de parar.",
    }
}

/// Stable, binary-level event bridge for Claude/Codex/OpenCode hooks. Events
/// only update local companion wellbeing state; they cannot invoke tools or
/// alter Bastion capabilities.
pub fn companion_event(kind: &str, source: &str) -> Result<String> {
    let valid_source = !source.is_empty()
        && source.len() <= 32
        && source
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character));
    if !valid_source {
        bail!("companion source must be 1-32 ASCII letters, digits, '-', '_' or '.'");
    }

    let mut state = CompanionState::load(false);
    match kind {
        "session-start" => state.start_session(source),
        "activity" => return companion_activity(&mut state, source),
        "session-stop" => state.stop_session(source),
        _ => bail!("unknown companion event '{kind}'"),
    }
    state.save()?;
    Ok(format!("evento do companion registrado: {kind} ({source})"))
}

fn companion_activity(state: &mut CompanionState, source: &str) -> Result<String> {
    let cue = state.record_source_activity(source);
    state.save()?;
    Ok(cue.map_or_else(
        || format!("evento do companion registrado: activity ({source})"),
        |due| care_cue(due).to_string(),
    ))
}

pub fn companion_care(action: &str) -> Result<String> {
    let mut state = CompanionState::load(false);
    let (care, message) = match action {
        "water" => (CareAction::Water, "companion hidratado"),
        "feed" => (CareAction::Feed, "companion alimentado"),
        "play" => (CareAction::Play, "pausa do companion registrada"),
        "sleep" | "rest" => (CareAction::Sleep, "companion descansando"),
        _ => bail!("unknown companion care action '{action}'"),
    };
    state.care(care);
    state.save()?;
    Ok(message.to_string())
}

pub fn companion_status() -> String {
    let state = CompanionState::load(false);
    format!(
        "{}\n{}\ngame mode: {}",
        state.status(),
        state.needs_status(),
        if state.game_enabled {
            "ligado"
        } else {
            "desligado"
        }
    )
}

struct App {
    owner_id: String,
    device_name: String,
    base_url: String,
    lines: Vec<Line>,
    input: String,
    thinking: bool,
    spinner_idx: usize,
    animation_tick: usize,
    visual_mode: VisualMode,
    settle_at: Option<Instant>,
    appearance: Appearance,
    /// Index into the live `command_matches(&input)` list — reset to 0 whenever
    /// the input text changes so it never points past the new match set.
    suggestion_idx: usize,
    companion: CompanionState,
}

fn spawn_key_reader(tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let mut stream = EventStream::new();
        while let Some(Ok(ev)) = stream.next().await {
            if let CEvent::Key(key) = ev {
                if tx.send(AppMsg::Key(key)).is_err() {
                    break;
                }
            }
        }
    });
}

fn spawn_ticker(tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(90));
        loop {
            interval.tick().await;
            if tx.send(AppMsg::Tick).is_err() {
                break;
            }
        }
    });
}

/// Reconnects on drop; today this only surfaces `mesh_sync` (see module doc).
fn spawn_sse_listener(tx: UnboundedSender<AppMsg>, client: Client, base_url: String, jwt: String) {
    tokio::spawn(async move {
        loop {
            if let Ok(resp) = client
                .get(format!("{base_url}/events"))
                .header("x-bastion-token", &jwt)
                .send()
                .await
            {
                if resp.status().is_success() {
                    let mut stream = resp.bytes_stream();
                    let mut buf = String::new();
                    while let Some(Ok(chunk)) = stream.next().await {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(pos) = buf.find("\n\n") {
                            let raw_event: String = buf.drain(..pos + 2).collect();
                            for line in raw_event.lines() {
                                if let Some(data) = line.strip_prefix("data:") {
                                    let text = data.trim().to_string();
                                    if tx.send(AppMsg::SseEvent(text)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

fn spawn_turn(
    tx: UnboundedSender<AppMsg>,
    client: Client,
    base_url: String,
    jwt: String,
    text: String,
) {
    tokio::spawn(async move {
        let outcome = match client
            .post(format!("{base_url}/webhook"))
            .header("x-bastion-token", &jwt)
            .json(&json!({ "text": text }))
            .send()
            .await
        {
            Ok(resp) if resp.status() == StatusCode::UNAUTHORIZED => TurnOutcome::Unauthorized,
            Ok(resp) if resp.status().is_success() => match resp.json::<WebhookOut>().await {
                Ok(out) => TurnOutcome::Reply(out.reply),
                Err(e) => TurnOutcome::Error(format!("resposta inválida: {e}")),
            },
            Ok(resp) => TurnOutcome::Error(format!("HTTP {}", resp.status())),
            Err(e) => TurnOutcome::Error(format!("requisição falhou: {e}")),
        };
        let _ = tx.send(AppMsg::Turn(outcome));
    });
}

fn wrapped_line_count(lines: &[RLine], width: u16) -> usize {
    let width = (width as usize).max(1);
    lines
        .iter()
        .map(|l| {
            let chars: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            chars.div_ceil(width).max(1)
        })
        .sum()
}

/// Height of the suggestion panel — 0 collapses it entirely (Constraint::Length(0)
/// renders nothing), plus 2 rows for its own border when there are matches.
fn suggestion_height(input: &str) -> u16 {
    let n = command_matches(input).len();
    if n == 0 {
        0
    } else {
        n as u16 + 2
    }
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let identity_height = visual::identity_height(area, &app.appearance);
    let mut companion_parts = Vec::new();
    if let Some(pet) = app.appearance.pet_label() {
        companion_parts.push(pet);
    }
    if app.companion.game_enabled {
        companion_parts.push(format!(
            "{} · {}",
            app.companion.status(),
            app.companion.needs_status()
        ));
    }
    let companion_status = (!companion_parts.is_empty()).then(|| companion_parts.join(" · "));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(identity_height),
            Constraint::Min(3),
            Constraint::Length(suggestion_height(&app.input)),
            Constraint::Length(3),
        ])
        .split(area);

    visual::render_identity(
        f,
        chunks[0],
        &app.appearance,
        Identity {
            owner: &app.owner_id,
            device: &app.device_name,
            runtime: &app.base_url,
            mode: app.visual_mode,
            tick: app.animation_tick,
            companion: companion_status.as_deref(),
        },
    );

    let transcript_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.appearance.muted()))
        .title(" conversa ");
    let inner = transcript_block.inner(chunks[1]);

    let mut text_lines: Vec<RLine> = Vec::new();
    for l in &app.lines {
        match l {
            Line::You(t) => {
                text_lines.push(RLine::from(vec![
                    Span::styled(
                        "❯ ",
                        Style::default()
                            .fg(app.appearance.user())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(t.clone()),
                ]));
                text_lines.push(RLine::raw(""));
            }
            Line::Bastion(t) => {
                text_lines.push(RLine::from(vec![
                    Span::styled(
                        "◈ Bastion  ",
                        Style::default()
                            .fg(app.appearance.accent(app.visual_mode))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(t.clone()),
                ]));
                text_lines.push(RLine::raw(""));
            }
            Line::Event(t) => {
                text_lines.push(RLine::styled(
                    format!("· {t}"),
                    Style::default().fg(app.appearance.muted()),
                ));
                text_lines.push(RLine::raw(""));
            }
            Line::System(t) => {
                text_lines.push(RLine::styled(
                    format!("⚠ {t}"),
                    Style::default().fg(app.appearance.warning()),
                ));
                text_lines.push(RLine::raw(""));
            }
        }
    }
    if app.thinking {
        let frame = SPINNER[app.spinner_idx % SPINNER.len()];
        text_lines.push(RLine::styled(
            format!("{frame} pensando…"),
            Style::default().fg(app.appearance.muted()),
        ));
    }

    // `Paragraph::line_count` is unstable (rendered-line-info) — approximate the
    // wrapped row count ourselves (char count is good enough for this text; no
    // wide-glyph handling needed for this chat transcript).
    let total = wrapped_line_count(&text_lines, inner.width);
    let scroll = total.saturating_sub(inner.height as usize) as u16;
    let paragraph = Paragraph::new(text_lines).wrap(Wrap { trim: false });
    f.render_widget(
        paragraph.block(transcript_block).scroll((scroll, 0)),
        chunks[1],
    );

    let suggestions = command_matches(&app.input);
    if !suggestions.is_empty() {
        let selected = app.suggestion_idx.min(suggestions.len().saturating_sub(1));
        let items: Vec<RLine> = suggestions
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == selected {
                    Style::default()
                        .fg(app.appearance.text())
                        .bg(app.appearance.user())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.appearance.text())
                };
                let marker = if i == selected { "▸ " } else { "  " };
                let tag = if c.name.starts_with("/pet") {
                    " [TUI]"
                } else if c.remote {
                    ""
                } else {
                    " [console]"
                };
                RLine::from(vec![
                    Span::styled(format!("{marker}{:<22}", c.usage), style),
                    Span::styled(
                        format!(" {}", c.desc),
                        Style::default().fg(app.appearance.muted()),
                    ),
                    Span::styled(tag, Style::default().fg(app.appearance.warning())),
                ])
            })
            .collect();
        let suggestion_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.appearance.muted()))
            .title(" ↑↓ escolhe · Tab/Enter completa ");
        f.render_widget(Paragraph::new(items).block(suggestion_block), chunks[2]);
    }

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.appearance.accent(app.visual_mode)))
        .title(" mensagem — Enter envia · Esc/Ctrl+C sai · Ctrl+U limpa ");
    let input = Paragraph::new(app.input.as_str())
        .style(Style::default().fg(app.appearance.text()))
        .block(input_block);
    f.render_widget(input, chunks[3]);

    let cursor_x = (chunks[3].x + 1 + app.input.chars().count() as u16)
        .min(chunks[3].x + chunks[3].width.saturating_sub(2));
    f.set_cursor_position((cursor_x, chunks[3].y + 1));
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    base_url: &str,
    session: &mut Session,
) -> Result<()> {
    let (tx, mut rx) = unbounded_channel::<AppMsg>();

    spawn_key_reader(tx.clone());
    spawn_ticker(tx.clone());
    spawn_sse_listener(
        tx.clone(),
        client.clone(),
        base_url.to_string(),
        session.jwt.clone(),
    );

    let mut appearance = Appearance::load();
    let companion = CompanionState::load(appearance.game_default);
    if let Some(path) = &companion.pet_path {
        let _ = appearance.use_pet(path);
    }
    let mut app = App {
        owner_id: session.owner_id.clone(),
        device_name: session.device_name.clone(),
        base_url: base_url.to_string(),
        lines: vec![Line::Event(
            "Bastion pronto. Conte o que precisa cuidar agora.".to_string(),
        )],
        input: String::new(),
        thinking: false,
        spinner_idx: 0,
        animation_tick: 0,
        visual_mode: VisualMode::Onboarding,
        settle_at: None,
        appearance,
        suggestion_idx: 0,
        companion,
    };

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let Some(msg) = rx.recv().await else {
            break;
        };
        match msg {
            AppMsg::Tick => {
                if app.appearance.animations {
                    app.animation_tick = app.animation_tick.wrapping_add(1);
                }
                if app.thinking {
                    app.spinner_idx = (app.spinner_idx + 1) % SPINNER.len();
                }
                if app
                    .settle_at
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    app.visual_mode = VisualMode::Guard;
                    app.settle_at = None;
                }
            }
            AppMsg::SseEvent(e) => {
                if let Some(mode) = visual::mode_for_event(&e) {
                    app.visual_mode = mode;
                }
                app.lines.push(Line::Event(e));
            }
            AppMsg::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(cue) = app.companion.record_activity() {
                    app.lines.push(Line::Event(care_cue(cue).to_string()));
                }
                let suggestions = command_matches(&app.input);
                let picked = suggestions
                    .get(app.suggestion_idx.min(suggestions.len().saturating_sub(1)))
                    .copied();
                match key.code {
                    // While the command menu is open, Up/Down navigate it instead of
                    // moving through transcript history (there is none to move through).
                    KeyCode::Up if !suggestions.is_empty() => {
                        app.suggestion_idx = if app.suggestion_idx == 0 {
                            suggestions.len() - 1
                        } else {
                            app.suggestion_idx - 1
                        };
                    }
                    KeyCode::Down if !suggestions.is_empty() => {
                        app.suggestion_idx = (app.suggestion_idx + 1) % suggestions.len();
                    }
                    // Tab always completes to the highlighted suggestion if the menu is open.
                    KeyCode::Tab if !suggestions.is_empty() => {
                        if let Some(cmd) = picked {
                            app.input = format!("{} ", cmd.name);
                            app.suggestion_idx = 0;
                        }
                    }
                    // Enter completes the suggestion UNLESS the input already exactly
                    // matches it (then the user finished typing args — send it).
                    KeyCode::Enter
                        if !suggestions.is_empty()
                            && picked.is_some_and(|c| c.name != app.input) =>
                    {
                        if let Some(cmd) = picked {
                            app.input = format!("{} ", cmd.name);
                            app.suggestion_idx = 0;
                        }
                    }
                    KeyCode::Enter => {
                        let text = app.input.trim().to_string();
                        if !text.is_empty() {
                            app.input.clear();
                            app.suggestion_idx = 0;
                            app.lines.push(Line::You(text.clone()));
                            if let Some((response, mode)) = companion_command(&mut app, &text) {
                                app.visual_mode = mode;
                                app.settle_at = Some(Instant::now() + settle_after(mode));
                                app.lines.push(Line::Bastion(response));
                                continue;
                            }
                            if let Some(response) = theme_command(&mut app, &text) {
                                app.visual_mode = VisualMode::Success;
                                app.settle_at = Some(Instant::now() + Duration::from_millis(1400));
                                app.lines.push(Line::Bastion(response));
                                continue;
                            }
                            if let Some(unknown) = unknown_command(&text) {
                                app.visual_mode = VisualMode::Unknown;
                                app.settle_at = Some(Instant::now() + settle_after(VisualMode::Unknown));
                                app.lines.push(Line::Bastion(unknown));
                                continue;
                            }
                            app.thinking = true;
                            app.visual_mode = visual::mode_for_request(&text);
                            app.settle_at = None;
                            spawn_turn(
                                tx.clone(),
                                client.clone(),
                                base_url.to_string(),
                                session.jwt.clone(),
                                text,
                            );
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.input.clear();
                        app.suggestion_idx = 0;
                    }
                    KeyCode::Esc => break,
                    KeyCode::Backspace => {
                        app.input.pop();
                        app.suggestion_idx = 0;
                    }
                    KeyCode::Char(c) => {
                        app.input.push(c);
                        app.suggestion_idx = 0;
                    }
                    _ => {}
                }
            }
            AppMsg::Key(_) => {}
            AppMsg::Turn(outcome) => {
                app.thinking = false;
                match outcome {
                    TurnOutcome::Reply(r) => {
                        let completed_mode = app.visual_mode;
                        if let Some(level) = app.companion.award_success(completed_mode) {
                            app.lines.push(Line::Event(format!(
                                "Companion chegou ao nível {level}. Nenhuma capability ou permissão mudou."
                            )));
                        }
                        if let Err(error) = app.companion.save() {
                            app.lines.push(Line::Event(format!(
                                "Progresso do companion não pôde ser salvo: {error}"
                            )));
                        }
                        app.visual_mode = VisualMode::Success;
                        app.settle_at = Some(Instant::now() + Duration::from_millis(1400));
                        app.lines.push(Line::Bastion(r));
                    }
                    TurnOutcome::Error(e) => {
                        app.visual_mode = VisualMode::Alert;
                        app.settle_at = None;
                        app.lines.push(Line::System(format!("erro: {e}")));
                    }
                    TurnOutcome::Unauthorized => {
                        app.visual_mode = VisualMode::Alert;
                        app.lines.push(Line::System(
                            "sessão expirada — repareando (tela suspensa)…".into(),
                        ));
                        terminal.draw(|f| draw(f, &app))?;
                        clear_session();
                        disable_raw_mode()?;
                        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                        let new_session = match local_bootstrap_token(base_url) {
                            Some(token) => token_session(&token, &session.owner_id),
                            None if is_local_url(base_url) => bail!(
                                "sessão local expirou e BASTION_BOOTSTRAP_TOKEN não está disponível"
                            ),
                            None => pair(client, base_url).await?,
                        };
                        *session = new_session;
                        enable_raw_mode()?;
                        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                        app.owner_id = session.owner_id.clone();
                        app.device_name = session.device_name.clone();
                        app.lines.push(Line::System(format!(
                            "repareado como '{}'.",
                            session.device_name
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

pub async fn run(url: &str, token: Option<&str>, owner: &str, auto_start: bool) -> Result<()> {
    let client = Client::new();
    ensure_runtime(&client, url, auto_start).await?;
    let saved_session = token.is_none().then(load_session).flatten();
    let bootstrap = token
        .is_none()
        .then(|| local_bootstrap_token(url))
        .flatten();

    let mut session = match (token, saved_session, bootstrap) {
        (Some(explicit), _, _) => token_session(explicit, owner),
        (None, Some(saved), _) => saved,
        (None, None, Some(local_token)) => token_session(&local_token, owner),
        (None, None, None) if is_local_url(url) => bail!(
            "runtime local pronto, mas BASTION_BOOTSTRAP_TOKEN não foi encontrado; execute a partir da instalação que contém `.env` ou passe --token"
        ),
        (None, None, None) => pair(&client, url).await?,
    };

    println!(
        "◈ Bastion conectado como '{}' em {}.",
        session.owner_id, url
    );

    install_panic_hook();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let result = run_app(&mut terminal, &client, url, &mut session).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_loopback_urls_as_local() {
        assert!(is_local_url("http://127.0.0.1:8080"));
        assert!(is_local_url("http://localhost:8080/"));
        assert!(is_local_url("http://[::1]:8080"));
        assert!(!is_local_url("https://bastion.example.com"));
        assert!(!is_local_url("not-a-url"));
    }

    #[tokio::test]
    async fn readiness_probe_accepts_ready_runtime() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/readyz",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        assert!(runtime_ready(&Client::new(), &format!("http://{address}"),).await);
        server.abort();
    }

    #[test]
    fn bootstrap_is_never_used_for_remote_targets() {
        assert_eq!(local_bootstrap_token("https://bastion.example.com"), None);
    }

    #[test]
    fn pet_subcommands_stay_visible_after_the_space() {
        let names: Vec<&str> = command_matches("/pet ").iter().map(|c| c.name).collect();
        assert_eq!(names.len(), PET_COMMANDS.len());
        assert!(names.contains(&"/pet stats"));

        let game: Vec<&str> = command_matches("/pet game")
            .iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(game, vec!["/pet game on", "/pet game off"]);

        // Comando completado (com espaço final) fecha o menu para o Enter enviar.
        assert!(command_matches("/pet stats ").is_empty());
        // Outros comandos mantêm o comportamento antigo: espaço fecha o menu.
        assert!(command_matches("/model ").is_empty());
        assert!(!command_matches("/pe").is_empty());
    }

    #[test]
    fn theme_menu_expands_and_unknown_commands_answer_locally() {
        let themes: Vec<&str> = command_matches("/theme ").iter().map(|c| c.name).collect();
        assert_eq!(themes.len(), THEME_COMMANDS.len());
        assert!(themes.contains(&"/theme rgb"));
        assert_eq!(
            command_matches("/theme m").iter().map(|c| c.name).collect::<Vec<_>>(),
            vec!["/theme magenta", "/theme mono"]
        );

        assert!(unknown_command("/naoexiste").is_some());
        assert!(unknown_command("/pet feed").is_none());
        assert!(unknown_command("/theme rgb").is_none());
        assert!(unknown_command("oi bastion").is_none());
    }

    #[test]
    fn compose_search_walks_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(temp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert_eq!(find_compose_dir(&nested), Some(temp.path().to_path_buf()));
    }
}
