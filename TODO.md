# GAMS LSP — Implementation Plan

## Architecture

Two parsing layers feed the LSP feature engine:

```
.gms source
    │
    ▼
┌─────────────────────────────────────────┐
│  Dollar-layer preprocessor              │
│  • Lexes $set / $include / $ifthen /... │
│  • Strips * comment lines (COMMENT_LINE)│
│  • Optimistic policy for unknown %vars% │
│  • Produces BodySegments with source map│
└─────────────┬───────────────────────────┘
              │  GAMS body text (no $ directives, no * comments)
              ▼
┌─────────────────────────────────────────┐
│  Tree-sitter GAMS parser                │
│  (C parser compiled via build.rs)       │
│  Produces AST per file / per branch     │
└─────────────┬───────────────────────────┘
              │
              ▼
┌─────────────────────────────────────────┐
│  Symbol table + LSP features            │
└─────────────────────────────────────────┘
```

**Key design decisions**
- Language: Rust
- LSP framework: `tower-lsp`
- Parser: `tree-sitter` crate binding the existing C grammar
- Client: VS Code extension (TypeScript, `vscode-languageclient`)
- Transport: stdio
- `*` line comments: handled by the dollar layer (emitted as `COMMENT_LINE` tokens,
  stripped from body text before tree-sitter). This avoids teaching tree-sitter
  a start-of-line constraint it cannot express without an external scanner.
- `$ifthen` with unknown `%var%`: optimistic policy — all branches treated as
  potentially active (no per-repo variable config; values change too often).
- `$include` following: yes, index all files reachable from the open document.

---

## Phase 0 — Project scaffold

**Goal:** server starts and responds to `initialize`/`shutdown`; VS Code client
activates on `*.gms` files.

```
gams-lsp/
├── Cargo.toml              ← workspace: members = ["server"]
├── server/
│   ├── Cargo.toml          ← tower-lsp, tokio, tree-sitter, ropey, dashmap
│   ├── build.rs            ← cc to compile tree-sitter-gams/src/parser.c
│   └── src/
│       ├── main.rs
│       └── backend.rs      ← tower-lsp Backend impl (stub)
└── client/
    ├── package.json        ← activationEvents: onLanguage:gams
    └── src/
        └── extension.ts    ← LanguageClient startup
```

Tasks:
- [x] `Cargo.toml` workspace root
- [x] `server/Cargo.toml` with dependencies
- [x] `server/build.rs` that compiles `tree-sitter-gams/src/parser.c`
- [x] `server/src/main.rs` + `backend.rs` stub (initialize, shutdown, did_open no-op)
- [x] `client/package.json` + `extension.ts` (spawn server binary, start client)
- [x] Verify: open a `.gms` file in VS Code, server starts without crash

---

## Phase 1 — Dollar-layer port to Rust

**Goal:** parse the precompiler layer of any `.gms` file and produce
`BodySegment`s (GAMS text chunks) with accurate `SourceLocation`s.

Port the Python `dollar-lexer-gams` package to Rust, module by module:

```
server/src/dollar/
├── mod.rs
├── types.rs     ← SourceLocation, Token, TokenKind, all DirectiveNode variants,
│                   DollarVariable, Diagnostic, Severity, BodySegment
├── lexer.rs     ← line-by-line lexer; same regex patterns as Python;
│                   * comment lines → COMMENT_LINE token (excluded from body)
└── evaluator.rs ← interpolate(), condition parser, optimistic None → both branches
```

The Python dataclasses map 1-to-1 to Rust structs (the Python source says so
explicitly). Use `enum` for `TokenKind`/`DirectiveNode` variants.

Tasks:
- [x] `types.rs`: port all structs and enums from `dollar-lexer-gams/types.py`
- [x] `lexer.rs`: port `_DIRECTIVE_PATTERNS` + lexer loop; add `*` comment detection
- [x] `evaluator.rs`: port `interpolate()` and condition evaluator with optimistic policy
- [x] Unit tests mirroring the Python behaviour (18 tests, all passing)

