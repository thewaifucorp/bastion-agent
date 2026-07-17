use anyhow::{bail, Context};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use super::companion::PetPack;

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
    /// Slash command that does not exist — the Keeper is in doubt (open eyes,
    /// not happy ones) with an amber question mark.
    Unknown,
    /// `/pet sleep` / end of a long session — resting face with zzz.
    Sleep,
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
            Self::Unknown => "UNKNOWN",
            Self::Sleep => "RESTING",
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

pub(super) const THEME_NAMES: &[&str] =
    &["rgb", "cyan", "blue", "magenta", "amber", "green", "mono"];

/// Theme preferences persisted by `/theme` — they live outside
/// `bastion.toml` so the command can save without rewriting project config.
/// Precedence: bastion.toml < tui.json < environment variables.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ThemePrefs {
    theme: Option<String>,
    accent: Option<String>,
}

fn theme_prefs_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("bastion")
        .join("tui.json")
}

impl ThemePrefs {
    fn load() -> Self {
        std::fs::read_to_string(theme_prefs_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> anyhow::Result<()> {
        let path = theme_prefs_path();
        let parent = path
            .parent()
            .context("theme prefs without a parent directory")?;
        super::ensure_private_dir(parent)?;
        super::write_private_file(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

#[derive(Clone, Debug)]
pub(super) struct Appearance {
    theme: Theme,
    accent: Option<Color>,
    pub(super) mascot: bool,
    pub(super) animations: bool,
    pub(super) game_default: bool,
    pub(super) pet: Option<PetPack>,
    /// True when the terminal answered the graphics-protocol query at startup
    /// (sixel/kitty/iTerm2) — the native mascot then renders as a real image.
    pub(super) graphics: bool,
    no_color: bool,
    prefs: ThemePrefs,
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
            graphics: false,
            no_color: std::env::var_os("NO_COLOR").is_some(),
            prefs: ThemePrefs::default(),
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
        let prefs = ThemePrefs::load();
        if let Some(name) = &prefs.theme {
            appearance.theme = parse_theme(name);
        }
        if let Some(hex) = &prefs.accent {
            appearance.accent = parse_hex_color(hex);
        }
        appearance.prefs = prefs;
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
            graphics: false,
            no_color: std::env::var_os("NO_COLOR").is_some(),
            prefs: ThemePrefs::default(),
        }
    }

    /// Applies a named theme instantly and persists it in `tui.json`. A named
    /// theme clears the custom accent — otherwise the accent would keep
    /// masking the switch.
    pub(super) fn apply_theme(&mut self, name: &str) -> anyhow::Result<()> {
        let name = name.trim().to_ascii_lowercase();
        if !THEME_NAMES.contains(&name.as_str()) {
            bail!("unknown theme '{name}'; use {}", THEME_NAMES.join("|"));
        }
        self.theme = parse_theme(&name);
        self.accent = None;
        self.prefs.theme = Some(name);
        self.prefs.accent = None;
        self.prefs.save()
    }

    /// Applies a `#RRGGBB` accent color instantly and persists it in `tui.json`.
    pub(super) fn apply_accent(&mut self, hex: &str) -> anyhow::Result<()> {
        let Some(color) = parse_hex_color(hex) else {
            bail!("invalid color '{hex}'; use #RRGGBB");
        };
        self.accent = Some(color);
        self.prefs.accent = Some(hex.trim().to_string());
        self.prefs.save()
    }

    pub(super) fn theme_status(&self) -> String {
        let theme = self
            .prefs
            .theme
            .clone()
            .unwrap_or_else(|| format!("{:?}", self.theme).to_ascii_lowercase());
        match &self.prefs.accent {
            Some(accent) => format!("theme {theme} · accent {accent}"),
            None => format!("theme {theme}"),
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
                VisualMode::Build | VisualMode::Unknown => RGB[4],
                VisualMode::Cabinet => RGB[3],
                VisualMode::Alert => Color::LightRed,
                VisualMode::Sleep => RGB[1],
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
    } else if appearance.pet.is_some() {
        // Pet packs draw text frames of up to 6 lines — they keep the older
        // panel and collapse on tight terminals.
        if area.width < 82 || area.height < 28 {
            6
        } else {
            9
        }
    } else if appearance.graphics {
        // Real pixels via sixel/kitty/iTerm2: small panel, one size only.
        7
    } else {
        // Glyph-face text fallback: 3 lines full, eye line only when tight.
        if area.width < 82 || area.height < 28 {
            4
        } else {
            5
        }
    }
}

pub(super) struct Identity<'a> {
    pub(super) owner: &'a str,
    pub(super) device: &'a str,
    pub(super) runtime: &'a str,
    pub(super) mode: VisualMode,
    pub(super) tick: usize,
    pub(super) companion: Option<&'a str>,
    /// Present when the terminal supports a graphics protocol: the mascot is
    /// drawn as a real image instead of text cells.
    pub(super) sprite: Option<&'a mut ratatui_image::protocol::StatefulProtocol>,
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

    let mascot_width = if identity.sprite.is_some() {
        18
    } else if appearance.pet.is_some() {
        23
    } else {
        // Glyph face: 9-cell box + fixed seal slot.
        14
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(mascot_width), Constraint::Min(20)])
        .split(inner);
    match identity.sprite {
        Some(protocol) => {
            let image = ratatui_image::StatefulImage::new().resize(ratatui_image::Resize::Fit(
                Some(image::imageops::FilterType::Nearest),
            ));
            frame.render_stateful_widget(image, columns[0].inner(Margin::new(1, 0)), protocol);
        }
        None => frame.render_widget(
            Paragraph::new(mascot_lines(
                identity.mode,
                identity.tick,
                appearance,
                area.height < 5,
            ))
            .alignment(Alignment::Center),
            columns[0],
        ),
    }

    let mut brand = vec![Span::styled("╺ ", Style::default().fg(appearance.muted()))];
    brand.extend(logo_spans(appearance, identity.tick));
    brand.push(Span::styled(" ╸", Style::default().fg(appearance.muted())));
    let mut info = vec![Line::from(brand)];
    if area.height >= 9 || identity.companion.is_none() {
        info.push(Line::styled(
            "LIFE OS // trustworthy by design",
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
    glyph_face(
        mode,
        appearance.animations && tick % 32 < 3,
        appearance,
        compact,
    )
}

/// Text fallback (no graphics protocol): a minimal glyph face — real font
/// glyphs render crisp at any size, unlike block-cell pixel art, which reads
/// as a wall of squares at text resolution. Eyes only, no mouth; the state
/// seal sits in a fixed slot on the right so nothing ever shifts.
fn glyph_face(
    mode: VisualMode,
    blink: bool,
    appearance: &Appearance,
    compact: bool,
) -> Vec<Line<'static>> {
    let steel = Color::Rgb(142, 163, 186);
    let blue = Color::Rgb(79, 141, 253);
    let cyan = Color::Rgb(56, 217, 245);
    let green = Color::Rgb(62, 224, 140);
    let magenta = Color::Rgb(232, 107, 240);
    let amber = Color::Rgb(245, 177, 61);
    let white = Color::Rgb(232, 242, 252);

    // ASCII-only face: geometric glyphs (◉ ◠ ○ …) are East-Asian-ambiguous
    // width — some terminals (Zed) draw them 2 cells wide while ratatui
    // counts 1, shearing the frame. ASCII and block elements are safe.
    let (eyes, face, seal, seal_color): (&str, Color, &str, Color) = match mode {
        VisualMode::Onboarding | VisualMode::Guard => {
            (if blink { "-   -" } else { "^   ^" }, cyan, " ", green)
        }
        VisualMode::Thinking | VisualMode::Build => ("o   o", cyan, ".", cyan),
        VisualMode::Success => ("^   ^", green, "*", green),
        VisualMode::Alert => ("x   x", magenta, "!", magenta),
        VisualMode::Unknown => ("o   o", amber, "?", amber),
        VisualMode::Sleep => ("-   -", blue, "z", cyan),
        VisualMode::Cabinet => ("^   ^", white, "+", magenta),
    };
    // Cabinet is Patchwork: frame stitched blue|green instead of steel.
    let (frame_left, frame_right) = if mode == VisualMode::Cabinet {
        (blue, green)
    } else {
        (steel, steel)
    };

    let style = |color: Color| {
        if appearance.no_color {
            Style::default()
        } else {
            Style::default().fg(color)
        }
    };
    let eye_line = Line::from(vec![
        Span::styled("▌ ", style(frame_left)),
        Span::styled(eyes.to_string(), style(face).add_modifier(Modifier::BOLD)),
        Span::styled(" ▐ ", style(frame_right)),
        Span::styled(
            seal.to_string(),
            style(seal_color).add_modifier(Modifier::BOLD),
        ),
    ]);
    if compact {
        return vec![eye_line];
    }
    // Every line must have the SAME width (the eye line carries a 2-cell seal
    // slot) — the paragraph is center-aligned, so unequal widths shift the
    // middle line sideways and tear the frame apart.
    let cap = |glyph: char| {
        Line::from(vec![
            Span::styled(
                format!(" {}", glyph.to_string().repeat(4)),
                style(frame_left),
            ),
            Span::styled(
                format!("{} ", glyph.to_string().repeat(3)),
                style(frame_right),
            ),
            Span::raw("  "),
        ])
    };
    vec![cap('▄'), eye_line, cap('▀')]
}

// Reaction seals rendered BESIDE the head (right side) — never on top, so
// they are always visible, including in the compact eye-band crop.
const SEAL_QUESTION: [&str; 7] = [
    ".aaa.", "aa.aa", "...aa", "..aa.", "..a..", ".....", "..a..",
];
const SEAL_BANG: [&str; 6] = ["mm", "mm", "mm", "mm", "..", "mm"];
const SEAL_ZZZ: [&str; 5] = ["ccccc", "...c.", "..c..", ".c...", "ccccc"];
const SEAL_DOTS: [&str; 1] = ["c.c.c"];
const SEAL_SHIELD: [&str; 6] = ["ggggg", "ggggg", "ggwgg", "ggggg", ".ggg.", "..g.."];
const SEAL_CHECK: [&str; 4] = ["....g", "...gg", "g.gg.", ".gg.."];

/// Every state reserves the same seal slot so the head NEVER shifts sideways
/// when a seal appears or changes — and the protocol image keeps a constant
/// aspect ratio (constant head size under `Resize::Fit`).
const SEAL_SLOT: usize = 5;

fn seal_for(mode: VisualMode) -> Option<&'static [&'static str]> {
    match mode {
        VisualMode::Onboarding | VisualMode::Guard => Some(&SEAL_SHIELD),
        VisualMode::Success => Some(&SEAL_CHECK),
        VisualMode::Unknown => Some(&SEAL_QUESTION),
        VisualMode::Alert => Some(&SEAL_BANG),
        VisualMode::Sleep => Some(&SEAL_ZZZ),
        VisualMode::Thinking | VisualMode::Build => Some(&SEAL_DOTS),
        VisualMode::Cabinet => None, // Patchwork carries its own right-side marks
    }
}

/// Head + optional seal, side by side, padded to a fixed total width.
/// `seal_top` is the pixel row where the seal starts.
fn compose(head: &[&str], seal: Option<&[&str]>, seal_top: usize) -> Vec<String> {
    const GAP: usize = 1;
    let head_width = head
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let total = head_width + GAP + SEAL_SLOT;
    head.iter()
        .enumerate()
        .map(|(y, row)| {
            let mut line = format!("{row:<head_width$}");
            line.push_str(&".".repeat(GAP));
            if let Some(seal_row) =
                seal.and_then(|s| y.checked_sub(seal_top).and_then(|i| s.get(i)))
            {
                line.push_str(seal_row);
            }
            while line.chars().count() < total {
                line.push('.');
            }
            line
        })
        .collect()
}

/// Protocol sprite (24×15 head + seal) for terminals with real graphics. The
/// key names the sprite for protocol caches; blink swaps the guard frame.
pub(super) fn native_sprite(mode: VisualMode, blink: bool) -> (&'static str, Vec<String>) {
    let (key, head): (&'static str, &[&str]) = match mode {
        VisualMode::Onboarding | VisualMode::Guard => {
            if blink {
                ("guard-blink", &KEEPER_GUARD_BLINK)
            } else {
                ("guard", &KEEPER_GUARD)
            }
        }
        VisualMode::Thinking | VisualMode::Build => ("thinking", &KEEPER_THINKING),
        VisualMode::Cabinet => ("cabinet", &PATCHWORK_CABINET),
        VisualMode::Success => ("success", &KEEPER_SUCCESS),
        VisualMode::Alert => ("alert", &KEEPER_ERROR),
        VisualMode::Unknown => ("unknown", &KEEPER_UNKNOWN),
        VisualMode::Sleep => ("sleep", &KEEPER_SLEEP),
    };
    (key, compose(head, seal_for(mode), 3))
}

/// Native sprite palette — the approved mascot-test stroke: light steel,
/// blue frame, dark visor, and a face colored per state.
fn sprite_color(ch: char) -> Option<Color> {
    match ch {
        's' => Some(Color::Rgb(142, 163, 186)),
        'S' => Some(Color::Rgb(44, 62, 87)),
        'k' => Some(Color::Rgb(11, 22, 38)),
        'b' => Some(Color::Rgb(79, 141, 253)),
        'c' => Some(Color::Rgb(56, 217, 245)),
        'g' => Some(Color::Rgb(62, 224, 140)),
        'm' => Some(Color::Rgb(232, 107, 240)),
        'a' => Some(Color::Rgb(245, 177, 61)),
        'w' => Some(Color::Rgb(232, 242, 252)),
        _ => None,
    }
}

// Keeper head, 24×15 px. Each state swaps only the face (rows 5-9) and the
// seal in the top-right corner (?, !, z, …).
macro_rules! keeper {
    ($r0:literal, $r5:literal, $r6:literal, $r7:literal, $r8:literal, $r9:literal) => {
        [
            $r0,
            "..ssssssssssssssssssss..",
            ".ssssssssssssssssssssss.",
            ".ssbbbbbbbbbbbbbbbbbbss.",
            ".ssbkkkkkkkkkkkkkkkkbss.",
            $r5,
            $r6,
            $r7,
            $r8,
            $r9,
            ".ssbkkkkkkkkkkkkkkkkbss.",
            ".ssbbbbbbbbbbbbbbbbbbss.",
            ".ssssssssssssssssssssss.",
            "..ssssssssssssssssssss..",
            "....ssssssssssssssss....",
        ]
    };
}

const KEEPER_GUARD: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkkcckkkkkkcckkkbsss",
    "sssbkkckkckkkkckkckkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkkckkkkkkckkkkbsss",
    ".ssbkkkkkcccccckkkkkbss."
);

const KEEPER_GUARD_BLINK: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkcccckkkkcccckkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkkckkkkkkckkkkbsss",
    ".ssbkkkkkcccccckkkkkbss."
);

const KEEPER_THINKING: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkcckkkkkkkkcckkbsss",
    "sssbkkcckkkkkkkkcckkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    ".ssbkkkkkkccckkkkkkkbss."
);

