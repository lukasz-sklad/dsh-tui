//! Persistent configuration, color themes, session storage, and MCP registry.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MODEL: &str = "deepseek-flash";
pub const DEFAULT_EFFORT: &str = "high";
pub const DEFAULT_MODE: &str = "workspace-write";
pub const DEFAULT_CONTEXT_WINDOW: u64 = 1_048_576;

/// Metadata for a supported DeepSeek model.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub context_window: u64,
    pub modalities: &'static str,
}

const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "deepseek-flash",
        display_name: "DeepSeek-V4.1-Flash",
        description: "Szybki model ogólny z oknem 1M tokenów i obsługą obrazów.",
        context_window: 1_048_576,
        modalities: "text, image",
    },
    ModelSpec {
        id: "deepseek-v4-pro",
        display_name: "DeepSeek-V4-Pro",
        description: "Mocniejszy model do trudnego kodowania, wiedzy i głębokiego rozumowania.",
        context_window: 1_048_576,
        modalities: "text, image",
    },
];

pub fn available_models() -> &'static [ModelSpec] {
    MODELS
}

/// Resolve friendly aliases (`flash`, `pro`) to canonical model IDs.
pub fn resolve_model_alias(input: &str) -> String {
    match input.trim().to_ascii_lowercase().as_str() {
        "flash" | "v4.1-flash" | "deepseek-v4.1-flash" => "deepseek-flash".to_string(),
        "pro" | "v4-pro" | "deepseek-pro" => "deepseek-v4-pro".to_string(),
        _ => input.trim().to_string(),
    }
}

/// Normalize reasoning effort (`off` | `low` | `high` | `max`), mapping aliases (`medium` -> `high`, `xhigh` -> `max`).
pub fn normalize_effort(input: &str) -> Option<&'static str> {
    match input.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "disabled" => Some("off"),
        "low" | "extra-low" => Some("low"),
        "medium" | "med" | "high" => Some("high"),
        "max" | "xhigh" => Some("max"),
        _ => None,
    }
}

/// Canonical execution/permission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    WorkspaceWrite,
    Auto,
    Plan,
    ReadOnly,
}

impl ExecutionMode {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "workspace-write" | "accept-edits" | "default" => Some(Self::WorkspaceWrite),
            "auto" | "danger-full-access" | "bypass" => Some(Self::Auto),
            "plan" => Some(Self::Plan),
            "read-only" | "readonly" => Some(Self::ReadOnly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::ReadOnly => "read-only",
        }
    }

    pub fn dsh_preset(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::Auto => "danger-full-access",
            Self::Plan | Self::ReadOnly => "read-only",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "🔒 workspace-write",
            Self::Auto => "⚡ auto",
            Self::Plan => "📋 plan",
            Self::ReadOnly => "👁 read-only",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "Zapis w przestrzeni roboczej za zgodą użytkownika (domyślny).",
            Self::Auto => "Automatyczna aprobata wszystkich wywołań (danger-full-access).",
            Self::Plan => "Tryb planowania i analizy bez modyfikacji plików (read-only).",
            Self::ReadOnly => "Tylko odczyt bez uprawnień zapisu i wykonania niebezpiecznych komend.",
        }
    }

    pub fn cycle_next(&self) -> Self {
        match self {
            Self::WorkspaceWrite => Self::Auto,
            Self::Auto => Self::Plan,
            Self::Plan => Self::ReadOnly,
            Self::ReadOnly => Self::WorkspaceWrite,
        }
    }

    pub fn all() -> &'static [ExecutionMode] {
        &[
            ExecutionMode::WorkspaceWrite,
            ExecutionMode::Auto,
            ExecutionMode::Plan,
            ExecutionMode::ReadOnly,
        ]
    }
}

/// Available UI color themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    Sepia,
    Neon,
}

