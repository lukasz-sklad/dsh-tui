//! Terminal chat over the DeepSeek Harness SDK profile.
//!
//! Spawns `dsh --profile sdk`, initializes the runtime, and drives a persisted
//! session while streaming events and interactive tool/question modals. On a TTY
//! it opens a full-screen ratatui interface; with `--print` or piped stdin/stdout
//! it runs in linear or structured JSON/NDJSON mode.

mod client;
mod markdown;
mod plain;
mod settings;
mod tui;

use client::Client;
use plain::{InputFormat, OutputFormat, PlainOptions};
use serde_json::json;
use settings::{
    ExecutionMode, McpRegistry, McpServer, SessionRecord, Settings, available_models,
    format_epoch_utc, format_relative_time, format_tokens, list_sessions, load_last_session,
    load_session, normalize_effort, resolve_model_alias, save_last_session, shorten_home,
};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, exit};
use uuid::Uuid;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
dsh-tui — interfejs terminalowy dla DeepSeek Harness (dsh --profile sdk)

Użycie: dsh-tui [opcje] [prompt]
        dsh-tui <podkomenda> [argumenty]

Opcje:
  --profile <nazwa>             profil dsh (domyślnie: sdk)
  --provider <id>               provider modelu (domyślnie: deepseek-official)
  --model <id>                  model (deepseek-flash | deepseek-v4-pro | flash | pro)
  --effort <poziom>             poziom rozumowania (off | low | medium | high | max)
  --mode <tryb>                 tryb uprawnień (workspace-write | accept-edits | auto | plan | read-only)
  --permission-mode <preset>    alias dla --mode (read-only | workspace-write | danger-full-access)
  --dangerously-skip-permissions  automatyczna aprobata wszystkich wywołań narzędzi
  --sandbox                     wymuś bezpieczny zapis tylko w workspace (workspace-write)
  --cwd <ścieżka>               katalog roboczy agenta (domyślnie: bieżący)
  --add-dir <ścieżka>           dodatkowy katalog kontekstu dla @wzmianek (powtarzalne)
  -c, --continue                wznów ostatnio używaną rozmowę
  --conversation <id>           wznów rozmowę o podanym ID (alias: --session-id)
  --session-id <id>             wznów rozmowę o podanym ID
  -p, --print                   pojedyncza tura bez TUI (prompt z argumentu lub stdin)
  -i, --prompt-interactive <t>  uruchom TUI i wyślij od razu podany prompt
  --output-format <fmt>         format wyjścia dla --print (text | json | stream-json)
  --input-format <fmt>          format wejścia dla --print (text | stream-json)
  --log-file <plik>             zapis wszystkich ramek JSON-RPC do pliku (alias: --log)
  --settings <json|plik>        nadpisanie ustawień tylko dla tej sesji
  --plain                       wymuś tryb liniowy REPL nawet na terminalu
  -V, --version                 pokaż wersję programu
  -h, --help                    ta pomoc

Podkomendy:
  models                        lista dostępnych modeli i poziomów effort
  agents                        lista trybów i profili agenta
  sessions                      lista zapisanych lokalnie rozmów
  mcp                           zarządzanie serwerami MCP (list, add, remove, enable, disable)
  plugin                        zarządzanie wtyczkami dsh (deleguje do dsh plugin)
  changelog                     historia zmian dsh-tui
  install                       instalacja binarki do ~/.local/bin i konfiguracja PATH
  help [podkomenda]             pomoc dla wybranej podkomendy

W TUI:
  /model, /effort, /mode, /load (Ctrl+O), /diff, /context, /tasks, /settings, /help
  Shift+Tab zmiana trybu · Tab autouzupełnianie / i @ · Ctrl+T panel narzędzi · Ctrl+R myślenie
";

#[derive(Debug, Clone)]
pub(crate) struct Options {
    pub profile: String,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<ExecutionMode>,
    pub dangerously_skip_permissions: bool,
    pub cwd: Option<String>,
    pub add_dirs: Vec<String>,
    pub log: Option<String>,
    pub settings_override: Option<String>,
    pub plain: bool,
    pub print_mode: bool,
    pub output_format: OutputFormat,
    pub input_format: InputFormat,
    pub initial_prompt: Option<String>,
    pub session_id: Option<String>,
    pub continue_last: bool,
}

