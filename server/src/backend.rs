use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::features;
use crate::store::DocumentStore;

// ---------------------------------------------------------------------------
// File-path completion helpers (4e)
// ---------------------------------------------------------------------------

/// True when the line prefix ends inside an open `%…` span (odd % count).
fn is_inside_dollar_var_ref(prefix: &str) -> bool {
    prefix.chars().filter(|&c| c == '%').count() % 2 == 1
}

/// If `prefix` is `$include <path>` (or `$batinclude`), return the path fragment.
fn include_path_prefix(prefix: &str) -> Option<String> {
    let t = prefix.trim_start().to_lowercase();
    for kw in ["$batinclude", "$include"] {
        if t.starts_with(kw) {
            let after = prefix.trim_start()[kw.len()..].trim_start().to_string();
            return Some(after);
        }
    }
    None
}

fn file_completions(prefix: &str, base_dir: &std::path::Path) -> Vec<CompletionItem> {
    let candidate = base_dir.join(prefix);
    let (dir, file_prefix): (std::path::PathBuf, String) =
        if prefix.ends_with('/') || prefix.ends_with(std::path::MAIN_SEPARATOR) {
            (candidate, String::new())
        } else {
            let name = candidate
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let parent = candidate
                .parent()
                .unwrap_or(base_dir)
                .to_path_buf();
            (parent, name)
        };

    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    entries
        .filter_map(|e| {
            let entry = e.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(&file_prefix) {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            let label = if is_dir { format!("{name}/") } else { name };
            Some(CompletionItem {
                label,
                kind: Some(if is_dir {
                    CompletionItemKind::FOLDER
                } else {
                    CompletionItemKind::FILE
                }),
                ..Default::default()
            })
        })
        .collect()
}

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
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "%".to_string(),
                        "/".to_string(),
                    ]),
                    ..Default::default()
                }),
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
        self.publish_diagnostics(&uri).await;
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
        self.publish_diagnostics(&uri).await;
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

    // -----------------------------------------------------------------------
    // 4d — Hover
    // -----------------------------------------------------------------------

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        // First: check for %var% reference in the raw source line.
        // Extract var_name while holding the doc guard, then release it so
        // merged_symbols can acquire its own guards across all included files.
        let dollar_var_name: Option<String> = {
            let Some(doc) = self.store.get(&uri) else { return Ok(None) };
            let line_idx = pos.line as usize;
            if line_idx < doc.rope.len_lines() {
                let line = doc.rope.line(line_idx).to_string();
                features::dollar_var_name_at_position(&line, pos.character as usize)
            } else {
                None
            }
        }; // doc guard released

        if let Some(var_name) = dollar_var_name {
            let table = self.store.merged_symbols(&uri);
            if let Some(dv) = table.dollar_var(&var_name) {
                let md = features::format_hover_dollar_var(dv);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: None,
                }));
            }
        }

        // Second: identifier in the tree-sitter AST.
        let name_opt: Option<String> = {
            let Some(doc) = self.store.get(&uri) else { return Ok(None) };
            let Some(tree) = doc.tree.as_ref() else { return Ok(None) };
            features::identifier_at_position(tree, doc.body_text.as_bytes(), &doc.source_map, pos)
        };

        let Some(name) = name_opt else { return Ok(None) };

        let table = self.store.merged_symbols(&uri);
        let mut markdown = String::new();
        for sym in table.lookup(&name) {
            if !markdown.is_empty() {
                markdown.push_str("\n\n---\n\n");
            }
            markdown.push_str(&features::format_hover_symbol(sym));
        }

        if markdown.is_empty() {
            return Ok(None);
        }
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: None,
        }))
    }

    // -----------------------------------------------------------------------
    // 4e — Completion
    // -----------------------------------------------------------------------

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        // Extract line prefix and base dir; release the doc guard before calling
        // merged_symbols (which acquires its own guards across included files).
        let (line_prefix, base_dir) = {
            let Some(doc) = self.store.get(&uri) else {
                return Ok(None);
            };
            let line_idx = pos.line as usize;
            let prefix = if line_idx < doc.rope.len_lines() {
                let line = doc.rope.line(line_idx).to_string();
                let end = (pos.character as usize).min(line.len());
                line[..end].to_string()
            } else {
                String::new()
            };
            let base = uri
                .to_file_path()
                .unwrap_or_default()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            (prefix, base)
        }; // doc guard released

        // All three completion contexts need the merged table (cross-file symbols
        // and cross-file dollar vars).
        let table = self.store.merged_symbols(&uri);

        let items: Vec<CompletionItem> = if is_inside_dollar_var_ref(&line_prefix) {
            features::dollar_var_completion_items(&table)
        } else if let Some(path_prefix) = include_path_prefix(&line_prefix) {
            file_completions(&path_prefix, &base_dir)
        } else {
            features::symbol_completion_items(&table)
        };

        Ok(if items.is_empty() { None } else { Some(CompletionResponse::Array(items)) })
    }
}

// ---------------------------------------------------------------------------
// Backend helpers (not part of LanguageServer trait)
// ---------------------------------------------------------------------------

impl Backend {
    async fn publish_diagnostics(&self, uri: &Url) {
        let diags: Vec<tower_lsp::lsp_types::Diagnostic> = {
            let Some(doc) = self.store.get(uri) else { return };
            features::collect_diagnostics(&*doc)
        };
        self.client
            .publish_diagnostics(uri.clone(), diags, None)
            .await;
    }
}