impl ThemeName {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "dark" | "ciemny" => Some(Self::Dark),
            "light" | "jasny" => Some(Self::Light),
            "sepia" | "pergamin" => Some(Self::Sepia),
            "neon" | "cyber" => Some(Self::Neon),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Sepia => "sepia",
            Self::Neon => "neon",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Dark => "Dark (ciemny)",
            Self::Light => "Light (jasny)",
            Self::Sepia => "Sepia (pergamin)",
            Self::Neon => "Neon (wysoki kontrast)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Dark => "Ciemny (domyślny kontrastowy)",
            Self::Light => "Jasny (dzienny)",
            Self::Sepia => "Sepia (ciepły pergamin)",
            Self::Neon => "Neon (wysoki kontrast cyberpunk)",
        }
    }

    pub fn all() -> &'static [ThemeName] {
        &[ThemeName::Dark, ThemeName::Light, ThemeName::Sepia, ThemeName::Neon]
    }

    pub fn palette(&self) -> ThemePalette {
        match self {
            Self::Dark => ThemePalette {
                bg: Color::Rgb(24, 24, 27),
                surface: Color::Rgb(35, 35, 40),
                fg: Color::Rgb(244, 244, 245),
                muted: Color::Rgb(138, 138, 152),
                accent: Color::Rgb(56, 189, 248),
                accent_alt: Color::Rgb(167, 139, 250),
                user_fg: Color::Rgb(103, 232, 249),
                user_bg: Color::Rgb(30, 41, 59),
                heading: Color::Rgb(125, 211, 252),
                bullet: Color::Rgb(250, 204, 21),
                link: Color::Rgb(96, 165, 250),
                code_bg: Color::Rgb(28, 30, 40),
                code_fg: Color::Rgb(214, 222, 235),
                code_keyword: Color::Rgb(199, 146, 234),
                code_string: Color::Rgb(173, 219, 103),
                code_comment: Color::Rgb(105, 119, 145),
                code_number: Color::Rgb(247, 140, 108),
                code_type: Color::Rgb(130, 170, 255),
                thinking_fg: Color::Rgb(161, 161, 170),
                thinking_border: Color::Rgb(99, 102, 241),
                border_active: Color::Rgb(56, 189, 248),
                border_inactive: Color::Rgb(63, 63, 70),
                status_bg: Color::Rgb(20, 20, 24),
                success: Color::Rgb(74, 222, 128),
                warning: Color::Rgb(250, 204, 21),
                error: Color::Rgb(248, 113, 113),
                diff_add_fg: Color::Rgb(134, 239, 172),
                diff_add_bg: Color::Rgb(20, 48, 34),
                diff_del_fg: Color::Rgb(252, 165, 165),
                diff_del_bg: Color::Rgb(60, 24, 28),
                diff_hunk: Color::Rgb(56, 189, 248),
                selection_bg: Color::Rgb(37, 99, 235),
                selection_fg: Color::Rgb(255, 255, 255),
            },
            Self::Light => ThemePalette {
                bg: Color::Rgb(244, 244, 245),
                surface: Color::Rgb(255, 255, 255),
                fg: Color::Rgb(24, 24, 27),
                muted: Color::Rgb(113, 113, 122),
                accent: Color::Rgb(2, 132, 199),
                accent_alt: Color::Rgb(109, 40, 217),
                user_fg: Color::Rgb(3, 105, 161),
                user_bg: Color::Rgb(224, 242, 254),
                heading: Color::Rgb(14, 116, 144),
                bullet: Color::Rgb(180, 83, 9),
                link: Color::Rgb(37, 99, 235),
                code_bg: Color::Rgb(236, 239, 244),
                code_fg: Color::Rgb(46, 52, 64),
                code_keyword: Color::Rgb(147, 51, 234),
                code_string: Color::Rgb(21, 128, 61),
                code_comment: Color::Rgb(107, 114, 128),
                code_number: Color::Rgb(194, 65, 12),
                code_type: Color::Rgb(29, 78, 216),
                thinking_fg: Color::Rgb(82, 82, 91),
                thinking_border: Color::Rgb(99, 102, 241),
                border_active: Color::Rgb(2, 132, 199),
                border_inactive: Color::Rgb(200, 200, 208),
                status_bg: Color::Rgb(228, 228, 231),
                success: Color::Rgb(22, 163, 74),
                warning: Color::Rgb(202, 138, 4),
                error: Color::Rgb(220, 38, 38),
                diff_add_fg: Color::Rgb(22, 101, 52),
                diff_add_bg: Color::Rgb(220, 252, 231),
                diff_del_fg: Color::Rgb(153, 27, 27),
                diff_del_bg: Color::Rgb(254, 226, 226),
                diff_hunk: Color::Rgb(2, 132, 199),
                selection_bg: Color::Rgb(37, 99, 235),
                selection_fg: Color::Rgb(255, 255, 255),
            },
            Self::Sepia => ThemePalette {
                bg: Color::Rgb(243, 234, 216),
                surface: Color::Rgb(253, 250, 243),
                fg: Color::Rgb(61, 50, 36),
                muted: Color::Rgb(125, 107, 84),
                accent: Color::Rgb(146, 64, 14),
                accent_alt: Color::Rgb(107, 79, 135),
                user_fg: Color::Rgb(120, 53, 15),
                user_bg: Color::Rgb(238, 226, 202),
                heading: Color::Rgb(124, 45, 18),
                bullet: Color::Rgb(161, 98, 7),
                link: Color::Rgb(29, 78, 216),
                code_bg: Color::Rgb(236, 226, 206),
                code_fg: Color::Rgb(54, 44, 32),
                code_keyword: Color::Rgb(147, 51, 234),
                code_string: Color::Rgb(22, 101, 52),
                code_comment: Color::Rgb(138, 121, 98),
                code_number: Color::Rgb(180, 83, 9),
                code_type: Color::Rgb(30, 64, 175),
                thinking_fg: Color::Rgb(108, 92, 72),
                thinking_border: Color::Rgb(161, 98, 7),
                border_active: Color::Rgb(180, 83, 9),
                border_inactive: Color::Rgb(210, 195, 170),
                status_bg: Color::Rgb(232, 220, 196),
                success: Color::Rgb(21, 128, 61),
                warning: Color::Rgb(180, 83, 9),
                error: Color::Rgb(185, 28, 28),
                diff_add_fg: Color::Rgb(20, 83, 45),
                diff_add_bg: Color::Rgb(220, 240, 212),
                diff_del_fg: Color::Rgb(153, 27, 27),
                diff_del_bg: Color::Rgb(245, 214, 206),
                diff_hunk: Color::Rgb(146, 64, 14),
                selection_bg: Color::Rgb(146, 64, 14),
                selection_fg: Color::Rgb(253, 250, 243),
            },
            Self::Neon => ThemePalette {
                bg: Color::Rgb(10, 12, 22),
                surface: Color::Rgb(16, 20, 36),
                fg: Color::Rgb(232, 244, 255),
                muted: Color::Rgb(116, 134, 170),
                accent: Color::Rgb(0, 255, 213),
                accent_alt: Color::Rgb(255, 77, 184),
                user_fg: Color::Rgb(0, 255, 213),
                user_bg: Color::Rgb(18, 38, 56),
                heading: Color::Rgb(255, 110, 199),
                bullet: Color::Rgb(255, 221, 64),
                link: Color::Rgb(56, 189, 248),
                code_bg: Color::Rgb(14, 18, 32),
                code_fg: Color::Rgb(220, 236, 255),
                code_keyword: Color::Rgb(255, 92, 184),
                code_string: Color::Rgb(80, 250, 123),
                code_comment: Color::Rgb(98, 114, 164),
                code_number: Color::Rgb(255, 184, 108),
                code_type: Color::Rgb(139, 233, 253),
                thinking_fg: Color::Rgb(168, 178, 216),
                thinking_border: Color::Rgb(189, 147, 249),
                border_active: Color::Rgb(0, 255, 213),
                border_inactive: Color::Rgb(52, 62, 98),
                status_bg: Color::Rgb(8, 10, 18),
                success: Color::Rgb(80, 250, 123),
                warning: Color::Rgb(241, 250, 140),
                error: Color::Rgb(255, 85, 85),
                diff_add_fg: Color::Rgb(80, 250, 123),
                diff_add_bg: Color::Rgb(16, 48, 36),
                diff_del_fg: Color::Rgb(255, 85, 85),
                diff_del_bg: Color::Rgb(56, 18, 30),
                diff_hunk: Color::Rgb(139, 233, 253),
                selection_bg: Color::Rgb(255, 77, 184),
                selection_fg: Color::Rgb(10, 12, 22),
            },
        }
    }
}