/// Everything the TUI/plain runner needs to launch or relaunch the runtime.
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub program: String,
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub mode: ExecutionMode,
    pub auto_approve: bool,
    pub cwd: String,
    pub add_dirs: Vec<String>,
    pub extra_env: Vec<(String, String)>,
    pub log_path: Option<PathBuf>,
    pub settings: Settings,
    pub initial_prompt: Option<String>,
}

impl LaunchSpec {
    fn from_options(options: &Options, settings: Settings) -> LaunchSpec {
        let cwd = options
            .cwd
            .clone()
            .or_else(|| env::current_dir().ok().map(|p| p.display().to_string()))
            .unwrap_or_else(|| ".".to_string());
        let model = options
            .model
            .clone()
            .unwrap_or_else(|| settings.model.clone());
        let effort = options
            .effort
            .clone()
            .unwrap_or_else(|| settings.effort.clone());
        let mode = if options.dangerously_skip_permissions {
            ExecutionMode::Auto
        } else {
            options.mode.unwrap_or_else(|| {
                ExecutionMode::parse(&settings.mode).unwrap_or(ExecutionMode::WorkspaceWrite)
            })
        };
        let auto_approve =
            options.dangerously_skip_permissions || settings.auto_approve || mode == ExecutionMode::Auto;

        LaunchSpec {
            program: env::var("DSH_BIN").unwrap_or_else(|_| "dsh".to_string()),
            profile: options.profile.clone(),
            provider: options.provider.clone(),
            model,
            effort,
            mode,
            auto_approve,
            cwd,
            add_dirs: options.add_dirs.clone(),
            extra_env: Vec::new(),
            log_path: options.log.as_ref().map(PathBuf::from),
            settings,
            initial_prompt: options.initial_prompt.clone(),
        }
    }
}

/// Launch the runtime and initialize it for one model, effort, and permission mode.
pub(crate) fn connect(
    spec: &LaunchSpec,
    model: &str,
    effort: &str,
    mode: ExecutionMode,
) -> Result<Client, String> {
    let mut env_vars = spec.extra_env.clone();
    env_vars.retain(|(k, _)| k != "DSH_PERMISSION_MODE");
    env_vars.push((
        "DSH_PERMISSION_MODE".to_string(),
        mode.dsh_preset().to_string(),
    ));

    let mut client = Client::launch(
        &spec.program,
        &["--profile", &spec.profile],
        &env_vars,
        spec.log_path.as_deref(),
    )
    .map_err(|error| {
        format!(
            "nie mogę uruchomić `{} --profile {}`: {error}",
            spec.program, spec.profile
        )
    })?;
    client
        .request(
            "initialize",
            Some(json!({
                "cwd": spec.cwd,
                "provider": spec.provider,
                "model": model,
                "reasoningEffort": effort,
                "approvalRequests": true,
                "capabilities": { "userQuestions": true },
            })),
        )
        .map_err(|error| format!("initialize nie powiodło się: {error}"))?;
    Ok(client)
}

fn main() {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    if let Some(code) = maybe_run_subcommand(&raw_args) {
        exit(code);
    }

    let options = match parse_args(&raw_args) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("dsh-tui: {err}");
            exit(1);
        }
    };

    if options.session_id.is_some() && options.continue_last {
        eprintln!("dsh-tui: użyj --conversation/--session-id albo -c, nie obu naraz");
        exit(1);
    }

    let mut settings = Settings::load();
    if let Some(override_spec) = &options.settings_override {
        if let Err(err) = settings.apply_override(override_spec) {
            eprintln!("dsh-tui: {err}");
            exit(1);
        }
    }

    let mut spec = LaunchSpec::from_options(&options, settings);

    // Peek target session metadata before initialize so `-c` / `--conversation`
    // restores the model and effort used in that session unless overridden on CLI.
    let target_resume_id = if let Some(id) = &options.session_id {
        Some(id.clone())
    } else if options.continue_last {
        load_last_session()
            .or_else(|| list_sessions().into_iter().next().map(|s| s.id))
    } else {
        None
    };

    if let Some(ref resume_id) = target_resume_id {
        if let Ok(saved) = load_session(resume_id) {
            if options.model.is_none() && !saved.model.is_empty() {
                spec.model = resolve_model_alias(&saved.model);
            }
            if options.effort.is_none() {
                if let Some(eff) = normalize_effort(&saved.effort) {
                    spec.effort = eff.to_string();
                }
            }
            if options.mode.is_none() && !options.dangerously_skip_permissions {
                if let Some(mode) = ExecutionMode::parse(&saved.mode) {
                    spec.mode = mode;
                    spec.auto_approve = mode == ExecutionMode::Auto || spec.settings.auto_approve;
                }
            }
        }
    }

    let mut client = match connect(&spec, &spec.model, &spec.effort, spec.mode) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("dsh-tui: {error}");
            exit(1);
        }
    };

    let (session_id, loaded_session) = resolve_session(&mut client, &options, target_resume_id);
    let tty = io::stdin().is_terminal() && io::stdout().is_terminal();

    if options.print_mode || options.plain || !tty {
        let plain_opts = PlainOptions {
            print_mode: options.print_mode,
            output_format: options.output_format,
            input_format: options.input_format,
            initial_prompt: options.initial_prompt.clone(),
            interactive_tty: tty,
        };
        let code = plain::run(&mut client, &session_id, loaded_session, &spec, &plain_opts);
        client.shutdown();
        exit(code);
    }

    if let Err(error) = tui::run(client, &session_id, loaded_session, spec) {
        eprintln!("dsh-tui: {error}");
        exit(1);
    }
}

