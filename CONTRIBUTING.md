# Contributing

## Workspace layout

```
gams-lsp/
├── dollar/          crate: dollar-gams  — GAMS precompiler layer (no LSP dependency)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── types.rs      SourceLocation, Token, TokenKind, DirectiveNode, Diagnostic, …
│   │   ├── lexer.rs      line-by-line lexer; emits Token stream
│   │   └── evaluator.rs  interpolate(), evaluate_condition(), recursive-descent expression parser
│   └── tests/
│       ├── lexer.rs      integration tests for the lexer
│       ├── evaluator.rs  integration tests for interpolation and condition evaluation
│       └── fixtures/     real .gms files used by file-based tests
└── server/          crate: gams-lsp-server — tower-lsp backend
    └── src/
        ├── main.rs
        └── backend.rs    LanguageServer trait impl (handlers)
```

The `dollar` crate is a pure Rust library with no LSP, no async, no tokio — just `regex`.
It can be developed and tested in complete isolation from the server.

## Build

```bash
cargo build                    # build everything
cargo build -p dollar-gams     # dollar crate only
cargo build -p gams-lsp-server # server only (also compiles the C tree-sitter parser)
```

The server's `build.rs` compiles `tree-sitter-gams/src/parser.c` via the `cc` crate.
`tree-sitter-gams/` is a symlink to the grammar repository; make sure it exists before
building the server.

## Test

```bash
cargo test                     # run all tests in the workspace
cargo test -p dollar-gams      # dollar crate tests only
```

Tests for `dollar-gams` live in `dollar/tests/` as Cargo integration tests (not `#[cfg(test)]`
inside the source files). This means they test only the public API, and each file compiles
as its own binary — errors in one test file do not prevent the others from running.

### Adding a test

**Unit-style test** (exercises public functions with inline data): add a `#[test]` function
to `dollar/tests/lexer.rs` or `dollar/tests/evaluator.rs`.

**File-based test** (exercises `tokenize_file` against a realistic `.gms` file): drop a
fixture into `dollar/tests/fixtures/` and reference it via the helper:

```rust
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
```

Fixtures are plain GAMS source files. Name them after what they exercise
(`ifthen_block.gms`, `macro_multiline.gms`, etc.).

## Key design decisions

### Two-layer parsing

GAMS source goes through the dollar layer first (this crate), which strips `*` comment
lines, resolves `$set`/`$setglobal`, expands `$ifthen` branches, and collects `$include`
paths. The resulting body text is then fed to tree-sitter for full AST parsing.

`*` comments are handled here — not by tree-sitter — because tree-sitter cannot enforce
a start-of-line constraint without an external scanner.

### Optimistic policy for unknown `%var%`

When a `$ifthen` condition references a `%variable%` that has no known value,
`evaluate_condition` returns `None`. The caller treats all branches as potentially
active. This avoids needing per-repo variable configuration files that would go stale.

### Three-valued logic

Condition evaluation uses `Option<bool>`:
- `Some(true)` / `Some(false)` — statically resolved
- `None` — unknown; caller keeps all branches

`or_tri` and `and_tri` implement the correct short-circuit rules
(`true OR unknown = true`, `false AND unknown = false`, etc.).

### Expression tokenizer: WORD stops at operator characters

The regex for the expression tokenizer uses `[^\s=!<>()]+` for bare word values,
not `\S+`. This ensures that unspaced expressions like `val==base` are tokenised
as `Word Op Word` rather than a single `Word("val==base")`.
