use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::types::{SourceLocation, Token, TokenKind};

// ---------------------------------------------------------------------------
// Compiled patterns (initialised once)
// ---------------------------------------------------------------------------

struct DirectivePattern {
    kind: TokenKind,
    re: Regex,
}

static DIRECTIVE_PATTERNS: LazyLock<Vec<DirectivePattern>> = LazyLock::new(|| {
    vec![
        // Variable assignment
        dp(TokenKind::SetGlobal,   r"(?i)^\$setglobal\s+(\S+)\s*(.*)"),
        dp(TokenKind::Set,         r"(?i)^\$set(?:local)?\s+(\S+)\s*(.*)"),
        dp(TokenKind::SetEnv,      r"(?i)^\$setenv\s+(\S+)\s*(.*)"),
        // Compile-time arithmetic — $evalGlobal before $eval (prefix match)
        // args: [variant_or_empty, varname, expression]  (see match_directive)
        dp(TokenKind::EvalGlobal,  r"(?i)^\$evalGlobal(?:\.(\w+))?\s+(\S+)\s*(.*)"),
        dp(TokenKind::Eval,        r"(?i)^\$eval(?:\.(\w+))?\s+(\S+)\s*(.*)"),
        // File inclusion
        dp(TokenKind::BatInclude,  r"(?i)^\$batinclude\s+(\S+)(.*)"),
        dp(TokenKind::Include,     r"(?i)^\$include\s+(.*)"),
        // $macro is handled separately in the main loop
        //
        // Multi-line conditionals — args: [tag_or_empty, condition]
        // Longer keywords (E/I suffix) must come before the plain form.
        dp(TokenKind::IfThenE,     r"(?i)^\$ifthenE(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::IfThenI,     r"(?i)^\$ifthenI(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::IfThen,      r"(?i)^\$ifthen(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::ElseIfE,     r"(?i)^\$elseifE(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::ElseIfI,     r"(?i)^\$elseifI(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::ElseIf,      r"(?i)^\$elseif(?:\.(\w+))?\s+(.*)"),
        dp(TokenKind::Else,        r"(?i)^\$else(?:\.(\w+))?"),
        dp(TokenKind::EndIf,       r"(?i)^\$endif(?:\.(\w+))?"),
        // Single-line conditionals — $ifE/$ifI before $if (prefix match)
        dp(TokenKind::IfE,         r"(?i)^\$ifE\s+(.*)"),
        dp(TokenKind::IfI,         r"(?i)^\$ifI\s+(.*)"),
        dp(TokenKind::If,          r"(?i)^\$if\s+(.*)"),
        // Flow control
        dp(TokenKind::Call,        r"(?i)^\$call\s+(.*)"),
        dp(TokenKind::Drop,        r"(?i)^\$drop(?:Env|Global|Local)?\s+(.*)"),
        dp(TokenKind::Label,       r"(?i)^\$label\s+(\S+)(.*)"),
        dp(TokenKind::Goto,        r"(?i)^\$goto\s+(\S+)(.*)"),
        dp(TokenKind::Exit,        r"(?i)^\$exit\b(.*)"),
        dp(TokenKind::Abort,       r"(?i)^\$abort\b(.*)"),
        // Text blocks
        dp(TokenKind::OnText,      r"(?i)^\$ontext\b(.*)"),
        dp(TokenKind::OffText,     r"(?i)^\$offtext\b(.*)"),
        dp(TokenKind::OnEmpty,     r"(?i)^\$onempty\b(.*)"),
        dp(TokenKind::OffEmpty,    r"(?i)^\$offempty\b(.*)"),
        // Output
        dp(TokenKind::Echo,        r"(?i)^\$echon?\s+(.*)"),
        dp(TokenKind::Log,         r"(?i)^\$log\s+(.*)"),
        // Comment markers
        dp(TokenKind::EolCom,      r"(?i)^\$eolcom\s+(.*)"),
        dp(TokenKind::InlineCom,   r"(?i)^\$inlinecom\s+(.*)"),
        // Catch-all
        dp(TokenKind::DollarOther, r"(?i)^\$(\S+)(.*)"),
    ]
});

static IS_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\$").unwrap());
static IS_COMMENT:   LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\*").unwrap());
static STRIP_LEAD:   LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*").unwrap());
static RE_MACRO_HDR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\$macro\b").unwrap());
static RE_ONTEXT:    LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\$ontext\b").unwrap());
static RE_OFFTEXT:   LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\$offtext\b").unwrap());
static RE_ENDMACRO:  LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*\$endmacro\b").unwrap());
static RE_MACRO_PARSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\$macro\s+(\w+)(?:\(([^)]*)\))?\s*(.*)").unwrap());

fn dp(kind: TokenKind, pattern: &str) -> DirectivePattern {
    DirectivePattern { kind, re: Regex::new(pattern).unwrap() }
}

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct LexerError {
    pub message: String,
    pub loc: SourceLocation,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.loc, self.message)
    }
}

