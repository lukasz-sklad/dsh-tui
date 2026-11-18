//! Full-screen ratatui interface for the DeepSeek Harness SDK runtime.

use crate::client::{Client, IncomingRequest, RecvOutcome};
use crate::markdown;
use crate::settings::{
    self, ChatEntry, ExecutionMode, McpRegistry, SessionRecord, Settings, ThemeName,
    ThemePalette, TodoItem, available_models, delete_session, fork_session, format_relative_time,
    format_tokens, list_sessions, normalize_effort, now_epoch, resolve_model_alias,
    save_last_session, save_session, shorten_home,
};
use crate::{LaunchSpec, new_session_id, truncate};
use base64::Engine;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TOOL_LOG_CAP: usize = 400;
const MESSAGE_CAP: usize = 400;

struct SlashCommandDef {
    name: &'static str,
    usage: &'static str,
    desc: &'static str,
    takes_arg: bool,
}

const SLASH_COMMANDS: &[SlashCommandDef] = &[
    SlashCommandDef { name: "/help", usage: "/help", desc: "Skróty klawiszowe i lista komend", takes_arg: false },
    SlashCommandDef { name: "/model", usage: "/model [id]", desc: "Wybierz model i poziom effort", takes_arg: false },
    SlashCommandDef { name: "/effort", usage: "/effort [poziom]", desc: "Zmień poziom rozumowania (off|low|high|max)", takes_arg: false },
    SlashCommandDef { name: "/mode", usage: "/mode [tryb]", desc: "Tryb uprawnień (workspace-write|auto|plan|read-only)", takes_arg: false },
    SlashCommandDef { name: "/permissions", usage: "/permissions [tryb]", desc: "Alias dla /mode", takes_arg: false },
    SlashCommandDef { name: "/load", usage: "/load", desc: "Przeglądaj i wznawiaj zapisane sesje (Ctrl+O)", takes_arg: false },
    SlashCommandDef { name: "/resume", usage: "/resume", desc: "Alias dla /load", takes_arg: false },
    SlashCommandDef { name: "/sessions", usage: "/sessions", desc: "Alias dla /load", takes_arg: false },
    SlashCommandDef { name: "/diff", usage: "/diff", desc: "Podgląd zmian git diff w repozytorium", takes_arg: false },
    SlashCommandDef { name: "/context", usage: "/context", desc: "Statystyki okna kontekstu 1M i środowiska", takes_arg: false },
    SlashCommandDef { name: "/usage", usage: "/usage", desc: "Alias dla /context", takes_arg: false },
    SlashCommandDef { name: "/tasks", usage: "/tasks", desc: "Lista zadań agenta, cel i podagenci", takes_arg: false },
    SlashCommandDef { name: "/settings", usage: "/settings", desc: "Konfiguracja motywów, myślenia i paneli", takes_arg: false },
    SlashCommandDef { name: "/theme", usage: "/theme [dark|light|sepia|neon]", desc: "Przełącz motyw kolorystyczny", takes_arg: true },
    SlashCommandDef { name: "/thinking", usage: "/thinking", desc: "Pokaż/ukryj bloki myślenia modelu (Ctrl+R)", takes_arg: false },
    SlashCommandDef { name: "/tools", usage: "/tools", desc: "Pokaż/ukryj boczny panel narzędzi (Ctrl+T)", takes_arg: false },
    SlashCommandDef { name: "/new", usage: "/new", desc: "Rozpocznij nową sesję", takes_arg: false },
    SlashCommandDef { name: "/clear", usage: "/clear", desc: "Wyczyść bieżący widok czatu", takes_arg: false },
    SlashCommandDef { name: "/rename", usage: "/rename <tytuł>", desc: "Zmień tytuł bieżącej rozmowy", takes_arg: true },
    SlashCommandDef { name: "/title", usage: "/title <tytuł>", desc: "Alias dla /rename", takes_arg: true },
    SlashCommandDef { name: "/fork", usage: "/fork", desc: "Utwórz kopię (fork) bieżącej sesji", takes_arg: false },
    SlashCommandDef { name: "/rewind", usage: "/rewind [n]", desc: "Cofnij ostatnie n tur w widoku", takes_arg: true },
    SlashCommandDef { name: "/copy", usage: "/copy [last|code|all]", desc: "Kopiuj odpowiedź, blok kodu lub czat", takes_arg: false },
    SlashCommandDef { name: "/export", usage: "/export [plik.md]", desc: "Zapisz transkrypt rozmowy do Markdown", takes_arg: true },
    SlashCommandDef { name: "/search", usage: "/search <fraza>", desc: "Szukaj w historii rozmowy (Ctrl+F)", takes_arg: true },
    SlashCommandDef { name: "/cwd", usage: "/cwd [ścieżka]", desc: "Pokaż lub zmień katalog roboczy agenta", takes_arg: true },
    SlashCommandDef { name: "/add-dir", usage: "/add-dir <ścieżka>", desc: "Dodaj katalog do kontekstu @wzmianek", takes_arg: true },
    SlashCommandDef { name: "/plan", usage: "/plan [zadanie]", desc: "Przełącz tryb plan lub wyślij plan", takes_arg: false },
    SlashCommandDef { name: "/goal", usage: "/goal <cel>", desc: "Uruchom autonomiczną pętlę celu w dsh", takes_arg: true },
    SlashCommandDef { name: "/compact", usage: "/compact", desc: "Kompaktuj historię kontekstu w dsh", takes_arg: false },
    SlashCommandDef { name: "/btw", usage: "/btw <pytanie>", desc: "Szybkie pytanie poboczne do agenta", takes_arg: true },
    SlashCommandDef { name: "/ask", usage: "/ask <treść>", desc: "Jawne wysłanie promptu", takes_arg: true },
    SlashCommandDef { name: "/mcp", usage: "/mcp", desc: "Pokaż skonfigurowane serwery MCP", takes_arg: false },
    SlashCommandDef { name: "/skills", usage: "/skills", desc: "Lista wykrytych skilli lokalnych", takes_arg: false },
    SlashCommandDef { name: "/agents", usage: "/agents", desc: "Informacje o trybach agenta i podagentach", takes_arg: false },
    SlashCommandDef { name: "/changelog", usage: "/changelog", desc: "Lista zmian dsh-tui", takes_arg: false },
    SlashCommandDef { name: "/exit", usage: "/exit", desc: "Zapisz sesję i wyjdź", takes_arg: false },
];

struct RunningTool {
    name: String,
    summary: String,
    diff: Option<String>,
    started: Instant,
}

#[derive(Clone)]
struct SubagentActivity {
    id: String,
    status: String,
}

#[derive(Clone)]
struct PendingReload {
    model: String,
    effort: String,
    mode: ExecutionMode,
    target_session_id: Option<String>,
    loaded_record: Option<SessionRecord>,
    reason_banner: String,
}

#[derive(Clone)]
struct CompletionItem {
    insert: String,
    label: String,
    detail: String,
}

enum CompletionKind {
    Slash,
    FileMention { token_start: usize, token_end: usize },
}

enum Overlay {
    ModelSelector {
        selected: usize,
        effort_idx: usize,
    },
    EffortSelector {
        selected: usize,
    },
    ModeSelector {
        selected: usize,
    },
    SessionPicker {
        sessions: Vec<SessionRecord>,
        query: String,
        selected: usize,
    },
    DiffViewer {
        lines: Vec<String>,
        scroll: usize,
        title: String,
    },
    ContextView,
    TasksView {
        scroll: usize,
    },
    SettingsMenu {
        selected: usize,
    },
    HelpView {
        scroll: usize,
    },
    SearchInput {
        query: String,
    },
}

struct App {
    spec: LaunchSpec,
    session: SessionRecord,
    settings: Settings,
    mode: ExecutionMode,
    base_auto_approve: bool,
    auto_approve: bool,
    git_branch: Option<String>,
    assistant_draft: String,
    reasoning_draft: String,
    running_tool: Option<RunningTool>,
    subagents: Vec<SubagentActivity>,
    turn_active: bool,
    received_inbox: bool,
    message_id: Option<String>,
    command_id: Option<String>,
    last_reason: Option<String>,
    input: String,
    cursor: usize,
    queued_prompts: VecDeque<String>,
    scroll: usize,
    approvals: VecDeque<IncomingRequest>,
    questions: VecDeque<IncomingRequest>,
    question_answers: Vec<(String, Vec<String>)>,
    pending_reload: Option<PendingReload>,
    overlay: Option<Overlay>,
    completion_idx: usize,
    selecting: bool,
    select_anchor: Option<usize>,
    select_cursor: usize,
    wrapped_lines: Vec<String>,
    visible_start: usize,
    visible_end: usize,
    frame: u64,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: String,
    quit: bool,
    cursor_pos: Option<(u16, u16)>,
    cached_files: Vec<String>,
    cached_files_cwd: String,
}

impl App {
    fn new(
        session_id: String,
        loaded_session: Option<SessionRecord>,
        spec: LaunchSpec,
    ) -> App {
        let mut session = loaded_session.unwrap_or_else(|| SessionRecord {
            id: session_id.clone(),
            title: None,
            cwd: spec.cwd.clone(),
            model: spec.model.clone(),
            effort: spec.effort.clone(),
            mode: spec.mode.as_str().to_string(),
            ..SessionRecord::default()
        });
        if session.entries.is_empty() {
            session.entries.push(ChatEntry::Info {
                text: format!(
                    "Sesja {} · /help pomoc · Tab uzupełnianie / i @ · Shift+Tab zmiana trybu · Ctrl+O sesje",
                    session.id
                ),
            });
        } else {
            session.entries.push(ChatEntry::Info {
                text: format!(
                    "Wznowiono sesję {} ({} wpisów) · model {} · effort {}",
                    session.id,
                    session.entries.len(),
                    spec.model,
                    spec.effort
                ),
            });
        }
        session.model = spec.model.clone();
        session.effort = spec.effort.clone();
        session.mode = spec.mode.as_str().to_string();
        if session.cwd.is_empty() {
            session.cwd = spec.cwd.clone();
        }

        let git_branch = detect_git_branch(&spec.cwd);
        let settings = spec.settings.clone();
        let mode = spec.mode;
        let auto_approve = spec.auto_approve;

        App {
            spec,
            session,
            settings,
            mode,
            base_auto_approve: auto_approve,
            auto_approve,
            git_branch,
            assistant_draft: String::new(),
            reasoning_draft: String::new(),
            running_tool: None,
            subagents: Vec::new(),
            turn_active: false,
            received_inbox: false,
            message_id: None,
            command_id: None,
            last_reason: None,
            input: String::new(),
            cursor: 0,
            queued_prompts: VecDeque::new(),
            scroll: 0,
            approvals: VecDeque::new(),
            questions: VecDeque::new(),
            question_answers: Vec::new(),
            pending_reload: None,
            overlay: None,
            completion_idx: 0,
            selecting: false,
            select_anchor: None,
            select_cursor: 0,
            wrapped_lines: Vec::new(),
            visible_start: 0,
            visible_end: 0,
            frame: 0,
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            quit: false,
            cursor_pos: None,
            cached_files: Vec::new(),
            cached_files_cwd: String::new(),
        }
    }

    fn palette(&self) -> ThemePalette {
        self.settings.theme.palette()
    }

    fn info(&mut self, text: &str) {
        self.session.entries.push(ChatEntry::Info {
            text: text.to_string(),
        });
        if self.session.entries.len() > MESSAGE_CAP {
            self.session.entries.remove(0);
        }
    }

    fn tool_log(&mut self, text: String) {
        self.session.tool_log.push(text);
        if self.session.tool_log.len() > TOOL_LOG_CAP {
            self.session.tool_log.remove(0);
        }
    }

    fn drain_stderr(&mut self, client: &Client) {
        while let Some(line) = client.try_recv_stderr() {
            self.tool_log(format!("ℹ {line}"));
        }
    }

    fn persist_session(&mut self) {
        self.session.model = self.spec.model.clone();
        self.session.effort = self.spec.effort.clone();
        self.session.mode = self.mode.as_str().to_string();
        self.session.updated_at_epoch = now_epoch();
        save_last_session(&self.session.id);
        let _ = save_session(&self.session);
    }

    fn refresh_files_cache(&mut self, force: bool) {
        if !force && self.cached_files_cwd == self.spec.cwd && !self.cached_files.is_empty() {
            return;
        }
        let mut roots = vec![PathBuf::from(&self.spec.cwd)];
        for extra in &self.spec.add_dirs {
            roots.push(PathBuf::from(extra));
        }
        self.cached_files = scan_workspace_files(&roots, 4, 500);
        self.cached_files_cwd = self.spec.cwd.clone();
    }