/// Complete resolved color palette for rendering the TUI and Markdown.
#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub bg: Color,
    pub surface: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub user_fg: Color,
    pub user_bg: Color,
    pub heading: Color,
    pub bullet: Color,
    pub link: Color,
    pub code_bg: Color,
    pub code_fg: Color,
    pub code_keyword: Color,
    pub code_string: Color,
    pub code_comment: Color,
    pub code_number: Color,
    pub code_type: Color,
    pub thinking_fg: Color,
    pub thinking_border: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub status_bg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub diff_add_fg: Color,
    pub diff_add_bg: Color,
    pub diff_del_fg: Color,
    pub diff_del_bg: Color,
    pub diff_hunk: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
}

/// Persistent user configuration under `$DSH_HOME/dsh-tui/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub theme: ThemeName,
    pub show_thinking: bool,
    pub show_tools_panel: bool,
    pub auto_approve: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            effort: DEFAULT_EFFORT.to_string(),
            mode: DEFAULT_MODE.to_string(),
            theme: ThemeName::Dark,
            show_thinking: true,
            show_tools_panel: true,
            auto_approve: false,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        let Ok(content) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&content).unwrap_or_default();
        settings.normalize();
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize settings: {e}"))?;
        fs::write(&path, format!("{body}\n"))
            .map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Apply a JSON object string or file path onto these settings (without saving).
    pub fn apply_override(&mut self, spec: &str) -> Result<(), String> {
        let trimmed = spec.trim();
        let raw = if trimmed.starts_with('{') {
            trimmed.to_string()
        } else {
            fs::read_to_string(trimmed)
                .map_err(|e| format!("nie można wczytać --settings `{trimmed}`: {e}"))?
        };
        let value: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("niepoprawny JSON w --settings: {e}"))?;
        let Some(obj) = value.as_object() else {
            return Err("--settings musi być obiektem JSON".to_string());
        };
        if let Some(m) = obj.get("model").and_then(Value::as_str) {
            self.model = resolve_model_alias(m);
        }
        if let Some(e) = obj.get("effort").and_then(Value::as_str) {
            if let Some(norm) = normalize_effort(e) {
                self.effort = norm.to_string();
            } else {
                return Err(format!("nieznany poziom effort: {e}"));
            }
        }
        if let Some(mode) = obj.get("mode").and_then(Value::as_str) {
            if let Some(parsed) = ExecutionMode::parse(mode) {
                self.mode = parsed.as_str().to_string();
            } else {
                return Err(format!("nieznany tryb mode: {mode}"));
            }
        }
        if let Some(theme) = obj.get("theme").and_then(Value::as_str) {
            if let Some(parsed) = ThemeName::parse(theme) {
                self.theme = parsed;
            } else {
                return Err(format!("nieznany motyw theme: {theme}"));
            }
        }
        if let Some(v) = obj.get("show_thinking").and_then(Value::as_bool) {
            self.show_thinking = v;
        }
        if let Some(v) = obj.get("show_tools_panel").and_then(Value::as_bool) {
            self.show_tools_panel = v;
        }
        if let Some(v) = obj.get("auto_approve").and_then(Value::as_bool) {
            self.auto_approve = v;
        }
        self.normalize();
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.model = resolve_model_alias(&self.model);
        self.effort = normalize_effort(&self.effort)
            .unwrap_or(DEFAULT_EFFORT)
            .to_string();
        self.mode = ExecutionMode::parse(&self.mode)
            .unwrap_or(ExecutionMode::WorkspaceWrite)
            .as_str()
            .to_string();
    }
}

