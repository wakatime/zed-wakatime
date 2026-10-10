use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, env, fs, path::PathBuf};

use arc_swap::ArcSwap;
use clap::{Arg, Command};
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use serde_json::Value;
use tokio::{process::Command as TokioCommand, sync::Mutex};
use tower_lsp::lsp_types::notification::Progress;
use tower_lsp::lsp_types::request::WorkDoneProgressCreate;
use tower_lsp::{jsonrpc::Result, lsp_types::*, Client, LanguageServer, LspService, Server};

#[cfg(all(test, unix))]
mod tests;

#[derive(Deserialize, Default)]
struct Settings {
    api_key: Option<String>,
    api_url: Option<String>,
}

impl Settings {
    fn apply(&self, command: &mut TokioCommand) {
        if let Some(ref key) = self.api_key {
            command.arg("--key").arg(key);
        }

        if let Some(ref api_url) = self.api_url {
            command.arg("--api-url").arg(api_url);
        }
    }
}

#[derive(Default, Debug)]
struct Event {
    uri: String,
    is_write: bool,
    language: Option<String>,
    lineno: Option<u64>,
    cursor_pos: Option<u64>,
}

#[derive(Debug)]
struct CurrentFile {
    uri: String,
    timestamp: Timestamp,
}

struct WakatimeLanguageServer {
    client: Client,
    settings: ArcSwap<Settings>,
    wakatime_path: String,
    project_folder: String,
    alternate_project: String,
    current_file: Mutex<CurrentFile>,
    // Only didOpen carries the language ID; retain it for later changes and saves.
    document_languages: Mutex<HashMap<url::Url, String>>,
    platform: ArcSwap<String>,
    work_done_progress: AtomicBool,
}

// Extract filepath string from 'file://' URI.
//
// Example:
// file:///var/log/test.txt    -> /var/log/test.txt
// file:///C:/path/to/file.txt -> C:\path\to\file.txt
fn extract_uri_string(uri: &url::Url) -> String {
    uri.to_file_path()
        .map(|path: std::path::PathBuf| path.to_string_lossy().to_string())
        .unwrap_or_else(|()| uri[url::Position::BeforeUsername..].to_string())
}

const STATUS_BAR_INTERVAL: Duration = Duration::from_secs(120);
const STATUS_BAR_RETRY: Duration = Duration::from_secs(30);
const STATUS_BAR_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_BAR_STALE: Duration = Duration::from_secs(600);
const STATUS_BAR_TOKEN: &str = "wakatime-today";
const STATUS_BAR_TITLE: &str = "WakaTime";
const STATUS_BAR_UNAVAILABLE: &str = "unavailable";

fn wakatime_home() -> Option<PathBuf> {
    for key in ["WAKATIME_HOME", "HOME", "USERPROFILE"] {
        if let Some(dir) = env::var_os(key).filter(|dir| !dir.is_empty()) {
            return Some(PathBuf::from(dir));
        }
    }

    None
}

fn status_bar_enabled() -> bool {
    let Some(config) = wakatime_home().map(|dir| dir.join(".wakatime.cfg")) else {
        return true;
    };

    let Ok(contents) = fs::read_to_string(config) else {
        return true;
    };

    let mut section = "";
    let mut enabled = true;

    for line in contents.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim();
            continue;
        }

        if section != "settings" || line.starts_with([';', '#']) {
            continue;
        }

        let Some((key, value)) = line.split_once(['=', ':']) else {
            continue;
        };

        if key.trim() == "status_bar_enabled" {
            let value = value.split([';', '#']).next().unwrap_or_default();
            enabled = !value.trim().eq_ignore_ascii_case("false");
        }
    }

    enabled
}

async fn today(wakatime_path: &str, plugin: &str, settings: &Settings) -> Option<String> {
    let mut command = TokioCommand::new(wakatime_path);

    command.arg("--today").kill_on_drop(true);

    if !plugin.is_empty() {
        command.arg("--plugin").arg(plugin);
    }

    settings.apply(&mut command);

    let output = tokio::time::timeout(STATUS_BAR_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let today = String::from_utf8_lossy(&output.stdout).trim().to_string();

    (!today.is_empty()).then_some(today)
}

async fn report_status_bar(client: &Client, message: String) {
    client
        .send_notification::<Progress>(ProgressParams {
            token: ProgressToken::String(STATUS_BAR_TOKEN.to_string()),
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                WorkDoneProgressReport {
                    message: Some(message),
                    ..Default::default()
                },
            )),
        })
        .await;
}