---

## Phase 2 — Document store & incremental parsing

**Goal:** maintain a live parse tree for every open document; re-parse efficiently
on every keystroke.

```
server/src/
├── language.rs   ← gams_language() — binds the compiled C grammar via extern "C"
├── document.rs   ← GamsDocument { rope, dollar_tokens, body_text, source_map, tree }
└── store.rs      ← DocumentStore (DashMap<Url, GamsDocument>)
```

`GamsDocument`:
- `rope: Rope` — source text (O(log n) edits via `ropey`)
- `dollar_tokens: Vec<Token>` — flat token list from the dollar-layer lexer
- `body_text: String` — filtered GAMS body (no `$` directives, no `*` comment lines)
- `source_map: SourceMap` — maps 0-based body-text line → 1-based original file line
- `tree: Option<Tree>` — tree-sitter parse result for `body_text`
- `dollar_diagnostics: Vec<Diagnostic>` — errors from the dollar-layer lexer

`DocumentStore` wraps a `DashMap<Url, GamsDocument>` and exposes:
- `open(uri, text, parser)` — runs both parse layers, stores result, recursively
  loads `$include`d files with cycle detection
- `change(uri, changes, parser)` — applies LSP incremental edits to the rope,
  reruns the dollar layer and tree-sitter
- `close(uri)` — evicts the document from the map

LSP handlers wired in `backend.rs`:
- `textDocument/didOpen` → `store.open()`
- `textDocument/didChange` → `store.change()`
- `textDocument/didClose` → `store.close()`

Tasks:
- [x] `document.rs` + `store.rs` skeleton
- [x] Wire `didOpen` / `didChange` / `didClose` in `backend.rs`
- [x] Incremental reparse (full dollar-layer + tree-sitter reparse on each change)
- [x] `$include` resolution: load included files into the store recursively
      (detect cycles to avoid infinite loops)

### Testing Phase 2

All tests live inside the server crate as `#[cfg(test)]` modules and run with:

```bash
cargo test -p gams-lsp-server          # server tests only (22 tests)
cargo test                              # full workspace (115 + 22 tests)
```

#### `document::tests` (15 tests)

| Test | What it verifies |
|---|---|
| `build_body_strips_directives_and_comments` | `$set` and `* comment` lines are absent from `body_text` |
| `build_body_empty_file_gives_empty_body` | empty input → empty body and empty source map |
| `build_body_ontext_block_excluded` | lines inside `$ontext`/`$offtext` are stripped |
| `build_body_source_map_tracks_original_lines` | `SourceMap` records correct original line numbers |
| `parse_produces_tree_for_body_text` | tree-sitter returns a tree for a plain GAMS line |
| `parse_strips_directives_from_body` | `$set` absent from `body_text` after full parse |
| `parse_strips_star_comments` | `* comment` absent from `body_text` after full parse |
| `parse_unclosed_ontext_produces_diagnostic` | missing `$offtext` → non-empty `dollar_diagnostics` |
| `incremental_change_full_replace` | `range: None` replaces entire rope content |
| `incremental_change_insert_at_start` | zero-length range insert prepends text |
| `incremental_change_delete_range` | range with empty replacement text deletes characters |
| `incremental_change_replace_word` | range covering a word swaps it for another |
| `incremental_change_multiline` | edit on line 1 of a 3-line rope leaves other lines intact |
| `update_reruns_dollar_layer_and_tree_sitter` | after `update()`, `body_text` and `tree` reflect the new content |

#### `store::tests` (7 tests)