/// Token and context window statistics for a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    pub last_context_tokens: u64,
    pub context_window: u64,
}

impl Default for TokenUsage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            total_tokens: 0,
            last_context_tokens: 0,
            context_window: DEFAULT_CONTEXT_WINDOW,
        }
    }
}

impl TokenUsage {
    /// Update counters from one `assistant/message` `usage` JSON object.
    pub fn record_step(&mut self, usage: &Value) {
        let input = usage.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
        let output = usage.get("outputTokens").and_then(Value::as_u64).unwrap_or(0);
        let cache_read = usage.get("cacheReadTokens").and_then(Value::as_u64).unwrap_or(0);
        let cache_write = usage.get("cacheWriteTokens").and_then(Value::as_u64).unwrap_or(0);
        let total = usage
            .get("totalTokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output + cache_read + cache_write);
        self.input_tokens = self.input_tokens.saturating_add(input);
        self.output_tokens = self.output_tokens.saturating_add(output);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(cache_read);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(cache_write);
        self.total_tokens = self.total_tokens.saturating_add(total);
        if total > 0 {
            self.last_context_tokens = total;
        }
    }

    pub fn fill_percent(&self) -> f64 {
        if self.context_window == 0 {
            return 0.0;
        }
        let used = self.last_context_tokens as f64;
        (used / self.context_window as f64 * 100.0).clamp(0.0, 100.0)
    }
}

/// Format token count compactly (e.g. `850`, `8.8k`, `1.0M`).
pub fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Single task tracked by the agent (`todo/write` event).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

impl Default for TodoItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            content: String::new(),
            status: "pending".to_string(),
        }
    }
}

