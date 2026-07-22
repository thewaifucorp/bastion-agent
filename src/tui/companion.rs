use super::visual::VisualMode;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

const MAX_PACK_BYTES: u64 = 64 * 1024;
const MAX_FRAME_ROWS: usize = 6;
const MAX_FRAME_WIDTH: usize = 21;
const WATER_DUE_SECS: u64 = 45 * 60;
const FOOD_DUE_SECS: u64 = 75 * 60;
const PLAY_DUE_SECS: u64 = 90 * 60;
const REST_DUE_SECS: u64 = 150 * 60;
const INPUT_XP_STEP: usize = 80;
const INPUT_XP_CAP: u64 = 3;
const ALLOWED_MARKUP: &[&str] = &[
    "primary",
    "secondary",
    "cyan",
    "blue",
    "magenta",
    "amber",
    "green",
    "red",
    "muted",
    "reset",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PetPack {
    pub(super) schema: u8,
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) rarity: Rarity,
    #[serde(default)]
    pub(super) palette: PetPalette,
    pub(super) states: PetStates,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Common => "COMMON",
            Self::Uncommon => "UNCOMMON",
            Self::Rare => "RARE",
            Self::Epic => "EPIC",
            Self::Legendary => "LEGENDARY",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PetPalette {
    pub(super) primary: Option<String>,
    pub(super) secondary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PetStates {
    pub(super) onboarding: Animation,
    pub(super) guard: Animation,
    pub(super) thinking: Animation,
    pub(super) build: Animation,
    pub(super) cabinet: Animation,
    pub(super) success: Animation,
    pub(super) alert: Animation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Animation {
    #[serde(default = "default_interval_ms")]
    pub(super) interval_ms: u64,
    pub(super) frames: Vec<Vec<String>>,
}

fn default_interval_ms() -> u64 {
    450
}

impl PetPack {
    pub(super) fn load(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("reading pet pack {}", path.display()))?;
        if metadata.len() > MAX_PACK_BYTES {
            bail!("pet pack exceeds the limit of {MAX_PACK_BYTES} bytes");
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading pet pack {}", path.display()))?;
        let pack: Self = toml::from_str(&contents)
            .with_context(|| format!("invalid pet pack TOML at {}", path.display()))?;
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            bail!("pet pack uses schema {}, expected 1", self.schema);
        }
        let id_parts: Vec<&str> = self.id.split('/').collect();
        if id_parts.len() != 2
            || id_parts.iter().any(|part| {
                part.is_empty()
                    || part
                        .chars()
                        .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            })
        {
            bail!("pet pack id must use lowercase publisher/name");
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 32 {
            bail!("pet pack name must be between 1 and 32 characters");
        }
        if let Some(color) = &self.palette.primary {
            validate_hex(color)?;
        }
        if let Some(color) = &self.palette.secondary {
            validate_hex(color)?;
        }
        for (name, animation) in [
            ("onboarding", &self.states.onboarding),
            ("guard", &self.states.guard),
            ("thinking", &self.states.thinking),
            ("build", &self.states.build),
            ("cabinet", &self.states.cabinet),
            ("success", &self.states.success),
            ("alert", &self.states.alert),
        ] {
            animation.validate(name)?;
        }
        Ok(())
    }

    pub(super) fn animation(&self, mode: VisualMode) -> &Animation {
        match mode {
            VisualMode::Onboarding => &self.states.onboarding,
            VisualMode::Guard | VisualMode::Sleep => &self.states.guard,
            VisualMode::Thinking => &self.states.thinking,
            VisualMode::Build => &self.states.build,
            VisualMode::Cabinet => &self.states.cabinet,
            VisualMode::Success => &self.states.success,
            VisualMode::Alert | VisualMode::Unknown => &self.states.alert,
        }
    }
}

impl Animation {
    fn validate(&self, state: &str) -> Result<()> {
        if !(90..=10_000).contains(&self.interval_ms) {
            bail!("state {state}: interval_ms must be between 90 and 10000");
        }
        if self.frames.is_empty() || self.frames.len() > 16 {
            bail!("state {state}: frames must contain between 1 and 16 frames");
        }
        for (frame_index, frame) in self.frames.iter().enumerate() {
            if frame.is_empty() || frame.len() > MAX_FRAME_ROWS {
                bail!("state {state}, frame {frame_index}: use 1 to {MAX_FRAME_ROWS} rows");
            }
            for line in frame {
                validate_line(state, frame_index, line)?;
            }
        }
        Ok(())
    }
}

fn validate_line(state: &str, frame_index: usize, line: &str) -> Result<()> {
    if line.chars().any(|c| c.is_control()) {
        bail!("state {state}, frame {frame_index}: control characters are not allowed");
    }
    let visible = strip_markup(line)?;
    if visible.width() > MAX_FRAME_WIDTH {
        bail!("state {state}, frame {frame_index}: max width is {MAX_FRAME_WIDTH} columns");
    }
    Ok(())
}

pub(super) fn strip_markup(line: &str) -> Result<String> {
    let mut visible = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        visible.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            bail!("color markup without a closing brace");
        };
        let tag = &after[..end];
        if !ALLOWED_MARKUP.contains(&tag) {
            bail!("unknown color markup: {{{tag}}}");
        }
        rest = &after[end + 1..];
    }
    visible.push_str(rest);
    if visible.contains('}') {
        bail!("color markup with an unexpected closing brace");
    }
    Ok(visible)
}

fn validate_hex(value: &str) -> Result<()> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid color '{value}'; use #RRGGBB");
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct CompanionState {
    #[serde(default)]
    pub(super) game_enabled: bool,
    #[serde(default)]
    pub(super) xp: u64,
    #[serde(default)]
    pub(super) successful_turns: u64,
    #[serde(default)]
    pub(super) pet_path: Option<PathBuf>,
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    active_since_water_secs: u64,
    #[serde(default)]
    active_since_food_secs: u64,
    #[serde(default)]
    active_since_play_secs: u64,
    #[serde(default)]
    active_since_rest_secs: u64,
    #[serde(default)]
    last_activity_unix: Option<u64>,
    #[serde(default)]
    last_source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CareAction {
    Water,
    Feed,
    Play,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CareCue {
    Water,
    Feed,
    Play,
    Sleep,
}

impl CompanionState {
    pub(super) fn load(default_game: bool) -> Self {
        let mut state: Self = std::fs::read_to_string(state_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if !state.initialized {
            if state.xp == 0 && state.successful_turns == 0 {
                state.game_enabled = default_game;
            }
            state.initialized = true;
        }
        state
    }

    pub(super) fn save(&self) -> Result<()> {
        let path = state_path();
        let parent = path
            .parent()
            .context("companion state without a parent directory")?;
        super::ensure_private_dir(parent)?;
        super::write_private_file(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub(super) fn award_success(&mut self, mode: VisualMode) -> Option<u64> {
        if !self.game_enabled {
            return None;
        }
        let previous_level = self.level();
        self.successful_turns = self.successful_turns.saturating_add(1);
        self.xp = self.xp.saturating_add(Self::success_xp(mode));
        (self.level() > previous_level).then(|| self.level())
    }

    pub(super) fn success_xp(mode: VisualMode) -> u64 {
        match mode {
            VisualMode::Build => 6,
            VisualMode::Cabinet => 5,
            _ => 3,
        }
    }

    pub(super) fn input_xp(chars: usize) -> u64 {
        ((chars / INPUT_XP_STEP) as u64).min(INPUT_XP_CAP)
    }

    pub(super) fn award_input(&mut self, chars: usize) -> Option<u64> {
        if !self.game_enabled {
            return None;
        }
        let reward = Self::input_xp(chars);
        if reward == 0 {
            return None;
        }
        let previous_level = self.level();
        self.xp = self.xp.saturating_add(reward);
        (self.level() > previous_level).then(|| self.level())
    }

    pub(super) fn momentum_status(chars: usize) -> String {
        let reward = Self::input_xp(chars);
        if reward == INPUT_XP_CAP {
            return format!("⌨ MOMENTUM MAX · +{reward} XP");
        }
        let current = chars % INPUT_XP_STEP;
        format!(
            "⌨ MOMENTUM {} {current}/{INPUT_XP_STEP} · +{reward} XP",
            progress_bar(current as u64, INPUT_XP_STEP as u64, 6)
        )
    }

    pub(super) fn record_activity(&mut self) -> Option<CareCue> {
        self.record_source_activity("bastion")
    }

    pub(super) fn record_source_activity(&mut self, source: &str) -> Option<CareCue> {
        if !self.game_enabled {
            return None;
        }
        let now = unix_now();
        if self.last_source.as_deref() != Some(source) {
            self.last_source = Some(source.to_string());
            self.last_activity_unix = Some(now);
            return None;
        }
        let previous = self.last_activity_unix.replace(now)?;
        let elapsed = now.saturating_sub(previous);
        // A gap longer than five minutes is a break, not active coding time.
        if elapsed == 0 || elapsed > 5 * 60 {
            return None;
        }
        let water_before = self.active_since_water_secs;
        let food_before = self.active_since_food_secs;
        let play_before = self.active_since_play_secs;
        let rest_before = self.active_since_rest_secs;
        self.active_since_water_secs = self.active_since_water_secs.saturating_add(elapsed);
        self.active_since_food_secs = self.active_since_food_secs.saturating_add(elapsed);
        self.active_since_play_secs = self.active_since_play_secs.saturating_add(elapsed);
        self.active_since_rest_secs = self.active_since_rest_secs.saturating_add(elapsed);

        if rest_before < REST_DUE_SECS && self.active_since_rest_secs >= REST_DUE_SECS {
            Some(CareCue::Sleep)
        } else if water_before < WATER_DUE_SECS && self.active_since_water_secs >= WATER_DUE_SECS {
            Some(CareCue::Water)
        } else if food_before < FOOD_DUE_SECS && self.active_since_food_secs >= FOOD_DUE_SECS {
            Some(CareCue::Feed)
        } else if play_before < PLAY_DUE_SECS && self.active_since_play_secs >= PLAY_DUE_SECS {
            Some(CareCue::Play)
        } else {
            None
        }
    }

    pub(super) fn start_session(&mut self, source: &str) {
        self.last_source = Some(source.to_string());
        self.last_activity_unix = Some(unix_now());
    }

    pub(super) fn stop_session(&mut self, source: &str) {
        if self.last_source.as_deref() == Some(source) {
            self.last_activity_unix = None;
            self.last_source = None;
        }
    }

    pub(super) fn care(&mut self, action: CareAction) {
        match action {
            CareAction::Water => self.active_since_water_secs = 0,
            CareAction::Feed => self.active_since_food_secs = 0,
            CareAction::Play => self.active_since_play_secs = 0,
            CareAction::Sleep => {
                self.active_since_rest_secs = 0;
                self.active_since_water_secs = 0;
                self.active_since_food_secs = 0;
                self.active_since_play_secs = 0;
                self.last_activity_unix = None;
                self.last_source = None;
            }
        }
    }

    pub(super) fn feed_choice(&mut self, choice: &str) -> Option<&'static str> {
        let (food, side, message) = match choice {
            "apple" => (
                70,
                Some(("water", 15)),
                "🍎 Apple served · food 70% · water +15%",
            ),
            "pizza" => (
                100,
                Some(("play", 20)),
                "🍕 Pizza served · food 100% · play +20%",
            ),
            "salad" => (
                80,
                Some(("water", 25)),
                "🥗 Salad served · food 80% · water +25%",
            ),
            "burger" => (
                100,
                Some(("rest", 10)),
                "🍔 Burger served · food 100% · rest +10%",
            ),
            "ice-cream" => (
                55,
                Some(("play", 35)),
                "🍨 Ice cream served · food 55% · play +35%",
            ),
            "carrot" => (
                70,
                Some(("water", 20)),
                "🥕 Carrot served · food 70% · water +20%",
            ),
            "chocolate" => (
                45,
                Some(("play", 30)),
                "🍫 Chocolate served · food 45% · play +30%",
            ),
            "steak" => (
                100,
                Some(("rest", 20)),
                "🥩 Steak served · food 100% · rest +20%",
            ),
            _ => return None,
        };
        restore_to(&mut self.active_since_food_secs, FOOD_DUE_SECS, food);
        if let Some((need, points)) = side {
            self.restore_need(need, points);
        }
        Some(message)
    }

    pub(super) fn play_choice(&mut self, choice: &str) -> Option<&'static str> {
        let (side, points, message) = match choice {
            "ball" => ("rest", 10, "⚽ Played ball · play 100% · rest +10%"),
            "run" => ("food", 20, "🏃 Ran around · play 100% · food +20%"),
            "sing" => ("rest", 25, "🎤 Sang songs · play 100% · rest +25%"),
            "draw" => ("rest", 20, "🎨 Drew pictures · play 100% · rest +20%"),
            "puzzle" => ("rest", 15, "🧩 Solved a puzzle · play 100% · rest +15%"),
            "dance" => ("food", 15, "💃 Dance party · play 100% · food +15%"),
            "read" => ("rest", 35, "📚 Read together · play 100% · rest +35%"),
            "hide-seek" => (
                "food",
                15,
                "🙈 Played hide-and-seek · play 100% · food +15%",
            ),
            _ => return None,
        };
        self.active_since_play_secs = 0;
        self.restore_need(side, points);
        Some(message)
    }

    pub(super) fn sleep_choice(&mut self, choice: &str) -> Option<&'static str> {
        let message = match choice {
            "nap" => {
                restore_to(&mut self.active_since_rest_secs, REST_DUE_SECS, 45);
                "😴 Short nap · rest restored to at least 45%"
            }
            "medium" => {
                restore_to(&mut self.active_since_rest_secs, REST_DUE_SECS, 70);
                restore_by(&mut self.active_since_play_secs, PLAY_DUE_SECS, 10);
                "💤 Medium sleep · rest 70% · play +10%"
            }
            "long" => {
                restore_to(&mut self.active_since_rest_secs, REST_DUE_SECS, 90);
                restore_by(&mut self.active_since_food_secs, FOOD_DUE_SECS, 15);
                "🌙 Long sleep · rest 90% · food +15%"
            }
            "night" => {
                self.active_since_rest_secs = 0;
                self.active_since_water_secs = 0;
                self.active_since_food_secs = 0;
                self.active_since_play_secs = 0;
                "🛏 Full night · all care needs restored"
            }
            _ => return None,
        };
        self.last_activity_unix = None;
        self.last_source = None;
        Some(message)
    }

    fn restore_need(&mut self, need: &str, points: u64) {
        match need {
            "water" => restore_by(&mut self.active_since_water_secs, WATER_DUE_SECS, points),
            "food" => restore_by(&mut self.active_since_food_secs, FOOD_DUE_SECS, points),
            "play" => restore_by(&mut self.active_since_play_secs, PLAY_DUE_SECS, points),
            "rest" => restore_by(&mut self.active_since_rest_secs, REST_DUE_SECS, points),
            _ => {}
        }
    }

    pub(super) fn needs_status(&self) -> String {
        format!(
            "water {} · food {} · play {} · rest {}",
            need_indicator(self.active_since_water_secs, WATER_DUE_SECS),
            need_indicator(self.active_since_food_secs, FOOD_DUE_SECS),
            need_indicator(self.active_since_play_secs, PLAY_DUE_SECS),
            need_indicator(self.active_since_rest_secs, REST_DUE_SECS),
        )
    }

    pub(super) fn level(&self) -> u64 {
        let mut level = 1;
        let mut spent = 0;
        loop {
            let threshold = level * 10;
            if self.xp < spent + threshold {
                return level;
            }
            spent += threshold;
            level += 1;
        }
    }

    pub(super) fn progress(&self) -> (u64, u64) {
        let level = self.level();
        let spent: u64 = (1..level).map(|n| n * 10).sum();
        (self.xp.saturating_sub(spent), level * 10)
    }

    pub(super) fn status(&self) -> String {
        let (current, target) = self.progress();
        format!(
            "LV {} · XP {} {current}/{target} · {} turns",
            self.level(),
            progress_bar(current, target, 10),
            self.successful_turns
        )
    }

    /// Fase A5 S5: single source of truth for `GET /companion`'s shape —
    /// the level/XP/need-percent formulas live ONLY here; the HTTP route
    /// (`src/loadout.rs`, via `tui::CompanionHandle::snapshot`) and this
    /// module's own `status_panel` both ultimately read the same numbers
    /// instead of the web layer recomputing them.
    pub(super) fn snapshot(&self) -> super::CompanionSnapshot {
        let (pack_name, frame) = self.frame_and_name();
        super::CompanionSnapshot {
            game_enabled: self.game_enabled,
            level: self.level(),
            xp: self.xp,
            successful_turns: self.successful_turns,
            needs: super::CompanionNeeds {
                water: need_percent(self.active_since_water_secs, WATER_DUE_SECS),
                food: need_percent(self.active_since_food_secs, FOOD_DUE_SECS),
                play: need_percent(self.active_since_play_secs, PLAY_DUE_SECS),
                rest: need_percent(self.active_since_rest_secs, REST_DUE_SECS),
            },
            cues: self.due_cues(),
            frame,
            pack_name,
        }
    }

    /// Needs currently "due now" (same threshold `need_indicator` uses for
    /// `● now`) — canonical care-action names (`water`/`feed`/`play`/`sleep`),
    /// so the web view can cross-reference them against `POST
    /// /companion/care`'s own action strings without a second naming table.
    fn due_cues(&self) -> Vec<&'static str> {
        let mut cues = Vec::new();
        if self.active_since_rest_secs >= REST_DUE_SECS {
            cues.push("sleep");
        }
        if self.active_since_water_secs >= WATER_DUE_SECS {
            cues.push("water");
        }
        if self.active_since_food_secs >= FOOD_DUE_SECS {
            cues.push("feed");
        }
        if self.active_since_play_secs >= PLAY_DUE_SECS {
            cues.push("play");
        }
        cues
    }

    /// Web's static representative frame: the pack's `guard` (idle) state,
    /// first animation frame, markup stripped — simpler than threading
    /// tick-based animation and turn-driven `VisualMode` (both TUI-only
    /// concepts, see `visual.rs`) through a stateless HTTP GET. Documented
    /// simplification: the web Buddy view shows one idle portrait, the TUI
    /// keeps the full animated experience.
    fn frame_and_name(&self) -> (String, super::CompanionFrame) {
        if let Some(pack) = self
            .pet_path
            .as_deref()
            .and_then(|path| PetPack::load(path).ok())
        {
            let animation = pack.animation(VisualMode::Guard);
            let rows: Vec<String> = animation
                .frames
                .first()
                .into_iter()
                .flatten()
                .map(|line| strip_markup(line).unwrap_or_else(|_| line.clone()))
                .collect();
            let width = rows.iter().map(|row| row.width()).max().unwrap_or(0);
            return (pack.name.clone(), super::CompanionFrame { rows, width });
        }
        default_web_frame()
    }

    pub(super) fn status_panel(&self) -> String {
        let (current, target) = self.progress();
        let game = if self.game_enabled {
            "ON"
        } else {
            "OFF · XP PAUSED"
        };
        format!(
            "╭─ KEEPER // GAME {game}\n\
             │ LEVEL {:02}  ·  {} completed turns\n\
             │ XP  {}  {current}/{target}\n\
             │ NEXT LEVEL  {} XP\n\
             ├─ CARE // ACTIVE TIME\n\
             │ WATER {} {:>3}%  FOOD {} {:>3}%\n\
             │ PLAY  {} {:>3}%  REST {} {:>3}%\n\
             ╰─ Cosmetic progress only · no capabilities unlocked",
            self.level(),
            self.successful_turns,
            progress_bar(current, target, 18),
            target.saturating_sub(current),
            need_bar(self.active_since_water_secs, WATER_DUE_SECS, 5),
            need_percent(self.active_since_water_secs, WATER_DUE_SECS),
            need_bar(self.active_since_food_secs, FOOD_DUE_SECS, 5),
            need_percent(self.active_since_food_secs, FOOD_DUE_SECS),
            need_bar(self.active_since_play_secs, PLAY_DUE_SECS, 5),
            need_percent(self.active_since_play_secs, PLAY_DUE_SECS),
            need_bar(self.active_since_rest_secs, REST_DUE_SECS, 5),
            need_percent(self.active_since_rest_secs, REST_DUE_SECS),
        )
    }
}

/// Shared by the standalone CLI (`bastion companion care`) and the daemon's
/// `POST /companion/care` (via `tui::CompanionHandle::care`) — one place
/// that maps the wire string to the enum, including the `"rest"` alias for
/// `Sleep` (Fase 3.6's clap alias mirrors this exact match).
pub(super) fn parse_care_action(action: &str) -> Result<CareAction> {
    match action {
        "water" => Ok(CareAction::Water),
        "feed" => Ok(CareAction::Feed),
        "play" => Ok(CareAction::Play),
        "sleep" | "rest" => Ok(CareAction::Sleep),
        _ => bail!("unknown companion care action '{action}'"),
    }
}

/// Default portrait when no custom pet pack is loaded (`pet_path` is
/// `None`) — a small ASCII Keeper face, independent of the TUI's
/// ratatui-coupled `glyph_face`/`native_sprite` renderers (`visual.rs`),
/// which build styled `Span`s for a terminal frame, not plain strings for
/// JSON.
fn default_web_frame() -> (String, super::CompanionFrame) {
    let rows: Vec<String> = [" .-\"\"\"-. ", "/  ^   ^ \\", "|    -    |", " '-.....-' "]
        .into_iter()
        .map(str::to_string)
        .collect();
    let width = rows.iter().map(|row| row.width()).max().unwrap_or(0);
    ("Keeper".to_string(), super::CompanionFrame { rows, width })
}

fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("bastion")
        .join("companion.json")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn need_indicator(active: u64, due: u64) -> &'static str {
    if active >= due {
        "● now"
    } else if active * 4 >= due * 3 {
        "◐ soon"
    } else {
        "○ ok"
    }
}

fn need_percent(active: u64, due: u64) -> u64 {
    if due == 0 {
        return 100;
    }
    100_u64.saturating_sub(active.min(due).saturating_mul(100) / due)
}

fn need_bar(active: u64, due: u64, width: usize) -> String {
    progress_bar(need_percent(active, due), 100, width)
}

fn restore_to(active: &mut u64, due: u64, remaining_percent: u64) {
    let target_active =
        due.saturating_mul(100_u64.saturating_sub(remaining_percent.min(100))) / 100;
    *active = (*active).min(target_active);
}

fn restore_by(active: &mut u64, due: u64, points: u64) {
    *active = active.saturating_sub(due.saturating_mul(points) / 100);
}

fn progress_bar(current: u64, target: u64, width: usize) -> String {
    let filled = if target == 0 {
        width
    } else {
        ((current.min(target) as u128 * width as u128) / target as u128) as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn animation() -> Animation {
        Animation {
            interval_ms: 450,
            frames: vec![vec!["{primary}[o_o]{reset}".to_string()]],
        }
    }

    #[test]
    fn validates_complete_declarative_pack() {
        let pack = PetPack {
            schema: 1,
            id: "acme/keeper".to_string(),
            name: "Keeper".to_string(),
            rarity: Rarity::Rare,
            palette: PetPalette::default(),
            states: PetStates {
                onboarding: animation(),
                guard: animation(),
                thinking: animation(),
                build: animation(),
                cabinet: animation(),
                success: animation(),
                alert: animation(),
            },
        };
        assert!(pack.validate().is_ok());
    }

    #[test]
    fn bundled_pet_pack_and_extension_manifest_follow_the_runtime_contract() {
        let pack: PetPack = toml::from_str(include_str!(
            "../../skills/bastion-pet-pack/assets/pet-pack/ui/pet.toml"
        ))
        .expect("bundled pet pack should parse");
        pack.validate().expect("bundled pet pack should validate");

        let manifest: bastion_extension_protocol::ExtensionManifest = toml::from_str(include_str!(
            "../../skills/bastion-pet-pack/assets/pet-pack/extension.toml"
        ))
        .expect("bundled extension manifest should parse");
        assert_eq!(manifest.id, "example/terminal-pet");
        assert_eq!(
            manifest.permissions,
            bastion_extension_protocol::PermissionSet::none()
        );
        assert!(manifest.provides.iter().any(|provided| matches!(
            provided,
            bastion_extension_protocol::Provided::Ui(name) if name == "pet"
        )));
    }

    #[test]
    fn rejects_ansi_and_unknown_markup() {
        assert!(validate_line("guard", 0, "\u{1b}[31mno").is_err());
        assert!(strip_markup("{network}nope").is_err());
    }

    #[test]
    fn level_curve_rewards_success_without_token_volume() {
        let mut state = CompanionState {
            game_enabled: true,
            ..CompanionState::default()
        };
        assert_eq!(state.award_success(VisualMode::Build), None);
        assert_eq!(state.xp, 6);
        assert_eq!(state.level(), 1);
        state.xp = 10;
        assert_eq!(state.level(), 2);
        assert_eq!(state.progress(), (0, 20));
    }

    #[test]
    fn input_xp_rewards_human_momentum_with_a_hard_cap() {
        assert_eq!(CompanionState::input_xp(79), 0);
        assert_eq!(CompanionState::input_xp(80), 1);
        assert_eq!(CompanionState::input_xp(160), 2);
        assert_eq!(CompanionState::input_xp(240), 3);
        assert_eq!(CompanionState::input_xp(8_000), 3);

        let mut state = CompanionState {
            game_enabled: true,
            xp: 9,
            ..CompanionState::default()
        };
        assert_eq!(state.award_input(80), Some(2));
        assert_eq!(state.xp, 10);
        assert!(CompanionState::momentum_status(240).contains("MAX · +3 XP"));
    }

    #[test]
    fn care_never_removes_progress() {
        let mut state = CompanionState {
            game_enabled: true,
            xp: 42,
            active_since_water_secs: WATER_DUE_SECS,
            ..CompanionState::default()
        };
        state.care(CareAction::Water);
        assert_eq!(state.xp, 42);
        assert_eq!(state.active_since_water_secs, 0);
    }

    #[test]
    fn status_panel_visualizes_xp_and_care_without_unlock_claims() {
        let state = CompanionState {
            game_enabled: true,
            xp: 15,
            successful_turns: 4,
            active_since_food_secs: FOOD_DUE_SECS * 3 / 4,
            active_since_rest_secs: REST_DUE_SECS,
            ..CompanionState::default()
        };

        let panel = state.status_panel();
        assert!(panel.contains("KEEPER // GAME ON"));
        assert!(panel.contains("LEVEL 02"));
        assert!(panel.contains("████░░░░░░░░░░░░░░  5/20"));
        assert!(panel.contains("FOOD █░░░░  25%"));
        assert!(panel.contains("REST ░░░░░   0%"));
        assert!(panel.contains("no capabilities unlocked"));
    }

    #[test]
    fn parses_care_actions_including_the_rest_alias() {
        assert_eq!(parse_care_action("water").unwrap(), CareAction::Water);
        assert_eq!(parse_care_action("feed").unwrap(), CareAction::Feed);
        assert_eq!(parse_care_action("play").unwrap(), CareAction::Play);
        assert_eq!(parse_care_action("sleep").unwrap(), CareAction::Sleep);
        assert_eq!(parse_care_action("rest").unwrap(), CareAction::Sleep);
        assert!(parse_care_action("nap").is_err());
    }

    #[test]
    fn snapshot_reports_needs_cues_and_a_default_frame() {
        let state = CompanionState {
            game_enabled: true,
            xp: 15,
            successful_turns: 4,
            active_since_water_secs: WATER_DUE_SECS,
            active_since_food_secs: FOOD_DUE_SECS * 3 / 4,
            ..CompanionState::default()
        };
        let snapshot = state.snapshot();
        assert!(snapshot.game_enabled);
        assert_eq!(snapshot.level, 2);
        assert_eq!(snapshot.xp, 15);
        assert_eq!(snapshot.successful_turns, 4);
        assert_eq!(snapshot.needs.water, 0);
        assert_eq!(snapshot.needs.food, 25);
        assert_eq!(snapshot.needs.play, 100);
        assert_eq!(snapshot.needs.rest, 100);
        assert_eq!(snapshot.cues, vec!["water"]);
        assert_eq!(snapshot.pack_name, "Keeper");
        assert!(!snapshot.frame.rows.is_empty());
        assert!(snapshot.frame.width > 0 && snapshot.frame.width <= MAX_FRAME_WIDTH);
    }

    #[test]
    fn care_choices_have_distinct_non_punitive_effects() {
        let mut state = CompanionState {
            game_enabled: true,
            xp: 42,
            active_since_water_secs: WATER_DUE_SECS,
            active_since_food_secs: FOOD_DUE_SECS,
            active_since_play_secs: PLAY_DUE_SECS,
            active_since_rest_secs: REST_DUE_SECS,
            ..CompanionState::default()
        };

        assert!(state.feed_choice("salad").is_some());
        assert_eq!(
            need_percent(state.active_since_food_secs, FOOD_DUE_SECS),
            80
        );
        assert_eq!(
            need_percent(state.active_since_water_secs, WATER_DUE_SECS),
            25
        );

        assert!(state.play_choice("read").is_some());
        assert_eq!(
            need_percent(state.active_since_play_secs, PLAY_DUE_SECS),
            100
        );
        assert_eq!(
            need_percent(state.active_since_rest_secs, REST_DUE_SECS),
            35
        );

        assert!(state.sleep_choice("medium").is_some());
        assert_eq!(
            need_percent(state.active_since_rest_secs, REST_DUE_SECS),
            70
        );
        assert_eq!(state.xp, 42);
    }
}
