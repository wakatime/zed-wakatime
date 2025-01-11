use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{DateTime, Local, TimeDelta};
use clap::{Arg, Command};
use serde::Deserialize;
use tokio::{process::Command as TokioCommand, sync::Mutex};
use tower_lsp::{jsonrpc::Result, lsp_types::*, Client, LanguageServer, LspService, Server};

#[derive(Deserialize, Default)]
struct Setting {
    api_key: Option<String>,
    api_url: Option<String>,
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
    timestamp: DateTime<Local>,
}

struct WakatimeLanguageServer {
    client: Client,
    settings: ArcSwap<Setting>,
    wakatime_path: String,
    current_file: Mutex<CurrentFile>,
    platform: ArcSwap<String>,
}

impl WakatimeLanguageServer {
    async fn send(&self, event: Event) {
        // if isWrite is false, and file has not changed since last heartbeat,
        // and less than 2 minutes since last heartbeat, and do nothing
        const INTERVAL: TimeDelta = TimeDelta::minutes(2);

        let mut current_file = self.current_file.lock().await;
        let now = Local::now();

        if event.uri == current_file.uri
            && now - current_file.timestamp < INTERVAL
            && event.is_write
        {
            return;
        }

        let mut command = TokioCommand::new(self.wakatime_path.as_str());

        command
            .arg("--time")
            .arg((now.timestamp() as f64).to_string())
            .arg("--write")
            .arg(event.is_write.to_string())
            .arg("--entity")
            .arg(event.uri.as_str());

        if !self.platform.load().is_empty() {
            command.arg("--plugin").arg(self.platform.load().as_str());
        }

        let settings = self.settings.load();

        if let Some(ref key) = settings.api_key {
            command.arg("--key").arg(key);
        }

        if let Some(ref api_url) = settings.api_url {
            command.arg("--api-url").arg(api_url);
        }

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
                        "Wakatime language server send msg faild: {e:?}, command: {:?}",
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

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Wakatime language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let event = Event {
            uri: params.text_document.uri[url::Position::BeforeUsername..].to_string(),
            is_write: false,
            lineno: None,
            language: Some(params.text_document.language_id.clone()),
            cursor_pos: None,
        };

        self.send(event).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let event = Event {
            uri: params.text_document.uri[url::Position::BeforeUsername..].to_string(),
            is_write: false,
            lineno: params
                .content_changes
                .get(0)
                .map_or_else(|| None, |c| c.range)
                .map(|c| c.start.line as u64),
            language: None,
            cursor_pos: params
                .content_changes
                .get(0)
                .map_or_else(|| None, |c| c.range)
                .map(|c| c.start.character as u64),
        };

        self.send(event).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let event = Event {
            uri: params.text_document.uri[url::Position::BeforeUsername..].to_string(),
            is_write: true,
            lineno: None,
            language: None,
            cursor_pos: None,
        };

        self.send(event).await;
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
        .get_matches();

    let wakatime_cli = if let Some(s) = matches.get_one::<String>("wakatime-cli") {
        s.to_string()
    } else {
        "wakatime-cli".to_string()
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        Arc::new(WakatimeLanguageServer {
            client,
            settings: ArcSwap::from_pointee(Setting::default()),
            wakatime_path: wakatime_cli,
            platform: ArcSwap::from_pointee(String::new()),
            current_file: Mutex::new(CurrentFile {
                uri: String::new(),
                timestamp: Local::now(),
            }),
        })
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