/// One persisted transcript entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Tool {
        name: String,
        summary: String,
        #[serde(default)]
        diff: Option<String>,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        elapsed_secs: f32,
    },
    Info {
        text: String,
    },
}

/// Persisted session snapshot stored under `$DSH_HOME/dsh-tui/sessions/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    pub id: String,
    pub title: Option<String>,
    pub cwd: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub created_at_epoch: u64,
    pub updated_at_epoch: u64,
    pub entries: Vec<ChatEntry>,
    pub tool_log: Vec<String>,
    pub usage: TokenUsage,
    pub todos: Vec<TodoItem>,
    pub active_goal: Option<String>,
}

impl Default for SessionRecord {
    fn default() -> Self {
        let now = now_epoch();
        Self {
            id: String::new(),
            title: None,
            cwd: ".".to_string(),
            model: DEFAULT_MODEL.to_string(),
            effort: DEFAULT_EFFORT.to_string(),
            mode: DEFAULT_MODE.to_string(),
            created_at_epoch: now,
            updated_at_epoch: now,
            entries: Vec::new(),
            tool_log: Vec::new(),
            usage: TokenUsage::default(),
            todos: Vec::new(),
            active_goal: None,
        }
    }
}

/// Directory holding dsh-tui's own state, under `$DSH_HOME` (`~/.dsh` by default).
pub fn state_dir() -> PathBuf {
    let home = env::var("DSH_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            env::var("HOME")
                .map(|home| format!("{home}/.dsh"))
                .unwrap_or_else(|_| ".dsh".to_string())
        });
    PathBuf::from(home).join("dsh-tui")
}

pub fn settings_path() -> PathBuf {
    state_dir().join("settings.json")
}

pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

pub fn session_path(session_id: &str) -> PathBuf {
    let sanitized: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    sessions_dir().join(format!("{sanitized}.json"))
}

pub fn last_session_path() -> PathBuf {
    state_dir().join("last-session")
}

pub fn mcp_path() -> PathBuf {
    state_dir().join("mcp.json")
}

/// Record the last used session id for `-c`/`--continue`.
pub fn save_last_session(session_id: &str) {
    let path = last_session_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("dsh-tui: nie mogę utworzyć {}: {error}", parent.display());
            return;
        }
    }
    if let Err(error) = fs::write(&path, session_id) {
        eprintln!("dsh-tui: nie mogę zapisać {}: {error}", path.display());
    }
}

