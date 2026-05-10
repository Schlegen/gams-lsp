# GAMS LSP — How It Works

A walkthrough of the language server pipeline using a concrete example file.

---

## Sample file

```gams
* Transport model — mixed directives example
* Author: test fixture

$set scenario base
$setglobal model_version 2

$ifthen.set scenario
* Scenario is defined — load scenario-specific data
$set datafile data_%scenario%.gms
$else
$set datafile data_default.gms
$endif

$ontext
This block is a multi-line comment.
It can contain anything: $set, $include, etc.
All ignored by the preprocessor.
$offtext

Sets
    i  'supply nodes'  / s1, s2 /
    j  'demand nodes'  / d1, d2 /;

* End of model
```

This is a realistic GAMS file. It mixes two distinct languages:
- **Dollar directives** (lines starting with `$`, and `*` comment lines) — handled by the GAMS precompiler before the model engine ever sees the file.
- **GAMS model code** (`Sets`, `Parameters`, equations, ...) — parsed and executed by the GAMS engine.

The LSP must handle both layers simultaneously.

---

## Architecture overview

```
.gms source text
      │
      ▼
┌──────────────────────────────────────────┐
│  Layer 1 — Dollar-layer preprocessor     │
│  Lexes $set / $ifthen / $ontext / *...   │
│  Produces BodySegments + SourceMap       │
└──────────────────┬───────────────────────┘
                   │  filtered GAMS body (no directives, no * lines)
                   ▼
┌──────────────────────────────────────────┐
│  Layer 2 — Tree-sitter GAMS parser       │
│  Produces a concrete syntax tree (CST)   │
└──────────────────┬───────────────────────┘
                   │
                   ▼
┌──────────────────────────────────────────┐
│  Symbol table + LSP feature handlers     │
│  go-to-definition, highlights, refs, ... │
└──────────────────────────────────────────┘
```

Both layers run on every keystroke. The source map bridges them: it records which line of the filtered body corresponds to which line of the original file.

---

## Layer 1 — Dollar-layer preprocessor

### What it does

The dollar-layer lexer (`gams_precompiler` crate, `lexer.rs`) scans the file line by line and classifies each line into tokens:

| Line | Token kind |
|---|---|
| `* Transport model ...` | `COMMENT_LINE` |
| `$set scenario base` | `SetDirective { name: "scenario", value: "base" }` |
| `$setglobal model_version 2` | `SetGlobal { name: "model_version", value: "2" }` |
| `$ifthen.set scenario` | `IfThen { tag: Some("set"), condition: ... }` |
| `$ontext` ... `$offtext` | `OnText` ... `OffText` (everything inside is `TextBlock`) |
| `Sets i ...` | `BodyLine` |

### Filtering to get `body_text`

`COMMENT_LINE`, `SetDirective`, `IfThen`, `OnText`/`OffText`, and all text inside `$ontext` blocks are **excluded** from the body. Only `BodyLine` tokens are stitched together:

```
body_text (what tree-sitter sees):
─────────────────────────────────
Sets
    i  'supply nodes'  / s1, s2 /
    j  'demand nodes'  / d1, d2 /;
```

Three lines — everything else was stripped.

### The source map

The source map records, for each body-text row (0-based), the original file line (1-based):

```
body row 0  →  original line 20   ("Sets")
body row 1  →  original line 21   ("    i  'supply nodes' ...")
body row 2  →  original line 22   ("    j  'demand nodes' ...")
```

This mapping is essential: every LSP position (line/column in the `.gms` file the user has open) must be translated to body coordinates before tree-sitter can answer questions about it, and tree-sitter node positions must be translated back to original coordinates before they are returned to the editor.

### The `$ifthen` / optimistic policy

When a condition references an unknown `%var%` (one that was never `$set`), the preprocessor cannot know which branch is active at runtime. It uses an **optimistic policy**: both branches are treated as potentially active. All symbols declared in either branch are linted; false positives are possible but are better than silent misses.

In the sample file, `scenario` was set to `"base"` so the condition resolves: the `$ifthen` branch wins, `$set datafile data_base.gms` is recorded, and the `$else` branch is skipped.

### Dollar variables

`$set` and `$setglobal` directives populate a separate namespace of **dollar variables** (`DollarVariable`). These are distinct from GAMS model symbols. In the sample:

```
DollarVariable { name: "scenario",      value: "base",          scope: Local  }
DollarVariable { name: "model_version", value: "2",             scope: Global }
DollarVariable { name: "datafile",      value: "data_base.gms", scope: Local  }
```

`%scenario%` references in the file body are interpolated using these values.

---

## Layer 2 — Tree-sitter parser

Tree-sitter receives the filtered `body_text` and produces a concrete syntax tree using the `tree-sitter-gams` C grammar (compiled via `build.rs`).

