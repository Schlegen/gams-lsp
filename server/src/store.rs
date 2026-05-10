use std::collections::HashSet;
use std::path::{Path, PathBuf};

use dashmap::DashMap;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use crate::document::GamsDocument;
use crate::symbols::SymbolTable;

// ---------------------------------------------------------------------------
// DocumentStore
// ---------------------------------------------------------------------------

pub struct DocumentStore {
    inner: DashMap<Url, GamsDocument>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self { inner: DashMap::new() }
    }

    /// Parse and store a newly opened document, then recursively load includes.
    pub fn open(&self, uri: Url, text: &str, parser: &mut tree_sitter::Parser) {
        let file = uri_to_path(&uri);
        let doc = GamsDocument::parse(file.clone(), text, parser);
        self.inner.insert(uri.clone(), doc);
        let mut visited = HashSet::from([file]);
        self.load_includes(&uri, parser, &mut visited);
    }

    /// Apply incremental changes and reparse.
    pub fn change(
        &self,
        uri: &Url,
        changes: &[TextDocumentContentChangeEvent],
        parser: &mut tree_sitter::Parser,
    ) {
        if let Some(mut doc) = self.inner.get_mut(uri) {
            let file = uri_to_path(uri);
            doc.update(changes, parser, &file);
        }
    }

    /// Remove a document from the store (called on `textDocument/didClose`).
    pub fn close(&self, uri: &Url) {
        self.inner.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<dashmap::mapref::one::Ref<'_, Url, GamsDocument>> {
        self.inner.get(uri)
    }

    // -----------------------------------------------------------------------
    // Include resolution
    // -----------------------------------------------------------------------

    /// Recursively load `$include`d files into the store.
    /// `visited` tracks canonical paths to break cycles.
    fn load_includes(
        &self,
        uri: &Url,
        parser: &mut tree_sitter::Parser,
        visited: &mut HashSet<PathBuf>,
    ) {
        let include_paths = {
            let Some(doc) = self.inner.get(uri) else { return };
            doc.include_paths()
        };

        let base_dir = uri_to_path(uri);
        let base_dir = base_dir.parent().unwrap_or(Path::new("."));

        for rel_path in include_paths {
            let full_path = base_dir.join(&rel_path);
            let Ok(canonical) = full_path.canonicalize() else { continue };
            if !visited.insert(canonical.clone()) {
                continue; // cycle or already loaded
            }
            let Ok(text) = std::fs::read_to_string(&canonical) else { continue };
            let Ok(include_uri) = Url::from_file_path(&canonical) else { continue };

            if !self.inner.contains_key(&include_uri) {
                let doc = GamsDocument::parse(canonical.clone(), &text, parser);
                self.inner.insert(include_uri.clone(), doc);
            }
            self.load_includes(&include_uri, parser, visited);
        }
    }
}

// ---------------------------------------------------------------------------
// Transitive include URI set
// ---------------------------------------------------------------------------

impl DocumentStore {
    /// All URIs reachable from `uri` via transitive `$include`s, including `uri`
    /// itself.  Used by the references feature to scope searches to the current
    /// include graph.
    pub fn transitive_uris(&self, uri: &Url) -> Vec<Url> {
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        self.collect_uris_recursive(uri, &mut visited, &mut result);
        result
    }

    fn collect_uris_recursive(
        &self,
        uri: &Url,
        visited: &mut HashSet<Url>,
        result: &mut Vec<Url>,
    ) {
        if !visited.insert(uri.clone()) {
            return;
        }
        result.push(uri.clone());
        let Some(doc) = self.inner.get(uri) else { return };
        let include_paths = doc.include_paths();
        let base_file = uri_to_path(uri);
        drop(doc);
        let base_dir = base_file.parent().unwrap_or(Path::new("."));
        for rel_path in include_paths {
            if rel_path.contains('%') {
                continue;
            }
            let Ok(canonical) = base_dir.join(&rel_path).canonicalize() else { continue };
            let Ok(inc_uri) = Url::from_file_path(&canonical) else { continue };
            self.collect_uris_recursive(&inc_uri, visited, result);
        }
    }
}

// ---------------------------------------------------------------------------
// Symbol lookup across includes
// ---------------------------------------------------------------------------

impl DocumentStore {
    /// Merged symbol table for `uri` and all its transitive `$include`s.
    pub fn merged_symbols(&self, uri: &Url) -> SymbolTable {
        let mut visited = HashSet::new();
        self.collect_symbols_recursive(uri, &mut visited)
    }

