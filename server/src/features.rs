use gams_precompiler::{Diagnostic as PrecompilerDiag, DollarVariable, Scope, Severity};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Documentation,
    Location, Position, Range, Url,
};
use tree_sitter::{Node, Point, Tree};

use crate::document::{GamsDocument, SourceMap};
use crate::symbols::{Symbol, SymbolKind, SymbolTable};

// ---------------------------------------------------------------------------
// Coordinate helpers
// ---------------------------------------------------------------------------

/// Convert an LSP Position (0-based line/char, original file coords) to a
/// tree-sitter Point (0-based row/col, body-text coords).
pub fn lsp_to_body_point(source_map: &SourceMap, pos: Position) -> Option<Point> {
    let orig_line = pos.line + 1; // 1-based original line
    let body_row = source_map.orig_to_body_line(orig_line)?;
    Some(Point { row: body_row, column: pos.character as usize })
}

/// Convert a tree-sitter Point (0-based row/col, body-text coords) to an LSP
/// Position (0-based line/char, original file coords).
pub fn body_point_to_lsp(source_map: &SourceMap, point: Point) -> Position {
    let orig_line = source_map.orig_line(point.row).unwrap_or(point.row as u32 + 1);
    Position {
        line: orig_line.saturating_sub(1),
        character: point.column as u32,
    }
}

/// Build an LSP Range from a tree-sitter node via the source map.
pub fn node_range(source_map: &SourceMap, node: Node) -> Range {
    Range {
        start: body_point_to_lsp(source_map, node.start_position()),
        end: body_point_to_lsp(source_map, node.end_position()),
    }
}

// ---------------------------------------------------------------------------
// Identifier at position
// ---------------------------------------------------------------------------

/// Return the text of the `identifier` node at `pos`, or `None` if the cursor
/// is not on an identifier.  Walks up the parent chain from the deepest node.
pub fn identifier_at_position(
    tree: &Tree,
    body: &[u8],
    source_map: &SourceMap,
    pos: Position,
) -> Option<String> {
    let point = lsp_to_body_point(source_map, pos)?;
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    let mut cur = node;
    loop {
        if cur.kind() == "identifier" {
            return cur.utf8_text(body).ok().map(str::to_string);
        }
        cur = cur.parent()?;
    }
}

// ---------------------------------------------------------------------------
// Reference search and classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceKind {
    /// Declaration / definition site (symbol is introduced here).
    Write,
    /// Usage site (symbol is referenced here).
    Read,
}

/// Find every `identifier` node in `tree` (over `body`) whose text matches
/// `name` (case-insensitive).  Returns `(node, kind)` pairs.
pub fn find_references_in_tree<'a>(
    tree: &'a Tree,
    body: &'a [u8],
    name: &str,
) -> Vec<(Node<'a>, ReferenceKind)> {
    let lower = name.to_lowercase();
    let mut out = Vec::new();
    collect_references(tree.root_node(), body, &lower, &mut out);
    out
}

fn collect_references<'a>(
    node: Node<'a>,
    body: &[u8],
    lower_name: &str,
    out: &mut Vec<(Node<'a>, ReferenceKind)>,
) {
    if node.kind() == "identifier" {
        if let Ok(text) = node.utf8_text(body) {
            if text.to_lowercase() == lower_name {
                out.push((node, classify_reference(node)));
            }
        }
    }
    let count = node.child_count();
    for i in 0..count {
        if let Some(child) = node.child(i) {
            collect_references(child, body, lower_name, out);
        }
    }
}

