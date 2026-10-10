use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;

struct TestServer {
    service: LspService<WakatimeLanguageServer>,
    directory: PathBuf,
}

impl TestServer {
    fn new() -> Self {
        static NEXT_DIRECTORY_ID: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "wakatime-ls-language-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        let binary = directory.join("wakatime-cli");
        // Capture the actual CLI arguments without sending any real heartbeats.
        fs::write(&binary, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let (service, socket) = LspService::new(|client| WakatimeLanguageServer {
            client,
            settings: ArcSwap::from_pointee(Settings::default()),
            wakatime_path: binary.to_string_lossy().into_owned(),
            project_folder: String::new(),
            alternate_project: String::new(),
            document_languages: Mutex::new(HashMap::new()),
            current_file: Mutex::new(CurrentFile {
                uri: String::new(),
                timestamp: Timestamp::now(),
            }),
            platform: ArcSwap::from_pointee(String::new()),
            work_done_progress: AtomicBool::new(false),
        });
        // These tests call the LSP handlers directly and do not consume client logs.
        drop(socket);
        Self { service, directory }
    }

    fn uri(&self, filename: &str) -> url::Url {
        url::Url::from_file_path(self.directory.join(filename)).unwrap()
    }

    async fn open(&self, uri: &url::Url, language: &str) {
        self.service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    language.to_string(),
                    1,
                    String::new(),
                ),
            })
            .await;
    }

    async fn change(&self, uri: &url::Url) {
        self.service
            .inner()
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 2),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(Range::default()),
                    range_length: None,
                    text: "x".to_string(),
                }],
            })
            .await;
    }

    async fn save(&self, uri: &url::Url) {
        self.service
            .inner()
            .did_save(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                text: None,
            })
            .await;
    }

    async fn close(&self, uri: &url::Url) {
        self.service
            .inner()
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
            })
            .await;
    }

    fn assert_language(&self, uri: &url::Url, expected: Option<&str>) {
        let capture = self.directory.join("wakatime-cli.args");
        let contents = fs::read_to_string(&capture).unwrap();
        let args: Vec<_> = contents.lines().collect();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--entity", &extract_uri_string(uri)]));
        let language = args.windows(2).find(|pair| pair[0] == "--language");
        assert_eq!(language.map(|pair| pair[1]), expected);
        assert_eq!(args.contains(&"--guess-language"), expected.is_none());
        // A later assertion must observe a new CLI invocation, not stale output.
        fs::remove_file(capture).unwrap();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn preserves_each_documents_zed_language_on_open_change_and_save() {
    let server = TestServer::new();
    let documents = [
        (server.uri("Program.cs"), "csharp"),
        (server.uri("notes.txt"), "CustomLanguage"),
    ];
    for (uri, language) in &documents {
        server.open(uri, language).await;
        server.assert_language(uri, Some(language));
    }
    for (uri, language) in &documents {
        server.change(uri).await;
        server.assert_language(uri, Some(language));
    }
    for (uri, language) in &documents {
        server.save(uri).await;
        server.assert_language(uri, Some(language));
    }
}

#[tokio::test]
async fn updates_language_when_reopening_even_if_open_heartbeat_is_throttled() {
    let server = TestServer::new();
    let uri = server.uri("Program.cs");
    server.open(&uri, "csharp").await;
    server.assert_language(&uri, Some("csharp"));
    server.close(&uri).await;
    server.open(&uri, "plaintext").await;
    assert!(!server.directory.join("wakatime-cli.args").exists());
    server.save(&uri).await;
    server.assert_language(&uri, Some("plaintext"));
}

#[tokio::test]
async fn closing_only_forgets_that_document_and_unknown_documents_use_guessing() {
    let server = TestServer::new();
    let closed = server.uri("Program.cs");
    let open = server.uri("main.rs");
    let unknown = server.uri("unknown.txt");
    server.open(&closed, "csharp").await;
    server.assert_language(&closed, Some("csharp"));
    server.open(&open, "rust").await;
    server.assert_language(&open, Some("rust"));
    server.close(&closed).await;
    server.save(&closed).await;
    server.assert_language(&closed, None);
    server.save(&open).await;
    server.assert_language(&open, Some("rust"));
    server.change(&unknown).await;
    server.assert_language(&unknown, None);
    server.save(&unknown).await;
    server.assert_language(&unknown, None);
}