async fn run_status_bar(
    client: Client,
    wakatime_path: String,
    plugin: Arc<String>,
    settings: Arc<Settings>,
) {
    let created = client
        .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
            token: ProgressToken::String(STATUS_BAR_TOKEN.to_string()),
        })
        .await;

    if created.is_err() {
        return;
    }

    let mut current = today(&wakatime_path, &plugin, &settings).await;
    let mut updated_at = tokio::time::Instant::now();

    client
        .send_notification::<Progress>(ProgressParams {
            token: ProgressToken::String(STATUS_BAR_TOKEN.to_string()),
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: STATUS_BAR_TITLE.to_string(),
                cancellable: Some(false),
                message: current.clone(),
                percentage: None,
            })),
        })
        .await;

    loop {
        let delay = if current.is_some() {
            STATUS_BAR_INTERVAL
        } else {
            STATUS_BAR_RETRY
        };

        tokio::time::sleep(delay).await;

        if let Some(today) = today(&wakatime_path, &plugin, &settings).await {
            updated_at = tokio::time::Instant::now();
            current = Some(today.clone());
            report_status_bar(&client, today).await;
        } else if current.is_some() && updated_at.elapsed() >= STATUS_BAR_STALE {
            current = None;
            report_status_bar(&client, STATUS_BAR_UNAVAILABLE.to_string()).await;
        }
    }
}