/// Pick the session to drive: explicit `--conversation`/`--session-id`, `-c` last session, or fresh.
fn resolve_session(
    client: &mut Client,
    options: &Options,
    target_resume_id: Option<String>,
) -> (String, Option<SessionRecord>) {
    if let Some(session_id) = target_resume_id {
        let local = load_session(&session_id).ok();
        match client.request("session/resume", Some(json!({"sessionId": session_id}))) {
            Ok(_) => {
                save_last_session(&session_id);
                return (session_id, local);
            }
            Err(error) => {
                if options.session_id.is_some() || local.is_none() {
                    eprintln!("dsh-tui: nie mogę wznowić sesji {session_id}: {error}");
                    exit(1);
                }
                // Local history exists even if runtime didn't flush JSONL (e.g., exited before turn 1).
                let fresh = new_session_id();
                save_last_session(&fresh);
                return (
                    fresh.clone(),
                    local.map(|mut r| {
                        r.id = fresh;
                        r
                    }),
                );
            }
        }
    }
    if options.continue_last {
        eprintln!("dsh-tui: brak zapisanej sesji — uruchom raz bez -c, żeby ją utworzyć");
        exit(1);
    }
    let session_id = new_session_id();
    save_last_session(&session_id);
    (session_id, None)
}

pub(crate) fn new_session_id() -> String {
    format!("session-{}", Uuid::new_v4().simple())
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let head: String = text.chars().take(max_chars - 1).collect();
    format!("{head}…")
}