For the sample, the tree looks like:

```
source_file
└── set_declaration
    ├── set_entry
    │   ├── identifier_with_domain
    │   │   └── identifier  "i"
    │   ├── string  "'supply nodes'"
    │   └── set_body  "/ s1, s2 /"
    └── set_entry
        ├── identifier_with_domain
        │   └── identifier  "j"
        ├── string  "'demand nodes'"
        └── set_body  "/ d1, d2 /"
```

The node positions (row/column) are in body-text coordinates and must be mapped back through the source map to yield original-file positions.

---

## Symbol table (Phase 3)

The symbol collector (`symbols.rs`) walks the tree-sitter AST and extracts all declarations:

| Grammar node | Symbol |
|---|---|
| `set_entry` inside `set_declaration` | `SymbolKind::Set` |
| `scalar_entry` inside `scalar_declaration` | `SymbolKind::Scalar` |
| `param_entry` inside `parameter_declaration` | `SymbolKind::Parameter` |
| `var_entry` inside `variable_declaration` | `SymbolKind::Variable` |
| `eq_entry` inside `equation_declaration` | `SymbolKind::Equation` |
| `equation_definition` (`name` field) | `SymbolKind::Equation` (definition site) |
| `model_entry` inside `model_declaration` | `SymbolKind::Model` |
| `alias_declaration` — identifiers[1..] | `SymbolKind::Alias` |

For the sample file the collected symbols are:

```
Symbol { name: "i", kind: Set, loc: line 21, description: "supply nodes" }
Symbol { name: "j", kind: Set, loc: line 22, description: "demand nodes" }
```

Lookup is always **case-insensitive**: `get("I")` and `get("i")` both return the same symbol.

### Cross-file merge via `$include`

If the file contained `$include data_base.gms`, the document store would load that file recursively (with cycle detection) and merge its symbol table into the current one. `DocumentStore::merged_symbols(uri)` returns the union of all reachable files on demand.

---

## LSP features (Phase 4)

All features go through the same pipeline:

1. Receive an LSP request with an original-file position.
2. Translate to body-text coordinates via `lsp_to_body_point(source_map, pos)`.
3. Query tree-sitter or the symbol table.
4. Translate results back to original-file coordinates via `body_point_to_lsp`.
5. Return an LSP response.

### Go to definition (`textDocument/definition`)

User clicks on `i` on line 21. The server:

1. Converts LSP position `{line: 20, char: 4}` → body point `{row: 1, col: 4}`.
2. Calls `descendant_for_point_range` on the tree → `identifier "i"`.
3. Looks up `"i"` in the symbol table → `Symbol { loc: line 21 }`.
4. Converts `loc` → LSP `Location { line: 20, char: 4 }`.
5. Returns the location — the editor jumps there.

If a symbol has both a declaration and a definition (e.g. an equation declared in `Equation obj 'objective'` and then defined in `obj.. z =e= ...`), both locations are returned as an array and the editor shows a picker.

### Document highlights (`textDocument/documentHighlight`)

User clicks on `i`. The server:

1. Finds the identifier name (`"i"`) under the cursor.
2. Does a full DFS over the tree-sitter AST.
3. Collects every `identifier` node whose text matches `"i"` (case-insensitive).
4. Classifies each occurrence:
   - Inside a `*_entry` node → `Write` (declaration)
   - Inside `equation_definition` name field → `Write`
   - Inside `alias_declaration` for alias targets → `Write`
   - Everywhere else → `Read`
5. Returns `DocumentHighlight` ranges — the editor paints all occurrences.

### References (`textDocument/references`)

Same as highlights, but across **all files** reachable via `$include`. `DocumentStore::transitive_uris(uri)` collects the full include graph (cycle-safe), and `find_references_in_tree` is run on each document.

---

## What is not yet implemented

| Feature | Status |
|---|---|
| Hover (`textDocument/hover`) | Planned — symbol kind + description |
| Completion (`textDocument/completion`) | Planned — symbols, `%var%` names, `$include` paths |
| Diagnostics (`textDocument/publishDiagnostics`) | Planned — tree-sitter errors, undefined `%var%`, undefined symbols |
| VS Code extension packaging | Planned |
| `execute`, `put`, inline `$` nodes | Tree-sitter grammar gaps to fill |

---

## Running the tests

```bash
# Full workspace (dollar-layer + server)
cargo test

# Server only (symbol table, document store, LSP features)
cargo test -p gams-lsp-server

# Focused suites
cargo test -p gams-lsp-server symbols
cargo test -p gams-lsp-server features
cargo test -p gams-lsp-server document
cargo test -p gams-lsp-server store
```

Current test count: 168 across the workspace (115 precompiler + 53 server).
