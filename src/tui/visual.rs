use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use serde::Deserialize;

const RGB: [Color; 6] = [
    Color::Rgb(0, 229, 255),
    Color::Rgb(88, 157, 246),
    Color::Rgb(188, 118, 255),
    Color::Rgb(255, 61, 242),
    Color::Rgb(255, 184, 77),
    Color::Rgb(61, 255, 174),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VisualMode {
    Onboarding,
    Guard,
    Thinking,
    Build,
    Cabinet,
    Success,
    Alert,
}

impl VisualMode {
    fn label(self) -> &'static str {
        match self {
            Self::Onboarding => "ONBOARDING",
            Self::Guard => "GUARD",
            Self::Thinking => "THINKING",
            Self::Build => "BUILDING",
            Self::Cabinet => "CABINET",
            Self::Success => "CONFIRMED",
            Self::Alert => "ATTENTION",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Theme {
    Rgb,
    Cyan,
    Blue,
    Magenta,
    Amber,
    Green,
    Mono,
}

#[derive(Clone, Debug)]
pub(super) struct Appearance {
    theme: Theme,
    accent: Option<Color>,
    pub(super) mascot: bool,
    pub(super) animations: bool,
    pub(super) game_default: bool,
    pub(super) pet: Option<PetPack>,
    no_color: bool,
}

#[derive(Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    tui: RawAppearance,
}

#[derive(Default, Deserialize)]
struct RawAppearance {
    theme: Option<String>,
    accent: Option<String>,
    mascot: Option<bool>,
    animations: Option<bool>,
    game: Option<bool>,
    pet: Option<String>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Rgb,
            accent: None,
            mascot: true,
            animations: true,
            game_default: false,
            pet: None,
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }
}

impl Appearance {
    pub(super) fn load() -> Self {
        let config_path =
            std::env::var("BASTION_CONFIG").unwrap_or_else(|_| "bastion.toml".to_string());
        let raw = std::fs::read_to_string(config_path)
            .ok()
            .and_then(|contents| toml::from_str::<FileConfig>(&contents).ok())
            .map(|config| config.tui)
            .unwrap_or_default();

        let mut appearance = Self::from_raw(raw);
        if let Ok(theme) = std::env::var("BASTION_TUI_THEME") {
            appearance.theme = parse_theme(&theme);
        }
        if let Ok(accent) = std::env::var("BASTION_TUI_ACCENT") {
            appearance.accent = parse_hex_color(&accent);
        }
        if let Ok(value) = std::env::var("BASTION_TUI_MASCOT") {
            appearance.mascot = parse_bool(&value).unwrap_or(appearance.mascot);
        }
        if let Ok(value) = std::env::var("BASTION_TUI_ANIMATIONS") {
            appearance.animations = parse_bool(&value).unwrap_or(appearance.animations);
        }
        if let Ok(value) = std::env::var("BASTION_TUI_GAME") {
            appearance.game_default = parse_bool(&value).unwrap_or(appearance.game_default);
        }
        if let Ok(path) = std::env::var("BASTION_TUI_PET") {
            appearance.pet = PetPack::load(std::path::Path::new(&path)).ok();
        }
        appearance.no_color = std::env::var_os("NO_COLOR").is_some();
        appearance
    }

    fn from_raw(raw: RawAppearance) -> Self {
        let pet = raw
            .pet
            .as_deref()
            .and_then(|path| PetPack::load(std::path::Path::new(path)).ok());
        Self {
            theme: raw.theme.as_deref().map(parse_theme).unwrap_or(Theme::Rgb),
            accent: raw.accent.as_deref().and_then(parse_hex_color),
            mascot: raw.mascot.unwrap_or(true),
            animations: raw.animations.unwrap_or(true),
            game_default: raw.game.unwrap_or(false),
            pet,
            no_color: std::env::var_os("NO_COLOR").is_some(),
        }
    }

    pub(super) fn use_pet(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        self.pet = Some(PetPack::load(path)?);
        Ok(())
    }

    pub(super) fn pet_label(&self) -> Option<String> {
        self.pet
            .as_ref()
            .map(|pet| format!("{} · {}", pet.name, pet.rarity.label()))
    }

    pub(super) fn accent(&self, mode: VisualMode) -> Color {
        if self.no_color {
            return Color::White;
        }
        if let Some(accent) = self.accent {
            return accent;
        }
        match self.theme {
            Theme::Rgb => match mode {
                VisualMode::Onboarding | VisualMode::Success => RGB[5],
                VisualMode::Guard | VisualMode::Thinking => RGB[0],
                VisualMode::Build => RGB[4],
                VisualMode::Cabinet => RGB[3],
                VisualMode::Alert => Color::LightRed,
            },
            Theme::Cyan => Color::Cyan,
            Theme::Blue => Color::Rgb(88, 157, 246),
            Theme::Magenta => Color::Magenta,
            Theme::Amber => Color::Rgb(255, 184, 77),
            Theme::Green => Color::Green,
            Theme::Mono => Color::Gray,
        }
    }

    pub(super) fn logo_color(&self, index: usize, tick: usize) -> Color {
        if self.no_color || self.theme == Theme::Mono {
            return Color::White;
        }
        if let Some(accent) = self.accent {
            return accent;
        }
        if self.theme == Theme::Rgb {
            let phase = if self.animations { tick / 4 } else { 0 };
            RGB[(index + phase) % RGB.len()]
        } else {
            self.accent(VisualMode::Guard)
        }
    }

    pub(super) fn muted(&self) -> Color {
        if self.no_color {
            Color::DarkGray
        } else {
            Color::Rgb(91, 108, 138)
        }
    }

    pub(super) fn text(&self) -> Color {
        if self.no_color {
            Color::Reset
        } else {
            Color::Rgb(218, 228, 248)
        }
    }

    pub(super) fn user(&self) -> Color {
        if self.no_color {
            Color::White
        } else if self.theme == Theme::Rgb && self.accent.is_none() {
            RGB[1]
        } else {
            self.accent(VisualMode::Guard)
        }
    }

    pub(super) fn warning(&self) -> Color {
        if self.no_color {
            Color::White
        } else {
            RGB[4]
        }
    }
}

pub(super) fn mode_for_request(input: &str) -> VisualMode {
    let normalized = input.to_lowercase();
    let cabinet = normalized.starts_with("/cabinet")
        || normalized.contains("modo cabinet")
        || normalized.contains("convoque o cabinet")
        || normalized.contains("reprioriz");
    if cabinet {
        return VisualMode::Cabinet;
    }

    const BUILD_SIGNALS: &[&str] = &[
        "código",
        "codigo",
        "code",
        "program",
        "implement",
        "refactor",
        "debug",
        "corrige",
        "conserta",
        "teste",
        "test",
        "commit",
        "arquivo",
        "function",
        "rust",
        "python",
        "typescript",
    ];
    if BUILD_SIGNALS
        .iter()
        .any(|signal| normalized.contains(signal))
    {
        VisualMode::Build
    } else {
        VisualMode::Thinking
    }
}

pub(super) fn mode_for_event(event: &str) -> Option<VisualMode> {
    let value: serde_json::Value = serde_json::from_str(event).ok()?;
    let kind = value
        .get("event")
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)?;
    match kind {
        "onboarding.started" => Some(VisualMode::Onboarding),
        "cabinet.started" => Some(VisualMode::Cabinet),
        "turn.started" => match value.get("mode").and_then(serde_json::Value::as_str) {
            Some("cabinet") => Some(VisualMode::Cabinet),
            Some("build" | "code") => Some(VisualMode::Build),
            _ => Some(VisualMode::Thinking),
        },
        "tool.started" => match value.get("category").and_then(serde_json::Value::as_str) {
            Some("code" | "filesystem" | "test") => Some(VisualMode::Build),
            _ => Some(VisualMode::Thinking),
        },
        "turn.completed" => Some(VisualMode::Success),
        "turn.failed" | "security.review" => Some(VisualMode::Alert),
        _ => None,
    }
}