/// Dispatch CLI subcommands. Returns `Some(exit_code)` if handled.
fn maybe_run_subcommand(args: &[String]) -> Option<i32> {
    let first = args.first()?.as_str();
    match first {
        "models" => {
            if args.get(1).is_some_and(|a| a == "-h" || a == "--help") {
                println!("Użycie: dsh-tui models\n\nWyświetla listę obsługiwanych modeli i poziomów effort.");
                return Some(0);
            }
            let settings = Settings::load();
            println!("Dostępne modele (DeepSeek Harness SDK):\n");
            for m in available_models() {
                let active = if m.id == settings.model { " *domyślny*" } else { "" };
                println!(
                    "  {:<18} {:<20} okno: {:<6} [{}]{}",
                    m.id,
                    m.display_name,
                    format_tokens(m.context_window),
                    m.modalities,
                    active
                );
                println!("    {}", m.description);
            }
            println!("\nPoziomy rozumowania (--effort / /effort): off | low | high (medium) | max");
            Some(0)
        }
        "agents" | "agent" => {
            if args.get(1).is_some_and(|a| a == "-h" || a == "--help") {
                println!("Użycie: dsh-tui agents\n\nWyświetla tryby działania agenta i profile DeepSeek Harness.");
                return Some(0);
            }
            println!("Tryby agenta i uprawnień (--mode / Shift+Tab):\n");
            for mode in ExecutionMode::all() {
                println!("  {:<18} {}", mode.as_str(), mode.description());
            }
            println!("\nWbudowane profile i możliwości dsh:");
            println!("  sdk                Domyślny profil JSON-RPC używany przez dsh-tui");
            println!("  /plan <zadanie>    Tryb planowania krok po kroku przed wykonaniem");
            println!("  /goal <cel>        Tryb autonomicznej pętli celu (goal loop)");
            println!("  subagents          Podagenci raportowani w panelu narzędzi i /tasks");
            Some(0)
        }
        "sessions" | "list" => {
            let sessions = list_sessions();
            if sessions.is_empty() {
                println!("Brak zapisanych sesji w {}.", settings::sessions_dir().display());
                return Some(0);
            }
            println!("Zapisane sesje ({}):\n", sessions.len());
            for s in sessions {
                let title = s.title.as_deref().unwrap_or("(bez tytułu)");
                println!(
                    "  {}  {:<26}  {:<15}  {:>3} wp.  {} ({})  {}",
                    s.id,
                    truncate(title, 26),
                    s.model,
                    s.entries.len(),
                    format_relative_time(s.updated_at_epoch),
                    format_epoch_utc(s.updated_at_epoch),
                    shorten_home(std::path::Path::new(&s.cwd))
                );
            }
            println!("\nWznów sesję: dsh-tui --conversation <id>  (lub -c dla ostatniej)");
            Some(0)
        }
        "mcp" => Some(run_mcp_subcommand(&args[1..])),
        "plugin" | "plugins" => Some(run_plugin_subcommand(&args[1..])),
        "changelog" => {
            println!(
                "dsh-tui changelog:\n\n\
                 v0.3.0\n\
                 - Naprawiono blokowanie TUI/plain podczas próśb o aprobatę i pytania agenta\n\
                 - Dodano paski nagłówka i statusu w TUI (model, effort, tryb, kontekst 1M, git branch)\n\
                 - Dodano interaktywne modale: /model, /effort, /mode, /load (Ctrl+O), /diff, /context, /tasks, /settings, /help\n\
                 - Dodano menu autouzupełniania komend `/` i ścieżek plików `@` (Tab/↑/↓)\n\
                 - Dodano bloki myślenia (`reasoning`, Ctrl+R), kolorowanie składni, tabele Markdown i podgląd diffów\n\
                 - Dodano 4 motywy: dark, light, sepia, neon oraz trwałe zapisywanie sesji i ustawień\n\
                 - Dodano flagi `-p`/`--print`, `--output-format json|stream-json`, `--effort`, `--mode`, `--dangerously-skip-permissions`\n\
                 - Dodano subkomendy: models, agents, sessions, mcp, plugin, changelog, install\n\n\
                 v0.2.0\n\
                 - Pierwszy klient TUI (ratatui) i tryb liniowy dla `dsh --profile sdk`"
            );
            Some(0)
        }
        "install" => Some(run_install_subcommand()),
        "help" => {
            if let Some(sub) = args.get(1) {
                match sub.as_str() {
                    "models" => {
                        println!("dsh-tui models — wyświetla obsługiwane modele i poziomy effort.");
                    }
                    "agents" | "agent" => {
                        println!("dsh-tui agents — wyświetla dostępne tryby agenta i profile.");
                    }
                    "sessions" | "list" => {
                        println!("dsh-tui sessions — wyświetla zapisane sesje do wznowienia.");
                    }
                    "mcp" => {
                        print_mcp_help();
                    }
                    "plugin" | "plugins" => {
                        println!("dsh-tui plugin <args...> — przekazuje polecenia do `dsh plugin --profile sdk`.");
                    }
                    "changelog" => {
                        println!("dsh-tui changelog — wyświetla listę zmian wersji dsh-tui.");
                    }
                    "install" => {
                        println!("dsh-tui install — instaluje binarkę w ~/.local/bin i sprawdza PATH.");
                    }
                    _ => print!("{USAGE}"),
                }
            } else {
                print!("{USAGE}");
            }
            Some(0)
        }
        _ => None,
    }
}

fn print_mcp_help() {
    println!(
        "Użycie: dsh-tui mcp <podkomenda>\n\n\
         Podkomendy:\n\
           list                         Pokaż skonfigurowane serwery MCP\n\
           add <nazwa> <komenda> [args] Dodaj lub zaktualizuj serwer MCP\n\
           remove <nazwa>               Usuń serwer MCP\n\
           enable <nazwa>               Włącz serwer MCP\n\
           disable <nazwa>              Wyłącz serwer MCP"
    );
}