    fn collect_symbols_recursive(
        &self,
        uri: &Url,
        visited: &mut HashSet<Url>,
    ) -> SymbolTable {
        if !visited.insert(uri.clone()) {
            return SymbolTable::new();
        }
        let Some(doc) = self.inner.get(uri) else { return SymbolTable::new() };
        let mut table = doc.symbol_table.clone();
        let include_paths = doc.include_paths();
        let base_file = uri_to_path(uri);
        drop(doc); // release DashMap guard before recursing

        let base_dir = base_file.parent().unwrap_or(Path::new("."));
        for rel_path in include_paths {
            if rel_path.contains('%') {
                continue;
            }
            let Ok(canonical) = base_dir.join(&rel_path).canonicalize() else { continue };
            let Ok(inc_uri) = Url::from_file_path(&canonical) else { continue };
            table.merge(self.collect_symbols_recursive(&inc_uri, visited));
        }
        table
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn uri_to_path(uri: &Url) -> PathBuf {
    uri.to_file_path().unwrap_or_else(|_| PathBuf::from(uri.path()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::gams_language;
    use tower_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent};

    fn make_parser() -> tree_sitter::Parser {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&gams_language()).unwrap();
        p
    }

    fn file_uri(path: &std::path::Path) -> Url {
        Url::from_file_path(path).unwrap()
    }

    // -----------------------------------------------------------------------
    // open / close
    // -----------------------------------------------------------------------

    #[test]
    fn open_stores_document() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "Scalar x / 1 /;\n", &mut parser);
        assert!(store.get(&uri).is_some());
    }

    #[test]
    fn close_removes_document() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "Scalar x / 1 /;\n", &mut parser);
        store.close(&uri);
        assert!(store.get(&uri).is_none());
    }

    // -----------------------------------------------------------------------
    // change
    // -----------------------------------------------------------------------

    #[test]
    fn change_updates_body_text() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "Scalar x / 1 /;\n", &mut parser);

        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "Scalar y / 2 /;\n".to_string(),
        };
        store.change(&uri, &[change], &mut parser);

        let doc = store.get(&uri).unwrap();
        assert!(doc.body_text.contains("Scalar y"));
        assert!(!doc.body_text.contains("Scalar x"));
    }

    #[test]
    fn change_incremental_insert() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "Scalar x / 1 /;\n", &mut parser);

        // Append a new line at the end
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 1, character: 0 },
                end:   Position { line: 1, character: 0 },
            }),
            range_length: None,
            text: "Scalar y / 2 /;\n".to_string(),
        };
        store.change(&uri, &[change], &mut parser);

        let doc = store.get(&uri).unwrap();
        assert!(doc.body_text.contains("Scalar x"));
        assert!(doc.body_text.contains("Scalar y"));
    }

    #[test]
    fn change_strips_new_directive() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "Scalar x / 1 /;\n", &mut parser);

        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "$set foo bar\nScalar x / 1 /;\n".to_string(),
        };
        store.change(&uri, &[change], &mut parser);

        let doc = store.get(&uri).unwrap();
        assert!(!doc.body_text.contains("$set"));
        assert!(doc.body_text.contains("Scalar x"));
    }

    // -----------------------------------------------------------------------
    // $include loading
    // -----------------------------------------------------------------------

    #[test]
    fn open_loads_include_into_store() {
        // Write two temp files: main.gms includes helper.gms
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper.gms");
        let main   = dir.path().join("main.gms");

        std::fs::write(&helper, "Scalar y / 2 /;\n").unwrap();
        let main_text = format!("$include helper.gms\nScalar x / 1 /;\n");
        std::fs::write(&main, &main_text).unwrap();

        let store = DocumentStore::new();
        let mut parser = make_parser();
        let main_uri = file_uri(&main);
        store.open(main_uri.clone(), &main_text, &mut parser);

        // main.gms itself is in the store
        assert!(store.get(&main_uri).is_some());

        // helper.gms should have been loaded automatically
        let helper_uri = file_uri(&helper);
        assert!(store.get(&helper_uri).is_some(), "helper.gms should be loaded via $include");
    }

    #[test]
    fn open_cycle_detection_does_not_loop() {
        // a.gms includes b.gms, b.gms includes a.gms — must not infinite loop
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.gms");
        let b = dir.path().join("b.gms");

        std::fs::write(&a, "$include b.gms\nScalar x / 1 /;\n").unwrap();
        std::fs::write(&b, "$include a.gms\nScalar y / 2 /;\n").unwrap();

        let a_text = std::fs::read_to_string(&a).unwrap();
        let store = DocumentStore::new();
        let mut parser = make_parser();
        // Should return without hanging
        store.open(file_uri(&a), &a_text, &mut parser);
    }

    #[test]
    fn open_missing_include_does_not_panic() {
        let store = DocumentStore::new();
        let mut parser = make_parser();
        // Include path refers to a file that doesn't exist — should be silently skipped
        let uri = Url::parse("file:///tmp/test.gms").unwrap();
        store.open(uri.clone(), "$include nonexistent_file.gms\n", &mut parser);
        assert!(store.get(&uri).is_some());
    }
}