/// Read the last used session id, if one was recorded.
pub fn load_last_session() -> Option<String> {
    let session_id = fs::read_to_string(last_session_path()).ok()?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        None
    } else {
        Some(session_id.to_string())
    }
}

pub fn save_session(record: &SessionRecord) -> Result<(), String> {
    if record.id.trim().is_empty() {
        return Ok(());
    }
    let dir = sessions_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {e}", dir.display()))?;
    let path = session_path(&record.id);
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| format!("serialize session {}: {e}", record.id))?;
    fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

pub fn load_session(session_id: &str) -> Result<SessionRecord, String> {
    let path = session_path(session_id);
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("nie mogę odczytać sesji {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("uszkodzony plik sesji {}: {e}", path.display()))
}

/// List all persisted sessions sorted newest-first.
pub fn list_sessions() -> Vec<SessionRecord> {
    let dir = sessions_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(record) = serde_json::from_str::<SessionRecord>(&raw) {
                if !record.id.is_empty() {
                    records.push(record);
                }
            }
        }
    }
    records.sort_by(|a, b| {
        b.updated_at_epoch
            .cmp(&a.updated_at_epoch)
            .then_with(|| b.id.cmp(&a.id))
    });
    records
}

pub fn delete_session(session_id: &str) -> Result<(), String> {
    let path = session_path(session_id);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("delete {}: {e}", path.display()))?;
    }
    Ok(())
}

pub fn fork_session(source: &SessionRecord, new_id: &str) -> SessionRecord {
    let now = now_epoch();
    let mut forked = source.clone();
    forked.id = new_id.to_string();
    forked.title = source
        .title
        .as_ref()
        .map(|t| format!("{t} (fork)"));
    forked.created_at_epoch = now;
    forked.updated_at_epoch = now;
    forked
}

/// One configured MCP server entry in `$DSH_HOME/dsh-tui/mcp.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub enabled: bool,
}

impl Default for McpServer {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpRegistry {
    pub servers: Vec<McpServer>,
}

impl McpRegistry {
    pub fn load() -> Self {
        let path = mcp_path();
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = mcp_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize mcp registry: {e}"))?;
        fs::write(&path, format!("{body}\n"))
            .map_err(|e| format!("write {}: {e}", path.display()))
    }

    pub fn upsert(&mut self, server: McpServer) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == server.name) {
            *existing = server;
        } else {
            self.servers.push(server);
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|s| s.name != name);
        self.servers.len() != before
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.enabled = enabled;
            true
        } else {
            false
        }
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Human-friendly relative timestamp (`przed chwilą`, `5m temu`, `2h temu`, `3d temu`).
pub fn format_relative_time(epoch: u64) -> String {
    if epoch == 0 {
        return "nieznana".to_string();
    }
    let now = now_epoch();
    let diff = now.saturating_sub(epoch);
    if diff < 60 {
        "przed chwilą".to_string()
    } else if diff < 3600 {
        format!("{}m temu", diff / 60)
    } else if diff < 86_400 {
        format!("{}h temu", diff / 3600)
    } else {
        format!("{}d temu", diff / 86_400)
    }
}

/// Format unix epoch seconds as `YYYY-MM-DD HH:MM UTC` without external date crates.
pub fn format_epoch_utc(epoch: u64) -> String {
    let days = (epoch / 86_400) as i64;
    let rem = epoch % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Howard Hinnant's civil-from-days algorithm (epoch 1970-01-01 = day 0).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m as u32, d as u32)
}

