//! Terminal UI client for the Bastion webhook + SSE API — the same surface the
//! mobile companion app pairs with (`/auth/exchange`, `/webhook`, `/events`).
//!
//! Pairing: on the machine running the daemon, type `/connect-app <device-name>`
//! in its interactive console to mint a one-time code, then paste it here.
//!
//! Known gap (2026-07-02): `/events` today only broadcasts `mesh_sync` messages
//! (`src/mesh/p2p.rs`) — there is no per-turn tool-call/progress event yet, so a
//! turn is a blocking `POST /webhook` with an animated spinner, not a live trace
//! of tool calls. The SSE panel is wired up so it starts working the day that lands.

use anyhow::{bail, Context, Result};
use clap::Parser;
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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

#[derive(Parser)]
struct Args {
    /// Base URL of the Bastion daemon's webhook server.
    #[arg(long, env = "POKEDEV_URL", default_value = "http://127.0.0.1:8080")]
    url: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct Session {
    jwt: String,
    device_name: String,
}

#[derive(Deserialize)]
struct WebhookOut {
    reply: String,
}

fn token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".pokedev-cli")
        .join("session.json")
}

fn load_session() -> Option<Session> {
    let raw = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_session(session: &Session) -> Result<()> {
    let path = token_path();
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir).context("creating ~/.pokedev-cli")?;
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

/// Plain-terminal pairing prompt — runs BEFORE raw mode / the alternate screen
/// are entered (and again, suspended back to plain mode, if a session expires
/// mid-run), so it stays simple stdin/stdout instead of an in-TUI text field.
async fn pair(client: &Client, base_url: &str) -> Result<Session> {
    println!("👾 Nenhuma sessão pareada encontrada.");
    println!("Na máquina onde o daemon está rodando, digite: /connect-app pokedev-cli");
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
    println!("Pareado como '{}'. 👾\n", session.device_name);
    Ok(session)
}

/// One transcript entry.
enum Line {
    You(String),
    Bastion(String),
    Event(String),
    System(String),
    /// Boxed welcome banner (mascot + brand + hints) — content rows only, the
    /// box border/width is computed at render time from these strings.
    Banner(Vec<String>),
}

/// Builds the boxed welcome banner shown once at the top of a session.
fn welcome_banner(device_name: &str, base_url: &str) -> Vec<String> {
    vec![
        "👾 Pokédev — companheiro de bolso do dev".to_string(),
        String::new(),
        format!("pareado como '{device_name}'"),
        format!("daemon: {base_url}"),
        String::new(),
        "Enter envia · Esc/Ctrl+C sai · Ctrl+U limpa a linha".to_string(),
    ]
}

/// Draws `content` inside a Unicode box, using display width (not char count)
/// so the border stays straight even with the wide mascot emoji in the title.
fn render_banner(content: &[String]) -> Vec<RLine<'static>> {
    use unicode_width::UnicodeWidthStr;

    let inner_width = content.iter().map(|l| l.width()).max().unwrap_or(0);
    let accent = Style::default().fg(Color::Magenta);

    let mut out = vec![RLine::styled(
        format!("╭{}╮", "─".repeat(inner_width + 2)),
        accent,
    )];
    for (i, l) in content.iter().enumerate() {
        let pad = " ".repeat(inner_width - l.width());
        let style = if i == 0 {
            accent.add_modifier(Modifier::BOLD)
        } else if l.is_empty() {
            Style::default()
        } else if l.starts_with("Enter") {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };
        out.push(RLine::from(vec![
            Span::styled("│ ", accent),
            Span::styled(format!("{l}{pad}"), style),
            Span::styled(" │", accent),
        ]));
    }
    out.push(RLine::styled(
        format!("╰{}╯", "─".repeat(inner_width + 2)),
        accent,
    ));
    out.push(RLine::raw(""));
    out
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

/// Matches only while typing the command token itself (no space yet) — once
/// there's a space the user is typing arguments, so suggestions disappear.
fn command_matches(input: &str) -> Vec<&'static CommandInfo> {
    if input.is_empty() || !input.starts_with('/') || input.contains(' ') {
        return vec![];
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(input))
        .collect()
}

