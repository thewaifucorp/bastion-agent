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
        let parent = path.parent().context("companion state without a parent directory")?;
        super::ensure_private_dir(parent)?;
        super::write_private_file(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub(super) fn award_success(&mut self, mode: VisualMode) -> Option<u64> {
        if !self.game_enabled {
            return None;
        }
        let previous_level = self.level();
        self.successful_turns = self.successful_turns.saturating_add(1);
        self.xp = self.xp.saturating_add(match mode {
            VisualMode::Build => 5,
            VisualMode::Cabinet => 4,
            _ => 2,
        });
        (self.level() > previous_level).then(|| self.level())
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

    pub(super) fn needs_status(&self) -> String {
        format!(
            "{} water · {} food · {} play · {} rest",
            need_label(self.active_since_water_secs, WATER_DUE_SECS),
            need_label(self.active_since_food_secs, FOOD_DUE_SECS),
            need_label(self.active_since_play_secs, PLAY_DUE_SECS),
            need_label(self.active_since_rest_secs, REST_DUE_SECS),
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
            "LV {} · XP {current}/{target} · {} turns",
            self.level(),
            self.successful_turns
        )
    }
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

fn need_label(active: u64, due: u64) -> &'static str {
    if active >= due {
        "now"
    } else if active * 4 >= due * 3 {
        "soon"
    } else {
        "ok"
    }
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
        assert_eq!(state.xp, 5);
        assert_eq!(state.level(), 1);
        state.xp = 10;
        assert_eq!(state.level(), 2);
        assert_eq!(state.progress(), (0, 20));
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
}
