use std::path::{Path, PathBuf};

use gams_precompiler::{tokenize_str, Diagnostic, Severity, Token, TokenKind};
use ropey::Rope;
use tower_lsp::lsp_types::{Position, TextDocumentContentChangeEvent};
use tree_sitter::Tree;

// ---------------------------------------------------------------------------
// Source map — maps tree-sitter (body-text) coordinates → original file lines
// ---------------------------------------------------------------------------

pub struct SourceMap {
    /// Maps 0-based body-text line index → 1-based original file line number.
    pub body_line_to_orig: Vec<u32>,
    pub orig_file: PathBuf,
}

impl SourceMap {
    pub fn orig_line(&self, body_line: usize) -> Option<u32> {
        self.body_line_to_orig.get(body_line).copied()
    }
}

// ---------------------------------------------------------------------------
// GamsDocument
// ---------------------------------------------------------------------------

pub struct GamsDocument {
    pub rope: Rope,
    pub dollar_tokens: Vec<Token>,
    /// Filtered GAMS body text fed to tree-sitter (no directives, no * comments).
    pub body_text: String,
    pub source_map: SourceMap,
    pub tree: Option<Tree>,
    /// Diagnostics produced by the dollar-layer lexer.
    pub dollar_diagnostics: Vec<Diagnostic>,
}

impl GamsDocument {
    pub fn parse(file: PathBuf, text: &str, parser: &mut tree_sitter::Parser) -> Self {
        let rope = Rope::from_str(text);
        let (dollar_tokens, dollar_diagnostics) = run_lexer(file.clone(), text);
        let (body_text, source_map) = build_body(&dollar_tokens, &file);
        let tree = parser.parse(body_text.as_bytes(), None);
        Self { rope, dollar_tokens, body_text, source_map, tree, dollar_diagnostics }
    }

    pub fn update(
        &mut self,
        changes: &[TextDocumentContentChangeEvent],
        parser: &mut tree_sitter::Parser,
        file: &Path,
    ) {
        for change in changes {
            apply_incremental_change(&mut self.rope, change);
        }
        let text = self.rope.to_string();
        let (dollar_tokens, dollar_diagnostics) = run_lexer(file.to_path_buf(), &text);
        let (body_text, source_map) = build_body(&dollar_tokens, file);
        self.tree = parser.parse(body_text.as_bytes(), None);
        self.dollar_tokens = dollar_tokens;
        self.body_text = body_text;
        self.source_map = source_map;
        self.dollar_diagnostics = dollar_diagnostics;
    }

    /// Literal `$include` paths (skips paths containing `%var%` references).
    pub fn include_paths(&self) -> Vec<String> {
        self.dollar_tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Include)
            .filter_map(|t| t.args.first().cloned())
            .filter(|p| !p.contains('%'))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_lexer(file: PathBuf, text: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    match tokenize_str(file.clone(), text) {
        Ok(tokens) => (tokens, vec![]),
        Err(err) => {
            let diag = Diagnostic::new(
                "L000",
                err.message.clone(),
                err.loc.clone(),
                Severity::Error,
            );
            (vec![], vec![diag])
        }
    }
}

/// Build the filtered body text and source map from the dollar-layer token list.
/// Each `BodyText` token is a single line (with its trailing newline included),
/// so concatenating them preserves line structure for tree-sitter.
pub fn build_body(tokens: &[Token], file: &Path) -> (String, SourceMap) {
    let mut body_parts: Vec<&str> = Vec::new();
    let mut orig_lines: Vec<u32> = Vec::new();

    for token in tokens {
        if token.kind == TokenKind::BodyText {
            body_parts.push(&token.raw);
            orig_lines.push(token.loc.line);
        }
    }

    let body_text: String = body_parts.concat();
    let source_map = SourceMap { body_line_to_orig: orig_lines, orig_file: file.to_path_buf() };

    (body_text, source_map)
}

/// Apply a single LSP incremental (or full) content change to a `Rope`.
pub fn apply_incremental_change(rope: &mut Rope, change: &TextDocumentContentChangeEvent) {
    match &change.range {
        None => {
            *rope = Rope::from_str(&change.text);
        }
        Some(range) => {
            let start = lsp_pos_to_char_idx(rope, range.start);
            let end = lsp_pos_to_char_idx(rope, range.end);
            if start != end {
                rope.remove(start..end);
            }
            if !change.text.is_empty() {
                rope.insert(start, &change.text);
            }
        }
    }
}

fn lsp_pos_to_char_idx(rope: &Rope, pos: Position) -> usize {
    let line = (pos.line as usize).min(rope.len_lines().saturating_sub(1));
    let line_start = rope.line_to_char(line);
    let line_len = rope.line(line).len_chars();
    // character is a UTF-16 code unit offset in LSP; for ASCII GAMS files this equals char offset
    line_start + (pos.character as usize).min(line_len)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::gams_language;
    use tower_lsp::lsp_types::{Range, TextDocumentContentChangeEvent};

    fn make_parser() -> tree_sitter::Parser {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&gams_language()).unwrap();
        p
    }