| Test | What it verifies |
|---|---|
| `open_stores_document` | document is retrievable after `open()` |
| `close_removes_document` | document is gone after `close()` |
| `change_updates_body_text` | full-replace change is reflected in `body_text` |
| `change_incremental_insert` | incremental insert appends a line correctly |
| `change_strips_new_directive` | after pasting a `$set` line, directive is absent from body |
| `open_loads_include_into_store` | `$include helper.gms` causes `helper.gms` to appear in the store |
| `open_cycle_detection_does_not_loop` | mutual `$include` (a↔b) terminates without hanging |
| `open_missing_include_does_not_panic` | include path that doesn't exist on disk is silently skipped |

---

## Phase 3 — Symbol table

**Goal:** for every open document (and its transitive includes), know where every
symbol is declared and what kind it is.

```
server/src/symbols.rs
```

### What is collected

Walk the tree-sitter AST and collect declarations:

| Grammar node | SymbolKind |
|---|---|
| `set_entry` inside `set_declaration` | `Set` |
| `scalar_entry` inside `scalar_declaration` | `Scalar` |
| `param_entry` inside `parameter_declaration` | `Parameter` |
| `table_declaration` (single name, direct child) | `Parameter` |
| `var_entry` inside `variable_declaration` | `Variable` |
| `variable_table_declaration` (single name) | `Variable` |
| `eq_entry` inside `equation_declaration` | `Equation` |
| `equation_definition` (`name` field) | `Equation` (definition site) |
| `model_entry` inside `model_declaration` | `Model` |
| `acronym_entry` inside `acronym_declaration` | `Acronym` |
| `alias_declaration` — identifiers[1..] are aliases of identifiers[0] | `Alias` |

Dollar-layer variables (`$set` / `$setglobal` / `$setenv`) are collected separately
as `DollarVariable`s (not `Symbol`s) since they live in a different namespace.

Per symbol: lowercased `name` (for lookup), `display_name` (original case),
`kind`, `loc` (mapped back through `SourceMap` to original file line), `description`
(inline `string` child of the entry node, quotes stripped).

### Key design decisions

**`identifier_with_domain` stripping**: entry nodes like `set_entry(i(j), ...)` have
an `identifier_with_domain` child whose first `identifier` child is the bare name.
The walker always extracts the bare name so `ij(i,j)` → name `ij`.

**Equation declaration vs. definition**: both are collected. The declaration
(`eq_entry`) carries the description string; the definition (`equation_definition`)
gives the source location of the equation body. `lookup("obj")` may return both.

**Alias direction**: `alias(i, j, k)` → `j` and `k` are the new names, with
`description = "alias of i"`. The source set `i` is not re-recorded.

### `SymbolTable` API

```rust
table.get("myvar")               // → Option<&Symbol>  (first match, case-insensitive)
table.lookup("myvar")            // → Iterator<Item=&Symbol>  (all matches)
table.at_line(42)                // → Iterator  (symbols declared on original line 42)
table.dollar_var("scenario")     // → Option<&DollarVariable>
table.merge(other)               // consume another table (for $include)
```

### Cross-file merge

`DocumentStore::merged_symbols(uri)` returns a `SymbolTable` that is the union of
the document's own table and all its transitive `$include`d files, with cycle
detection. This is computed on demand (not cached) so it is always fresh.

Tasks:
- [x] `SymbolKind` enum + `Symbol` struct + `SymbolTable`
- [x] `collect_symbols(tree, body, source_map, tokens) -> SymbolTable` tree-walker
- [x] `SymbolTable` lookup: by name (case-insensitive), by original line
- [x] `GamsDocument::symbol_table` populated on every parse/update
- [x] `DocumentStore::merged_symbols(uri)` — merge across `$include` with cycle detection

### Testing Phase 3

```bash
cargo test -p gams-lsp-server                       # 44 tests (includes 22 from Phase 2)
cargo test -p gams-lsp-server symbols               # symbol tests only
cargo test                                           # 159 total
```

#### `symbols::tests` (22 tests)