impl std::error::Error for LexerError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Lex a file from disk and return a flat list of tokens.
pub fn tokenize_file(path: &Path) -> Result<Vec<Token>, LexerError> {
    let text = std::fs::read_to_string(path).map_err(|e| LexerError {
        message: e.to_string(),
        loc: SourceLocation::new(path.to_path_buf(), 1, 1),
    })?;
    tokenize_str(path.to_path_buf(), &text)
}

/// Lex an in-memory string. `file` is used only for `SourceLocation` reporting.
pub fn tokenize_str(file: PathBuf, text: &str) -> Result<Vec<Token>, LexerError> {
    lex(file, text)
}

// ---------------------------------------------------------------------------
// Core lexer
// ---------------------------------------------------------------------------

fn lex(file: PathBuf, text: &str) -> Result<Vec<Token>, LexerError> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut tokens = Vec::new();
    let mut in_block_comment = false;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let line_num = (i + 1) as u32;
        let loc = SourceLocation::new(file.clone(), line_num, 1);

        // ------------------------------------------------------------------ //
        // Inside $ontext block: only look for $offtext                        //
        // ------------------------------------------------------------------ //
        if in_block_comment {
            let stripped = STRIP_LEAD.replace(line, "");
            if RE_OFFTEXT.is_match(&stripped) {
                in_block_comment = false;
                tokens.push(Token::new(TokenKind::OffText, strip_newline(line), loc));
            }
            // Inside block comment — emit nothing (it's a comment)
            i += 1;
            continue;
        }

        // ------------------------------------------------------------------ //
        // Native GAMS comment line (starts with *)                            //
        // ------------------------------------------------------------------ //
        if IS_COMMENT.is_match(line) {
            tokens.push(Token::new(TokenKind::CommentLine, strip_newline(line), loc));
            i += 1;
            continue;
        }

        // ------------------------------------------------------------------ //
        // Dollar directive line                                                //
        // ------------------------------------------------------------------ //
        if IS_DIRECTIVE.is_match(line) {
            let stripped = STRIP_LEAD.replace(line, "");
            let stripped = strip_newline(&stripped);

            // $$ is the GAMS way to write a directive in a non-column-1 position.
            // Strip one leading $ so that $$set, $$ifthen, … are treated as $set, $ifthen, …
            let effective: &str = stripped.strip_prefix('$').unwrap_or(stripped);
            let effective = if stripped.starts_with("$$") { effective } else { stripped };

            // Special case: $macro (may span multiple lines)
            if RE_MACRO_HDR.is_match(effective) {
                let (tok, consumed) = lex_macro(effective, &lines, i, &file, line_num);
                tokens.push(tok);
                i += consumed;
                continue;
            }

            // Special case: entering a block comment
            if RE_ONTEXT.is_match(effective) {
                in_block_comment = true;
                tokens.push(Token::new(TokenKind::OnText, effective, loc).with_args(vec![]));
                i += 1;
                continue;
            }

            tokens.push(match_directive(effective, loc));
            i += 1;
            continue;
        }

        // ------------------------------------------------------------------ //
        // Plain GAMS body text                                                 //
        // ------------------------------------------------------------------ //
        tokens.push(Token::new(TokenKind::BodyText, line, loc));
        i += 1;
    }

    let eof_line = (lines.len() + 1) as u32;
    tokens.push(Token::new(
        TokenKind::Eof,
        "",
        SourceLocation::new(file.clone(), eof_line, 1),
    ));

    if in_block_comment {
        return Err(LexerError {
            message: "Unterminated $ontext block (no matching $offtext found)".into(),
            loc: SourceLocation::new(file, eof_line - 1, 1),
        });
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_newline(s: &str) -> &str {
    s.trim_end_matches('\n').trim_end_matches('\r')
}

fn is_ifthen_kind(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IfThen  | TokenKind::IfThenE  | TokenKind::IfThenI
        | TokenKind::ElseIf | TokenKind::ElseIfE | TokenKind::ElseIfI
    )
}