fn run_mcp_subcommand(args: &[String]) -> i32 {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let mut reg = McpRegistry::load();
    match sub {
        "-h" | "--help" | "help" => {
            print_mcp_help();
            0
        }
        "list" => {
            if reg.servers.is_empty() {
                println!(
                    "Brak skonfigurowanych serwerów MCP w {}.",
                    settings::mcp_path().display()
                );
                return 0;
            }
            for s in &reg.servers {
                let state = if s.enabled { "włączony" } else { "wyłączony" };
                println!("  {:<18} [{}]  {} {}", s.name, state, s.command, s.args.join(" "));
            }
            0
        }
        "add" => {
            if args.len() < 3 {
                eprintln!("Użycie: dsh-tui mcp add <nazwa> <komenda> [argumenty...]");
                return 1;
            }
            let name = args[1].clone();
            let command = args[2].clone();
            let rest = args[3..].to_vec();
            reg.upsert(McpServer {
                name: name.clone(),
                command,
                args: rest,
                env: BTreeMap::new(),
                enabled: true,
            });
            if let Err(err) = reg.save() {
                eprintln!("dsh-tui: {err}");
                return 1;
            }
            println!("Zapisano serwer MCP `{name}`.");
            0
        }
        "remove" | "rm" => {
            let Some(name) = args.get(1) else {
                eprintln!("Użycie: dsh-tui mcp remove <nazwa>");
                return 1;
            };
            if !reg.remove(name) {
                eprintln!("Nie znaleziono serwera MCP `{name}`.");
                return 1;
            }
            if let Err(err) = reg.save() {
                eprintln!("dsh-tui: {err}");
                return 1;
            }
            println!("Usunięto serwer MCP `{name}`.");
            0
        }
        "enable" | "disable" => {
            let Some(name) = args.get(1) else {
                eprintln!("Użycie: dsh-tui mcp {sub} <nazwa>");
                return 1;
            };
            let enable = sub == "enable";
            if !reg.set_enabled(name, enable) {
                eprintln!("Nie znaleziono serwera MCP `{name}`.");
                return 1;
            }
            if let Err(err) = reg.save() {
                eprintln!("dsh-tui: {err}");
                return 1;
            }
            println!(
                "Serwer MCP `{name}`: {}.",
                if enable { "włączony" } else { "wyłączony" }
            );
            0
        }
        other => {
            eprintln!("Nieznana podkomenda mcp `{other}`");
            print_mcp_help();
            1
        }
    }
}

fn run_plugin_subcommand(args: &[String]) -> i32 {
    let program = env::var("DSH_BIN").unwrap_or_else(|_| "dsh".to_string());
    let mut cmd = Command::new(&program);
    cmd.arg("plugin").arg("--profile").arg("sdk");
    if args.is_empty() {
        cmd.arg("--help");
    } else {
        cmd.args(args);
    }
    match cmd.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("dsh-tui: nie mogę uruchomić `{program} plugin`: {err}");
            1
        }
    }
}