| Test | What it verifies |
|---|---|
| `collect_set_single` | `Sets i / 1*3 /` → `Symbol { kind: Set, name: "i" }` |
| `collect_set_multiple_entries` | multi-entry `Sets i ..., j ...` → both symbols |
| `collect_set_with_description` | inline string `'regions'` → `description` field |
| `collect_set_lookup_case_insensitive` | `MySet` found via `get("myset")` and `get("MYSET")` |
| `collect_set_with_domain` | `ij(i,j)` → bare name `ij` (domain args stripped) |
| `collect_scalar` | `Scalar x 'x value' / 1 /` → Scalar with description |
| `collect_scalar_multiple` | three scalars in one declaration |
| `collect_parameter` | `Parameter a(i) 'costs'` → Parameter |
| `collect_variable` | `Positive Variable x(i)` → Variable |
| `collect_equation_declaration` | `Equation obj 'objective'` → Equation with description |
| `collect_equation_definition` | `obj.. z =e= ...` → Equation at definition site |
| `collect_model` | `Model transport 'desc' / all /` → Model with description |
| `collect_alias_simple` | `alias(i, j)` → j is Alias with description "alias of i" |
| `collect_alias_multiple` | `alias(i, j, k)` → both j and k collected |
| `source_map_line_after_directive` | `$set` on line 1 shifts declaration to original line 2 |
| `source_map_line_after_comment` | `* comment` on line 1 shifts declaration to original line 2 |
| `dollar_vars_collected_from_set` | `$set scenario base` → `dollar_var("scenario").value == "base"` |
| `dollar_vars_multiple_assignments` | two `$set x` → two definitions, latest wins |
| `dollar_vars_setglobal_scope` | `$setglobal` → `Scope::Global` |
| `merge_combines_symbols` | merge two tables → all symbols visible |
| `merge_combines_dollar_vars` | merge propagates dollar vars |
| `at_line_finds_symbol_on_correct_line` | `at_line(1)` finds x, `at_line(2)` finds y |

---

## Phase 4 — LSP features

Implement in this priority order:

### 4a. Go to definition (`textDocument/definition`)
- Find the identifier node under the cursor
- Look up in the symbol table (current file + included files)
- Return declaration `SourceLocation` mapped back to the original file

**Key design decisions**
- `identifier_at_position(tree, body, source_map, pos)`: converts LSP Position
  (0-based, original file) → body-text `Point` via `orig_to_body_line` reverse
  lookup, then calls `descendant_for_point_range` and walks up to the nearest
  `identifier` ancestor.
- `sym_to_location(sym)`: converts `SourceLocation` (1-based line/col) to LSP
  `Location` (0-based).  End column is inferred from `display_name.len()`.
- Multiple declarations (e.g. equation declaration + definition) are returned as
  `GotoDefinitionResponse::Array`; the editor shows a picker.

Tasks:
- [x] `identifier_at_position()` + coordinate helpers in `features.rs`
- [x] `textDocument/definition` handler in `backend.rs`

### 4b. Document highlights (`textDocument/documentHighlight`)
- Find all occurrences of the symbol in the current file
- `DocumentHighlightKind::Write` for `*_entry` declaration nodes and the
  `name` field of `equation_definition` / alias targets
- `DocumentHighlightKind::Read` everywhere else

**Key design decisions**
- `find_references_in_tree(tree, body, name)`: full DFS over the tree-sitter
  AST; only `identifier` leaf nodes whose text matches `name` (case-insensitive)
  are collected.
- `classify_reference(id_node)`: walks up the parent chain:
  - any `*_entry` → Write
  - `equation_definition` / `name` field (first identifier) → Write
  - `alias_declaration` / identifiers[1..] → Write
  - everything else → Read
- `node_range(source_map, node)`: maps tree-sitter start/end points through
  `body_point_to_lsp` to produce LSP coordinates in the original file.

Tasks:
- [x] `find_references_in_tree()` + `ReferenceKind` + `classify_reference()` in `features.rs`
- [x] `textDocument/documentHighlight` handler in `backend.rs`

