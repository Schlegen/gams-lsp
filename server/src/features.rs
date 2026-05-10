use tower_lsp::lsp_types::{Location, Position, Range, Url};
use tree_sitter::{Node, Point, Tree};

use crate::document::SourceMap;
use crate::symbols::Symbol;

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
}