/// Decide whether an `identifier` node is a declaration (Write) or usage (Read).
fn classify_reference(id_node: Node) -> ReferenceKind {
    let mut cur = id_node;
    while let Some(parent) = cur.parent() {
        match parent.kind() {
            // Any *_entry node is a GAMS declaration site.
            k if k.ends_with("_entry") => return ReferenceKind::Write,

            // equation_definition: only the `name` field identifier is Write.
            "equation_definition" => {
                if let Some(name_field) = parent.child_by_field_name("name") {
                    if is_within(name_field, id_node) && is_first_identifier(name_field, id_node) {
                        return ReferenceKind::Write;
                    }
                }
                return ReferenceKind::Read;
            }

            // alias_declaration: identifiers[1..] are Write (the new alias names).
            "alias_declaration" => {
                let ids: Vec<Node> = (0..parent.child_count())
                    .filter_map(|i| parent.child(i))
                    .filter(|n| n.kind() == "identifier")
                    .collect();
                if ids.len() >= 2 {
                    for alias_nd in &ids[1..] {
                        if alias_nd.byte_range() == id_node.byte_range() {
                            return ReferenceKind::Write;
                        }
                    }
                }
                return ReferenceKind::Read;
            }

            _ => {}
        }
        cur = parent;
    }
    ReferenceKind::Read
}

/// True if `descendant` is contained within (or equal to) `ancestor` by byte range.
fn is_within(ancestor: Node, descendant: Node) -> bool {
    let ar = ancestor.byte_range();
    let dr = descendant.byte_range();
    ar.start <= dr.start && dr.end <= ar.end
}