    fn active_completions(&mut self) -> Option<(CompletionKind, Vec<CompletionItem>)> {
        if self.overlay.is_some()
            || !self.approvals.is_empty()
            || !self.questions.is_empty()
            || self.selecting
        {
            return None;
        }
        let before = &self.input[..self.cursor];
        // 1. Slash command completion (starts with `/`, no whitespace before cursor)
        if before.starts_with('/') && !before.contains(char::is_whitespace) && !self.input.contains('\n') {
            let needle = before.to_ascii_lowercase();
            let items: Vec<CompletionItem> = SLASH_COMMANDS
                .iter()
                .filter(|cmd| cmd.name.starts_with(&needle))
                .map(|cmd| CompletionItem {
                    insert: if cmd.takes_arg {
                        format!("{} ", cmd.name)
                    } else {
                        cmd.name.to_string()
                    },
                    label: cmd.usage.to_string(),
                    detail: cmd.desc.to_string(),
                })
                .collect();
            if items.is_empty() {
                return None;
            }
            // If exact match with no extra chars and user already typed full command, still show if partial matches exist or just 1 exact?
            if items.len() == 1 && items[0].insert.trim() == before && !before.ends_with(' ') {
                return Some((CompletionKind::Slash, items));
            }
            return Some((CompletionKind::Slash, items));
        }

        // 2. `@path` file mention completion
        if let Some((token_start, query)) = active_at_token(before) {
            self.refresh_files_cache(false);
            let q = query.to_ascii_lowercase();
            let mut items: Vec<CompletionItem> = self
                .cached_files
                .iter()
                .filter(|path| q.is_empty() || path.to_ascii_lowercase().contains(&q))
                .take(8)
                .map(|path| CompletionItem {
                    insert: format!("@{path} "),
                    label: format!("@{path}"),
                    detail: if path.ends_with('/') {
                        "katalog".to_string()
                    } else {
                        "plik".to_string()
                    },
                })
                .collect();
            if items.is_empty() {
                return None;
            }
            items.truncate(8);
            return Some((
                CompletionKind::FileMention {
                    token_start,
                    token_end: self.cursor,
                },
                items,
            ));
        }

        None
    }

    fn apply_completion(&mut self) -> bool {
        let Some((kind, items)) = self.active_completions() else {
            return false;
        };
        let idx = self.completion_idx.min(items.len().saturating_sub(1));
        let chosen = items[idx].clone();
        match kind {
            CompletionKind::Slash => {
                self.input = chosen.insert;
                self.cursor = self.input.len();
            }
            CompletionKind::FileMention {
                token_start,
                token_end,
            } => {
                let mut replacement = chosen.insert;
                if replacement.ends_with("/ ") {
                    replacement.pop();
                }
                self.input.replace_range(token_start..token_end, &replacement);
                self.cursor = token_start + replacement.len();
            }
        }
        self.completion_idx = 0;
        true
    }