fn is_tag_only_kind(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Else | TokenKind::EndIf)
}

fn is_eval_kind(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Eval | TokenKind::EvalGlobal)
}

fn match_directive(stripped: &str, loc: SourceLocation) -> Token {
    for dp in DIRECTIVE_PATTERNS.iter() {
        if let Some(caps) = dp.re.captures(stripped) {
            // IfThen family: args = [tag, condition]  (tag is "" when absent)
            if is_ifthen_kind(&dp.kind) {
                let tag = caps.get(1).map_or("", |m| m.as_str().trim()).to_string();
                let condition = caps.get(2).map_or("", |m| m.as_str().trim()).to_string();
                return Token::new(dp.kind.clone(), stripped, loc)
                    .with_args(vec![tag, condition]);
            }

            // Else / EndIf: args = [tag]  (tag is "" when absent)
            if is_tag_only_kind(&dp.kind) {
                let tag = caps.get(1).map_or("", |m| m.as_str().trim()).to_string();
                return Token::new(dp.kind.clone(), stripped, loc).with_args(vec![tag]);
            }

            // Eval / EvalGlobal: args = [variant_or_empty, varname, expression]
            if is_eval_kind(&dp.kind) {
                let variant = caps.get(1).map_or("", |m| m.as_str().trim()).to_string();
                let varname = caps.get(2).map_or("", |m| m.as_str().trim()).to_string();
                let expr    = caps.get(3).map_or("", |m| m.as_str().trim()).to_string();
                return Token::new(dp.kind.clone(), stripped, loc)
                    .with_args(vec![variant, varname, expr]);
            }

            // All other directives: collect non-empty captures as before
            let mut args: Vec<String> = caps
                .iter()
                .skip(1)
                .filter_map(|g| g.map(|m| m.as_str().trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();

            // Strip surrounding quotes from include paths
            if dp.kind == TokenKind::Include || dp.kind == TokenKind::BatInclude {
                if let Some(first) = args.first_mut() {
                    *first = first.trim_matches('"').trim_matches('\'').to_string();
                }
            }

            return Token::new(dp.kind.clone(), stripped, loc).with_args(args);
        }
    }
    Token::new(TokenKind::DollarOther, stripped, loc)
}

fn lex_macro<'a>(
    header: &str,
    all_lines: &[&'a str],
    start_index: usize,
    file: &Path,
    start_line_num: u32,
) -> (Token, usize) {
    let loc = SourceLocation::new(file.to_path_buf(), start_line_num, 1);

    let Some(caps) = RE_MACRO_PARSE.captures(header) else {
        return (Token::new(TokenKind::Macro, header, loc), 1);
    };

    let macro_name = caps.get(1).map_or("", |m| m.as_str()).to_string();
    let params_raw = caps.get(2).map_or("", |m| m.as_str());
    let params: Vec<String> = params_raw
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let inline_body = caps.get(3).map_or("", |m| m.as_str()).trim().to_string();

    if !inline_body.is_empty() {
        let args = vec![macro_name, params.join(","), inline_body];
        return (Token::new(TokenKind::Macro, header, loc).with_args(args), 1);
    }

    // Multi-line macro: collect until $endmacro
    let mut body_lines: Vec<&str> = Vec::new();
    let mut i = start_index + 1;
    while i < all_lines.len() {
        let body_line = strip_newline(all_lines[i]);
        if RE_ENDMACRO.is_match(body_line) {
            i += 1;
            break;
        }
        body_lines.push(body_line);
        i += 1;
    }

    let body = body_lines.join("\n");
    let lines_consumed = i - start_index;
    let args = vec![macro_name, params.join(","), body];
    (Token::new(TokenKind::Macro, header, loc).with_args(args), lines_consumed)
}