impl WakatimeLanguageServer {
    async fn send(&self, event: Event) {
        // if is_write is false, and file has not changed since last heartbeat,
        // and less than 2 minutes since last heartbeat, and do nothing
        const INTERVAL: SignedDuration = SignedDuration::from_mins(2);

        let mut current_file = self.current_file.lock().await;
        let now = Timestamp::now();

        #[cfg(debug_assertions)]
        self.client
            .log_message(
                MessageType::LOG,
                format!("Wakatime language server send called, event: {event:?}",),
            )
            .await;

        if event.uri == current_file.uri
            && now.duration_since(current_file.timestamp) < INTERVAL
            && !event.is_write
        {
            return;
        }

        let mut command = TokioCommand::new(self.wakatime_path.as_str());

        command
            .arg("--time")
            .arg((now.as_second() as f64).to_string())
            .arg("--write")
            .arg(event.is_write.to_string())
            .arg("--entity")
            .arg(event.uri.as_str());

        if !self.project_folder.is_empty() {
            command
                .arg("--project-folder")
                .arg(self.project_folder.as_str());
        }

        if !self.alternate_project.is_empty() {
            command
                .arg("--alternate-project")
                .arg(self.alternate_project.as_str());
        }

        if !self.platform.load().is_empty() {
            command.arg("--plugin").arg(self.platform.load().as_str());
        }

        self.settings.load().apply(&mut command);

        if let Some(ref language) = event.language {
            command.arg("--language").arg(language);
        } else {
            command.arg("--guess-language");
        }

        if let Some(lineno) = event.lineno {
            command.arg("--lineno").arg(lineno.to_string());
        }

        if let Some(cursor_pos) = event.cursor_pos {
            command.arg("--cursorpos").arg(cursor_pos.to_string());
        }

        self.client
            .log_message(
                MessageType::LOG,
                format!("Wakatime  command: {:?}", command.as_std()),
            )
            .await;

        if let Err(e) = command.output().await {
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "Wakatime language server send msg failed: {e:?}, command: {:?}",
                        command.as_std()
                    ),
                )
                .await;
        };

        current_file.uri = event.uri;
        current_file.timestamp = now;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for WakatimeLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let work_done_progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|window| window.work_done_progress)
            .unwrap_or(false);

        self.work_done_progress
            .store(work_done_progress, Ordering::Relaxed);

        if let Some(ref client_info) = params.client_info {
            let mut platform = String::new();
            platform.push_str("Zed");

            if let Some(ref version) = client_info.version {
                platform.push('/');
                platform.push_str(version.as_str());
            }

            platform.push(' ');
            platform.push_str(format!("Zed-wakatime/{}", env!("CARGO_PKG_VERSION")).as_str());

            self.platform.store(Arc::new(platform));
        }

        if let Some(initialization_options) = params.initialization_options {
            let initialization_options: Value = serde_json::from_value(initialization_options)
                .map_err(|_| "Could not parse settings (this should never happen)".to_string())
                .unwrap();

            let mut settings = Settings::default();

            if let Some(api_url) = initialization_options
                .get("api-url")
                .and_then(Value::as_str)
            {
                settings.api_url = Some(api_url.to_string());
            }

            if let Some(api_key) = initialization_options
                .get("api-key")
                .and_then(Value::as_str)
            {
                settings.api_key = Some(api_key.to_string());
            }

            self.settings.swap(Arc::from(settings));
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Wakatime language server initialized")
            .await;

        if self.work_done_progress.load(Ordering::Relaxed) && status_bar_enabled() {
            tokio::spawn(run_status_bar(
                self.client.clone(),
                self.wakatime_path.clone(),
                self.platform.load_full(),
                self.settings.load_full(),
            ));
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        // Update even when the opening heartbeat is throttled, e.g. after a language switch.
        self.document_languages.lock().await.insert(
            params.text_document.uri.clone(),
            params.text_document.language_id.clone(),
        );

        let event = Event {
            uri: extract_uri_string(&params.text_document.uri),
            is_write: false,
            lineno: None,
            language: Some(params.text_document.language_id),
            cursor_pos: None,
        };

        self.send(event).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let language = self
            .document_languages
            .lock()
            .await
            .get(&params.text_document.uri)
            .cloned();
        let event = Event {
            uri: extract_uri_string(&params.text_document.uri),
            is_write: false,
            lineno: params
                .content_changes
                .first()
                .map_or_else(|| None, |c| c.range)
                .map(|c| c.start.line as u64),
            language,
            cursor_pos: params
                .content_changes
                .first()
                .map_or_else(|| None, |c| c.range)
                .map(|c| c.start.character as u64),
        };

        self.send(event).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let language = self
            .document_languages
            .lock()
            .await
            .get(&params.text_document.uri)
            .cloned();
        let event = Event {
            uri: extract_uri_string(&params.text_document.uri),
            is_write: true,
            lineno: None,
            language,
            cursor_pos: None,
        };

        self.send(event).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.document_languages
            .lock()
            .await
            .remove(&params.text_document.uri);
    }
}

#[tokio::main]
async fn main() {
    let matches = Command::new("wakatime_ls")
        .version(env!("CARGO_PKG_VERSION"))
        .author("bestgopher <84328409@qq.com>")
        .about("A simple WakaTime language server tool")
        .arg(
            Arg::new("wakatime-cli")
                .short('p')
                .long("wakatime-cli")
                .help("wakatime-cli path")
                .required(true),
        )
        .arg(
            Arg::new("project-folder")
                .long("project-folder")
                .help("project folder path"),
        )
        .arg(
            Arg::new("alternate-project")
                .long("alternate-project")
                .help("alternate project name"),
        )
        .get_matches();

    let wakatime_cli = if let Some(s) = matches.get_one::<String>("wakatime-cli") {
        s.to_string()
    } else {
        "wakatime-cli".to_string()
    };

    let project_folder = if let Some(s) = matches.get_one::<String>("project-folder") {
        s.to_string()
    } else {
        String::new()
    };

    let alternate_project = if let Some(s) = matches.get_one::<String>("alternate-project") {
        s.to_string()
    } else {
        String::new()
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        Arc::new(WakatimeLanguageServer {
            client,
            settings: ArcSwap::from_pointee(Settings::default()),
            wakatime_path: wakatime_cli,
            project_folder,
            alternate_project,
            platform: ArcSwap::from_pointee(String::new()),
            work_done_progress: AtomicBool::new(false),
            document_languages: Mutex::new(HashMap::new()),
            current_file: Mutex::new(CurrentFile {
                uri: String::new(),
                timestamp: Timestamp::now(),
            }),
        })
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
