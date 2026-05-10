use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::features;
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
                definition_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
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

    // -----------------------------------------------------------------------
    // Document sync
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // 4a — Go to definition
    // -----------------------------------------------------------------------

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        // Resolve the identifier name under the cursor (release guard before await).
        let name_opt: Option<String> = {
            let Some(doc) = self.store.get(&uri) else { return Ok(None) };
            let Some(tree) = doc.tree.as_ref() else { return Ok(None) };
            features::identifier_at_position(tree, doc.body_text.as_bytes(), &doc.source_map, pos)
        };

        let Some(name) = name_opt else { return Ok(None) };

        let table = self.store.merged_symbols(&uri);
        let locations: Vec<Location> = table
            .lookup(&name)
            .filter_map(features::sym_to_location)
            .collect();

        Ok(match locations.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(locations.into_iter().next().unwrap())),
            _ => Some(GotoDefinitionResponse::Array(locations)),
        })
    }

    // -----------------------------------------------------------------------
    // 4b — Document highlights
    // -----------------------------------------------------------------------

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let highlights: Vec<DocumentHighlight> = {
            let Some(doc) = self.store.get(&uri) else { return Ok(None) };
            let Some(tree) = doc.tree.as_ref() else { return Ok(None) };

            let Some(name) =
                features::identifier_at_position(tree, doc.body_text.as_bytes(), &doc.source_map, pos)
            else {
                return Ok(None);
            };

            let refs =
                features::find_references_in_tree(tree, doc.body_text.as_bytes(), &name);

            refs.iter()
                .map(|(node, kind)| DocumentHighlight {
                    range: features::node_range(&doc.source_map, *node),
                    kind: Some(match kind {
                        features::ReferenceKind::Write => DocumentHighlightKind::WRITE,
                        features::ReferenceKind::Read => DocumentHighlightKind::READ,
                    }),
                })
                .collect()
        };

        Ok(if highlights.is_empty() { None } else { Some(highlights) })
    }

    // -----------------------------------------------------------------------
    // 4c — References
    // -----------------------------------------------------------------------

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        // Get the identifier name under cursor.
        let name_opt: Option<String> = {
            let Some(doc) = self.store.get(&uri) else { return Ok(None) };
            let Some(tree) = doc.tree.as_ref() else { return Ok(None) };
            features::identifier_at_position(tree, doc.body_text.as_bytes(), &doc.source_map, pos)
        };

        let Some(name) = name_opt else { return Ok(None) };

        // Search every file in the transitive include closure.
        let all_uris = self.store.transitive_uris(&uri);
        let mut locations: Vec<Location> = Vec::new();

        for file_uri in &all_uris {
            let Some(doc) = self.store.get(file_uri) else { continue };
            let Some(tree) = doc.tree.as_ref() else { continue };

            let refs = features::find_references_in_tree(tree, doc.body_text.as_bytes(), &name);
            for (node, _kind) in refs {
                locations.push(Location {
                    uri: file_uri.clone(),
                    range: features::node_range(&doc.source_map, node),
                });
            }
        }

        Ok(if locations.is_empty() { None } else { Some(locations) })
    }
}
