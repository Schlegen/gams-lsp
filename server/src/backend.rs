use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::store::DocumentStore;

pub struct Backend {
    client: Client,
    store: DocumentStore,
    parser: Mutex<tree_sitter::Parser>,
}

impl Backend {
    pub fn new(client: Client, language: tree_sitter::Language) -> Self {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).expect("failed to load GAMS grammar");
        Self { client, store: DocumentStore::new(), parser: Mutex::new(parser) }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "gams-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "gams-lsp server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        {
            let mut parser = self.parser.lock().unwrap();
            self.store.open(uri.clone(), &text, &mut parser);
        }
        self.client
            .log_message(MessageType::INFO, format!("opened and parsed: {uri}"))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut parser = self.parser.lock().unwrap();
            self.store.change(&uri, &params.content_changes, &mut parser);
        }
        self.client
            .log_message(MessageType::INFO, format!("reparsed: {uri}"))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.store.close(&uri);
        self.client
            .log_message(MessageType::INFO, format!("closed: {uri}"))
            .await;
    }
}