fn run_install_subcommand() -> i32 {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let target_dir = PathBuf::from(&home).join(".local").join("bin");
    let dest = target_dir.join("dsh-tui");
    if let Err(err) = fs::create_dir_all(&target_dir) {
        eprintln!("dsh-tui: nie mogę utworzyć {}: {err}", target_dir.display());
        return 1;
    }
    match env::current_exe() {
        Ok(exe) => {
            if exe != dest {
                if let Err(err) = fs::copy(&exe, &dest) {
                    eprintln!("dsh-tui: nie mogę skopiować {} -> {}: {err}", exe.display(), dest.display());
                    return 1;
                }
            }
            println!("Zainstalowano dsh-tui w {}", dest.display());
            let path_var = env::var("PATH").unwrap_or_default();
            if !path_var.split(':').any(|p| PathBuf::from(p) == target_dir) {
                println!(
                    "Dodaj do ~/.bashrc lub ~/.zshrc:\n  export PATH=\"{}:$PATH\"",
                    target_dir.display()
                );
            }
            0
        }
        Err(err) => {
            eprintln!("dsh-tui: nie można ustalić ścieżki pliku wykonywalnego: {err}");
            1
        }
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        profile: "sdk".to_string(),
        provider: "deepseek-official".to_string(),
        model: None,
        effort: None,
        mode: None,
        dangerously_skip_permissions: false,
        cwd: None,
        add_dirs: Vec::new(),
        log: None,
        settings_override: None,
        plain: false,
        print_mode: false,
        output_format: OutputFormat::Text,
        input_format: InputFormat::Text,
        initial_prompt: None,
        session_id: None,
        continue_last: false,
    };

    let mut positional: Vec<String> = Vec::new();
    let mut index = 0;
    let mut after_double_dash = false;

    while index < args.len() {
        let arg = args[index].as_str();
        if after_double_dash {
            positional.push(arg.to_string());
            index += 1;
            continue;
        }
        match arg {
            "--" => after_double_dash = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                exit(0);
            }
            "-V" | "--version" => {
                println!("dsh-tui {VERSION}");
                exit(0);
            }
            "-c" | "--continue" => options.continue_last = true,
            "--plain" => options.plain = true,
            "-p" | "--print" | "--prompt" => options.print_mode = true,
            "--dangerously-skip-permissions" => options.dangerously_skip_permissions = true,
            "--sandbox" => options.mode = Some(ExecutionMode::WorkspaceWrite),
            "-i" | "--prompt-interactive" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("{arg} wymaga treści promptu"))?;
                options.initial_prompt = Some(value);
            }
            "--conversation" | "--session-id" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("{arg} wymaga wartości"))?;
                options.session_id = Some(value);
            }
            "--mode" | "--permission-mode" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{arg} wymaga wartości"))?;
                let parsed = ExecutionMode::parse(value)
                    .ok_or_else(|| format!("nieznany tryb `{value}` (dozwolone: workspace-write, accept-edits, auto, plan, read-only, danger-full-access)"))?;
                options.mode = Some(parsed);
            }
            "--effort" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--effort wymaga wartości (off|low|medium|high|max)".to_string())?;
                let norm = normalize_effort(value)
                    .ok_or_else(|| format!("nieznany poziom --effort `{value}`"))?;
                options.effort = Some(norm.to_string());
            }
            "--output-format" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--output-format wymaga wartości (text|json|stream-json)".to_string())?;
                options.output_format = OutputFormat::parse(value)
                    .ok_or_else(|| format!("nieznany --output-format `{value}`"))?;
            }
            "--input-format" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--input-format wymaga wartości (text|stream-json)".to_string())?;
                options.input_format = InputFormat::parse(value)
                    .ok_or_else(|| format!("nieznany --input-format `{value}`"))?;
            }
            "--log" | "--log-file" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("{arg} wymaga ścieżki pliku"))?;
                options.log = Some(value);
            }
            "--settings" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--settings wymaga obiektu JSON lub ścieżki".to_string())?;
                options.settings_override = Some(value);
            }
            "--profile" => {
                index += 1;
                options.profile = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--profile wymaga wartości".to_string())?;
            }
            "--provider" => {
                index += 1;
                options.provider = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--provider wymaga wartości".to_string())?;
            }
            "--model" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--model wymaga wartości".to_string())?;
                options.model = Some(resolve_model_alias(value));
            }
            "--cwd" => {
                index += 1;
                options.cwd = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| "--cwd wymaga ścieżki".to_string())?,
                );
            }
            "--add-dir" => {
                index += 1;
                options.add_dirs.push(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| "--add-dir wymaga ścieżki".to_string())?,
                );
            }
            unknown if unknown.starts_with('-') => {
                return Err(format!("nieznana opcja `{unknown}` (użyj --help)"));
            }
            word => {
                positional.push(word.to_string());
            }
        }
        index += 1;
    }

    if !positional.is_empty() && options.initial_prompt.is_none() {
        options.initial_prompt = Some(positional.join(" "));
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_cli_flags() {
        let opts = parse_args(&vec_args(&[
            "-p",
            "--output-format",
            "json",
            "--model",
            "pro",
            "--effort",
            "medium",
            "--mode",
            "plan",
            "--add-dir",
            "/tmp/a",
            "--add-dir",
            "/tmp/b",
            "--conversation",
            "session-123",
            "Przeanalizuj",
            "kod",
        ]))
        .unwrap();

        assert!(opts.print_mode);
        assert_eq!(opts.output_format, OutputFormat::Json);
        assert_eq!(opts.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(opts.effort.as_deref(), Some("high"));
        assert_eq!(opts.mode, Some(ExecutionMode::Plan));
        assert_eq!(opts.add_dirs, vec!["/tmp/a", "/tmp/b"]);
        assert_eq!(opts.session_id.as_deref(), Some("session-123"));
        assert_eq!(opts.initial_prompt.as_deref(), Some("Przeanalizuj kod"));
    }

    #[test]
    fn rejects_unknown_flags() {
        let err = parse_args(&vec_args(&["--nonexistent-flag"])).unwrap_err();
        assert!(err.contains("--nonexistent-flag"));
    }
}
