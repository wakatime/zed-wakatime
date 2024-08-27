use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{DateTime, Local, TimeDelta};
use clap::{Arg, Command};
use tokio::{process::Command as TokioCommand, sync::Mutex};
use tower_lsp::{jsonrpc::Result, lsp_types::*, Client, LanguageServer, LspService, Server};

#[derive(Default, Debug)]
struct Event {
    uri: String,
    is_write: bool,
    language: Option<String>,
    lineno: Option<u64>,
    cursorpos: Option<u64>,
}

#[derive(Debug)]
struct CurrentFile {
    uri: String,
    timestamp: DateTime<Local>,
}

struct WakatimeLanguageServer {
    client: Client,
    wakatime_path: String,
    api_key: Option<String>,
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

        if let Some(ref key) = self.api_key {
            command.arg("--key").arg(key);
        }

        if let Some(ref language) = event.language {
            command.arg("--language").arg(language);
        } else {
            command.arg("--guess-language");
        }

        if let Some(lineno) = event.lineno {
            command.arg("--lineno").arg(lineno.to_string());
        }

        if let Some(cursorpos) = event.cursorpos {
            command.arg("--cursorpos").arg(cursorpos.to_string());
        }

        self.client
            .log_message(MessageType::LOG, format!("command: {:?}", command.as_std()))
            .await;

        match command.output().await {
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!("Wakatime language server send msg faild: {e:?}"),
                    )
                    .await
            }
            Ok(o) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Wakatime language server send msg successful: {o:?}"),
                    )
                    .await
            }
        }

        current_file.uri = event.uri;
        current_file.timestamp = now;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for WakatimeLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(ref client_info) = params.client_info {
            let mut platform = String::new();
            platform.push_str(client_info.name.as_str());

            if let Some(ref version) = client_info.version {
                platform.push('/');
                platform.push_str(version.as_str());
            }

            platform.push(' ');
            platform.push_str(
                format!(
                    "{}-wakatime/{}",
                    client_info.name.as_str(),
                    env!("CARGO_PKG_VERSION")
                )
                .as_str(),
            );

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
            cursorpos: None,
        };

        self.send(event).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let event = Event {
            uri: params.text_document.uri[url::Position::BeforeUsername..].to_string(),
            is_write: false,
            lineno: None, // todo
            language: None,
            cursorpos: None,
        };

        self.send(event).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let event = Event {
            uri: params.text_document.uri[url::Position::BeforeUsername..].to_string(),
            is_write: true,
            lineno: None,
            language: None,
            cursorpos: None,
        };

        self.send(event).await;
    }
}

#[tokio::main]
async fn main() {
    let matches = Command::new("wakatime_ls")
        .version(env!("CARGO_PKG_VERSION"))
        .author("bestgopher <84328409@qq.com>")
        .about("A simple wakaTime language server tool")
        .arg(
            Arg::new("wakatime-cli")
                .short('p')
                .long("wakatime-cli")
                .help("wakatime-cli path")
                .required(true),
        )
        .arg(
            Arg::new("api-key")
                .short('k')
                .long("api-key")
                .help("the api key of wakatime-cli"),
        )
        .get_matches();

    let wakatime_cli = matches.get_one::<String>("wakatime-cli").unwrap();
    let api_key = matches.get_one::<String>("api-key").map(|x| x.to_string());

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        Arc::new(WakatimeLanguageServer {
            client,
            wakatime_path: wakatime_cli.to_string(),
            api_key,
            platform: ArcSwap::from_pointee(String::new()),
            current_file: Mutex::new(CurrentFile {
                uri: String::new(),
                timestamp: Local::now(),
            }),
        })
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