/// Shorten a filesystem path by replacing `$HOME` with `~`.
pub fn shorten_home(path: &Path) -> String {
    let text = path.display().to_string();
    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() && text.starts_with(&home) {
            return format!("~{}", &text[home.len()..]);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn effort_and_model_aliases_normalize() {
        assert_eq!(resolve_model_alias("flash"), "deepseek-flash");
        assert_eq!(resolve_model_alias("pro"), "deepseek-v4-pro");
        assert_eq!(resolve_model_alias("deepseek-v4-pro"), "deepseek-v4-pro");

        assert_eq!(normalize_effort("off"), Some("off"));
        assert_eq!(normalize_effort("medium"), Some("high"));
        assert_eq!(normalize_effort("xhigh"), Some("max"));
        assert_eq!(normalize_effort("unknown"), None);
    }

    #[test]
    fn execution_mode_roundtrip_and_cycle() {
        assert_eq!(
            ExecutionMode::parse("accept-edits"),
            Some(ExecutionMode::WorkspaceWrite)
        );
        assert_eq!(ExecutionMode::parse("auto"), Some(ExecutionMode::Auto));
        assert_eq!(
            ExecutionMode::Auto.dsh_preset(),
            "danger-full-access"
        );
        assert_eq!(ExecutionMode::Plan.dsh_preset(), "read-only");
        assert_eq!(
            ExecutionMode::WorkspaceWrite.cycle_next(),
            ExecutionMode::Auto
        );
        assert_eq!(
            ExecutionMode::ReadOnly.cycle_next(),
            ExecutionMode::WorkspaceWrite
        );
    }

    #[test]
    fn token_usage_tracks_steps_and_fill() {
        let mut usage = TokenUsage::default();
        usage.record_step(&serde_json::json!({
            "inputTokens": 1000,
            "outputTokens": 200,
            "cacheReadTokens": 8000,
            "cacheWriteTokens": 0,
            "totalTokens": 9200
        }));
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 200);
        assert_eq!(usage.cache_read_tokens, 8000);
        assert_eq!(usage.last_context_tokens, 9200);
        assert!(usage.fill_percent() > 0.8 && usage.fill_percent() < 1.0);
        assert_eq!(format_tokens(9200), "9.2k");
        assert_eq!(format_tokens(1_048_576), "1.0M");
    }

    #[test]
    fn settings_and_sessions_persist_under_dsh_home() {
        let _guard = env_lock().lock().unwrap();
        let temp_dir = env::temp_dir().join(format!("dsh-tui-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        unsafe {
            env::set_var("DSH_HOME", &temp_dir);
        }

        let mut settings = Settings::default();
        settings
            .apply_override(r#"{"model":"pro","effort":"max","theme":"sepia","auto_approve":true}"#)
            .unwrap();
        assert_eq!(settings.model, "deepseek-v4-pro");
        assert_eq!(settings.effort, "max");
        assert_eq!(settings.theme, ThemeName::Sepia);
        assert!(settings.auto_approve);
        settings.save().unwrap();
        let loaded = Settings::load();
        assert_eq!(loaded, settings);

        let record = SessionRecord {
            id: "session-test-1".to_string(),
            title: Some("Testowa rozmowa".to_string()),
            cwd: "/tmp".to_string(),
            model: "deepseek-flash".to_string(),
            effort: "high".to_string(),
            mode: "workspace-write".to_string(),
            entries: vec![
                ChatEntry::User {
                    text: "Cześć".to_string(),
                },
                ChatEntry::Assistant {
                    text: "Hej!".to_string(),
                },
            ],
            ..SessionRecord::default()
        };
        save_session(&record).unwrap();
        let listed = list_sessions();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("Testowa rozmowa"));

        let loaded_sess = load_session("session-test-1").unwrap();
        assert_eq!(loaded_sess.entries.len(), 2);

        let mut mcp = McpRegistry::default();
        mcp.upsert(McpServer {
            name: "fs".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server".to_string()],
            env: BTreeMap::new(),
            enabled: true,
        });
        mcp.save().unwrap();
        assert_eq!(McpRegistry::load().servers.len(), 1);

        let _ = fs::remove_dir_all(&temp_dir);
        unsafe {
            env::remove_var("DSH_HOME");
        }
    }

    #[test]
    fn civil_date_formats_epoch_correctly() {
        assert_eq!(format_epoch_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_epoch_utc(1_700_000_000), "2023-11-14 22:13 UTC");
    }
}