struct App {
    device_name: String,
    base_url: String,
    lines: Vec<Line>,
    input: String,
    thinking: bool,
    spinner_idx: usize,
    /// Index into the live `command_matches(&input)` list — reset to 0 whenever
    /// the input text changes so it never points past the new match set.
    suggestion_idx: usize,
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(suggestion_height(&app.input)),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(RLine::from(vec![
        Span::styled(
            " 👾 Pokédev ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {} · {} ", app.device_name, app.base_url)),
    ]));
    f.render_widget(header, chunks[0]);

    let transcript_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
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
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(t.clone()),
                ]));
                text_lines.push(RLine::raw(""));
            }
            Line::Bastion(t) => {
                text_lines.push(RLine::from(vec![
                    Span::styled(
                        "👾 Pokédev  ",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(t.clone()),
                ]));
                text_lines.push(RLine::raw(""));
            }
            Line::Event(t) => {
                text_lines.push(RLine::styled(
                    format!("· {t}"),
                    Style::default().fg(Color::DarkGray),
                ));
                text_lines.push(RLine::raw(""));
            }
            Line::System(t) => {
                text_lines.push(RLine::styled(
                    format!("⚠ {t}"),
                    Style::default().fg(Color::Yellow),
                ));
                text_lines.push(RLine::raw(""));
            }
            Line::Banner(content) => text_lines.extend(render_banner(content)),
        }
    }
    if app.thinking {
        let frame = SPINNER[app.spinner_idx % SPINNER.len()];
        text_lines.push(RLine::styled(
            format!("{frame} pensando…"),
            Style::default().fg(Color::DarkGray),
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
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let marker = if i == selected { "▸ " } else { "  " };
                let tag = if c.remote { "" } else { " [console]" };
                RLine::from(vec![
                    Span::styled(format!("{marker}{:<22}", c.usage), style),
                    Span::styled(format!(" {}", c.desc), Style::default().fg(Color::DarkGray)),
                    Span::styled(tag, Style::default().fg(Color::Yellow)),
                ])
            })
            .collect();
        let suggestion_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" ↑↓ escolhe · Tab/Enter completa ");
        f.render_widget(Paragraph::new(items).block(suggestion_block), chunks[2]);
    }

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" mensagem — Enter envia · Esc/Ctrl+C sai · Ctrl+U limpa ");
    let input = Paragraph::new(app.input.as_str())
        .style(Style::default().fg(Color::White))
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

    let mut app = App {
        device_name: session.device_name.clone(),
        base_url: base_url.to_string(),
        lines: vec![Line::Banner(welcome_banner(&session.device_name, base_url))],
        input: String::new(),
        thinking: false,
        spinner_idx: 0,
        suggestion_idx: 0,
    };

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let Some(msg) = rx.recv().await else {
            break;
        };
        match msg {
            AppMsg::Tick => {
                if app.thinking {
                    app.spinner_idx = (app.spinner_idx + 1) % SPINNER.len();
                }
            }
            AppMsg::SseEvent(e) => app.lines.push(Line::Event(e)),
            AppMsg::Key(key) if key.kind == KeyEventKind::Press => {
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
                            app.thinking = true;
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
                    TurnOutcome::Reply(r) => app.lines.push(Line::Bastion(r)),
                    TurnOutcome::Error(e) => app.lines.push(Line::System(format!("erro: {e}"))),
                    TurnOutcome::Unauthorized => {
                        app.lines.push(Line::System(
                            "sessão expirada — repareando (tela suspensa)…".into(),
                        ));
                        terminal.draw(|f| draw(f, &app))?;
                        clear_session();
                        disable_raw_mode()?;
                        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                        let new_session = pair(client, base_url).await?;
                        *session = new_session;
                        enable_raw_mode()?;
                        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::new();

    let mut session = match load_session() {
        Some(s) => s,
        None => pair(&client, &args.url).await?,
    };

    println!(
        "👾 Pareado como '{}' em {}. Entrando no Pokédev…",
        session.device_name, args.url
    );

    install_panic_hook();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let result = run_app(&mut terminal, &client, &args.url, &mut session).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