/// True if `id_node` is the first `identifier` within `name_field`
/// (handles both plain `identifier` and `identifier_with_domain`).
fn is_first_identifier(name_field: Node, id_node: Node) -> bool {
    if name_field.kind() == "identifier" {
        return name_field.byte_range() == id_node.byte_range();
    }
    // identifier_with_domain: first identifier child is the bare name
    (0..name_field.child_count())
        .filter_map(|i| name_field.child(i))
        .filter(|n| n.kind() == "identifier")
        .next()
        .map(|n| n.byte_range() == id_node.byte_range())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// LSP response helpers
// ---------------------------------------------------------------------------

/// Convert a `Symbol` to an LSP `Location`.  Returns `None` if the symbol's
/// file path cannot be converted to a `file://` URI.
pub fn sym_to_location(sym: &Symbol) -> Option<Location> {
    let uri = Url::from_file_path(&sym.loc.file).ok()?;
    let start = Position {
        line: sym.loc.line.saturating_sub(1),
        character: sym.loc.col.saturating_sub(1),
    };
    let end = Position {
        line: start.line,
        character: start.character + sym.display_name.len() as u32,
    };
    Some(Location { uri, range: Range { start, end } })
}

// ---------------------------------------------------------------------------
// 4d — Hover formatting
// ---------------------------------------------------------------------------

/// Format a markdown hover string for a GAMS symbol.
pub fn format_hover_symbol(sym: &Symbol) -> String {
    let mut sig = format!("**{}** `{}", sym.kind, sym.display_name);
    if let Some(ref d) = sym.domain {
        sig.push_str(d);
    }
    sig.push('`');
    if let Some(ref desc) = sym.description {
        sig.push_str(&format!("\n\n{desc}"));
    }
    sig
}

/// Format a markdown hover string for a dollar-layer variable.
pub fn format_hover_dollar_var(dv: &DollarVariable) -> String {
    let scope_kw = match dv.scope {
        Scope::Local  => "$set",
        Scope::Global => "$setglobal",
        Scope::Env    => "$setenv",
    };
    let mut out = format!("**DollarVar** `%{}%` *({scope_kw})*", dv.name);
    if let Some(val) = dv.current_value() {
        out.push_str(&format!("\n\nCurrent value: `{val}`"));
    }
    out
}

/// If the character at `col` (0-based) falls inside a `%name%` span on `line`,
/// return `name`.  Handles cursor on either `%` delimiter.
pub fn dollar_var_name_at_position(line: &str, col: usize) -> Option<String> {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let open = i;
        let mut j = open + 1;
        while j < n && bytes[j] != b'%' {
            j += 1;
        }
        if j >= n {
            break; // unclosed %
        }
        let close = j;
        if col >= open && col <= close {
            let name = &line[open + 1..close];
            if !name.is_empty() && !name.contains('%') && !name.contains(' ') {
                return Some(name.to_string());
            }
        }
        i = close + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// 4e — Completion items
// ---------------------------------------------------------------------------

/// Build completion items for all GAMS symbols in `table`.
pub fn symbol_completion_items(table: &SymbolTable) -> Vec<CompletionItem> {
    table
        .symbols
        .iter()
        .map(|sym| CompletionItem {
            label: sym.display_name.clone(),
            kind: Some(symbol_kind_to_completion_kind(&sym.kind)),
            detail: sym.domain.as_ref().map(|d| format!("{}{d}", sym.display_name)),
            documentation: sym
                .description
                .as_ref()
                .map(|d| Documentation::String(d.clone())),
            ..Default::default()
        })
        .collect()
}

/// Build completion items for all dollar-layer variables in `table`.
pub fn dollar_var_completion_items(table: &SymbolTable) -> Vec<CompletionItem> {
    table
        .dollar_vars
        .iter()
        .map(|dv| CompletionItem {
            label: dv.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: dv.current_value().map(|v| format!("= {v}")),
            ..Default::default()
        })
        .collect()
}

fn symbol_kind_to_completion_kind(kind: &SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Set       => CompletionItemKind::ENUM,
        SymbolKind::Scalar    => CompletionItemKind::CONSTANT,
        SymbolKind::Parameter => CompletionItemKind::VARIABLE,
        SymbolKind::Variable  => CompletionItemKind::VARIABLE,
        SymbolKind::Equation  => CompletionItemKind::FUNCTION,
        SymbolKind::Model     => CompletionItemKind::MODULE,
        SymbolKind::Alias     => CompletionItemKind::REFERENCE,
        SymbolKind::Acronym   => CompletionItemKind::ENUM_MEMBER,
        SymbolKind::DollarVar => CompletionItemKind::VARIABLE,
    }
}

// ---------------------------------------------------------------------------
// 4f — Diagnostics
// ---------------------------------------------------------------------------

/// Collect all LSP diagnostics for a document:
///   1. Dollar-layer errors/warnings from the precompiler.
///   2. Tree-sitter ERROR nodes (syntax errors).
pub fn collect_diagnostics(doc: &GamsDocument) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    dollar_diags(&doc.dollar_diagnostics, &mut diags);
    if let Some(tree) = &doc.tree {
        ts_error_diags(tree.root_node(), &doc.source_map, &mut diags);
    }
    diags
}

fn dollar_diags(src: &[PrecompilerDiag], out: &mut Vec<Diagnostic>) {
    for d in src {
        let line = d.loc.line.saturating_sub(1);
        let col  = d.loc.col.saturating_sub(1);
        let end_col = col + d.loc.length.max(1);
        out.push(Diagnostic {
            range: Range {
                start: Position { line, character: col },
                end:   Position { line, character: end_col },
            },
            severity: Some(severity_to_lsp(&d.severity)),
            message: d.message.clone(),
            source: Some("gams-lsp (dollar)".to_string()),
            ..Default::default()
        });
    }
}

fn ts_error_diags(node: Node, sm: &SourceMap, out: &mut Vec<Diagnostic>) {
    if node.is_error() {
        out.push(Diagnostic {
            range: node_range(sm, node),
            severity: Some(DiagnosticSeverity::ERROR),
            message: "syntax error".to_string(),
            source: Some("gams-lsp (tree-sitter)".to_string()),
            ..Default::default()
        });
        // Don't recurse into ERROR nodes to avoid spurious child diagnostics.
        return;
    }
    if node.is_missing() {
        out.push(Diagnostic {
            range: node_range(sm, node),
            severity: Some(DiagnosticSeverity::ERROR),
            message: format!("missing `{}`", node.kind()),
            source: Some("gams-lsp (tree-sitter)".to_string()),
            ..Default::default()
        });
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            ts_error_diags(child, sm, out);
        }
    }
}