const KEEPER_UNKNOWN: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkkaakkkkkkaakkkbsss",
    "sssbkkkaakkkkkkaakkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkkkkkaakkkkkkkbsss",
    ".ssbkkkkkkkaakkkkkkkbss."
);

const KEEPER_ERROR: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkmkmkkkkkkmkmkkbsss",
    "sssbkkkmkkkkkkkkmkkkbsss",
    "sssbkkmkmkkkkkkmkmkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    ".ssbkkkkkmmmmmmkkkkkbss."
);

const KEEPER_SUCCESS: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkkggkkkkkkggkkkbsss",
    "sssbkkgkkgkkkkgkkgkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkgkkkkkkkkgkkkbsss",
    ".ssbkkkkggggggggkkkkbss."
);

const KEEPER_SLEEP: [&str; 15] = keeper!(
    "....ssssssssssssssss....",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkbbbbkkkkbbbbkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    "sssbkkkkkkkkkkkkkkkkbsss",
    ".ssbkkkkkkkbbkkkkkkkbss."
);

// Patchwork, 24×12 px: square-eyed head + a domain heart beside it — shows
// up when the Cabinet is convened.
const PATCHWORK_CABINET: [&str; 12] = [
    "...ssssssssssss.........",
    "..ssssssssssssss..mm.mm.",
    ".sbbbbbbbbbbbbbbs.mmmmm.",
    ".sbkkkkkkkkkkkkbs..mmm..",
    ".sbkkcckkkkcckkbs...m...",
    ".sbkkcckkkkcckkbs.......",
    ".sbkkkkkkkkkkkkbs...aa..",
    ".sbkkkkcccckkkkbs..aaaa.",
    ".sbkkkkkkkkkkkkbs..a..a.",
    ".sbbbbbbbbbbbbbbs..aaaa.",
    "..ssssssssssssss........",
    "...ssssssssssss.........",
];

