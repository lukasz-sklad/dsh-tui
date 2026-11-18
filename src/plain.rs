//! Line-based REPL and non-interactive `--print` runner (`text` / `json` / `stream-json`).

use crate::client::{Client, IncomingRequest, RecvOutcome};
use crate::settings::{
    self, ChatEntry, ExecutionMode, SessionRecord, TokenUsage, save_last_session, save_session,
};
use crate::{LaunchSpec, new_session_id, truncate};
use serde_json::{Value, json};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    StreamJson,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "stream-json" | "ndjson" => Some(Self::StreamJson),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Text,
    StreamJson,
}

impl InputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "stream-json" | "ndjson" => Some(Self::StreamJson),
            _ => None,
        }
    }
}

pub struct PlainOptions {
    pub print_mode: bool,
    pub output_format: OutputFormat,
    pub input_format: InputFormat,
    pub initial_prompt: Option<String>,
    pub interactive_tty: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordedToolCall {
    name: String,
    arguments: String,
    is_error: bool,
}

/// Run either single-shot `--print` mode or the line-based REPL.
pub fn run(
    client: &mut Client,
    initial_session_id: &str,
    loaded_session: Option<SessionRecord>,
    spec: &LaunchSpec,
    options: &PlainOptions,
) -> i32 {
    let mut record = loaded_session.unwrap_or_else(|| SessionRecord {
        id: initial_session_id.to_string(),
        title: None,
        cwd: spec.cwd.clone(),
        model: spec.model.clone(),
        effort: spec.effort.clone(),
        mode: spec.mode.as_str().to_string(),
        ..SessionRecord::default()
    });
    let mut auto_approve = spec.auto_approve || spec.mode == ExecutionMode::Auto;

    drain_stderr(client);

    if options.print_mode {
        return run_print_mode(client, &mut record, spec, options, auto_approve);
    }

    println!(
        "dsh-tui: sesja {} · model {} · effort {} · tryb {} · /help · /exit",
        record.id, record.model, record.effort, record.mode
    );

    if let Some(prompt) = &options.initial_prompt {
        if !prompt.trim().is_empty() {
            if let Err(error) = run_turn(
                client,
                &mut record,
                prompt,
                options.output_format,
                auto_approve,
                false,
                options.interactive_tty,
            ) {
                eprintln!("\ndsh-tui: {error}");
            }
            save_last_session(&record.id);
            let _ = save_session(&record);
        }
    }

    let stdin = io::stdin();
    loop {
        drain_stderr(client);
        print!("❯ ");
        io::stdout().flush().ok();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("dsh-tui: błąd czytania wejścia: {error}");
                break;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line {
            "/exit" | "/quit" => break,
            "/new" => {
                let new_id = new_session_id();
                save_last_session(&new_id);
                record = SessionRecord {
                    id: new_id,
                    cwd: spec.cwd.clone(),
                    model: record.model.clone(),
                    effort: record.effort.clone(),
                    mode: record.mode.clone(),
                    ..SessionRecord::default()
                };
                println!("dsh-tui: nowa sesja {}", record.id);
                continue;
            }
            "/clear" => {
                record.entries.clear();
                record.tool_log.clear();
                let _ = save_session(&record);
                println!("dsh-tui: wyczyszczono historię sesji {}", record.id);
                continue;
            }
            "/help" => {
                println!(
                    "/ask <treść> — wyślij wiadomość\n\
                     /model <id> — pokaż lub zmień model\n\
                     /mode <tryb> — zmień tryb (workspace-write|auto|plan|read-only)\n\
                     /new — nowa sesja · /clear — wyczyść widok · /exit — wyjście"
                );
                continue;
            }
            _ => {}
        }
        if line == "/model" {
            println!("obecny model: {} (effort: {})", record.model, record.effort);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/mode ") {
            if let Some(mode) = ExecutionMode::parse(rest) {
                auto_approve = mode == ExecutionMode::Auto || spec.auto_approve;
                record.mode = mode.as_str().to_string();
                let cmd = format!("/permission {}", mode.dsh_preset());
                if let Err(error) = run_turn(
                    client,
                    &mut record,
                    &cmd,
                    options.output_format,
                    auto_approve,
                    false,
                    options.interactive_tty,
                ) {
                    eprintln!("dsh-tui: {error}");
                }
                let _ = save_session(&record);
                continue;
            }
            eprintln!("dsh-tui: nieznany tryb `{rest}`");
            continue;
        }
        let prompt = match line.strip_prefix("/ask ") {
            Some(rest) => rest,
            None => match line.strip_prefix("/btw ") {
                Some(rest) => rest,
                None => line,
            },
        };
        if prompt.is_empty() {
            println!("dsh-tui: użycie: /ask <treść>");
            continue;
        }
        if let Err(error) = run_turn(
            client,
            &mut record,
            prompt,
            options.output_format,
            auto_approve,
            false,
            options.interactive_tty,
        ) {
            eprintln!("\ndsh-tui: {error}");
        }
        save_last_session(&record.id);
        let _ = save_session(&record);
    }
    0
}

fn run_print_mode(
    client: &mut Client,
    record: &mut SessionRecord,
    _spec: &LaunchSpec,
    options: &PlainOptions,
    auto_approve: bool,
) -> i32 {
    if options.input_format == InputFormat::StreamJson {
        let stdin = io::stdin();
        for line_res in stdin.lock().lines() {
            let Ok(line) = line_res else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(prompt) = extract_stream_json_prompt(trimmed) else {
                eprintln!("dsh-tui: pominięto niepoprawną ramkę JSON: {trimmed}");
                continue;
            };
            if let Err(error) = run_turn(
                client,
                record,
                &prompt,
                options.output_format,
                auto_approve,
                true,
                false,
            ) {
                eprintln!("dsh-tui: {error}");
                return 1;
            }
            save_last_session(&record.id);
            let _ = save_session(record);
        }
        return 0;
    }

    let prompt = match &options.initial_prompt {
        Some(p) if !p.trim().is_empty() => p.clone(),
        _ => {
            let mut buf = String::new();
            if io::stdin().is_terminal() {
                eprintln!("dsh-tui: --print wymaga argumentu promptu lub wejścia na stdin");
                return 1;
            }
            if let Err(error) = io::stdin().read_to_string(&mut buf) {
                eprintln!("dsh-tui: błąd czytania stdin: {error}");
                return 1;
            }
            buf
        }
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        eprintln!("dsh-tui: pusty prompt w trybie --print");
        return 1;
    }
    match run_turn(
        client,
        record,
        prompt,
        options.output_format,
        auto_approve,
        true,
        false,
    ) {
        Ok(()) => {
            save_last_session(&record.id);
            let _ = save_session(record);
            0
        }
        Err(error) => {
            eprintln!("dsh-tui: {error}");
            1
        }
    }
}

/// Extract text prompt from an incoming NDJSON line (`--input-format stream-json`).
pub fn extract_stream_json_prompt(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    for key in ["text", "prompt", "content"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    if let Some(blocks) = value
        .pointer("/message/content")
        .or_else(|| value.get("contentBlocks"))
        .and_then(Value::as_array)
    {
        let joined: Vec<&str> = blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect();
        if !joined.is_empty() {
            return Some(joined.join("\n"));
        }
    }
    None
}

/// Send one input through `session/query` and drive notifications + server requests
/// until the turn settles.
fn run_turn(
    client: &mut Client,
    record: &mut SessionRecord,
    prompt: &str,
    output_format: OutputFormat,
    auto_approve: bool,
    print_mode: bool,
    interactive_tty: bool,
) -> Result<(), crate::client::Error> {
    while client.try_recv_notification().is_some() {}
    drain_stderr(client);

    record.entries.push(ChatEntry::User {
        text: prompt.to_string(),
    });

    let result = client.request(
        "session/query",
        Some(json!({
            "sessionId": record.id,
            "contentBlocks": [{"type": "text", "text": prompt}],
        })),
    )?;

    let kind = result.pointer("/kind").and_then(Value::as_str).unwrap_or("");
    let message_id = result
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let command_id = result
        .pointer("/execution/commandId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut turn = Turn::new(
        message_id,
        command_id,
        kind == "command",
        output_format,
        print_mode,
    );

    loop {
        while let Some(request) = client.try_recv_request() {
            handle_server_request(client, &request, auto_approve, interactive_tty, print_mode);
        }
        drain_stderr(client);

        match client.recv_notification_step(Duration::from_millis(40)) {
            RecvOutcome::Item(notification) => {
                if turn.handle_notification(&notification, record) {
                    break;
                }
            }
            RecvOutcome::Timeout => {}
            RecvOutcome::Closed => {
                return Err(crate::client::Error::Closed);
            }
        }
    }

    turn.finish(record);
    drain_stderr(client);
    Ok(())
}

fn handle_server_request(
    client: &mut Client,
    request: &IncomingRequest,
    auto_approve: bool,
    interactive_tty: bool,
    print_mode: bool,
) {
    match request.method.as_str() {
        "approval/request" => {
            let outcome = if auto_approve {
                "allowed-once"
            } else if interactive_tty && !print_mode {
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
                eprint!("\n[aprobata] narzędzie `{tool}` ({reason}) — [a] zezwól / [r] odrzuć / [c] anuluj: ");
                io::stderr().flush().ok();
                let mut answer = String::new();
                let _ = io::stdin().read_line(&mut answer);
                match answer.trim().to_ascii_lowercase().as_str() {
                    "a" | "y" | "yes" | "tak" => "allowed-once",
                    "c" | "cancel" => "cancelled",
                    _ => "rejected",
                }
            } else {
                eprintln!(
                    "dsh-tui: odrzucono prośbę o aprobatę narzędzia `{}` (użyj --dangerously-skip-permissions lub --mode auto)",
                    request
                        .params
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                );
                "rejected"
            };
            let _ = client.respond(&request.id, json!({ "outcome": outcome }));
        }
        "user-question/ask" => {
            if interactive_tty && !print_mode {
                let questions = request
                    .params
                    .get("questions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut answers = Vec::new();
                for q in &questions {
                    let id = q.get("id").and_then(Value::as_str).unwrap_or("");
                    let prompt = q.get("question").and_then(Value::as_str).unwrap_or("");
                    eprintln!("\n[pytanie] {prompt}");
                    if let Some(opts) = q.get("options").and_then(Value::as_array) {
                        for (idx, opt) in opts.iter().enumerate() {
                            let label = opt.get("label").and_then(Value::as_str).unwrap_or("");
                            eprintln!("  {}. {label}", idx + 1);
                        }
                    }
                    eprint!("Odpowiedź (numer lub tekst, puste = anuluj): ");
                    io::stderr().flush().ok();
                    let mut line = String::new();
                    let _ = io::stdin().read_line(&mut line);
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        let _ = client.respond(&request.id, json!({ "cancelled": true }));
                        return;
                    }
                    if let Ok(num) = trimmed.parse::<usize>() {
                        if let Some(label) = q
                            .get("options")
                            .and_then(Value::as_array)
                            .and_then(|opts| opts.get(num.saturating_sub(1)))
                            .and_then(|o| o.get("label"))
                            .and_then(Value::as_str)
                        {
                            answers.push(json!({ "id": id, "selected": [label] }));
                            continue;
                        }
                    }
                    answers.push(json!({ "id": id, "selected": [], "custom": trimmed }));
                }
                let _ = client.respond(&request.id, json!({ "answers": answers }));
            } else {
                let _ = client.respond(&request.id, json!({ "cancelled": true }));
            }
        }
        _ => {
            let _ = client.respond(&request.id, json!({}));
        }
    }
}

/// Print any runtime stderr lines queued since the last drain.
fn drain_stderr(client: &Client) {
    while let Some(line) = client.try_recv_stderr() {
        eprintln!("{line}");
    }
}

struct Turn {
    message_id: String,
    command_id: String,
    is_command: bool,
    received_inbox: bool,
    last_reason: Option<String>,
    response_buf: String,
    reasoning_buf: String,
    tool_calls: Vec<RecordedToolCall>,
    turn_usage: TokenUsage,
    printed_text: bool,
    line_open: bool,
    output_format: OutputFormat,
    print_mode: bool,
}

impl Turn {
    fn new(
        message_id: String,
        command_id: String,
        is_command: bool,
        output_format: OutputFormat,
        print_mode: bool,
    ) -> Self {
        Self {
            message_id,
            command_id,
            is_command,
            received_inbox: false,
            last_reason: None,
            response_buf: String::new(),
            reasoning_buf: String::new(),
            tool_calls: Vec::new(),
            turn_usage: TokenUsage::default(),
            printed_text: false,
            line_open: false,
            output_format,
            print_mode,
        }
    }

    fn handle_notification(
        &mut self,
        notification: &crate::client::Notification,
        record: &mut SessionRecord,
    ) -> bool {
        if self.output_format == OutputFormat::StreamJson {
            let frame = json!({
                "method": notification.method,
                "params": notification.params,
            });
            println!("{frame}");
            io::stdout().flush().ok();
        }

        match notification.method.as_str() {
            "session.event" => {
                if notification.params.pointer("/sessionId").and_then(Value::as_str)
                    != Some(record.id.as_str())
                {
                    return false;
                }
                if let Some(event) = notification.params.get("event") {
                    self.handle_event(event, record);
                }
                false
            }
            "session.status" => {
                let same_session = notification.params.pointer("/sessionId").and_then(Value::as_str)
                    == Some(record.id.as_str());
                let idle =
                    notification.params.pointer("/status").and_then(Value::as_str) == Some("idle");
                !self.is_command && same_session && idle && self.received_inbox
            }
            "session.query.completed" => {
                if !self.is_command {
                    return false;
                }
                let same_session = notification.params.pointer("/sessionId").and_then(Value::as_str)
                    == Some(record.id.as_str());
                let same_command = notification.params.pointer("/commandId").and_then(Value::as_str)
                    == Some(self.command_id.as_str());
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
                        .unwrap_or("completed");
                    if !final_response.is_empty() {
                        self.response_buf = final_response.to_string();
                        if self.output_format == OutputFormat::Text {
                            println!("{final_response}");
                            self.printed_text = true;
                        }
                    }
                    self.last_reason = Some(finish_reason.to_string());
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn handle_event(&mut self, event: &Value, record: &mut SessionRecord) {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return;
        };
        match event_type {
            "agent/inbox/spliced" => {
                let Some(inserted) = event.pointer("/data/inserted").and_then(Value::as_array)
                else {
                    return;
                };
                for message in inserted {
                    if message.get("id").and_then(Value::as_str) == Some(self.message_id.as_str()) {
                        self.received_inbox = true;
                    }
                }
            }
            "session/title" => {
                if let Some(title) = event.pointer("/data/title").and_then(Value::as_str) {
                    record.title = Some(title.to_string());
                }
            }
            "request/context" => {
                if let Some(cw) = event.pointer("/data/contextWindow").and_then(Value::as_u64) {
                    record.usage.context_window = cw;
                    self.turn_usage.context_window = cw;
                }
            }
            "permission/preset" => {
                if let Some(preset) = event.pointer("/data/preset").and_then(Value::as_str) {
                    if let Some(mode) = ExecutionMode::parse(preset) {
                        record.mode = mode.as_str().to_string();
                    }
                }
            }
            "assistant/message" => {
                if let Some(usage) = event.pointer("/data/usage") {
                    record.usage.record_step(usage);
                    self.turn_usage.record_step(usage);
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
                                if !text.is_empty() {
                                    self.reasoning_buf.push_str(text);
                                }
                            }
                        }
                        Some("text") => {
                            let Some(text) = block.get("text").and_then(Value::as_str) else {
                                continue;
                            };
                            if text.is_empty() {
                                continue;
                            }
                            self.response_buf.push_str(text);
                            if self.output_format == OutputFormat::Text {
                                print!("{text}");
                                io::stdout().flush().ok();
                                self.printed_text = true;
                                self.line_open = !text.ends_with('\n');
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
                    .unwrap_or("?")
                    .to_string();
                let arguments = event
                    .pointer("/data/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if self.output_format == OutputFormat::Text && !self.print_mode {
                    if self.line_open {
                        println!();
                    }
                    println!("\x1b[90m⚙ {name} {}\x1b[0m", truncate(&arguments, 200));
                    self.line_open = false;
                }
                self.tool_calls.push(RecordedToolCall {
                    name,
                    arguments,
                    is_error: false,
                });
            }
            "tool/result" => {
                let is_error = event
                    .pointer("/data/message/isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(last) = self.tool_calls.last_mut() {
                    last.is_error = is_error;
                }
                if is_error && self.output_format == OutputFormat::Text && !self.print_mode {
                    if self.line_open {
                        println!();
                    }
                    println!("\x1b[90m✗ narzędzie zwróciło błąd\x1b[0m");
                    self.line_open = false;
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

    fn finish(&self, record: &mut SessionRecord) {
        if !self.reasoning_buf.is_empty() {
            record.entries.push(ChatEntry::Reasoning {
                text: self.reasoning_buf.clone(),
            });
        }
        for call in &self.tool_calls {
            record.entries.push(ChatEntry::Tool {
                name: call.name.clone(),
                summary: truncate(&call.arguments, 120),
                diff: None,
                is_error: call.is_error,
                elapsed_secs: 0.0,
            });
        }
        if !self.response_buf.is_empty() {
            record.entries.push(ChatEntry::Assistant {
                text: self.response_buf.clone(),
            });
        }
        record.updated_at_epoch = settings::now_epoch();

        match self.output_format {
            OutputFormat::Text => {
                if self.line_open || (!self.printed_text && !self.print_mode) {
                    println!();
                } else if self.print_mode && self.printed_text && !self.response_buf.ends_with('\n') {
                    println!();
                }
                if !self.print_mode {
                    let reason = self
                        .last_reason
                        .as_deref()
                        .map(|r| format!(" · {r}"))
                        .unwrap_or_default();
                    let label = if self.is_command { "komenda" } else { "koniec tury" };
                    println!("\x1b[90m─ {label}{reason}\x1b[0m");
                }
            }
            OutputFormat::Json => {
                let payload = json!({
                    "sessionId": record.id,
                    "model": record.model,
                    "effort": record.effort,
                    "mode": record.mode,
                    "response": self.response_buf,
                    "reasoning": self.reasoning_buf,
                    "finishReason": self.last_reason.as_deref().unwrap_or("completed"),
                    "usage": self.turn_usage,
                    "toolCalls": self.tool_calls,
                });
                println!("{payload}");
            }
            OutputFormat::StreamJson => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_stream_json_prompts() {
        assert_eq!(
            extract_stream_json_prompt(r#""cześć""#),
            Some("cześć".to_string())
        );
        assert_eq!(
            extract_stream_json_prompt(r#"{"text":"podsumuj kod"}"#),
            Some("podsumuj kod".to_string())
        );
        assert_eq!(
            extract_stream_json_prompt(
                r#"{"message":{"content":[{"type":"text","text":"linia 1"},{"type":"text","text":"linia 2"}]}}"#
            ),
            Some("linia 1\nlinia 2".to_string())
        );
        assert_eq!(extract_stream_json_prompt("not json"), None);
    }
}