    /// Handle one pressed key. Returns `false` when the app should quit.
    fn handle_key(&mut self, client: &mut Client, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if self.selecting {
                        self.copy_selection();
                        return true;
                    }
                    if self.overlay.is_some() {
                        self.overlay = None;
                        return true;
                    }
                    self.persist_session();
                    self.quit = true;
                    return false;
                }
                KeyCode::Char('d') if self.input.is_empty() && !self.selecting && self.overlay.is_none() => {
                    self.persist_session();
                    self.quit = true;
                    return false;
                }
                KeyCode::Char('o') => {
                    self.open_session_picker();
                    return true;
                }
                KeyCode::Char('t') => {
                    self.settings.show_tools_panel = !self.settings.show_tools_panel;
                    let _ = self.settings.save();
                    return true;
                }
                KeyCode::Char('r') => {
                    self.settings.show_thinking = !self.settings.show_thinking;
                    let _ = self.settings.save();
                    return true;
                }
                KeyCode::Char('f') => {
                    self.overlay = Some(Overlay::SearchInput {
                        query: String::new(),
                    });
                    return true;
                }
                KeyCode::Char('l') => {
                    self.session.entries.clear();
                    self.info("wyczyszczono widok (historia zapisana w sesji)");
                    return true;
                }
                KeyCode::Char('s') => {
                    self.selecting = !self.selecting;
                    if self.selecting {
                        self.select_anchor = None;
                        self.select_cursor =
                            self.visible_start.max(self.visible_end.saturating_sub(1));
                        self.info("tryb zaznaczania: ↑/↓ ruch · Enter kopiuj · Esc wyjście");
                    } else {
                        self.select_anchor = None;
                    }
                    return true;
                }
                _ => {}
            }
        }

        // Priority 1: agent questions
        if !self.questions.is_empty() {
            return self.handle_question_key(client, key);
        }

        // Priority 2: approval requests
        if !self.approvals.is_empty() {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('y') => {
                    self.answer_approval(client, "allowed-once");
                }
                KeyCode::Char('A') => {
                    self.auto_approve = true;
                    self.mode = ExecutionMode::Auto;
                    self.answer_approval(client, "allowed-once");
                    self.info("włączono auto-aprobatę dla bieżącej sesji (tryb auto)");
                }
                KeyCode::Char('r') | KeyCode::Char('n') => {
                    self.answer_approval(client, "rejected");
                }
                KeyCode::Char('c') | KeyCode::Esc => {
                    self.answer_approval(client, "cancelled");
                }
                _ => {}
            }
            return true;
        }

        // Priority 3: active modal overlay
        if self.overlay.is_some() {
            return self.handle_overlay_key(client, key);
        }

        // Priority 4: selection mode
        if self.selecting {
            return self.handle_selection_key(key);
        }

        // Global Shift+Tab: cycle execution mode
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            let next = self.mode.cycle_next();
            self.apply_mode(client, next);
            return true;
        }

        // Handle active `/` or `@` completion dropdown
        if let Some((kind, items)) = self.active_completions() {
            match key.code {
                KeyCode::Tab => {
                    self.apply_completion();
                    return true;
                }
                KeyCode::Up => {
                    if self.completion_idx > 0 {
                        self.completion_idx -= 1;
                    } else {
                        self.completion_idx = items.len().saturating_sub(1);
                    }
                    return true;
                }
                KeyCode::Down => {
                    self.completion_idx = (self.completion_idx + 1) % items.len();
                    return true;
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
                    if matches!(kind, CompletionKind::Slash) {
                        let chosen = items[self.completion_idx.min(items.len() - 1)].clone();
                        let trimmed_chosen = chosen.insert.trim();
                        if self.input.trim() != trimmed_chosen {
                            self.input = chosen.insert;
                            self.cursor = self.input.len();
                            self.completion_idx = 0;
                            // If command does not expect an argument, execute it directly.
                            if !self.input.ends_with(' ') {
                                self.submit(client);
                            }
                            return true;
                        }
                    }
                }
                KeyCode::Esc => {
                    // Close file mention / clear slash hint cursor without breaking text
                    if matches!(kind, CompletionKind::Slash) {
                        self.input.clear();
                        self.cursor = 0;
                        self.completion_idx = 0;
                        return true;
                    }
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc if self.turn_active && !self.queued_prompts.is_empty() => {
                self.queued_prompts.clear();
                self.info("wyczyszczono kolejkę oczekujących promptów");
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.input.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            KeyCode::Enter => {
                if self.input.ends_with('\\') {
                    let position = self.input.len() - 1;
                    self.input.replace_range(position.., "\n");
                    self.cursor = self.input.len();
                } else {
                    self.submit(client);
                }
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(8),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Up => self.history_up(),
            KeyCode::Down => self.history_down(),
            _ => {
                self.edit_input(key);
                self.completion_idx = 0;
            }
        }
        true
    }

    fn handle_selection_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.selecting = false;
                self.select_anchor = None;
            }
            KeyCode::Enter => self.copy_selection(),
            KeyCode::Up => {
                if self.select_anchor.is_none() {
                    self.select_anchor = Some(self.select_cursor);
                }
                self.select_cursor = self
                    .select_cursor
                    .saturating_sub(1)
                    .max(self.visible_start);
            }
            KeyCode::Down => {
                if self.select_anchor.is_none() {
                    self.select_anchor = Some(self.select_cursor);
                }
                self.select_cursor =
                    (self.select_cursor + 1).min(self.visible_end.saturating_sub(1));
            }
            KeyCode::PageUp => {
                if self.select_anchor.is_none() {
                    self.select_anchor = Some(self.select_cursor);
                }
                self.select_cursor = self
                    .select_cursor
                    .saturating_sub(8)
                    .max(self.visible_start);
            }
            KeyCode::PageDown => {
                if self.select_anchor.is_none() {
                    self.select_anchor = Some(self.select_cursor);
                }
                self.select_cursor =
                    (self.select_cursor + 8).min(self.visible_end.saturating_sub(1));
            }
            _ => {}
        }
        true
    }

    fn handle_overlay_key(&mut self, client: &mut Client, key: KeyEvent) -> bool {
        let Some(mut overlay) = self.overlay.take() else {
            return true;
        };
        let mut keep = true;
        match &mut overlay {
            Overlay::ModelSelector {
                selected,
                effort_idx,
            } => {
                let models = available_models();
                let efforts = ["off", "low", "high", "max"];
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(models.len().saturating_sub(1)),
                    KeyCode::Left => *effort_idx = effort_idx.saturating_sub(1),
                    KeyCode::Right => *effort_idx = (*effort_idx + 1).min(efforts.len() - 1),
                    KeyCode::Enter => {
                        let new_model = models[*selected].id.to_string();
                        let new_effort = efforts[*effort_idx].to_string();
                        keep = false;
                        self.schedule_runtime_reload(
                            new_model,
                            new_effort,
                            self.mode,
                            None,
                            None,
                            "przełączam model i poziom effort",
                        );
                    }
                    _ => {}
                }
            }
            Overlay::EffortSelector { selected } => {
                let efforts = ["off", "low", "high", "max"];
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(efforts.len() - 1),
                    KeyCode::Enter => {
                        let new_effort = efforts[*selected].to_string();
                        keep = false;
                        self.schedule_runtime_reload(
                            self.spec.model.clone(),
                            new_effort,
                            self.mode,
                            None,
                            None,
                            "przełączam poziom rozumowania",
                        );
                    }
                    _ => {}
                }
            }
            Overlay::ModeSelector { selected } => {
                let modes = ExecutionMode::all();
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(modes.len() - 1),
                    KeyCode::Enter => {
                        let next_mode = modes[*selected];
                        keep = false;
                        self.apply_mode(client, next_mode);
                    }
                    _ => {}
                }
            }
            Overlay::SessionPicker {
                sessions,
                query,
                selected,
            } => {
                let filtered: Vec<usize> = sessions
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| session_matches(s, query))
                    .map(|(i, _)| i)
                    .collect();
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        if !filtered.is_empty() {
                            *selected = (*selected + 1).min(filtered.len() - 1);
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *selected = 0;
                    }
                    KeyCode::Delete => {
                        if let Some(&orig_idx) = filtered.get(*selected) {
                            let target_id = sessions[orig_idx].id.clone();
                            if target_id != self.session.id {
                                let _ = delete_session(&target_id);
                                sessions.remove(orig_idx);
                                let remaining = sessions
                                    .iter()
                                    .filter(|s| session_matches(s, query))
                                    .count();
                                *selected = (*selected).min(remaining.saturating_sub(1));
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(&orig_idx) = filtered.get(*selected) {
                            let target = sessions[orig_idx].clone();
                            keep = false;
                            let target_model = resolve_model_alias(&target.model);
                            let target_effort = normalize_effort(&target.effort)
                                .unwrap_or(self.spec.effort.as_str())
                                .to_string();
                            let target_mode = ExecutionMode::parse(&target.mode).unwrap_or(self.mode);
                            let tid = target.id.clone();
                            self.schedule_runtime_reload(
                                target_model,
                                target_effort,
                                target_mode,
                                Some(tid.clone()),
                                Some(target),
                                &format!("wczytuję sesję {tid}"),
                            );
                        }
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        query.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
            }
            Overlay::DiffViewer { scroll, lines, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => keep = false,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = (*scroll + 1).min(lines.len().saturating_sub(1)),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(12),
                KeyCode::PageDown => *scroll = (*scroll + 12).min(lines.len().saturating_sub(1)),
                _ => {}
            },
            Overlay::ContextView => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => keep = false,
                _ => {}
            },
            Overlay::TasksView { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => keep = false,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                _ => {}
            },
            Overlay::SettingsMenu { selected } => {
                const SETTINGS_ROWS: usize = 5;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => keep = false,
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(SETTINGS_ROWS - 1),
                    KeyCode::Enter | KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                        let dir_back = key.code == KeyCode::Left;
                        match *selected {
                            0 => {
                                let themes = ThemeName::all();
                                let cur = themes
                                    .iter()
                                    .position(|t| *t == self.settings.theme)
                                    .unwrap_or(0);
                                let next = if dir_back {
                                    (cur + themes.len() - 1) % themes.len()
                                } else {
                                    (cur + 1) % themes.len()
                                };
                                self.settings.theme = themes[next];
                            }
                            1 => self.settings.show_thinking = !self.settings.show_thinking,
                            2 => self.settings.show_tools_panel = !self.settings.show_tools_panel,
                            3 => {
                                self.settings.auto_approve = !self.settings.auto_approve;
                                self.base_auto_approve = self.settings.auto_approve;
                                self.auto_approve =
                                    self.base_auto_approve || self.mode == ExecutionMode::Auto;
                            }
                            4 => {
                                let efforts = ["off", "low", "high", "max"];
                                let cur = efforts
                                    .iter()
                                    .position(|e| *e == self.settings.effort)
                                    .unwrap_or(2);
                                let next = if dir_back {
                                    (cur + efforts.len() - 1) % efforts.len()
                                } else {
                                    (cur + 1) % efforts.len()
                                };
                                self.settings.effort = efforts[next].to_string();
                            }
                            _ => {}
                        }
                        let _ = self.settings.save();
                    }
                    _ => {}
                }
            }
            Overlay::HelpView { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => keep = false,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                _ => {}
            },
            Overlay::SearchInput { query } => match key.code {
                KeyCode::Esc => keep = false,
                KeyCode::Enter => {
                    let q = query.clone();
                    keep = false;
                    if !q.trim().is_empty() {
                        self.run_search(&q);
                    }
                }
                KeyCode::Backspace => {
                    query.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    query.push(c);
                }
                _ => {}
            },
        }
        if keep {
            self.overlay = Some(overlay);
        }
        true
    }

    fn edit_input(&mut self, key: KeyEvent) {
        if self.history_index.is_some() {
            self.history_index = None;
            self.history_draft.clear();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') => {
                    let start = self.input[..self.cursor]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    self.cursor = start;
                }
                KeyCode::Char('e') => {
                    let end = self.input[self.cursor..]
                        .find('\n')
                        .map(|i| self.cursor + i)
                        .unwrap_or(self.input.len());
                    self.cursor = end;
                }
                KeyCode::Char('u') => {
                    let start = self.input[..self.cursor]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    self.input.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
                KeyCode::Char('k') => {
                    let end = self.input[self.cursor..]
                        .find('\n')
                        .map(|i| self.cursor + i)
                        .unwrap_or(self.input.len());
                    self.input.replace_range(self.cursor..end, "");
                }
                KeyCode::Char('w') => self.delete_prev_word(),
                _ => {}
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Backspace {
            self.delete_prev_word();
            return;
        }

        match key.code {
            KeyCode::Char(character) => {
                self.input.insert(self.cursor, character);
                self.cursor += character.len_utf8();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let previous = self.input[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    self.input.replace_range(previous..self.cursor, "");
                    self.cursor = previous;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    let next = self.input[self.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(index, _)| self.cursor + index)
                        .unwrap_or(self.input.len());
                    self.input.replace_range(self.cursor..next, "");
                }
            }
            KeyCode::Left => {
                self.cursor = self.input[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(index, _)| index)
                    .unwrap_or(0);
            }
            KeyCode::Right => {
                self.cursor = self.input[self.cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(index, _)| self.cursor + index)
                    .unwrap_or(self.input.len());
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            _ => {}
        }
    }

    fn delete_prev_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = self.cursor;
        let before = &self.input[..self.cursor];
        let mut iter = before.char_indices().rev().peekable();
        while let Some(&(idx, ch)) = iter.peek() {
            if ch.is_whitespace() && ch != '\n' {
                pos = idx;
                iter.next();
            } else {
                break;
            }
        }
        while let Some(&(idx, ch)) = iter.peek() {
            if !ch.is_whitespace() {
                pos = idx;
                iter.next();
            } else {
                break;
            }
        }
        self.input.replace_range(pos..self.cursor, "");
        self.cursor = pos;
    }

    fn record_history(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
        self.history_index = None;
        self.history_draft.clear();
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.history_draft = std::mem::take(&mut self.input);
                self.history_index = Some(self.history.len() - 1);
            }
            Some(0) => {}
            Some(index) => self.history_index = Some(index - 1),
        }
        if let Some(index) = self.history_index {
            self.input = self.history[index].clone();
            self.cursor = self.input.len();
        }
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.input = self.history[index + 1].clone();
        } else {
            self.history_index = None;
            self.input = std::mem::take(&mut self.history_draft);
        }
        self.cursor = self.input.len();
    }

    fn copy_text_clipboard(&mut self, text: &str, label: &str) {
        let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
        let _ = stdout.flush();
        let fallback = settings::state_dir().join("copied.txt");
        let res = fs::create_dir_all(settings::state_dir()).and_then(|_| fs::write(&fallback, text));
        match res {
            Ok(()) => self.info(&format!("skopiowano {label} (OSC 52 + {})", fallback.display())),
            Err(err) => self.info(&format!("skopiowano {label} przez OSC 52 ({err})")),
        }
    }

    fn copy_selection(&mut self) {
        let Some(anchor) = self.select_anchor else {
            self.info("najpierw zaznacz linie strzałkami ↑/↓");
            return;
        };
        let start = anchor.min(self.select_cursor);
        let end = anchor
            .max(self.select_cursor)
            .min(self.wrapped_lines.len().saturating_sub(1));
        if start >= self.wrapped_lines.len() {
            self.selecting = false;
            self.select_anchor = None;
            return;
        }
        let text = self.wrapped_lines[start..=end].join("\n");
        let count = end - start + 1;
        self.copy_text_clipboard(&text, &format!("{count} linii"));
        self.selecting = false;
        self.select_anchor = None;
    }

    fn handle_question_key(&mut self, client: &mut Client, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.answer_question(client, true),
            KeyCode::Enter => {
                if self.questions_answered() {
                    self.answer_question(client, false);
                }
            }
            KeyCode::Char(character) if ('1'..='9').contains(&character) => {
                let index = character.to_digit(10).unwrap() as usize - 1;
                self.toggle_question_option(index);
            }
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char(_) => {
                self.edit_input(key);
            }
            _ => {}
        }
        true
    }

    fn toggle_question_option(&mut self, global_index: usize) {
        let Some(request) = self.questions.front() else {
            return;
        };
        let Some(questions) = request.params.get("questions").and_then(Value::as_array) else {
            return;
        };
        let mut seen = 0usize;
        for question in questions {
            let multi = question
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let Some(options) = question.get("options").and_then(Value::as_array) else {
                continue;
            };
            let id = question.get("id").and_then(Value::as_str).unwrap_or("");
            for option in options {
                if seen == global_index {
                    let label = option.get("label").and_then(Value::as_str).unwrap_or("");
                    let Some(slot) = self.question_answers.iter_mut().find(|(qid, _)| qid == id)
                    else {
                        continue;
                    };
                    if multi {
                        if let Some(pos) = slot.1.iter().position(|existing| existing == label) {
                            slot.1.remove(pos);
                        } else {
                            slot.1.push(label.to_string());
                        }
                    } else if slot.1.first().map(String::as_str) == Some(label) {
                        slot.1.clear();
                    } else {
                        slot.1.clear();
                        slot.1.push(label.to_string());
                    }
                    return;
                }
                seen += 1;
            }
        }
    }

    fn questions_answered(&self) -> bool {
        let Some(request) = self.questions.front() else {
            return false;
        };
        let Some(questions) = request.params.get("questions").and_then(Value::as_array) else {
            return false;
        };
        for question in questions {
            let has_options = question
                .get("options")
                .and_then(Value::as_array)
                .is_some_and(|options| !options.is_empty());
            let id = question.get("id").and_then(Value::as_str).unwrap_or("");
            let answered = self
                .question_answers
                .iter()
                .any(|(question_id, selected)| question_id == id && !selected.is_empty());
            if !answered && !has_options && self.input.trim().is_empty() {
                return false;
            }
        }
        true
    }

    fn answer_question(&mut self, client: &mut Client, cancelled: bool) {
        let Some(request) = self.questions.pop_front() else {
            return;
        };
        let custom = self.input.trim().to_string();
        let result = if cancelled {
            json!({ "cancelled": true })
        } else {
            let answers: Vec<Value> = self
                .question_answers
                .iter()
                .map(|(id, selected)| {
                    if selected.is_empty() {
                        json!({ "id": id, "selected": selected, "custom": custom })
                    } else {
                        json!({ "id": id, "selected": selected })
                    }
                })
                .collect();
            json!({ "answers": answers })
        };
        if let Err(error) = client.respond(&request.id, result) {
            self.info(&format!("błąd odpowiedzi na pytanie: {error}"));
        }
        self.question_answers.clear();
        self.input.clear();
        self.cursor = 0;
    }

    fn apply_mode(&mut self, client: &mut Client, next: ExecutionMode) {
        self.mode = next;
        self.session.mode = next.as_str().to_string();
        self.auto_approve = self.base_auto_approve || next == ExecutionMode::Auto;
        self.info(&format!("tryb zmieniony na: {} ({})", next.label(), next.description()));
        if !self.turn_active {
            let cmd = format!("/permission {}", next.dsh_preset());
            let _ = client.request(
                "session/query",
                Some(json!({
                    "sessionId": self.session.id,
                    "contentBlocks": [{"type": "text", "text": cmd}],
                })),
            );
        } else {
            self.queued_prompts
                .push_front(format!("/permission {}", next.dsh_preset()));
        }
        self.persist_session();
    }

    fn schedule_runtime_reload(
        &mut self,
        model: String,
        effort: String,
        mode: ExecutionMode,
        target_session_id: Option<String>,
        loaded_record: Option<SessionRecord>,
        banner: &str,
    ) {
        self.info(&format!("{banner}…"));
        self.pending_reload = Some(PendingReload {
            model,
            effort,
            mode,
            target_session_id,
            loaded_record,
            reason_banner: banner.to_string(),
        });
    }

    fn open_session_picker(&mut self) {
        self.persist_session();
        let sessions = list_sessions();
        self.overlay = Some(Overlay::SessionPicker {
            sessions,
            query: String::new(),
            selected: 0,
        });
    }

    fn submit(&mut self, client: &mut Client) {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        self.completion_idx = 0;
        self.scroll = 0;
        if line.is_empty() {
            return;
        }
        self.record_history(&line);

        // Local shell execution via `!command`
        if let Some(cmd) = line.strip_prefix('!') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                self.info("użycie: !<polecenie shell>");
                return;
            }
            self.run_local_shell(cmd);
            return;
        }

        // Exact or parameterized local slash commands
        if self.try_handle_local_command(client, &line) {
            return;
        }

        let prompt = match line.strip_prefix("/ask ") {
            Some(rest) => rest.to_string(),
            None => match line.strip_prefix("/btw ") {
                Some(rest) => rest.to_string(),
                None => line,
            },
        };
        if prompt.trim().is_empty() {
            self.info("użycie: /ask <treść>");
            return;
        }

        if self.turn_active {
            self.queued_prompts.push_back(prompt.clone());
            self.info(&format!("dodano do kolejki ({}): {}", self.queued_prompts.len(), truncate(&prompt, 80)));
        } else {
            self.send_prompt(client, &prompt);
        }
    }

    fn try_handle_local_command(&mut self, client: &mut Client, line: &str) -> bool {
        match line {
            "/exit" | "/quit" => {
                self.persist_session();
                self.quit = true;
                return true;
            }
            "/new" => {
                self.persist_session();
                let fresh = new_session_id();
                save_last_session(&fresh);
                self.session = SessionRecord {
                    id: fresh.clone(),
                    cwd: self.spec.cwd.clone(),
                    model: self.spec.model.clone(),
                    effort: self.spec.effort.clone(),
                    mode: self.mode.as_str().to_string(),
                    ..SessionRecord::default()
                };
                self.subagents.clear();
                self.info(&format!("nowa sesja {fresh}"));
                return true;
            }
            "/clear" => {
                self.session.entries.clear();
                self.info("wyczyszczono bieżący widok czatu");
                return true;
            }
            "/fork" => {
                self.persist_session();
                let fresh = new_session_id();
                let forked = fork_session(&self.session, &fresh);
                let _ = save_session(&forked);
                save_last_session(&fresh);
                self.session = forked;
                self.info(&format!("utworzono fork sesji: {fresh}"));
                return true;
            }
            "/help" | "/keybindings" => {
                self.overlay = Some(Overlay::HelpView { scroll: 0 });
                return true;
            }
            "/model" => {
                let models = available_models();
                let selected = models
                    .iter()
                    .position(|m| m.id == self.spec.model)
                    .unwrap_or(0);
                let effort_idx = ["off", "low", "high", "max"]
                    .iter()
                    .position(|e| *e == self.spec.effort)
                    .unwrap_or(2);
                self.overlay = Some(Overlay::ModelSelector {
                    selected,
                    effort_idx,
                });
                return true;
            }
            "/effort" => {
                let selected = ["off", "low", "high", "max"]
                    .iter()
                    .position(|e| *e == self.spec.effort)
                    .unwrap_or(2);
                self.overlay = Some(Overlay::EffortSelector { selected });
                return true;
            }
            "/mode" | "/permissions" => {
                let selected = ExecutionMode::all()
                    .iter()
                    .position(|m| *m == self.mode)
                    .unwrap_or(0);
                self.overlay = Some(Overlay::ModeSelector { selected });
                return true;
            }
            "/load" | "/resume" | "/sessions" => {
                self.open_session_picker();
                return true;
            }
            "/diff" => {
                let (title, lines) = run_git_diff_lines(&self.spec.cwd);
                self.overlay = Some(Overlay::DiffViewer {
                    lines,
                    scroll: 0,
                    title,
                });
                return true;
            }
            "/context" | "/usage" => {
                self.overlay = Some(Overlay::ContextView);
                return true;
            }
            "/tasks" | "/todos" => {
                self.overlay = Some(Overlay::TasksView { scroll: 0 });
                return true;
            }
            "/settings" => {
                self.overlay = Some(Overlay::SettingsMenu { selected: 0 });
                return true;
            }
            "/thinking" => {
                self.settings.show_thinking = !self.settings.show_thinking;
                let _ = self.settings.save();
                self.info(if self.settings.show_thinking {
                    "widoczność bloków myślenia: włączona"
                } else {
                    "widoczność bloków myślenia: ukryta"
                });
                return true;
            }
            "/tools" => {
                self.settings.show_tools_panel = !self.settings.show_tools_panel;
                let _ = self.settings.save();
                self.info(if self.settings.show_tools_panel {
                    "boczny panel narzędzi: włączony"
                } else {
                    "boczny panel narzędzi: ukryty"
                });
                return true;
            }
            "/plan" => {
                self.apply_mode(client, ExecutionMode::Plan);
                return true;
            }
            "/mcp" => {
                let reg = McpRegistry::load();
                if reg.servers.is_empty() {
                    self.info(&format!(
                        "brak skonfigurowanych serwerów MCP (dodaj przez `dsh-tui mcp add ...`)"
                    ));
                } else {
                    let list: Vec<String> = reg
                        .servers
                        .iter()
                        .map(|s| {
                            format!(
                                "{} [{}]: {} {}",
                                s.name,
                                if s.enabled { "on" } else { "off" },
                                s.command,
                                s.args.join(" ")
                            )
                        })
                        .collect();
                    self.info(&format!("serwery MCP: {}", list.join(" · ")));
                }
                return true;
            }
            "/skills" => {
                let found = discover_skills(&self.spec.cwd);
                if found.is_empty() {
                    self.info("brak lokalnych skilli w .agents/skills ani ~/.agents/skills");
                } else {
                    self.info(&format!("wykryte skille ({}): {}", found.len(), found.join(", ")));
                }
                return true;
            }
            "/agents" => {
                self.info("tryby agenta: workspace-write, auto, plan, read-only · podagenci: /tasks · cele: /goal <cel>");
                return true;
            }
            "/changelog" => {
                self.info("dsh-tui v0.3.0: tryby uprawnień, /model+/effort, /load, /diff, /context, /tasks, motywy dark/light/sepia/neon, podgląd diffów, autouzupełnianie / i @");
                return true;
            }
            "/cwd" => {
                self.info(&format!("bieżący katalog roboczy: {}", self.spec.cwd));
                return true;
            }
            "/copy" => {
                self.copy_by_scope("last");
                return true;
            }
            "/export" => {
                let file = format!("chat-{}.md", self.session.id);
                self.export_history(&file);
                return true;
            }
            "/rewind" => {
                self.rewind_turns(1);
                return true;
            }
            _ => {}
        }

        if let Some(arg) = line.strip_prefix("/model ") {
            let model = resolve_model_alias(arg);
            if model.is_empty() {
                self.info("użycie: /model <id>");
            } else {
                self.schedule_runtime_reload(
                    model.clone(),
                    self.spec.effort.clone(),
                    self.mode,
                    None,
                    None,
                    &format!("przełączam model na {model}"),
                );
            }
            return true;
        }
        if let Some(arg) = line.strip_prefix("/effort ") {
            if let Some(eff) = normalize_effort(arg) {
                self.schedule_runtime_reload(
                    self.spec.model.clone(),
                    eff.to_string(),
                    self.mode,
                    None,
                    None,
                    &format!("przełączam effort na {eff}"),
                );
            } else {
                self.info("użycie: /effort <off|low|medium|high|max>");
            }
            return true;
        }
        if let Some(arg) = line
            .strip_prefix("/mode ")
            .or_else(|| line.strip_prefix("/permissions "))
        {
            if let Some(mode) = ExecutionMode::parse(arg) {
                self.apply_mode(client, mode);
            } else {
                self.info("użycie: /mode <workspace-write|accept-edits|auto|plan|read-only>");
            }
            return true;
        }
        if let Some(title) = line
            .strip_prefix("/rename ")
            .or_else(|| line.strip_prefix("/title "))
        {
            let title = title.trim();
            if title.is_empty() {
                self.info("użycie: /rename <tytuł>");
            } else {
                self.session.title = Some(title.to_string());
                self.persist_session();
                self.info(&format!("zmieniono tytuł rozmowy na: {title}"));
            }
            return true;
        }
        if let Some(arg) = line.strip_prefix("/theme ") {
            if let Some(theme) = ThemeName::parse(arg) {
                self.settings.theme = theme;
                let _ = self.settings.save();
                self.info(&format!("zmieniono motyw na: {}", theme.label()));
            } else {
                self.info("dostępne motywy: dark, light, sepia, neon");
            }
            return true;
        }
        if let Some(arg) = line.strip_prefix("/copy ") {
            self.copy_by_scope(arg.trim());
            return true;
        }
        if let Some(path) = line.strip_prefix("/export ") {
            let path = path.trim();
            if path.is_empty() {
                self.info("użycie: /export <plik>");
            } else {
                self.export_history(path);
            }
            return true;
        }
        if let Some(arg) = line.strip_prefix("/rewind ") {
            let count = arg.trim().parse::<usize>().unwrap_or(1).max(1);
            self.rewind_turns(count);
            return true;
        }
        if let Some(query) = line.strip_prefix("/search ") {
            self.run_search(query.trim());
            return true;
        }
        if let Some(path) = line.strip_prefix("/cwd ") {
            let path = path.trim();
            if !path.is_empty() {
                match client.request(
                    "session/working-directory/set",
                    Some(json!({ "sessionId": self.session.id, "path": path })),
                ) {
                    Ok(res) => {
                        let resolved = res
                            .get("cwd")
                            .and_then(Value::as_str)
                            .unwrap_or(path)
                            .to_string();
                        self.spec.cwd = resolved.clone();
                        self.session.cwd = resolved.clone();
                        self.git_branch = detect_git_branch(&resolved);
                        self.refresh_files_cache(true);
                        self.persist_session();
                        self.info(&format!("katalog roboczy zmieniony na: {resolved}"));
                    }
                    Err(err) => self.info(&format!("błąd zmiany cwd: {err}")),
                }
            }
            return true;
        }
        if let Some(path) = line.strip_prefix("/add-dir ") {
            let path = path.trim();
            if !path.is_empty() {
                self.spec.add_dirs.push(path.to_string());
                self.refresh_files_cache(true);
                self.info(&format!("dodano katalog kontekstu: {path}"));
            }
            return true;
        }
        false
    }

    fn rewind_turns(&mut self, turns: usize) {
        let mut removed_turns = 0usize;
        while removed_turns < turns {
            let Some(pos) = self
                .session
                .entries
                .iter()
                .rposition(|e| matches!(e, ChatEntry::User { .. }))
            else {
                break;
            };
            self.session.entries.truncate(pos);
            removed_turns += 1;
        }
        self.persist_session();
        self.info(&format!("cofnięto {removed_turns} turę/y w widoku rozmowy"));
    }

    fn copy_by_scope(&mut self, scope: &str) {
        match scope {
            "" | "last" => {
                let text = self
                    .session
                    .entries
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        ChatEntry::Assistant { text } => Some(text.clone()),
                        _ => None,
                    });
                match text {
                    Some(t) => self.copy_text_clipboard(&t, "ostatnią odpowiedź"),
                    None => self.info("brak odpowiedzi asystenta do skopiowania"),
                }
            }
            "code" => {
                let code = self
                    .session
                    .entries
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        ChatEntry::Assistant { text } => extract_last_code_block(text),
                        _ => None,
                    });
                match code {
                    Some(c) => self.copy_text_clipboard(&c, "ostatni blok kodu"),
                    None => self.info("brak bloku kodu w odpowiedziach"),
                }
            }
            "all" => {
                let mut buf = String::new();
                for entry in &self.session.entries {
                    match entry {
                        ChatEntry::User { text } => buf.push_str(&format!("Ty: {text}\n\n")),
                        ChatEntry::Assistant { text } => buf.push_str(&format!("Asystent:\n{text}\n\n")),
                        _ => {}
                    }
                }
                self.copy_text_clipboard(&buf, "całą rozmowę");
            }
            _ => self.info("użycie: /copy [last|code|all]"),
        }
    }

    fn run_search(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }
        let q = query.to_ascii_lowercase();
        let mut hits = 0usize;
        let mut first_sample = None;
        for entry in &self.session.entries {
            let text = match entry {
                ChatEntry::User { text }
                | ChatEntry::Assistant { text }
                | ChatEntry::Reasoning { text }
                | ChatEntry::Info { text } => text.as_str(),
                ChatEntry::Tool { summary, .. } => summary.as_str(),
            };
            if text.to_ascii_lowercase().contains(&q) {
                hits += 1;
                if first_sample.is_none() {
                    first_sample = Some(truncate(text.lines().next().unwrap_or(""), 90));
                }
            }
        }
        if hits == 0 {
            self.info(&format!("szukaj `{query}`: brak dopasowań"));
        } else {
            // Also jump scroll near the last matching line in wrapped_lines.
            if let Some(pos) = self
                .wrapped_lines
                .iter()
                .rposition(|line| line.to_ascii_lowercase().contains(&q))
            {
                let total = self.wrapped_lines.len();
                let dist = total.saturating_sub(pos + 3);
                self.scroll = dist;
            }
            let sample = first_sample.unwrap_or_default();
            self.info(&format!("szukaj `{query}`: {hits} trafień (np. {sample})"));
        }
    }

    fn run_local_shell(&mut self, cmd: &str) {
        self.tool_log(format!("⚙ !{cmd}"));
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.spec.cwd)
            .output();
        match output {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let mut combined = String::new();
                if !stdout.is_empty() {
                    combined.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !combined.is_empty() && !combined.ends_with('\n') {
                        combined.push('\n');
                    }
                    combined.push_str(&stderr);
                }
                let is_err = !out.status.success();
                if is_err {
                    self.tool_log(format!("✗ !{} (exit {code})", truncate(cmd, 40)));
                } else {
                    self.tool_log(format!("✓ !{} (exit 0)", truncate(cmd, 40)));
                }
                self.session.entries.push(ChatEntry::Tool {
                    name: "shell".to_string(),
                    summary: format!("!{cmd} (exit {code})"),
                    diff: None,
                    is_error: is_err,
                    elapsed_secs: 0.0,
                });
                if !combined.trim().is_empty() {
                    let clipped: String = combined.lines().take(30).collect::<Vec<_>>().join("\n");
                    self.session.entries.push(ChatEntry::Assistant {
                        text: format!("```sh\n{clipped}\n```"),
                    });
                }
                self.persist_session();
            }
            Err(err) => {
                self.info(&format!("błąd wykonania `!{cmd}`: {err}"));
            }
        }
    }

    fn export_history(&mut self, path: &str) {
        let title = self
            .session
            .title
            .as_deref()
            .unwrap_or(self.session.id.as_str());
        let mut text = format!("# dsh-tui · {title}\n\n");
        for entry in &self.session.entries {
            match entry {
                ChatEntry::User { text: content } => {
                    text.push_str("**Ty:** ");
                    text.push_str(content);
                    text.push_str("\n\n");
                }
                ChatEntry::Reasoning { text: content } => {
                    text.push_str("<details><summary>Myślenie</summary>\n\n");
                    text.push_str(content);
                    text.push_str("\n\n</details>\n\n");
                }
                ChatEntry::Assistant { text: content } => {
                    text.push_str("**Asystent:**\n\n");
                    text.push_str(content);
                    text.push_str("\n\n");
                }
                ChatEntry::Tool {
                    name,
                    summary,
                    diff,
                    is_error,
                    elapsed_secs,
                } => {
                    let icon = if *is_error { "✗" } else { "✓" };
                    text.push_str(&format!("- `{icon} {name}` ({elapsed_secs:.1}s): {summary}\n"));
                    if let Some(d) = diff {
                        text.push_str("\n```diff\n");
                        text.push_str(d);
                        text.push_str("\n```\n\n");
                    }
                }
                ChatEntry::Info { text: content } => {
                    text.push('_');
                    text.push_str(content);
                    text.push_str("_\n\n");
                }
            }
        }
        match fs::write(path, text) {
            Ok(()) => self.info(&format!("historia zapisana w {path}")),
            Err(error) => self.info(&format!("nie mogę zapisać {path}: {error}")),
        }
    }

    fn send_prompt(&mut self, client: &mut Client, prompt: &str) {
        while client.try_recv_notification().is_some() {}
        let method = if prompt.starts_with('/') {
            "session/query"
        } else {
            "session/prompt"
        };
        let result = client.request(
            method,
            Some(json!({
                "sessionId": self.session.id,
                "contentBlocks": [{"type": "text", "text": prompt}],
            })),
        );
        match result {
            Ok(result) => {
                self.session.entries.push(ChatEntry::User {
                    text: prompt.to_string(),
                });
                if self.session.entries.len() > MESSAGE_CAP {
                    self.session.entries.remove(0);
                }
                self.assistant_draft.clear();
                self.reasoning_draft.clear();
                self.turn_active = true;
                self.received_inbox = false;
                self.message_id = result
                    .get("messageId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.command_id = if result.pointer("/kind").and_then(Value::as_str) == Some("command") {
                    result
                        .pointer("/execution/commandId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                };
                self.last_reason = None;
            }
            Err(error) => {
                self.info(&format!("błąd wysyłki: {error}"));
            }
        }
    }

    fn answer_approval(&mut self, client: &mut Client, outcome: &str) {
        let Some(request) = self.approvals.pop_front() else {
            return;
        };
        let result = json!({ "outcome": outcome });
        if let Err(error) = client.respond(&request.id, result) {
            self.info(&format!("błąd odpowiedzi na aprobatę: {error}"));
        }
    }

    /// Consume queued runtime requests and notifications.
    fn drain(&mut self, client: &mut Client) {
        while let Some(request) = client.try_recv_request() {
            match request.method.as_str() {
                "approval/request" if self.auto_approve => {
                    let tool = request
                        .params
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    self.tool_log(format!("⚡ auto-aprobata: {tool}"));
                    let _ = client.respond(&request.id, json!({ "outcome": "allowed-once" }));
                }
                "user-question/ask" => {
                    let answers = request
                        .params
                        .get("questions")
                        .and_then(Value::as_array)
                        .map(|questions| {
                            questions
                                .iter()
                                .map(|question| {
                                    (
                                        question
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                            .to_string(),
                                        Vec::new(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    self.question_answers = answers;
                    self.questions.push_back(request);
                }
                _ => self.approvals.push_back(request),
            }
        }
        while let Some(notification) = client.try_recv_notification() {
            self.handle_notification(&notification);
        }
    }

    fn handle_notification(&mut self, notification: &crate::client::Notification) {
        match notification.method.as_str() {
            "session.event" => {
                if notification.params.pointer("/sessionId").and_then(Value::as_str)
                    != Some(self.session.id.as_str())
                {
                    return;
                }
                if let Some(event) = notification.params.get("event") {
                    self.handle_event(event);
                }
            }
            "session.status" => {
                let same_session = notification.params.pointer("/sessionId").and_then(Value::as_str)
                    == Some(self.session.id.as_str());
                match notification.params.pointer("/status").and_then(Value::as_str) {
                    Some("running") if same_session => self.turn_active = true,
                    Some("idle") if same_session && self.turn_active && self.received_inbox => {
                        self.finalize_turn()
                    }
                    _ => {}
                }
            }
            "session.query.completed" => {
                let same_session = notification.params.pointer("/sessionId").and_then(Value::as_str)
                    == Some(self.session.id.as_str());
                let same_command = notification.params.pointer("/commandId").and_then(Value::as_str)
                    == self.command_id.as_deref();
                if same_session && same_command {
                    let final_response = notification
                        .params
                        .pointer("/finalResponse")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let finish_reason = notification
                        .params
                        .pointer("/finishReason")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !final_response.is_empty() {
                        self.session.entries.push(ChatEntry::Assistant {
                            text: final_response.to_string(),
                        });
                        if self.session.entries.len() > MESSAGE_CAP {
                            self.session.entries.remove(0);
                        }
                    }
                    self.last_reason = Some(finish_reason.to_string());
                    self.turn_active = false;
                    self.command_id = None;
                    self.received_inbox = false;
                    self.persist_session();
                }
            }
            "subagent.started" => {
                let child = notification
                    .params
                    .pointer("/childSessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                self.tool_log(format!("⚙ podagent wystartował {child}"));
                self.subagents.push(SubagentActivity {
                    id: child.to_string(),
                    status: "running".to_string(),
                });
            }
            "subagent.finished" => {
                let child = notification
                    .params
                    .pointer("/childSessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let status = notification
                    .params
                    .pointer("/status")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                self.tool_log(format!("⚙ podagent zakończył {child} ({status})"));
                if let Some(sub) = self.subagents.iter_mut().find(|s| s.id == child) {
                    sub.status = status.to_string();
                } else {
                    self.subagents.push(SubagentActivity {
                        id: child.to_string(),
                        status: status.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, event: &Value) {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return;
        };
        match event_type {
            "agent/inbox/spliced" => {
                let Some(inserted) = event.pointer("/data/inserted").and_then(Value::as_array)
                else {
                    return;
                };
                let expected = self.message_id.as_deref().unwrap_or("");
                for message in inserted {
                    if message.get("id").and_then(Value::as_str) == Some(expected) {
                        self.received_inbox = true;
                    }
                }
            }
            "session/title" => {
                if let Some(title) = event.pointer("/data/title").and_then(Value::as_str) {
                    if !title.trim().is_empty() {
                        self.session.title = Some(title.trim().to_string());
                    }
                }
            }
            "request/context" => {
                if let Some(cw) = event.pointer("/data/contextWindow").and_then(Value::as_u64) {
                    self.session.usage.context_window = cw;
                }
            }
            "permission/preset" => {
                if let Some(preset) = event.pointer("/data/preset").and_then(Value::as_str) {
                    if let Some(m) = ExecutionMode::parse(preset) {
                        // Preserve Plan distinction if user explicitly selected Plan (both map to read-only preset).
                        if !(self.mode == ExecutionMode::Plan && m == ExecutionMode::ReadOnly) {
                            self.mode = m;
                            self.session.mode = m.as_str().to_string();
                            self.auto_approve =
                                self.base_auto_approve || m == ExecutionMode::Auto;
                        }
                    }
                }
            }
            "todo/write" => {
                if let Some(items) = event.pointer("/data/todos").and_then(Value::as_array) {
                    let parsed: Vec<TodoItem> = items
                        .iter()
                        .filter_map(|v| serde_json::from_value::<TodoItem>(v.clone()).ok())
                        .collect();
                    self.session.todos = parsed;
                }
            }
            "goal/change" => {
                let op = event
                    .pointer("/data/operation")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if op == "clear" {
                    self.session.active_goal = None;
                } else if let Some(goal) = event.pointer("/data/goal") {
                    let phase = goal.get("phase").and_then(Value::as_str).unwrap_or("active");
                    let title = goal
                        .get("title")
                        .or_else(|| goal.get("goal"))
                        .or_else(|| goal.get("objective"))
                        .and_then(Value::as_str)
                        .unwrap_or("cel");
                    if phase == "active" {
                        self.session.active_goal = Some(title.to_string());
                    } else {
                        self.session.active_goal = Some(format!("[{phase}] {title}"));
                    }
                }
            }
            "assistant/message" => {
                if let Some(usage) = event.pointer("/data/usage") {
                    self.session.usage.record_step(usage);
                }
                let Some(blocks) = event
                    .pointer("/data/message/content")
                    .and_then(Value::as_array)
                else {
                    return;
                };
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("reasoning") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                self.reasoning_draft.push_str(text);
                            }
                        }
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                self.assistant_draft.push_str(text);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "tool/call" => {
                let name = event
                    .pointer("/data/name")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let arguments = event
                    .pointer("/data/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let summary = tool_summary(name, arguments);
                let diff = build_tool_diff(name, arguments);
                self.tool_log(format!("⚙ {summary}"));
                self.running_tool = Some(RunningTool {
                    name: name.to_string(),
                    summary,
                    diff,
                    started: Instant::now(),
                });
            }
            "tool/result" => {
                let is_error = event
                    .pointer("/data/message/isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let finished = self.running_tool.take();
                let (name, summary, diff, elapsed) = match finished {
                    Some(rt) => (
                        rt.name,
                        rt.summary,
                        rt.diff,
                        rt.started.elapsed().as_secs_f32(),
                    ),
                    None => ("?".to_string(), "?".to_string(), None, 0.0),
                };
                if is_error {
                    let code = event
                        .pointer("/data/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("błąd");
                    self.tool_log(format!("✗ {name} ({elapsed:.1}s) · {code}"));
                } else {
                    self.tool_log(format!("✓ {name} ({elapsed:.1}s)"));
                }
                self.session.entries.push(ChatEntry::Tool {
                    name,
                    summary,
                    diff,
                    is_error,
                    elapsed_secs: elapsed,
                });
                if self.session.entries.len() > MESSAGE_CAP {
                    self.session.entries.remove(0);
                }
            }
            "turn/end" => {
                self.last_reason = event
                    .pointer("/data/reason/kind")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }

    fn finalize_turn(&mut self) {
        if !self.reasoning_draft.trim().is_empty() {
            let reasoning = std::mem::take(&mut self.reasoning_draft);
            self.session.entries.push(ChatEntry::Reasoning { text: reasoning });
        } else {
            self.reasoning_draft.clear();
        }
        if !self.assistant_draft.is_empty() {
            let draft = std::mem::take(&mut self.assistant_draft);
            self.session.entries.push(ChatEntry::Assistant { text: draft });
        }
        while self.session.entries.len() > MESSAGE_CAP {
            self.session.entries.remove(0);
        }
        self.turn_active = false;
        self.received_inbox = false;
        self.message_id = None;
        self.git_branch = detect_git_branch(&self.spec.cwd);
        self.persist_session();
    }
}

/// Run the full-screen TUI until the user quits.
pub fn run(
    mut client: Client,
    session_id: &str,
    loaded_session: Option<SessionRecord>,
    spec: LaunchSpec,
) -> Result<(), String> {
    enable_raw_mode().map_err(|error| format!("raw mode: {error}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|error| format!("alternate screen: {error}"))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|error| format!("terminal: {error}"))?;

    let result = app_loop(&mut terminal, &mut client, session_id, loaded_session, spec);
    client.shutdown();

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn app_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &mut Client,
    session_id: &str,
    loaded_session: Option<SessionRecord>,
    spec: LaunchSpec,
) -> Result<(), String> {
    let mut app = App::new(session_id.to_string(), loaded_session, spec);
    if let Some(prompt) = app.spec.initial_prompt.take() {
        if !prompt.trim().is_empty() {
            app.send_prompt(client, &prompt);
        }
    }

    loop {
        if let Some(reload) = app.pending_reload.take() {
            let target_id = reload
                .target_session_id
                .clone()
                .unwrap_or_else(|| app.session.id.clone());
            match restart_runtime(
                client,
                &target_id,
                &reload.model,
                &reload.effort,
                reload.mode,
                &app.spec,
            ) {
                Ok(active_session) => {
                    app.spec.model = reload.model.clone();
                    app.spec.effort = reload.effort.clone();
                    app.mode = reload.mode;
                    app.auto_approve =
                        app.base_auto_approve || reload.mode == ExecutionMode::Auto;
                    if let Some(mut loaded) = reload.loaded_record {
                        loaded.id = active_session.clone();
                        app.session = loaded;
                        app.git_branch = detect_git_branch(&app.session.cwd);
                        if !app.session.cwd.is_empty() {
                            app.spec.cwd = app.session.cwd.clone();
                        }
                    } else {
                        app.session.id = active_session.clone();
                    }
                    app.turn_active = false;
                    app.received_inbox = false;
                    app.message_id = None;
                    app.command_id = None;
                    app.persist_session();
                    app.info(&format!(
                        "{} · gotowe (sesja {}, model {}, effort {})",
                        reload.reason_banner, app.session.id, app.spec.model, app.spec.effort
                    ));
                }
                Err(err) => {
                    app.info(&format!("nie udało się przeładować runtime: {err}"));
                }
            }
        }

        app.drain(client);
        app.drain_stderr(client);

        if !app.turn_active
            && app.approvals.is_empty()
            && app.questions.is_empty()
        {
            if let Some(queued) = app.queued_prompts.pop_front() {
                app.send_prompt(client, &queued);
            }
        }

        // Non-blocking / bounded wait so approvals, user-questions, spinner,
        // scrolling, and input queueing ALWAYS stay responsive even during an active turn.
        match client.recv_notification_step(Duration::from_millis(35)) {
            RecvOutcome::Item(notification) => {
                app.handle_notification(&notification);
                app.drain(client);
            }
            RecvOutcome::Timeout => {}
            RecvOutcome::Closed => {
                if app.turn_active {
                    app.finalize_turn();
                    app.info("runtime dsh zakończył pracę podczas tury");
                }
            }
        }
        app.drain_stderr(client);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .map_err(|error| format!("draw: {error}"))?;

        match app.cursor_pos {
            Some((x, y)) => {
                let _ = terminal.set_cursor_position((x, y));
                let _ = terminal.show_cursor();
            }
            None => {
                let _ = terminal.hide_cursor();
            }
        }

        if app.quit {
            break;
        }

        while event::poll(Duration::ZERO).map_err(|error| format!("poll: {error}"))? {
            let ev = event::read().map_err(|error| format!("read: {error}"))?;
            if let Event::Key(key) = ev {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    if !app.handle_key(client, key) {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

fn restart_runtime(
    client: &mut Client,
    session_id: &str,
    model: &str,
    effort: &str,
    mode: ExecutionMode,
    spec: &LaunchSpec,
) -> Result<String, String> {
    client.shutdown();
    let mut next = crate::connect(spec, model, effort, mode)?;
    match next.request("session/resume", Some(json!({"sessionId": session_id}))) {
        Ok(_) => {
            *client = next;
            Ok(session_id.to_string())
        }
        Err(error) if error.to_string().contains("not found") => {
            let fresh = new_session_id();
            save_last_session(&fresh);
            *client = next;
            Ok(fresh)
        }
        Err(error) => {
            next.shutdown();
            Err(format!("nie mogę wznowić sesji: {error}"))
        }
    }
}

// --- rendering -----------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    app.cursor_pos = None;
    let palette = app.palette();
    let area = frame.area();

    // Paint the entire frame background so Light/Sepia/Neon themes apply cleanly.
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.bg).fg(palette.fg)),
        area,
    );

    let completions = app.active_completions();
    let comp_height = completions
        .as_ref()
        .map(|(_, items)| (items.len().min(8) as u16) + 2)
        .unwrap_or(0);
    let queue_height = if app.queued_prompts.is_empty() { 0 } else { 1 };
    let input_height = (app.input.split('\n').count().saturating_add(2)).clamp(3, 8) as u16;

    let vertical = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(comp_height),
        Constraint::Length(queue_height),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, vertical[0], &palette);

    let main_area = vertical[1];
    if app.settings.show_tools_panel && main_area.width >= 78 {
        let horizontal =
            Layout::horizontal([Constraint::Min(28), Constraint::Length(40)]).split(main_area);
        draw_chat(frame, app, horizontal[0], &palette);
        draw_tools(frame, app, horizontal[1], &palette);
    } else {
        draw_chat(frame, app, main_area, &palette);
    }

    if let Some((_, items)) = completions {
        draw_completions(frame, app.completion_idx, &items, vertical[2], &palette);
    }

    if queue_height > 0 {
        let q_text = format!(
            " ⏳ W kolejce: {} (Esc czyści kolejkę)",
            app.queued_prompts.len()
        );
        frame.render_widget(
            Paragraph::new(Span::styled(q_text, Style::default().fg(palette.warning))),
            vertical[3],
        );
    }

    draw_input(frame, app, vertical[4], &palette);
    draw_status(frame, app, vertical[5], &palette);

    if let Some(request) = app.questions.front() {
        draw_question(frame, area, request, &app.question_answers, &app.input, &palette);
    } else if let Some(request) = app.approvals.front() {
        draw_approval(frame, area, request, &palette);
    } else if let Some(overlay) = &app.overlay {
        draw_overlay(frame, area, app, overlay, &palette);
    }

    if !app.questions.is_empty()
        || !app.approvals.is_empty()
        || app.overlay.is_some()
        || app.selecting
    {
        app.cursor_pos = None;
    }
    app.frame = app.frame.wrapping_add(1);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, palette: &ThemePalette) {
    let title = app
        .session
        .title
        .as_deref()
        .map(|t| truncate(t, 38))
        .unwrap_or_else(|| truncate(&app.session.id, 22));
    let mut left_spans = vec![
        Span::styled(
            " ✦ dsh-tui ",
            Style::default()
                .fg(palette.bg)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " DeepSeek Harness ",
            Style::default().fg(palette.muted).bg(palette.status_bg),
        ),
        Span::styled(
            format!("│ {title} "),
            Style::default()
                .fg(palette.fg)
                .bg(palette.status_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(goal) = &app.session.active_goal {
        left_spans.push(Span::styled(
            format!("│ 🎯 {} ", truncate(goal, 26)),
            Style::default().fg(palette.warning).bg(palette.status_bg),
        ));
    }

    let short_cwd = shorten_home(Path::new(&app.spec.cwd));
    let branch_part = app
        .git_branch
        .as_deref()
        .map(|b| format!(" ({b})"))
        .unwrap_or_default();
    let todos_part = if !app.session.todos.is_empty() {
        let done = app
            .session
            .todos
            .iter()
            .filter(|t| t.status == "completed")
            .count();
        format!(" ☑ {done}/{}", app.session.todos.len())
    } else {
        String::new()
    };
    let right_text = format!("📁 {short_cwd}{branch_part}{todos_part} ");

    let chunks =
        Layout::horizontal([Constraint::Min(20), Constraint::Percentage(45)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(palette.status_bg)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            right_text,
            Style::default().fg(palette.muted).bg(palette.status_bg),
        ))
        .alignment(ratatui::layout::Alignment::Right)
        .style(Style::default().bg(palette.status_bg)),
        chunks[1],
    );
}

fn draw_chat(frame: &mut Frame, app: &mut App, area: Rect, palette: &ThemePalette) {
    let width = area.width.saturating_sub(2).max(1) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    for entry in &app.session.entries {
        match entry {
            ChatEntry::User { text } => {
                for wrapped in wrap_paragraph(text, width.saturating_sub(2).max(1)) {
                    lines.push(Line::from(Span::styled(
                        format!("❯ {wrapped}"),
                        Style::default()
                            .fg(palette.user_fg)
                            .bg(palette.user_bg)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
                lines.push(Line::from(""));
            }
            ChatEntry::Reasoning { text } => {
                if app.settings.show_thinking {
                    lines.extend(render_thinking_block(text, width, palette, false));
                }
            }
            ChatEntry::Assistant { text } => {
                lines.extend(markdown::render_with_theme(text, width, palette));
                lines.push(Line::from(""));
            }
            ChatEntry::Tool {
                name,
                summary,
                diff,
                is_error,
                elapsed_secs,
            } => {
                let (icon, color) = if *is_error {
                    ("✗", palette.error)
                } else {
                    ("✓", palette.success)
                };
                let badge = format!("{icon} {name} ({elapsed_secs:.1}s)");
                let header = format!("  {badge} · {}", truncate(summary, width.saturating_sub(badge.len() + 6)));
                lines.push(Line::from(Span::styled(header, Style::default().fg(color))));
                if let Some(diff_text) = diff {
                    for d_line in markdown::render_diff(diff_text, width.saturating_sub(4).max(1), palette) {
                        let mut spans = vec![Span::styled("    ".to_string(), Style::default())];
                        spans.extend(d_line.spans);
                        lines.push(Line::from(spans));
                    }
                }
            }
            ChatEntry::Info { text } => {
                for wrapped in wrap_paragraph(text, width) {
                    lines.push(Line::from(Span::styled(
                        wrapped,
                        Style::default().fg(palette.muted),
                    )));
                }
            }
        }
    }

    if app.settings.show_thinking && !app.reasoning_draft.trim().is_empty() {
        lines.extend(render_thinking_block(
            &app.reasoning_draft,
            width,
            palette,
            true,
        ));
    }
    if !app.assistant_draft.is_empty() {
        lines.extend(markdown::render_with_theme(&app.assistant_draft, width, palette));
    }

    app.wrapped_lines = markdown::plain_lines(&lines);
    let height = area.height.saturating_sub(2).max(1) as usize;
    let max_scroll = lines.len().saturating_sub(height);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }
    let start = lines.len().saturating_sub(height).saturating_sub(app.scroll);
    app.visible_start = start;
    app.visible_end = lines.len().min(start + height);

    let selected_range = if app.selecting {
        app.select_anchor.map(|anchor| {
            let begin = anchor.min(app.select_cursor);
            let end = anchor
                .max(app.select_cursor)
                .min(app.wrapped_lines.len().saturating_sub(1));
            (begin, end)
        })
    } else {
        None
    };

    let highlight = Style::default()
        .bg(palette.selection_bg)
        .fg(palette.selection_fg);
    let visible: Vec<Line> = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(index, line)| {
            if let Some((begin, end)) = selected_range {
                if index >= begin && index <= end {
                    return Line::from(Span::styled(app.wrapped_lines[index].clone(), highlight));
                }
            }
            line.clone()
        })
        .collect();

    let border_color = if app.turn_active {
        palette.border_active
    } else {
        palette.border_inactive
    };
    let title = if app.scroll > 0 {
        format!(" Rozmowa (↑ +{}) ", app.scroll)
    } else {
        " Rozmowa ".to_string()
    };

    frame.render_widget(
        Paragraph::new(visible)
            .style(Style::default().bg(palette.bg).fg(palette.fg))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(border_color))
                    .style(Style::default().bg(palette.bg).fg(palette.fg)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_thinking_block(
    text: &str,
    width: usize,
    palette: &ThemePalette,
    streaming: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let label = if streaming {
        " 💭 Myślenie (na żywo)… "
    } else {
        " 💭 Myślenie "
    };
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(palette.thinking_border)
            .add_modifier(Modifier::BOLD),
    )));
    let inner_w = width.saturating_sub(3).max(1);
    for wrapped in wrap_paragraph(text.trim(), inner_w).into_iter().take(18) {
        lines.push(Line::from(vec![
            Span::styled(" │ ".to_string(), Style::default().fg(palette.thinking_border)),
            Span::styled(
                wrapped,
                Style::default()
                    .fg(palette.thinking_fg)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

fn draw_tools(frame: &mut Frame, app: &App, area: Rect, palette: &ThemePalette) {
    let width = area.width.saturating_sub(2).max(1) as usize;
    let mut wrapped: Vec<Line> = Vec::new();
    for entry in &app.session.tool_log {
        let style = tool_style(entry, palette);
        for line in wrap_paragraph(entry, width) {
            wrapped.push(Line::from(Span::styled(line, style)));
        }
    }
    if let Some(rt) = &app.running_tool {
        let spinners = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spin = spinners[(app.frame % 10) as usize];
        let elapsed = rt.started.elapsed().as_secs_f32();
        let active = format!("{spin} {} ({elapsed:.1}s)", truncate(&rt.summary, width.saturating_sub(12)));
        wrapped.push(Line::from(Span::styled(
            active,
            Style::default().fg(palette.warning).add_modifier(Modifier::BOLD),
        )));
    }
    let height = area.height.saturating_sub(2).max(1) as usize;
    let start = wrapped.len().saturating_sub(height);
    let visible: Vec<Line> = wrapped.into_iter().skip(start).collect();
    let title = if app.turn_active {
        " ⚙ Narzędzia (aktywne) "
    } else {
        " ⚙ Narzędzia "
    };
    frame.render_widget(
        Paragraph::new(visible)
            .style(Style::default().bg(palette.surface).fg(palette.fg))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(palette.border_inactive))
                    .style(Style::default().bg(palette.surface).fg(palette.fg)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_completions(
    frame: &mut Frame,
    selected: usize,
    items: &[CompletionItem],
    area: Rect,
    palette: &ThemePalette,
) {
    frame.render_widget(Clear, area);
    let max_items = items.len().min(8);
    let lines: Vec<Line> = items
        .iter()
        .take(max_items)
        .enumerate()
        .map(|(idx, item)| {
            let is_sel = idx == selected.min(max_items.saturating_sub(1));
            let row_style = if is_sel {
                Style::default()
                    .bg(palette.selection_bg)
                    .fg(palette.selection_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(palette.surface).fg(palette.fg)
            };
            let detail_style = if is_sel {
                row_style
            } else {
                Style::default().bg(palette.surface).fg(palette.muted)
            };
            Line::from(vec![
                Span::styled(format!(" {:<24} ", truncate(&item.label, 24)), row_style),
                Span::styled(item.detail.clone(), detail_style),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(palette.surface))
            .block(
                Block::bordered()
                    .title(" Podpowiedzi (Tab wybierz · ↑/↓ zmień) ")
                    .border_style(Style::default().fg(palette.accent))
                    .style(Style::default().bg(palette.surface).fg(palette.fg)),
            ),
        area,
    );
}

fn draw_input(frame: &mut Frame, app: &mut App, area: Rect, palette: &ThemePalette) {
    let title = if app.turn_active {
        format!(" ❯ {} (kolejkowanie aktywne) ", app.mode.label())
    } else {
        format!(" ❯ {} ", app.mode.label())
    };
    let border_color = if app.turn_active {
        palette.warning
    } else {
        palette.border_active
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(palette.surface).fg(palette.fg));
    let inner = block.inner(area);

    if app.input.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Wpisz prompt, /komenda, @plik lub !polecenie shell…",
                Style::default().fg(palette.muted),
            ))
            .style(Style::default().bg(palette.surface))
            .block(block),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(app.input.as_str())
                .style(Style::default().bg(palette.surface).fg(palette.fg))
                .block(block),
            area,
        );
    }

    let before = &app.input[..app.cursor];
    let row = before.matches('\n').count();
    let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = UnicodeWidthStr::width(&app.input[line_start..app.cursor]);
    let column = column.min(inner.width.saturating_sub(1) as usize) as u16;
    let y = inner
        .y
        .saturating_add(row as u16)
        .min(inner.y.saturating_add(inner.height.saturating_sub(1)));
    app.cursor_pos = Some((inner.x.saturating_add(column), y));
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, palette: &ThemePalette) {
    let (marker, state_label, color) = if !app.approvals.is_empty() || !app.questions.is_empty() {
        ("◆", "oczekuje na decyzję", palette.warning)
    } else if app.turn_active {
        let spinners = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        (spinners[(app.frame % 10) as usize], "pracuje", palette.warning)
    } else {
        ("●", "gotowy", palette.success)
    };

    let ctx_used = format_tokens(app.session.usage.last_context_tokens);
    let ctx_cap = format_tokens(app.session.usage.context_window);
    let ctx_pct = app.session.usage.fill_percent();
    let auto_tag = if app.auto_approve { " · AUTO-OK" } else { "" };

    let left = format!(
        " {marker} {state_label} · {} · effort:{} · {}{auto_tag} · 📊 {ctx_used}/{ctx_cap} ({ctx_pct:.1}%)",
        app.spec.model,
        app.spec.effort,
        app.mode.as_str(),
    );
    let right = if app.selecting {
        "↑/↓ zaznacz · Enter kopiuj · Esc koniec "
    } else {
        "Shift+Tab tryb · Ctrl+O sesje · Ctrl+R myśl · /help "
    };

    let chunks = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)]).split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(left, Style::default().fg(color).bg(palette.status_bg)))
            .style(Style::default().bg(palette.status_bg)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            right,
            Style::default().fg(palette.muted).bg(palette.status_bg),
        ))
        .alignment(ratatui::layout::Alignment::Right)
        .style(Style::default().bg(palette.status_bg)),
        chunks[1],
    );
}

fn draw_approval(
    frame: &mut Frame,
    area: Rect,
    request: &IncomingRequest,
    palette: &ThemePalette,
) {
    let popup = centered_rect(72, 52, area);
    frame.render_widget(Clear, popup);
    let tool = request
        .params
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let reason = request
        .params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    let width = popup.width.saturating_sub(4).max(1) as usize;
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Narzędzie: {tool}"),
            Style::default().fg(palette.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if reason.is_empty() {
        lines.push(Line::from(Span::styled(
            "bez podanego powodu",
            Style::default().fg(palette.muted),
        )));
    } else {
        for wrapped in wrap_paragraph(reason, width) {
            lines.push(Line::from(Span::styled(wrapped, Style::default().fg(palette.fg))));
        }
    }
    if let Some(args) = request.params.get("arguments").or_else(|| request.params.get("input")) {
        lines.push(Line::from(""));
        let preview = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
        for l in preview.lines().take(6) {
            lines.push(Line::from(Span::styled(
                truncate(l, width),
                Style::default().fg(palette.code_fg).bg(palette.code_bg),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[a] zezwól raz   [A] zezwól zawsze w tej sesji   [r] odrzuć   [c] anuluj",
        Style::default().fg(palette.warning).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(palette.surface).fg(palette.fg))
            .block(
                Block::bordered()
                    .title(" Aprobata wykonania narzędzia ")
                    .border_style(Style::default().fg(palette.warning))
                    .style(Style::default().bg(palette.surface).fg(palette.fg)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_question(
    frame: &mut Frame,
    area: Rect,
    request: &IncomingRequest,
    answers: &[(String, Vec<String>)],
    input: &str,
    palette: &ThemePalette,
) {
    let popup = centered_rect(80, 62, area);
    frame.render_widget(Clear, popup);
    let width = popup.width.saturating_sub(4).max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let questions = request
        .params
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| questions.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut option_number = 0usize;
    for question in questions {
        if let Some(header) = question.get("header").and_then(Value::as_str) {
            lines.push(Line::from(Span::styled(
                header.to_string(),
                Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
            )));
        }
        if let Some(text) = question.get("question").and_then(Value::as_str) {
            for wrapped in wrap_paragraph(text, width) {
                lines.push(Line::from(Span::styled(wrapped, Style::default().fg(palette.fg))));
            }
        }
        if let Some(detail) = question.get("detail").and_then(Value::as_str) {
            for wrapped in wrap_paragraph(detail, width) {
                lines.push(Line::from(Span::styled(
                    wrapped,
                    Style::default().fg(palette.muted),
                )));
            }
        }
        let question_id = question.get("id").and_then(Value::as_str).unwrap_or("");
        let selected = answers
            .iter()
            .find(|(id, _)| id == question_id)
            .map(|(_, selected)| selected.as_slice())
            .unwrap_or(&[]);
        if let Some(options) = question.get("options").and_then(Value::as_array) {
            for option in options {
                option_number += 1;
                let label = option.get("label").and_then(Value::as_str).unwrap_or("");
                let marker = if selected.iter().any(|s| s == label) {
                    "●"
                } else {
                    "○"
                };
                lines.push(Line::from(Span::styled(
                    format!(" [{option_number}] {marker} {label}"),
                    Style::default().fg(palette.accent),
                )));
            }
        }
        lines.push(Line::from(""));
    }
    if !input.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            format!("Twoja odpowiedź: {input}"),
            Style::default().fg(palette.success),
        )));
    }
    lines.push(Line::from(Span::styled(
        "1-9 wybór · pisz własną odpowiedź · Enter wyślij · Esc anuluj",
        Style::default().fg(palette.warning),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(palette.surface).fg(palette.fg))
            .block(
                Block::bordered()
                    .title(" Pytanie agenta ")
                    .border_style(Style::default().fg(palette.accent))
                    .style(Style::default().bg(palette.surface).fg(palette.fg)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_overlay(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    overlay: &Overlay,
    palette: &ThemePalette,
) {
    match overlay {
        Overlay::ModelSelector {
            selected,
            effort_idx,
        } => {
            let popup = centered_rect(72, 54, area);
            frame.render_widget(Clear, popup);
            let models = available_models();
            let efforts = ["off", "low", "high", "max"];
            let mut lines = vec![
                Line::from(Span::styled(
                    "Wybierz model (↑/↓) oraz poziom rozumowania (←/→):",
                    Style::default().fg(palette.muted),
                )),
                Line::from(""),
            ];
            for (idx, m) in models.iter().enumerate() {
                let is_sel = idx == *selected;
                let is_cur = m.id == app.spec.model;
                let prefix = if is_sel { "❯" } else { " " };
                let active_badge = if is_cur { " [aktywny]" } else { "" };
                let style = if is_sel {
                    Style::default()
                        .bg(palette.selection_bg)
                        .fg(palette.selection_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.fg)
                };
                lines.push(Line::from(Span::styled(
                    format!(
                        "{prefix} {:<17} ({}) · okno {}{active_badge}",
                        m.id,
                        m.display_name,
                        format_tokens(m.context_window)
                    ),
                    style,
                )));
                lines.push(Line::from(Span::styled(
                    format!("    {}", m.description),
                    Style::default().fg(palette.muted),
                )));
                lines.push(Line::from(""));
            }
            let mut eff_spans = vec![Span::styled(
                "Poziom effort: ".to_string(),
                Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
            )];
            for (i, e) in efforts.iter().enumerate() {
                let st = if i == *effort_idx {
                    Style::default()
                        .bg(palette.accent)
                        .fg(palette.bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.muted)
                };
                eff_spans.push(Span::styled(format!(" {e} "), st));
                eff_spans.push(Span::raw(" "));
            }
            lines.push(Line::from(eff_spans));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter zatwierdź (restart runtime + wznowienie sesji) · Esc zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, " Wybór modelu i effort ", lines, palette);
        }
        Overlay::EffortSelector { selected } => {
            let popup = centered_rect(54, 42, area);
            frame.render_widget(Clear, popup);
            let options = [
                ("off", "Wyłączone myślenie — najszybsze odpowiedzi"),
                ("low", "Krótkie rozumowanie — niskie opóźnienie"),
                ("high", "Głębokie rozumowanie (domyślne / medium)"),
                ("max", "Maksymalny budżet myślenia — najtrudniejsze zadania"),
            ];
            let mut lines = Vec::new();
            for (idx, (id, desc)) in options.iter().enumerate() {
                let is_sel = idx == *selected;
                let style = if is_sel {
                    Style::default()
                        .bg(palette.selection_bg)
                        .fg(palette.selection_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.fg)
                };
                lines.push(Line::from(Span::styled(
                    format!("{} {:<6} — {desc}", if is_sel { "❯" } else { " " }, id),
                    style,
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter zastosuj · Esc zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, " Poziom rozumowania (Effort) ", lines, palette);
        }
        Overlay::ModeSelector { selected } => {
            let popup = centered_rect(72, 46, area);
            frame.render_widget(Clear, popup);
            let mut lines = Vec::new();
            for (idx, m) in ExecutionMode::all().iter().enumerate() {
                let is_sel = idx == *selected;
                let cur = if *m == app.mode { " [aktywny]" } else { "" };
                let style = if is_sel {
                    Style::default()
                        .bg(palette.selection_bg)
                        .fg(palette.selection_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.fg)
                };
                lines.push(Line::from(Span::styled(
                    format!("{} {}{cur}", if is_sel { "❯" } else { " " }, m.label()),
                    style,
                )));
                lines.push(Line::from(Span::styled(
                    format!("    {}", m.description()),
                    Style::default().fg(palette.muted),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter wybierz · Shift+Tab szybkie przełączanie · Esc zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, " Tryb pracy i uprawnienia ", lines, palette);
        }
        Overlay::SessionPicker {
            sessions,
            query,
            selected,
        } => {
            let popup = centered_rect(82, 70, area);
            frame.render_widget(Clear, popup);
            let filtered: Vec<&SessionRecord> = sessions
                .iter()
                .filter(|s| session_matches(s, query))
                .collect();
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Szukaj: ", Style::default().fg(palette.accent).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        if query.is_empty() {
                            "(wpisz frazę aby filtrować…)".to_string()
                        } else {
                            query.clone()
                        },
                        Style::default().fg(if query.is_empty() { palette.muted } else { palette.fg }),
                    ),
                ]),
                Line::from(""),
            ];
            if filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    "Brak zapisanych sesji pasujących do filtra.",
                    Style::default().fg(palette.muted),
                )));
            } else {
                let max_rows = popup.height.saturating_sub(8) as usize;
                let start = selected.saturating_sub(max_rows.saturating_sub(1));
                for (vis_idx, s) in filtered.iter().enumerate().skip(start).take(max_rows) {
                    let is_sel = vis_idx == *selected;
                    let is_cur = s.id == app.session.id;
                    let title = s.title.as_deref().unwrap_or("(bez tytułu)");
                    let style = if is_sel {
                        Style::default()
                            .bg(palette.selection_bg)
                            .fg(palette.selection_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.fg)
                    };
                    let marker = if is_sel { "❯" } else if is_cur { "*" } else { " " };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{marker} {:<26} │ {:<14} │ {:>3} wp. │ {:<11} │ {}",
                            truncate(title, 26),
                            truncate(&s.model, 14),
                            s.entries.len(),
                            format_relative_time(s.updated_at_epoch),
                            shorten_home(Path::new(&s.cwd))
                        ),
                        style,
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter wznów · Delete usuń · ↑/↓ nawiguj · Esc zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, " Zapisane rozmowy (/load) ", lines, palette);
        }
        Overlay::DiffViewer {
            lines,
            scroll,
            title,
        } => {
            let popup = centered_rect(88, 78, area);
            frame.render_widget(Clear, popup);
            let width = popup.width.saturating_sub(4).max(1) as usize;
            let height = popup.height.saturating_sub(4).max(1) as usize;
            let joined = lines
                .iter()
                .skip(*scroll)
                .take(height)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let mut rendered = markdown::render_diff(&joined, width, palette);
            rendered.push(Line::from(Span::styled(
                "↑/↓/PgUp/PgDn przewijanie · Esc/q zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, &format!(" {title} "), rendered, palette);
        }
        Overlay::ContextView => {
            let popup = centered_rect(72, 56, area);
            frame.render_widget(Clear, popup);
            let u = &app.session.usage;
            let pct = u.fill_percent();
            let bar_w = 32usize;
            let filled = ((pct / 100.0) * bar_w as f64).round() as usize;
            let bar = format!(
                "[{}{}] {:.1}%",
                "█".repeat(filled.min(bar_w)),
                "░".repeat(bar_w.saturating_sub(filled)),
                pct
            );
            let add_dirs = if app.spec.add_dirs.is_empty() {
                "(brak)".to_string()
            } else {
                app.spec.add_dirs.join(", ")
            };
            let lines = vec![
                Line::from(Span::styled(
                    format!("Sesja: {} ({})", app.session.id, app.session.title.as_deref().unwrap_or("bez tytułu")),
                    Style::default().fg(palette.accent).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("Model: {} · Effort: {} · Tryb: {}", app.spec.model, app.spec.effort, app.mode.as_str()),
                    Style::default().fg(palette.fg),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("Okno kontekstu: {bar}  ({} / {})", format_tokens(u.last_context_tokens), format_tokens(u.context_window)),
                    Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(format!("  Input tokens (suma):       {}", u.input_tokens), Style::default().fg(palette.fg))),
                Line::from(Span::styled(format!("  Output tokens (suma):      {}", u.output_tokens), Style::default().fg(palette.fg))),
                Line::from(Span::styled(format!("  Cache read tokens (suma):  {}", u.cache_read_tokens), Style::default().fg(palette.success))),
                Line::from(Span::styled(format!("  Cache write tokens (suma): {}", u.cache_write_tokens), Style::default().fg(palette.fg))),
                Line::from(Span::styled(format!("  Total tokens (suma):       {}", u.total_tokens), Style::default().fg(palette.accent))),
                Line::from(""),
                Line::from(Span::styled(format!("Katalog roboczy: {}", app.spec.cwd), Style::default().fg(palette.muted))),
                Line::from(Span::styled(format!("Dodatkowe katalogi: {add_dirs}"), Style::default().fg(palette.muted))),
                Line::from(""),
                Line::from(Span::styled("Enter / Esc zamknij", Style::default().fg(palette.warning))),
            ];
            render_modal_box(frame, popup, " Kontekst i zużycie tokenów (/context) ", lines, palette);
        }
        Overlay::TasksView { scroll } => {
            let popup = centered_rect(76, 62, area);
            frame.render_widget(Clear, popup);
            let mut lines = Vec::new();
            if let Some(goal) = &app.session.active_goal {
                lines.push(Line::from(Span::styled(
                    format!("🎯 Aktywny cel: {goal}"),
                    Style::default().fg(palette.warning).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                format!("Zadania (todo/write): {}", app.session.todos.len()),
                Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
            )));
            if app.session.todos.is_empty() {
                lines.push(Line::from(Span::styled("  (brak aktywnych zadań)", Style::default().fg(palette.muted))));
            } else {
                for t in &app.session.todos {
                    let (icon, col) = match t.status.as_str() {
                        "completed" => ("☑", palette.success),
                        "in_progress" => ("▶", palette.warning),
                        _ => ("☐", palette.muted),
                    };
                    lines.push(Line::from(Span::styled(
                        format!("  {icon} {}", t.content),
                        Style::default().fg(col),
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Podagenci: {}", app.subagents.len()),
                Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
            )));
            if app.subagents.is_empty() {
                lines.push(Line::from(Span::styled("  (brak uruchomionych podagentów)", Style::default().fg(palette.muted))));
            } else {
                for s in &app.subagents {
                    lines.push(Line::from(Span::styled(
                        format!("  ⚙ {} — {}", s.id, s.status),
                        Style::default().fg(palette.fg),
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ przewijaj · Esc/q zamknij",
                Style::default().fg(palette.warning),
            )));
            let visible: Vec<Line> = lines.into_iter().skip(*scroll).collect();
            render_modal_box(frame, popup, " Zadania i podagenci (/tasks) ", visible, palette);
        }
        Overlay::SettingsMenu { selected } => {
            let popup = centered_rect(68, 48, area);
            frame.render_widget(Clear, popup);
            let rows = [
                ("Motyw UI", app.settings.theme.label().to_string()),
                (
                    "Pokazuj myślenie (reasoning)",
                    if app.settings.show_thinking { "Tak" } else { "Nie" }.to_string(),
                ),
                (
                    "Boczny panel narzędzi",
                    if app.settings.show_tools_panel { "Tak" } else { "Nie" }.to_string(),
                ),
                (
                    "Auto-aprobata narzędzi",
                    if app.settings.auto_approve { "Tak" } else { "Nie" }.to_string(),
                ),
                ("Domyślny effort", app.settings.effort.clone()),
            ];
            let mut lines = vec![
                Line::from(Span::styled(
                    format!("Plik konfiguracji: {}", settings::settings_path().display()),
                    Style::default().fg(palette.muted),
                )),
                Line::from(""),
            ];
            for (idx, (k, v)) in rows.iter().enumerate() {
                let is_sel = idx == *selected;
                let style = if is_sel {
                    Style::default()
                        .bg(palette.selection_bg)
                        .fg(palette.selection_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.fg)
                };
                lines.push(Line::from(Span::styled(
                    format!("{} {:<32}  {v}", if is_sel { "❯" } else { " " }, k),
                    style,
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter/Spacja/←/→ przełącz i zapisz · Esc zamknij",
                Style::default().fg(palette.warning),
            )));
            render_modal_box(frame, popup, " Ustawienia dsh-tui (/settings) ", lines, palette);
        }
        Overlay::HelpView { scroll } => {
            let popup = centered_rect(82, 74, area);
            frame.render_widget(Clear, popup);
            let mut lines = vec![
                Line::from(Span::styled(
                    "Skróty klawiszowe:",
                    Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("  Enter / Alt+Enter       Wyślij prompt / Nowa linia (też \\ na końcu)", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Tab                     Autouzupełnianie komend / i plików @", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Shift+Tab               Cykliczna zmiana trybu (workspace-write/auto/plan/read-only)", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Ctrl+O                  Lista i wznawianie zapisanych sesji (/load)", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Ctrl+T / Ctrl+R         Pokaż/ukryj panel narzędzi / bloki myślenia", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Ctrl+F / Ctrl+L         Szukaj w rozmowie / Wyczyść widok", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Ctrl+S                  Zaznaczanie linii czatu do skopiowania", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  Ctrl+A/E/W/K/U          Edycja linii (początek, koniec, usuń słowo/linię)", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  PgUp / PgDn             Przewijanie historii czatu", Style::default().fg(palette.fg))),
                Line::from(Span::styled("  !<komenda>              Wykonanie lokalnej komendy shell w cwd", Style::default().fg(palette.fg))),
                Line::from(""),
                Line::from(Span::styled(
                    "Komendy slash (/):",
                    Style::default().fg(palette.heading).add_modifier(Modifier::BOLD),
                )),
            ];
            for cmd in SLASH_COMMANDS {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<26} ", cmd.usage), Style::default().fg(palette.accent)),
                    Span::styled(cmd.desc, Style::default().fg(palette.fg)),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓/PgUp/PgDn przewijanie · Esc/q zamknij",
                Style::default().fg(palette.warning),
            )));
            let visible: Vec<Line> = lines.into_iter().skip(*scroll).collect();
            render_modal_box(frame, popup, " Pomoc i skróty klawiszowe (/help) ", visible, palette);
        }
        Overlay::SearchInput { query } => {
            let popup = centered_rect(60, 22, area);
            frame.render_widget(Clear, popup);
            let lines = vec![
                Line::from(Span::styled(
                    "Wpisz szukaną frazę w transkrypcie rozmowy:",
                    Style::default().fg(palette.muted),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("❯ {query}_"),
                    Style::default().fg(palette.accent).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter szukaj · Esc anuluj",
                    Style::default().fg(palette.warning),
                )),
            ];
            render_modal_box(frame, popup, " Szukaj w rozmowie (Ctrl+F) ", lines, palette);
        }
    }
}

fn render_modal_box(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    palette: &ThemePalette,
) {
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(palette.surface).fg(palette.fg))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(palette.accent))
                    .style(Style::default().bg(palette.surface).fg(palette.fg)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    let horizontal = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1]);
    horizontal[1]
}

// --- helpers -------------------------------------------------------------

fn session_matches(s: &SessionRecord, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let q = query.to_ascii_lowercase();
    s.id.to_ascii_lowercase().contains(&q)
        || s.title
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(&q)
        || s.cwd.to_ascii_lowercase().contains(&q)
        || s.model.to_ascii_lowercase().contains(&q)
}

fn active_at_token(before_cursor: &str) -> Option<(usize, String)> {
    let bytes = before_cursor.as_bytes();
    let mut start = before_cursor.len();
    while start > 0 && !bytes[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    let token = &before_cursor[start..];
    token
        .strip_prefix('@')
        .map(|rest| (start, rest.to_string()))
}

fn scan_workspace_files(roots: &[PathBuf], max_depth: usize, max_items: usize) -> Vec<String> {
    let mut out = Vec::new();
    for root in roots {
        scan_dir_recursive(root, root, 0, max_depth, max_items, &mut out);
        if out.len() >= max_items {
            break;
        }
    }
    out.sort();
    out.dedup();
    out
}

fn scan_dir_recursive(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    max_items: usize,
    out: &mut Vec<String>,
) {
    if depth > max_depth || out.len() >= max_items {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        if out.len() >= max_items {
            break;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if matches!(
            name,
            ".git" | "target" | "node_modules" | ".cache" | ".DS_Store"
        ) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        if path.is_dir() {
            out.push(format!("{rel}/"));
            scan_dir_recursive(root, &path, depth + 1, max_depth, max_items, out);
        } else {
            out.push(rel);
        }
    }
}

fn detect_git_branch(cwd: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() { None } else { Some(branch) }
}

fn run_git_diff_lines(cwd: &str) -> (String, Vec<String>) {
    let status = Command::new("git")
        .args(["status", "--short"])
        .current_dir(cwd)
        .output();
    let diff = Command::new("git")
        .args(["diff", "--no-color"])
        .current_dir(cwd)
        .output();

    let mut lines = Vec::new();
    match status {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if !text.trim().is_empty() {
                lines.push("@@ Status repozytorium @@".to_string());
                lines.extend(text.lines().map(str::to_string));
                lines.push(String::new());
            }
        }
        _ => {}
    }

    match diff {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                if lines.is_empty() {
                    lines.push("(brak niezatwierdzonych zmian w git diff)".to_string());
                }
            } else {
                lines.extend(text.lines().map(str::to_string));
            }
            ("git diff".to_string(), lines)
        }
        _ => (
            "git diff".to_string(),
            vec!["(katalog nie jest repozytorium Git lub brak git w PATH)".to_string()],
        ),
    }
}

fn discover_skills(cwd: &str) -> Vec<String> {
    let mut dirs = vec![PathBuf::from(cwd).join(".agents").join("skills")];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".agents").join("skills"));
        dirs.push(PathBuf::from(&home).join(".gemini").join("config").join("skills"));
    }
    let mut found = Vec::new();
    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() && p.join("SKILL.md").exists() {
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        found.push(name.to_string());
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

fn extract_last_code_block(md: &str) -> Option<String> {
    let mut last: Option<String> = None;
    let mut in_block = false;
    let mut buf = Vec::new();
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if in_block {
                last = Some(buf.join("\n"));
                buf.clear();
                in_block = false;
            } else {
                in_block = true;
            }
        } else if in_block {
            buf.push(line);
        }
    }
    last
}

fn tool_style(entry: &str, palette: &ThemePalette) -> Style {
    if entry.starts_with('✗') {
        Style::default().fg(palette.error)
    } else if entry.starts_with('✓') {
        Style::default().fg(palette.success)
    } else if entry.starts_with('⚙') || entry.starts_with('⚡') {
        Style::default().fg(palette.warning)
    } else if entry.starts_with('ℹ') {
        Style::default().fg(palette.muted)
    } else {
        Style::default().fg(palette.fg)
    }
}

fn tool_summary(name: &str, arguments: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(arguments) {
        match name {
            "bash" => {
                if let Some(command) = value.get("command").and_then(Value::as_str) {
                    let first_line = command.lines().next().unwrap_or("").trim();
                    return format!("bash {first_line}");
                }
            }
            "read" | "write" | "edit" | "str_replace_editor" => {
                if let Some(path) = value
                    .get("file_path")
                    .or_else(|| value.get("path"))
                    .and_then(Value::as_str)
                {
                    return format!("{name} {path}");
                }
            }
            "glob" | "grep" => {
                if let Some(pat) = value.get("pattern").and_then(Value::as_str) {
                    return format!("{name} {pat}");
                }
            }
            _ => {}
        }
    }
    format!("{name} {}", truncate(arguments, 100))
}

fn build_tool_diff(name: &str, arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    let path = value
        .get("file_path")
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("plik");

    match name {
        "edit" | "str_replace_editor" => {
            let old = value
                .get("old_string")
                .or_else(|| value.get("old_str"))
                .or_else(|| value.get("target"))
                .and_then(Value::as_str)?;
            let new = value
                .get("new_string")
                .or_else(|| value.get("new_str"))
                .or_else(|| value.get("replacement"))
                .and_then(Value::as_str)?;
            let mut out = vec![format!("--- {path}"), format!("+++ {path}")];
            for l in old.lines().take(12) {
                out.push(format!("-{l}"));
            }
            for l in new.lines().take(12) {
                out.push(format!("+{l}"));
            }
            Some(out.join("\n"))
        }
        "write" => {
            let content = value.get("content").and_then(Value::as_str)?;
            let mut out = vec![format!("+++ {path}")];
            let total = content.lines().count();
            for l in content.lines().take(12) {
                out.push(format!("+{l}"));
            }
            if total > 12 {
                out.push(format!("@@ ... (+{} kolejnych linii) @@", total - 12));
            }
            Some(out.join("\n"))
        }
        _ => None,
    }
}

fn wrap_paragraph(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0usize;
        for word in paragraph.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);
            if line_width > 0 && line_width + 1 + word_width > width {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if word_width <= width {
                if !line.is_empty() {
                    line.push(' ');
                    line_width += 1;
                }
                line.push_str(word);
                line_width += word_width;
                continue;
            }
            for character in word.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                if line_width > 0 && line_width + character_width > width {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0;
                }
                line.push(character);
                line_width += character_width;
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_at_mentions_and_tool_diffs() {
        assert_eq!(
            active_at_token("sprawdź @src/main.r"),
            Some((9, "src/main.r".to_string()))
        );
        assert_eq!(
            active_at_token("check @src/main.r"),
            Some((6, "src/main.r".to_string()))
        );
        assert_eq!(active_at_token("zwykły tekst"), None);

        let diff = build_tool_diff(
            "edit",
            r#"{"file_path":"src/main.rs","old_string":"let a = 1;","new_string":"let a = 2;"}"#,
        )
        .unwrap();
        assert!(diff.contains("--- src/main.rs"));
        assert!(diff.contains("-let a = 1;"));
        assert!(diff.contains("+let a = 2;"));
    }

    #[test]
    fn extracts_last_fenced_code_block() {
        let md = "Tekst\n```rs\nfn a() {}\n```\nDalej\n```sh\necho ok\n```";
        assert_eq!(extract_last_code_block(md).as_deref(), Some("echo ok"));
    }
}