pub(super) fn identity_height(area: Rect, appearance: &Appearance) -> u16 {
    if !appearance.mascot || area.width < 56 || area.height < 18 {
        1
    } else if area.width < 82 || area.height < 28 {
        6
    } else {
        9
    }
}

pub(super) struct Identity<'a> {
    pub(super) owner: &'a str,
    pub(super) device: &'a str,
    pub(super) runtime: &'a str,
    pub(super) mode: VisualMode,
    pub(super) tick: usize,
    pub(super) companion: Option<&'a str>,
}

pub(super) fn render_identity(
    frame: &mut Frame,
    area: Rect,
    appearance: &Appearance,
    identity: Identity<'_>,
) {
    if area.height == 1 {
        let mut header = vec![Span::styled(
            " ◈ ",
            Style::default()
                .fg(appearance.accent(identity.mode))
                .add_modifier(Modifier::BOLD),
        )];
        header.extend(logo_spans(appearance, identity.tick));
        header.push(Span::styled(
            format!(
                "  {} · {}@{} ",
                identity.mode.label(),
                identity.owner,
                identity.device
            ),
            Style::default().fg(appearance.muted()),
        ));
        frame.render_widget(Paragraph::new(Line::from(header)), area);
        return;
    }

    let accent = appearance.accent(identity.mode);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            format!(" {} ", identity.mode.label()),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(23), Constraint::Min(20)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(mascot_lines(
            identity.mode,
            identity.tick,
            appearance,
            area.height < 9,
        ))
        .alignment(Alignment::Center),
        columns[0],
    );

    let mut brand = vec![Span::styled("╺ ", Style::default().fg(appearance.muted()))];
    brand.extend(logo_spans(appearance, identity.tick));
    brand.push(Span::styled(" ╸", Style::default().fg(appearance.muted())));
    let mut info = vec![Line::from(brand)];
    if area.height >= 9 || identity.companion.is_none() {
        info.push(Line::styled(
            "LIFE OS // confiável por design",
            Style::default().fg(appearance.muted()),
        ));
    }
    info.push(Line::from(vec![
        Span::styled("● ", Style::default().fg(accent)),
        Span::styled(identity.mode.label(), Style::default().fg(accent)),
        Span::styled(
            format!("  {}@{}", identity.owner, identity.device),
            Style::default().fg(appearance.text()),
        ),
    ]));
    if let Some(status) = identity.companion {
        info.push(Line::styled(
            status.to_string(),
            Style::default().fg(appearance.muted()),
        ));
    }
    if area.height >= 9 {
        info.push(Line::styled(
            format!("runtime  {}", identity.runtime),
            Style::default().fg(appearance.muted()),
        ));
    }
    frame.render_widget(Paragraph::new(info).alignment(Alignment::Left), columns[1]);
}

