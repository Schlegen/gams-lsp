use std::collections::HashMap;

use gams_precompiler::{DollarVariable, Scope, SourceLocation, Token, TokenKind};
use tree_sitter::{Node, Tree};

use crate::document::SourceMap;

// ---------------------------------------------------------------------------
// SymbolKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Set,
    Scalar,
    Parameter, // includes Table declarations
    Variable,  // includes VariableTable declarations
    Equation,
    Model,
    Alias,
    Acronym,
    DollarVar, // %var% set by $set / $setglobal / $setenv
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Set       => write!(f, "Set"),
            Self::Scalar    => write!(f, "Scalar"),
            Self::Parameter => write!(f, "Parameter"),
            Self::Variable  => write!(f, "Variable"),
            Self::Equation  => write!(f, "Equation"),
            Self::Model     => write!(f, "Model"),
            Self::Alias     => write!(f, "Alias"),
            Self::Acronym   => write!(f, "Acronym"),
            Self::DollarVar => write!(f, "DollarVar"),
        }
    }
}

// ---------------------------------------------------------------------------
// Symbol
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Symbol {
    /// Normalised (lowercased) name used for case-insensitive lookup.
    pub name: String,
    /// Original-case name as it appears in the source.
    pub display_name: String,
    pub kind: SymbolKind,
    /// Location of the identifier in the original (pre-filtered) file.
    pub loc: SourceLocation,
    /// Inline description string from the declaration (quotes stripped), if any.
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// SymbolTable
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SymbolTable {
    /// All GAMS declarations, in source order.
    pub symbols: Vec<Symbol>,
    /// Dollar-layer variables ($set / $setglobal / $setenv).
    pub dollar_vars: Vec<DollarVariable>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { symbols: Vec::new(), dollar_vars: Vec::new() }
    }

    /// All symbols whose normalised name matches `name` (case-insensitive).
    pub fn lookup<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a Symbol> {
        let lower = name.to_lowercase();
        self.symbols.iter().filter(move |s| s.name == lower)
    }

    /// First symbol matching `name` (declaration site preferred).
    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.lookup(name).next()
    }

    /// All symbols whose declaration is on `line` in the original file.
    pub fn at_line(&self, line: u32) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter().filter(move |s| s.loc.line == line)
    }

    /// Dollar variable by name (case-insensitive).
    pub fn dollar_var(&self, name: &str) -> Option<&DollarVariable> {
        let lower = name.to_lowercase();
        self.dollar_vars.iter().find(|v| v.name.to_lowercase() == lower)
    }

    /// Consume `other` and append its contents (used for $include merging).
    pub fn merge(&mut self, other: SymbolTable) {
        self.symbols.extend(other.symbols);
        self.dollar_vars.extend(other.dollar_vars);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Walk `tree` (over `body`) and collect all GAMS symbol declarations,
/// then collect dollar-layer variables from `tokens`.
pub fn collect_symbols(
    tree: &Tree,
    body: &[u8],
    source_map: &SourceMap,
    tokens: &[Token],
) -> SymbolTable {
    let mut table = SymbolTable::new();
    visit(tree.root_node(), body, source_map, &mut table);
    table.dollar_vars = collect_dollar_vars(tokens);
    table
}

// ---------------------------------------------------------------------------
// Tree walker
// ---------------------------------------------------------------------------

fn visit(node: Node, body: &[u8], sm: &SourceMap, table: &mut SymbolTable) {
    match node.kind() {
        "set_declaration"            => entries(node, body, sm, SymbolKind::Set,       table),
        "scalar_declaration"         => entries(node, body, sm, SymbolKind::Scalar,    table),
        "parameter_declaration"      => entries(node, body, sm, SymbolKind::Parameter, table),
        "variable_declaration"       => entries(node, body, sm, SymbolKind::Variable,  table),
        "equation_declaration"       => entries(node, body, sm, SymbolKind::Equation,  table),
        "model_declaration"          => entries(node, body, sm, SymbolKind::Model,     table),
        "acronym_declaration"        => entries(node, body, sm, SymbolKind::Acronym,   table),
        "table_declaration"          => single(node, body, sm, SymbolKind::Parameter,  table),
        "variable_table_declaration" => single(node, body, sm, SymbolKind::Variable,   table),
        "equation_definition"        => eq_def(node, body, sm, table),
        "alias_declaration"          => alias(node, body, sm, table),
        _ => {
            // Recurse into all children for any other node kind.
            for child in children(node) {
                visit(child, body, sm, table);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declaration collectors
// ---------------------------------------------------------------------------

/// Collect one symbol per `*_entry` child of a declaration node.
fn entries(decl: Node, body: &[u8], sm: &SourceMap, kind: SymbolKind, table: &mut SymbolTable) {
    for child in children(decl) {
        if child.kind().ends_with("_entry") {
            if let Some(sym) = entry_symbol(child, body, sm, kind.clone()) {
                table.symbols.push(sym);
            }
        }
    }
}

/// Single-name declarations (table, variable_table): identifier is a direct child.
fn single(decl: Node, body: &[u8], sm: &SourceMap, kind: SymbolKind, table: &mut SymbolTable) {
    if let Some((id_node, display_name)) = name_node(decl, body) {
        table.symbols.push(Symbol {
            name: display_name.to_lowercase(),
            display_name,
            kind,
            loc: map_loc(id_node.start_position(), sm),
            description: description(decl, body),
        });
    }
}

/// `equation_definition`: name comes from the `name` field.
fn eq_def(def_node: Node, body: &[u8], sm: &SourceMap, table: &mut SymbolTable) {
    let Some(name_nd) = def_node.child_by_field_name("name") else { return };
    let (id_node, display_name) = if name_nd.kind() == "identifier_with_domain" {
        // First identifier child of identifier_with_domain is the bare name.
        children(name_nd)
            .find(|n| n.kind() == "identifier")
            .and_then(|n| n.utf8_text(body).ok().map(|t| (n, t.to_string())))
            .unwrap_or_else(|| {
                let t = name_nd.utf8_text(body).unwrap_or("").to_string();
                (name_nd, t)
            })
    } else {
        let t = name_nd.utf8_text(body).unwrap_or("").to_string();
        (name_nd, t)
    };

    table.symbols.push(Symbol {
        name: display_name.to_lowercase(),
        display_name,
        kind: SymbolKind::Equation,
        loc: map_loc(id_node.start_position(), sm),
        description: None,
    });
}

/// `alias_declaration`: first identifier is the original set; subsequent ones are aliases.
/// e.g. `alias(i, j, k)` → j and k are aliases of i.
fn alias(decl: Node, body: &[u8], sm: &SourceMap, table: &mut SymbolTable) {
    let ids: Vec<Node> = children(decl)
        .filter(|n| n.kind() == "identifier")
        .collect();

    if ids.len() < 2 {
        return;
    }
    let source_name = ids[0].utf8_text(body).unwrap_or("").to_string();
    let desc = format!("alias of {source_name}");

    for id_node in &ids[1..] {
        let display_name = id_node.utf8_text(body).unwrap_or("").to_string();
        table.symbols.push(Symbol {
            name: display_name.to_lowercase(),
            display_name,
            kind: SymbolKind::Alias,
            loc: map_loc(id_node.start_position(), sm),
            description: Some(desc.clone()),
        });
    }
}

// ---------------------------------------------------------------------------
// Entry-node helpers
// ---------------------------------------------------------------------------

/// Build a `Symbol` from an `*_entry` node.
fn entry_symbol(entry: Node, body: &[u8], sm: &SourceMap, kind: SymbolKind) -> Option<Symbol> {
    let (id_node, display_name) = name_node(entry, body)?;
    Some(Symbol {
        name: display_name.to_lowercase(),
        display_name,
        kind,
        loc: map_loc(id_node.start_position(), sm),
        description: description(entry, body),
    })
}

/// Find the identifier that names this node.
/// Prefers `identifier_with_domain` → first `identifier` child;
/// falls back to plain `identifier`.
fn name_node<'a>(node: Node<'a>, body: &[u8]) -> Option<(Node<'a>, String)> {
    // Try identifier_with_domain first.
    for child in children(node) {
        if child.kind() == "identifier_with_domain" {
            for gc in children(child) {
                if gc.kind() == "identifier" {
                    let t = gc.utf8_text(body).ok()?.to_string();
                    return Some((gc, t));
                }
            }
        }
    }
    // Fall back to plain identifier.
    for child in children(node) {
        if child.kind() == "identifier" {
            let t = child.utf8_text(body).ok()?.to_string();
            return Some((child, t));
        }
    }
    None
}

/// Extract the first `string` child of `node` and strip its surrounding quotes.
fn description(node: Node, body: &[u8]) -> Option<String> {
    children(node)
        .find(|n| n.kind() == "string")
        .and_then(|n| n.utf8_text(body).ok())
        .map(|raw| raw.trim_matches('"').trim_matches('\'').to_string())
}

// ---------------------------------------------------------------------------
// Dollar-variable collection
// ---------------------------------------------------------------------------

fn collect_dollar_vars(tokens: &[Token]) -> Vec<DollarVariable> {
    let mut map: HashMap<String, DollarVariable> = HashMap::new();

    for token in tokens {
        let (name, value, scope) = match &token.kind {
            TokenKind::Set      if token.args.len() >= 2 =>
                (token.args[0].clone(), token.args[1].clone(), Scope::Local),
            TokenKind::SetGlobal if token.args.len() >= 2 =>
                (token.args[0].clone(), token.args[1].clone(), Scope::Global),
            TokenKind::SetEnv   if token.args.len() >= 2 =>
                (token.args[0].clone(), token.args[1].clone(), Scope::Env),
            _ => continue,
        };

        let entry = map
            .entry(name.to_lowercase())
            .or_insert_with(|| DollarVariable::new(name.clone(), scope));
        entry.definitions.push((value, token.loc.clone()));
    }

    map.into_values().collect()
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Collect a node's named children into a `Vec` to allow nested iteration
/// (tree-sitter cursors cannot be shared across loops).
fn children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let count = node.child_count();
    (0..count).filter_map(move |i| node.child(i))
}

fn map_loc(point: tree_sitter::Point, sm: &SourceMap) -> SourceLocation {
    let orig_line = sm.orig_line(point.row).unwrap_or(point.row as u32 + 1);
    SourceLocation::new(sm.orig_file.clone(), orig_line, point.column as u32 + 1)
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

    fn parse_and_collect(gams: &str) -> SymbolTable {
        let file = PathBuf::from("test.gms");
        let mut parser = make_parser();
        let doc = GamsDocument::parse(file, gams, &mut parser);
        doc.symbol_table
    }

    // -----------------------------------------------------------------------
    // Set declarations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_set_single() {
        let table = parse_and_collect("Sets i / 1*3 /;\n");
        let sym = table.get("i").expect("symbol 'i' not found");
        assert_eq!(sym.kind, SymbolKind::Set);
        assert_eq!(sym.display_name, "i");
    }

    #[test]
    fn collect_set_multiple_entries() {
        let table = parse_and_collect("Sets i / 1*3 /, j / a b c /;\n");
        assert!(table.get("i").is_some());
        assert!(table.get("j").is_some());
    }

    #[test]
    fn collect_set_with_description() {
        let table = parse_and_collect("Sets i 'regions' / 1*3 /;\n");
        let sym = table.get("i").unwrap();
        assert_eq!(sym.description.as_deref(), Some("regions"));
    }

    #[test]
    fn collect_set_lookup_case_insensitive() {
        let table: SymbolTable = parse_and_collect("Sets MySet / a b /;\n");
        assert!(table.get("myset").is_some());
        assert!(table.get("MYSET").is_some());
        assert_eq!(table.get("myset").unwrap().display_name, "MySet");
    }

    #[test]
    fn collect_set_with_domain() {
        // set_entry with identifier_with_domain: the name is the bare identifier
        let table = parse_and_collect("Sets ij(i,j) 'cross product' / *a *b /;\n");
        let sym = table.get("ij").expect("symbol 'ij' not found");
        assert_eq!(sym.kind, SymbolKind::Set);
    }

    // -----------------------------------------------------------------------
    // Scalar declarations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_scalar() {
        let table = parse_and_collect("Scalar x 'x value' / 1 /;\n");
        let sym = table.get("x").expect("'x' not found");
        assert_eq!(sym.kind, SymbolKind::Scalar);
        assert_eq!(sym.description.as_deref(), Some("x value"));
    }

    #[test]
    fn collect_scalar_multiple() {
        let table = parse_and_collect("Scalars x / 1 /, y / 2 /, z / 3 /;\n");
        for name in ["x", "y", "z"] {
            assert!(table.get(name).is_some(), "missing scalar '{name}'");
        }
    }

    // -----------------------------------------------------------------------
    // Parameter declarations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_parameter() {
        let table = parse_and_collect("Parameter a(i) 'costs' / 1 2 /;\n");
        let sym = table.get("a").expect("'a' not found");
        assert_eq!(sym.kind, SymbolKind::Parameter);
        assert_eq!(sym.description.as_deref(), Some("costs"));
    }

    // -----------------------------------------------------------------------
    // Variable declarations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_variable() {
        let table = parse_and_collect("Positive Variable x(i) 'production';\n");
        let sym = table.get("x").expect("'x' not found");
        assert_eq!(sym.kind, SymbolKind::Variable);
    }

    // -----------------------------------------------------------------------
    // Equation declarations and definitions
    // -----------------------------------------------------------------------

    #[test]
    fn collect_equation_declaration() {
        let table = parse_and_collect("Equation obj 'objective';\n");
        let sym = table.get("obj").expect("'obj' not found");
        assert_eq!(sym.kind, SymbolKind::Equation);
        assert_eq!(sym.description.as_deref(), Some("objective"));
    }

    #[test]
    fn collect_equation_definition() {
        let gams = "Equation obj;\nobj.. z =e= sum(i, c(i)*x(i));\n";
        let table = parse_and_collect(gams);
        let matches: Vec<_> = table.lookup("obj").collect();
        // Both declaration and definition should appear
        assert!(matches.len() >= 1, "no 'obj' symbols found");
        assert!(matches.iter().all(|s| s.kind == SymbolKind::Equation));
    }

    // -----------------------------------------------------------------------
    // Model declarations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_model() {
        let table = parse_and_collect("Model transport 'a transportation model' / all /;\n");
        let sym = table.get("transport").expect("'transport' not found");
        assert_eq!(sym.kind, SymbolKind::Model);
        assert_eq!(sym.description.as_deref(), Some("a transportation model"));
    }

    // -----------------------------------------------------------------------
    // Alias
    // -----------------------------------------------------------------------

    #[test]
    fn collect_alias_simple() {
        let table = parse_and_collect("Sets i / 1*3 /;\nAlias(i, j);\n");
        let sym = table.get("j").expect("alias 'j' not found");
        assert_eq!(sym.kind, SymbolKind::Alias);
        assert!(sym.description.as_deref().unwrap_or("").contains('i'));
    }

    #[test]
    fn collect_alias_multiple() {
        let table = parse_and_collect("Sets i / 1*3 /;\nAlias(i, j, k);\n");
        assert!(table.get("j").is_some());
        assert!(table.get("k").is_some());
    }

    // -----------------------------------------------------------------------
    // Source map — original line numbers
    // -----------------------------------------------------------------------

    #[test]
    fn source_map_line_after_directive() {
        // Line 1: directive (stripped from body)
        // Line 2: declaration → original line 2
        let gams = "$set foo bar\nScalar x / 1 /;\n";
        let table = parse_and_collect(gams);
        let sym = table.get("x").expect("'x' not found");
        assert_eq!(sym.loc.line, 2, "should point to original line 2");
    }

    #[test]
    fn source_map_line_after_comment() {
        // Line 1: * comment (stripped)
        // Line 2: declaration → original line 2
        let gams = "* comment\nScalar y / 2 /;\n";
        let table = parse_and_collect(gams);
        let sym = table.get("y").expect("'y' not found");
        assert_eq!(sym.loc.line, 2);
    }

    // -----------------------------------------------------------------------
    // Dollar variables
    // -----------------------------------------------------------------------

    #[test]
    fn dollar_vars_collected_from_set() {
        let table = parse_and_collect("$set scenario base\n");
        let dv = table.dollar_var("scenario").expect("dollar var not found");
        assert_eq!(dv.current_value(), Some("base"));
    }

    #[test]
    fn dollar_vars_multiple_assignments() {
        let table = parse_and_collect("$set x 1\n$set x 2\n");
        let dv = table.dollar_var("x").unwrap();
        assert_eq!(dv.definitions.len(), 2);
        assert_eq!(dv.current_value(), Some("2"));
    }

    #[test]
    fn dollar_vars_setglobal_scope() {
        let table = parse_and_collect("$setglobal gvar hello\n");
        let dv = table.dollar_var("gvar").unwrap();
        assert_eq!(dv.scope, Scope::Global);
    }

    // -----------------------------------------------------------------------
    // SymbolTable merge
    // -----------------------------------------------------------------------

    #[test]
    fn merge_combines_symbols() {
        let mut a = parse_and_collect("Scalar x / 1 /;\n");
        let b     = parse_and_collect("Scalar y / 2 /;\n");
        a.merge(b);
        assert!(a.get("x").is_some());
        assert!(a.get("y").is_some());
    }

    #[test]
    fn merge_combines_dollar_vars() {
        let mut a = parse_and_collect("$set foo bar\n");
        let b     = parse_and_collect("$set baz qux\n");
        a.merge(b);
        assert!(a.dollar_var("foo").is_some());
        assert!(a.dollar_var("baz").is_some());
    }

    // -----------------------------------------------------------------------
    // From GAMS model library — Brick Design (gamslib #437)
    // -----------------------------------------------------------------------

    #[test]
    fn collect_scalars_brick_design() {
        // Three scalars with descriptions and initial values
        let gams = "Scalars\n    L 'min length' / 0.5 /\n    W 'min width'  / 0.5 /\n    H 'min height' / 0.5 /;\n";
        let table = parse_and_collect(gams);
        for (name, desc) in [("L", "min length"), ("W", "min width"), ("H", "min height")] {
            let sym = table.get(name).unwrap_or_else(|| panic!("scalar '{name}' not found"));
            assert_eq!(sym.kind, SymbolKind::Scalar);
            assert_eq!(sym.description.as_deref(), Some(desc));
        }
    }

    #[test]
    fn collect_variable_no_domain() {
        let gams = "Variable obj 'total cost';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("obj").expect("'obj' not found");
        assert_eq!(sym.kind, SymbolKind::Variable);
        assert_eq!(sym.description.as_deref(), Some("total cost"));
    }

    // -----------------------------------------------------------------------
    // From GAMS model library — Binary Knapsack (gamslib #436)
    // -----------------------------------------------------------------------

    #[test]
    fn collect_set_without_body() {
        // Set declared with a description but no element enumeration
        let gams = "Set i 'items';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("i").expect("'i' not found");
        assert_eq!(sym.kind, SymbolKind::Set);
        assert_eq!(sym.description.as_deref(), Some("items"));
    }

    #[test]
    fn collect_parameters_multiple_with_domain() {
        // Two parameters in one declaration block, each indexed over (i)
        let gams = "Parameters\n    p(i) 'profits'\n    w(i) 'weights';\n";
        let table = parse_and_collect(gams);
        let p = table.get("p").expect("'p' not found");
        assert_eq!(p.kind, SymbolKind::Parameter);
        assert_eq!(p.description.as_deref(), Some("profits"));
        let w = table.get("w").expect("'w' not found");
        assert_eq!(w.description.as_deref(), Some("weights"));
    }

    #[test]
    fn collect_binary_variable() {
        let gams = "Binary Variable x 'item selected';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("x").expect("'x' not found");
        assert_eq!(sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn collect_free_variable() {
        let gams = "Free Variable z 'objective';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("z").expect("'z' not found");
        assert_eq!(sym.kind, SymbolKind::Variable);
        assert_eq!(sym.description.as_deref(), Some("objective"));
    }

    #[test]
    fn collect_equations_multiple_with_description() {
        // Multiple equations in one Equations block, each with description
        let gams = "Equations\n    cap_restr 'capacity constraint'\n    utility   'objective value';\n";
        let table = parse_and_collect(gams);
        let cap = table.get("cap_restr").expect("'cap_restr' not found");
        assert_eq!(cap.kind, SymbolKind::Equation);
        assert_eq!(cap.description.as_deref(), Some("capacity constraint"));
        assert!(table.get("utility").is_some());
    }

    // -----------------------------------------------------------------------
    // From GAMS model library — Nurse Scheduling (gamslib #428)
    // -----------------------------------------------------------------------

    #[test]
    fn collect_set_multidimensional() {
        // Multi-dimensional set with description, domain declared inline
        let gams = "Sets nurse, shift, day;\nSet s(nurse,shift,day) 'shift assignment';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("s").expect("'s' not found");
        assert_eq!(sym.kind, SymbolKind::Set);
        assert_eq!(sym.description.as_deref(), Some("shift assignment"));
    }

    #[test]
    fn collect_alias_nurse_scheduling() {
        // Two alias declarations from the nurse scheduling model
        let gams = "Sets nurse, day;\nAlias(nurse, n);\nAlias(day, d);\n";
        let table = parse_and_collect(gams);
        let n = table.get("n").expect("alias 'n' not found");
        assert_eq!(n.kind, SymbolKind::Alias);
        assert!(n.description.as_deref().unwrap_or("").contains("nurse"));
        let d = table.get("d").expect("alias 'd' not found");
        assert_eq!(d.kind, SymbolKind::Alias);
        assert!(d.description.as_deref().unwrap_or("").contains("day"));
    }

    #[test]
    fn collect_variable_multidimensional() {
        // Variable indexed over four sets, mixed-case name
        let gams = "Variable nurseAssignments(nurse,shift,department,day) 'assign nurse to shift';\n";
        let table = parse_and_collect(gams);
        let sym = table.get("nurseAssignments").expect("'nurseAssignments' not found");
        assert_eq!(sym.kind, SymbolKind::Variable);
        assert_eq!(sym.display_name, "nurseAssignments");
        assert_eq!(sym.description.as_deref(), Some("assign nurse to shift"));
    }

    // -----------------------------------------------------------------------
    // at_line lookup
    // -----------------------------------------------------------------------

    #[test]
    fn at_line_finds_symbol_on_correct_line() {
        let gams = "Scalar x / 1 /;\nScalar y / 2 /;\n";
        let table = parse_and_collect(gams);
        let line1: Vec<_> = table.at_line(1).collect();
        let line2: Vec<_> = table.at_line(2).collect();
        assert!(line1.iter().any(|s| s.name == "x"));
        assert!(line2.iter().any(|s| s.name == "y"));
    }
}