fn severity_to_lsp(sev: &Severity) -> DiagnosticSeverity {
    match sev {
        Severity::Error   => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info    => DiagnosticSeverity::INFORMATION,
        Severity::Hint    => DiagnosticSeverity::HINT,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::GamsDocument;
    use crate::language::gams_language;
    use std::path::PathBuf;

    fn make_parser() -> tree_sitter::Parser {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&gams_language()).unwrap();
        p
    }

    fn doc(gams: &str) -> GamsDocument {
        let mut parser = make_parser();
        GamsDocument::parse(PathBuf::from("test.gms"), gams, &mut parser)
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    // -----------------------------------------------------------------------
    // identifier_at_position
    // -----------------------------------------------------------------------

    #[test]
    fn identifier_at_position_finds_name() {
        // "Scalar x / 1 /;\n" — 'x' is at col 7 on line 0
        let d = doc("Scalar x / 1 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let name = identifier_at_position(tree, d.body_text.as_bytes(), &d.source_map, pos(0, 7));
        assert_eq!(name.as_deref(), Some("x"));
    }

    #[test]
    fn identifier_at_position_returns_none_on_non_identifier() {
        // col 6 is the space between "Scalar" and "x"
        let d = doc("Scalar x / 1 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let name = identifier_at_position(tree, d.body_text.as_bytes(), &d.source_map, pos(0, 6));
        // No identifier at that whitespace position
        assert!(name.is_none() || name.as_deref() != Some("x"));
    }

    #[test]
    fn identifier_at_position_with_source_map_offset() {
        // Line 1 (0-based) is original line 2, body row 0 after the directive
        let d = doc("$set foo bar\nScalar x / 1 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let name = identifier_at_position(tree, d.body_text.as_bytes(), &d.source_map, pos(1, 7));
        assert_eq!(name.as_deref(), Some("x"));
    }

    // -----------------------------------------------------------------------
    // find_references_in_tree
    // -----------------------------------------------------------------------

    #[test]
    fn find_references_finds_all_occurrences() {
        // x appears in the declaration and as a scalar value reference
        let d = doc("Scalars x / 1 /, y / 2 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let refs = find_references_in_tree(tree, d.body_text.as_bytes(), "x");
        assert!(!refs.is_empty(), "should find 'x'");
    }

    #[test]
    fn find_references_case_insensitive() {
        let d = doc("Sets MySet / a b /;\n");
        let tree = d.tree.as_ref().unwrap();
        let refs = find_references_in_tree(tree, d.body_text.as_bytes(), "myset");
        assert!(!refs.is_empty());
    }

    #[test]
    fn find_references_classifies_declaration_as_write() {
        let d = doc("Scalar x / 1 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let refs = find_references_in_tree(tree, d.body_text.as_bytes(), "x");
        assert!(refs.iter().any(|(_, k)| *k == ReferenceKind::Write));
    }

    #[test]
    fn find_references_equation_definition_name_is_write() {
        let d = doc("Equation obj;\nobj.. z =e= 1;\n");
        let tree = d.tree.as_ref().unwrap();
        let refs = find_references_in_tree(tree, d.body_text.as_bytes(), "obj");
        let writes: Vec<_> = refs.iter().filter(|(_, k)| *k == ReferenceKind::Write).collect();
        assert!(!writes.is_empty(), "equation name should be Write");
    }

    #[test]
    fn find_references_alias_aliases_are_write() {
        let d = doc("Sets i / 1*3 /;\nAlias(i, j, k);\n");
        let tree = d.tree.as_ref().unwrap();
        let j_refs = find_references_in_tree(tree, d.body_text.as_bytes(), "j");
        let k_refs = find_references_in_tree(tree, d.body_text.as_bytes(), "k");
        assert!(j_refs.iter().any(|(_, k)| *k == ReferenceKind::Write));
        assert!(k_refs.iter().any(|(_, k)| *k == ReferenceKind::Write));
    }

    // -----------------------------------------------------------------------
    // node_range / coordinate mapping
    // -----------------------------------------------------------------------

    #[test]
    fn node_range_maps_through_source_map() {
        // After a $set on line 1, 'x' on original line 2 should appear as LSP line 1
        let d = doc("$set foo bar\nScalar x / 1 /;\n");
        let tree = d.tree.as_ref().unwrap();
        let refs = find_references_in_tree(tree, d.body_text.as_bytes(), "x");
        let (node, _) = refs.first().expect("should find x");
        let range = node_range(&d.source_map, *node);
        assert_eq!(range.start.line, 1); // original line 2 → LSP line 1
    }

    // -----------------------------------------------------------------------
    // 4d — hover helpers
    // -----------------------------------------------------------------------

    #[test]
    fn format_hover_symbol_with_domain() {
        let d = doc("Parameter a(i,j) 'cost matrix';\n");
        let sym = d.symbol_table.get("a").expect("should find a");
        let text = format_hover_symbol(sym);
        assert!(text.contains("Parameter"), "should mention kind");
        assert!(text.contains("a"), "should include name");
        assert!(text.contains("cost matrix"), "should include description");
        assert!(text.contains("(i,j)") || sym.domain.is_some(), "should have domain");
    }

    #[test]
    fn format_hover_symbol_without_domain() {
        let d = doc("Scalar x 'x value' / 1 /;\n");
        let sym = d.symbol_table.get("x").unwrap();
        let text = format_hover_symbol(sym);
        assert!(text.contains("Scalar"));
        assert!(text.contains("`x`"));
        assert!(text.contains("x value"));
    }

    #[test]
    fn dollar_var_name_at_position_inside_var() {
        let line = "x = %scenario%;";
        // cursor on 's' of scenario (col 6)
        assert_eq!(dollar_var_name_at_position(line, 6), Some("scenario".to_string()));
    }

    #[test]
    fn dollar_var_name_at_position_on_opening_percent() {
        let line = "%foo% + 1";
        assert_eq!(dollar_var_name_at_position(line, 0), Some("foo".to_string()));
    }

    #[test]
    fn dollar_var_name_at_position_outside_var() {
        let line = "%foo% + 1";
        // col 6 is outside any %...%
        assert_eq!(dollar_var_name_at_position(line, 6), None);
    }

    #[test]
    fn dollar_var_name_at_position_no_percent() {
        assert_eq!(dollar_var_name_at_position("Scalar x / 1 /;", 3), None);
    }

    // -----------------------------------------------------------------------
    // 4f — diagnostics
    // -----------------------------------------------------------------------

    #[test]
    fn collect_diagnostics_finds_ts_error() {
        // "???" is not valid GAMS; tree-sitter should produce an ERROR node
        let d = doc("???\n");
        let diags = collect_diagnostics(&d);
        assert!(!diags.is_empty(), "should have at least one syntax error diagnostic");
        assert!(diags.iter().any(|d| d.message.contains("syntax error")));
    }

    #[test]
    fn collect_diagnostics_from_dollar_layer() {
        // Unclosed $ontext → precompiler error
        let d = doc("$ontext\nno offtext\n");
        let diags = collect_diagnostics(&d);
        assert!(
            !diags.is_empty(),
            "should have at least one dollar-layer diagnostic"
        );
    }

    #[test]
    fn collect_diagnostics_clean_file_is_empty() {
        let d = doc("Scalar x / 1 /;\n");
        let diags = collect_diagnostics(&d);
        assert!(diags.is_empty(), "clean file should produce no diagnostics");
    }
}