/// Renders a sprite map as a real RGBA image for terminals with a graphics
/// protocol (sixel/kitty/iTerm2) — the exact pixel-art from the mascot test,
/// no character-grid compromises. Upscaled ×8 with hard edges so any protocol
/// downscale keeps the pixels crisp. Empty pixels stay transparent.
pub(super) fn sprite_image(map: &[String]) -> image::DynamicImage {
    const SCALE: u32 = 8;
    let height = map.len() as u32;
    let width = map.iter().map(|row| row.chars().count()).max().unwrap_or(0) as u32;
    let mut img = image::RgbaImage::new(width * SCALE, height * SCALE);
    for (y, row) in map.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let Some(Color::Rgb(r, g, b)) = sprite_color(ch) else {
                continue;
            };
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    img.put_pixel(
                        x as u32 * SCALE + dx,
                        y as u32 * SCALE + dy,
                        image::Rgba([r, g, b, 255]),
                    );
                }
            }
        }
    }
    image::DynamicImage::ImageRgba8(img)
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
        assert_eq!(identity_height(Rect::new(0, 0, 100, 40), &appearance), 5);
        assert_eq!(identity_height(Rect::new(0, 0, 70, 24), &appearance), 4);
        assert_eq!(identity_height(Rect::new(0, 0, 50, 40), &appearance), 1);

        let graphics = Appearance {
            graphics: true,
            ..Appearance::default()
        };
        assert_eq!(identity_height(Rect::new(0, 0, 100, 40), &graphics), 7);
        assert_eq!(identity_height(Rect::new(0, 0, 70, 24), &graphics), 7);
    }

    const ALL_MODES: [VisualMode; 9] = [
        VisualMode::Onboarding,
        VisualMode::Guard,
        VisualMode::Thinking,
        VisualMode::Build,
        VisualMode::Cabinet,
        VisualMode::Success,
        VisualMode::Alert,
        VisualMode::Unknown,
        VisualMode::Sleep,
    ];

    #[test]
    fn built_in_sprites_fit_the_identity_panel() {
        let appearance = Appearance {
            no_color: false,
            ..Appearance::default()
        };
        for mode in ALL_MODES {
            let full = mascot_lines(mode, 1, &appearance, false);
            assert_eq!(full.len(), 3, "{mode:?}: glyph face is 3 lines");
            let compact = mascot_lines(mode, 1, &appearance, true);
            assert_eq!(compact.len(), 1, "{mode:?}: compact is the eye line");
            for line in full.iter().chain(compact.iter()) {
                let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(width <= 12, "{mode:?}: face too wide ({width})");
            }
        }
    }

    #[test]
    fn cabinet_uses_the_patchwork_sprite() {
        let appearance = Appearance {
            no_color: false,
            ..Appearance::default()
        };
        let guard = render_plain(VisualMode::Guard);
        let cabinet = render_plain(VisualMode::Cabinet);
        let unknown = render_plain(VisualMode::Unknown);
        assert_ne!(guard, cabinet);
        assert_ne!(guard, unknown);
        let _ = appearance;
    }

    #[test]
    fn sprite_images_have_constant_dimensions_across_states() {
        // Fixed seal slot: every Keeper state renders 30×15 px so the head
        // never changes size or position when the seal appears/changes.
        // (Cabinet is Patchwork — a different character with its own size.)
        for mode in ALL_MODES {
            if mode == VisualMode::Cabinet {
                continue;
            }
            let (_, map) = native_sprite(mode, false);
            let image = sprite_image(&map);
            assert_eq!(image.width(), 30 * 8, "{mode:?}");
            assert_eq!(image.height(), 15 * 8, "{mode:?}");
        }
    }

    #[test]
    fn glyph_faces_have_constant_dimensions_across_states() {
        let appearance = Appearance::default();
        for mode in ALL_MODES {
            let full = glyph_face(mode, false, &appearance, false);
            assert_eq!(full.len(), 3, "{mode:?}");
            let widths: Vec<usize> = full
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.chars().count()).sum())
                .collect();
            // Center-aligned paragraph: unequal widths shear the frame.
            assert!(
                widths.iter().all(|w| *w == 11),
                "{mode:?}: all lines must be 11 wide, got {widths:?}"
            );
            assert_eq!(
                glyph_face(mode, false, &appearance, true).len(),
                1,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn sprite_maps_have_consistent_row_widths() {
        for map in [
            KEEPER_GUARD.as_slice(),
            KEEPER_GUARD_BLINK.as_slice(),
            KEEPER_THINKING.as_slice(),
            KEEPER_UNKNOWN.as_slice(),
            KEEPER_ERROR.as_slice(),
            KEEPER_SUCCESS.as_slice(),
            KEEPER_SLEEP.as_slice(),
            PATCHWORK_CABINET.as_slice(),
        ] {
            for row in map {
                assert_eq!(row.chars().count(), 24, "row with wrong width: {row}");
                assert!(
                    row.chars().all(|c| c == '.' || sprite_color(c).is_some()),
                    "character outside the palette: {row}"
                );
            }
        }
    }

    fn render_plain(mode: VisualMode) -> String {
        let appearance = Appearance {
            no_color: false,
            ..Appearance::default()
        };
        mascot_lines(mode, 1, &appearance, false)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