    // -----------------------------------------------------------------------
    // build_body
    // -----------------------------------------------------------------------

    #[test]
    fn build_body_strips_directives_and_comments() {
        let text = "* comment\n$set x 1\nSets i / 1*3 /;\n";
        let tokens = gams_precompiler::tokenize_str(
            std::path::PathBuf::from("test.gms"),
            text,
        )
        .unwrap();
        let (body, map) = build_body(&tokens, std::path::Path::new("test.gms"));
        // Only the body line survives
        assert_eq!(body, "Sets i / 1*3 /;\n");
        // That one body line maps to original line 3
        assert_eq!(map.orig_line(0), Some(3));
    }

    #[test]
    fn build_body_empty_file_gives_empty_body() {
        let tokens = gams_precompiler::tokenize_str(
            std::path::PathBuf::from("test.gms"),
            "",
        )
        .unwrap();
        let (body, map) = build_body(&tokens, std::path::Path::new("test.gms"));
        assert!(body.is_empty());
        assert!(map.body_line_to_orig.is_empty());
    }

    #[test]
    fn build_body_ontext_block_excluded() {
        let text = "Scalar x / 1 /;\n$ontext\nthis is a comment\n$offtext\nScalar y / 2 /;\n";
        let tokens = gams_precompiler::tokenize_str(
            std::path::PathBuf::from("test.gms"),
            text,
        )
        .unwrap();
        let (body, map) = build_body(&tokens, std::path::Path::new("test.gms"));
        // Lines inside $ontext/$offtext are excluded from body
        assert!(body.contains("Scalar x"));
        assert!(body.contains("Scalar y"));
        assert!(!body.contains("this is a comment"));
        assert_eq!(map.body_line_to_orig.len(), 2);
    }

    #[test]
    fn build_body_source_map_tracks_original_lines() {
        // Line 1: directive (stripped)
        // Line 2: body
        // Line 3: comment (stripped)
        // Line 4: body
        let text = "$set x 1\nSets i / 1*3 /;\n* comment\nScalar y / 2 /;\n";
        let tokens = gams_precompiler::tokenize_str(
            std::path::PathBuf::from("test.gms"),
            text,
        )
        .unwrap();
        let (_, map) = build_body(&tokens, std::path::Path::new("test.gms"));
        assert_eq!(map.orig_line(0), Some(2)); // first body line was original line 2
        assert_eq!(map.orig_line(1), Some(4)); // second body line was original line 4
        assert_eq!(map.orig_line(2), None);    // no third body line
    }

    // -----------------------------------------------------------------------
    // GamsDocument::parse
    // -----------------------------------------------------------------------

    #[test]
    fn parse_produces_tree_for_body_text() {
        let mut parser = make_parser();
        let text = "Sets i / 1*3 /;\n";
        let doc = GamsDocument::parse(
            std::path::PathBuf::from("test.gms"),
            text,
            &mut parser,
        );
        assert!(doc.tree.is_some());
        assert_eq!(doc.body_text, text);
        assert!(doc.dollar_diagnostics.is_empty());
    }