fn logo_spans(appearance: &Appearance, tick: usize) -> Vec<Span<'static>> {
    let mut rendered = Vec::new();
    for (index, letter) in "BASTION".chars().enumerate() {
        if index > 0 {
            rendered.push(Span::raw(" "));
        }
        rendered.push(Span::styled(
            letter.to_string(),
            Style::default()
                .fg(appearance.logo_color(index, tick))
                .add_modifier(Modifier::BOLD),
        ));
    }
    rendered
}

fn mascot_lines(
    mode: VisualMode,
    tick: usize,
    appearance: &Appearance,
    compact: bool,
) -> Vec<Line<'static>> {
    if let Some(pack) = &appearance.pet {
        return custom_pet_lines(pack, mode, tick, appearance, compact);
    }
    match mode {
        VisualMode::Onboarding => familiar_lines(tick, appearance, compact),
        VisualMode::Build | VisualMode::Cabinet => patchwork_lines(mode, tick, appearance, compact),
        _ => keeper_lines(mode, tick, appearance, compact),
    }
}

fn custom_pet_lines(
    pack: &PetPack,
    mode: VisualMode,
    tick: usize,
    appearance: &Appearance,
    compact: bool,
) -> Vec<Line<'static>> {
    let animation = pack.animation(mode);
    let elapsed_ms = tick.saturating_mul(90) as u64;
    let frame_index = ((elapsed_ms / animation.interval_ms) as usize) % animation.frames.len();
    let limit = if compact { 4 } else { 6 };
    animation.frames[frame_index]
        .iter()
        .take(limit)
        .map(|line| render_pet_line(line, pack, appearance, mode))
        .collect()
}