### 4c. References (`textDocument/references`)
- All usages across all open/included files
- Reuses `find_references_in_tree()` per file in the transitive include graph

**Key design decisions**
- `DocumentStore::transitive_uris(uri)` collects all URIs reachable via
  `$include` from the given file (cycle-safe, same pattern as `merged_symbols`).
- The references handler iterates over `transitive_uris`, runs
  `find_references_in_tree` on each document's tree, and maps results to
  `Location` values.

Tasks:
- [x] `DocumentStore::transitive_uris(uri)` in `store.rs`
- [x] `textDocument/references` handler in `backend.rs`

### Testing Phase 4a–4c

All tests live in `server/src/features.rs` as `#[cfg(test)]` and run with:

```bash
cargo test -p gams-lsp-server features   # features tests only (9 tests)
cargo test -p gams-lsp-server            # all server tests (53 tests)
cargo test                               # full workspace (168 tests)
```

#### `features::tests` (9 tests)

| Test | What it verifies |
|---|---|
| `identifier_at_position_finds_name` | cursor on `x` at col 7 → `"x"` |
| `identifier_at_position_returns_none_on_non_identifier` | cursor on whitespace → `None` or non-`x` |
| `identifier_at_position_with_source_map_offset` | after `$set`, original line 2 found via body-map reverse lookup |
| `find_references_finds_all_occurrences` | all identifier nodes matching name are returned |
| `find_references_case_insensitive` | `"myset"` finds `MySet` |
| `find_references_classifies_declaration_as_write` | `scalar_entry` → `Write` |
| `find_references_equation_definition_name_is_write` | `obj..` definition name → `Write` |
| `find_references_alias_aliases_are_write` | `alias(i, j, k)`: `j` and `k` → `Write` |
| `node_range_maps_through_source_map` | body row 0 after directive maps to LSP line 1 |

### 4d. Hover (`textDocument/hover`)
- Symbol kind, domain (e.g. `(i,j)`), and the declaration comment/description
- For `%var%` references: show current value from the dollar layer

Tasks:
- [ ] `format_hover(symbol) -> String`
- [ ] `textDocument/hover` handler

### 4e. Completion (`textDocument/completion`)
- In GAMS body: offer symbol names in scope
- Inside `%...%`: offer dollar variable names
- In `$include` path argument: file-system path completion

Tasks:
- [ ] `textDocument/completion` handler

### 4f. Diagnostics (`textDocument/publishDiagnostics`)
- Tree-sitter parse errors (ERROR nodes in the tree)
- Dollar layer: undefined `%var%` references, mismatched `$ifthen`/`$endif`
- GAMS layer: references to undefined symbols

Tasks:
- [ ] `collect_diagnostics(document) -> Vec<Diagnostic>`
- [ ] Push diagnostics on every reparse

---

## Phase 5 — VS Code extension client

**Goal:** polished editor experience for `.gms` files.

Tasks:
- [x] `client/package.json`: `activationEvents`, `contributes.languages` for `gams`,
      file associations `*.gms`, `*.gms2`
- [x] `client/syntaxes/gams.tmLanguage.json`: TextMate grammar for basic syntax
      highlighting (keywords, strings, `$` directives, `%var%` references,
      `*` comment lines)
- [x] `client/language-configuration.json`: comment characters, bracket pairs,
      auto-close pairs
- [x] `client/src/extension.ts`: start server process, `LanguageClient` wiring
- [x] Package and test the `.vsix` locally (`npm run package-vsix` in `client/`)

---

## Phase 6 — Grammar completions (ongoing, parallel)

Tree-sitter grammar gaps to fill incrementally (does not block earlier phases
once `*` comments are handled at the dollar layer):

- [ ] `execute` statement (common in real GAMS files)
- [ ] `put` / `putclose` statements (report writing)
- [ ] Inline `$` directives as transparent/skipped nodes (so unknown directives
      don't corrupt the parse tree)
- [ ] Expand test corpus with examples from the official GAMS model library