    #[test]
    fn parse_strips_directives_from_body() {
        let mut parser = make_parser();
        let text = "$set scenario base\nSets i / 1*3 /;\n";
        let doc = GamsDocument::parse(
            std::path::PathBuf::from("test.gms"),
            text,
            &mut parser,
        );
        assert_eq!(doc.body_text, "Sets i / 1*3 /;\n");
        assert!(!doc.body_text.contains("$set"));
    }

    #[test]
    fn parse_strips_star_comments() {
        let mut parser = make_parser();
        let text = "* this is a comment\nScalar x / 1 /;\n";
        let doc = GamsDocument::parse(
            std::path::PathBuf::from("test.gms"),
            text,
            &mut parser,
        );
        assert!(!doc.body_text.contains("* this is a comment"));
        assert!(doc.body_text.contains("Scalar x"));
    }

    #[test]
    fn parse_unclosed_ontext_produces_diagnostic() {
        let mut parser = make_parser();
        let text = "$ontext\nno matching offtext\n";
        let doc = GamsDocument::parse(
            std::path::PathBuf::from("test.gms"),
            text,
            &mut parser,
        );
        assert!(!doc.dollar_diagnostics.is_empty());
    }

    // -----------------------------------------------------------------------
    // apply_incremental_change
    // -----------------------------------------------------------------------

    #[test]
    fn incremental_change_full_replace() {
        let mut rope = Rope::from_str("hello world");
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "goodbye".to_string(),
        };
        apply_incremental_change(&mut rope, &change);
        assert_eq!(rope.to_string(), "goodbye");
    }

    #[test]
    fn incremental_change_insert_at_start() {
        let mut rope = Rope::from_str("world");
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 0, character: 0 },
                end:   Position { line: 0, character: 0 },
            }),
            range_length: None,
            text: "hello ".to_string(),
        };
        apply_incremental_change(&mut rope, &change);
        assert_eq!(rope.to_string(), "hello world");
    }

    #[test]
    fn incremental_change_delete_range() {
        let mut rope = Rope::from_str("hello world");
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 0, character: 5 },
                end:   Position { line: 0, character: 11 },
            }),
            range_length: None,
            text: String::new(),
        };
        apply_incremental_change(&mut rope, &change);
        assert_eq!(rope.to_string(), "hello");
    }

    #[test]
    fn incremental_change_replace_word() {
        let mut rope = Rope::from_str("Sets i / 1*3 /;");
        // Replace "Sets" (chars 0-4) with "Parameter"
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 0, character: 0 },
                end:   Position { line: 0, character: 4 },
            }),
            range_length: None,
            text: "Parameter".to_string(),
        };
        apply_incremental_change(&mut rope, &change);
        assert_eq!(rope.to_string(), "Parameter i / 1*3 /;");
    }

    #[test]
    fn incremental_change_multiline() {
        let mut rope = Rope::from_str("line1\nline2\nline3\n");
        // Replace "line2" on line 1 with "updated"
        let change = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 1, character: 0 },
                end:   Position { line: 1, character: 5 },
            }),
            range_length: None,
            text: "updated".to_string(),
        };
        apply_incremental_change(&mut rope, &change);
        assert_eq!(rope.to_string(), "line1\nupdated\nline3\n");
    }

    // -----------------------------------------------------------------------
    // GamsDocument::update
    // -----------------------------------------------------------------------

    #[test]
    fn update_reruns_dollar_layer_and_tree_sitter() {
        let mut parser = make_parser();
        let initial = "* comment\nScalar x / 1 /;\n";
        let mut doc = GamsDocument::parse(
            std::path::PathBuf::from("test.gms"),
            initial,
            &mut parser,
        );
        assert_eq!(doc.body_text, "Scalar x / 1 /;\n");

        // Append another scalar via a full-replace change
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "* comment\nScalar x / 1 /;\nScalar y / 2 /;\n".to_string(),
        };
        doc.update(
            &[change],
            &mut parser,
            std::path::Path::new("test.gms"),
        );
        assert!(doc.body_text.contains("Scalar y"));
        assert!(doc.tree.is_some());
    }
}