fn render_pet_line(
    line: &str,
    pack: &PetPack,
    appearance: &Appearance,
    mode: VisualMode,
) -> Line<'static> {
    let primary = appearance.accent.unwrap_or_else(|| {
        pack.palette
            .primary
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or_else(|| appearance.accent(mode))
    });
    let secondary = pack
        .palette
        .secondary
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or_else(|| appearance.user());
    let mut spans = Vec::new();
    let mut color = primary;
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        if start > 0 {
            spans.push(Span::styled(
                rest[..start].to_string(),
                Style::default().fg(color),
            ));
        }
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            spans.push(Span::styled(
                rest[start..].to_string(),
                Style::default().fg(color),
            ));
            return Line::from(spans);
        };
        color = if appearance.no_color {
            Color::White
        } else {
            match &after[..end] {
                "primary" => primary,
                "secondary" => secondary,
                "cyan" => RGB[0],
                "blue" => RGB[1],
                "magenta" => RGB[3],
                "amber" => RGB[4],
                "green" => RGB[5],
                "red" => Color::LightRed,
                "muted" => appearance.muted(),
                "reset" => appearance.text(),
                _ => primary,
            }
        };
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), Style::default().fg(color)));
    }
    Line::from(spans)
}

fn keeper_lines(
    mode: VisualMode,
    tick: usize,
    appearance: &Appearance,
    compact: bool,
) -> Vec<Line<'static>> {
    let accent = appearance.accent(mode);
    let eyes = if appearance.animations && tick.is_multiple_of(32) {
        "─  ─"
    } else {
        "▪  ▪"
    };
    let mouth = if mode == VisualMode::Success {
        " ︶ "
    } else {
        " ── "
    };
    let mut lines = vec![
        Line::styled("   ▄██████▄    ◢◣", Style::default().fg(accent)),
        Line::styled(format!("  █  {eyes}  █   █▌"), Style::default().fg(accent)),
        Line::styled(format!("  █  {mouth}  █   █▌"), Style::default().fg(accent)),
        Line::styled("   ▀██████▀    ▼ ", Style::default().fg(accent)),
    ];
    if !compact {
        lines.insert(0, Line::raw(""));
        lines.push(Line::styled(
            "    KEEPER",
            Style::default().fg(appearance.muted()),
        ));
    }
    lines
}

fn familiar_lines(tick: usize, appearance: &Appearance, compact: bool) -> Vec<Line<'static>> {
    let blue = if appearance.accent.is_some() {
        appearance.accent(VisualMode::Onboarding)
    } else {
        RGB[1]
    };
    let green = if appearance.no_color {
        Color::White
    } else {
        RGB[5]
    };
    let cursor = if !appearance.animations || (tick / 5).is_multiple_of(2) {
        "▮"
    } else {
        " "
    };
    let mut lines = vec![
        Line::styled("  ╭─╮ ╭─╮ ╭─╮", Style::default().fg(blue)),
        Line::styled("  │   ▪   ▪   │", Style::default().fg(green)),
        Line::styled(
            format!("  │ bastion:~{cursor} │"),
            Style::default().fg(green),
        ),
        Line::styled("  ╰────────────╯", Style::default().fg(blue)),
    ];
    if !compact {
        lines.insert(0, Line::raw(""));
        lines.push(Line::styled(
            "    FAMILIAR",
            Style::default().fg(appearance.muted()),
        ));
    }
    lines
}

fn patchwork_lines(
    mode: VisualMode,
    tick: usize,
    appearance: &Appearance,
    compact: bool,
) -> Vec<Line<'static>> {
    let accent = appearance.accent(mode);
    let colors = if appearance.no_color {
        [Color::White; 4]
    } else {
        [RGB[3], RGB[4], RGB[5], RGB[0]]
    };
    let shift = if appearance.animations {
        (tick / 6) % 4
    } else {
        0
    };
    let icon = |index: usize, text: &'static str| {
        Span::styled(text, Style::default().fg(colors[(index + shift) % 4]))
    };
    let mut lines = vec![
        Line::from(vec![icon(0, "  ♥"), Span::raw("      "), icon(1, "▣")]),
        Line::styled("      ▄████▄", Style::default().fg(accent)),
        Line::styled("     █ ▪  ▪ █", Style::default().fg(accent)),
        Line::from(vec![
            icon(2, "  ▰  "),
            Span::styled("▀████▀", Style::default().fg(accent)),
            Span::raw("  "),
            icon(3, "♟"),
        ]),
    ];
    if !compact {
        lines.insert(0, Line::raw(""));
        lines.push(Line::styled(
            if mode == VisualMode::Cabinet {
                "    CABINET"
            } else {
                "    PATCHWORK"
            },
            Style::default().fg(appearance.muted()),
        ));
    }
    lines
}

fn parse_theme(value: &str) -> Theme {
    match value.trim().to_ascii_lowercase().as_str() {
        "cyan" => Theme::Cyan,
        "blue" => Theme::Blue,
        "magenta" | "pink" => Theme::Magenta,
        "amber" | "yellow" => Theme::Amber,
        "green" | "matrix" => Theme::Green,
        "mono" | "monochrome" => Theme::Mono,
        _ => Theme::Rgb,
    }
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if value.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&value[0..2], 16).ok()?;
    let green = u8::from_str_radix(&value[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_cabinet_before_build_signals() {
        assert_eq!(
            mode_for_request("repriorize meu projeto no modo cabinet"),
            VisualMode::Cabinet
        );
        assert_eq!(mode_for_request("refatore este código"), VisualMode::Build);
        assert_eq!(mode_for_request("como está meu dia?"), VisualMode::Thinking);
    }

    #[test]
    fn consumes_structured_runtime_modes_without_guessing() {
        assert_eq!(
            mode_for_event(r#"{"event":"tool.started","category":"code"}"#),
            Some(VisualMode::Build)
        );
        assert_eq!(
            mode_for_event(r#"{"event":"cabinet.started"}"#),
            Some(VisualMode::Cabinet)
        );
        assert_eq!(mode_for_event("mesh_sync"), None);
    }

    #[test]
    fn parses_theme_and_custom_accent() {
        assert_eq!(parse_theme("amber"), Theme::Amber);
        assert_eq!(parse_theme("unknown"), Theme::Rgb);
        assert_eq!(parse_hex_color("#12abEF"), Some(Color::Rgb(18, 171, 239)));
        assert_eq!(parse_hex_color("nope"), None);
    }

    #[test]
    fn mascot_collapses_when_space_is_tight() {
        let appearance = Appearance {
            no_color: false,
            ..Appearance::default()
        };
        assert_eq!(identity_height(Rect::new(0, 0, 100, 40), &appearance), 9);
        assert_eq!(identity_height(Rect::new(0, 0, 70, 24), &appearance), 6);
        assert_eq!(identity_height(Rect::new(0, 0, 50, 40), &appearance), 1);
    }

    #[test]
    fn built_in_family_changes_with_runtime_mode() {
        let appearance = Appearance {
            no_color: false,
            ..Appearance::default()
        };
        let onboarding = mascot_lines(VisualMode::Onboarding, 1, &appearance, false);
        let guard = mascot_lines(VisualMode::Guard, 1, &appearance, false);
        let build = mascot_lines(VisualMode::Build, 1, &appearance, false);
        assert!(line_text(&onboarding).contains("FAMILIAR"));
        assert!(line_text(&guard).contains("KEEPER"));
        assert!(line_text(&build).contains("PATCHWORK"));
    }

    fn line_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
use super::companion::PetPack;
