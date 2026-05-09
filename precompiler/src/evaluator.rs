use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::types::{Diagnostic, Severity, SourceLocation};

static VAR_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%([^%]+)%").unwrap());

// ---------------------------------------------------------------------------
// Variable interpolation
// ---------------------------------------------------------------------------

/// Replace `%varname%` references in `text` using `env`.
///
/// Returns `(expanded_text, fully_resolved)`.
/// `fully_resolved` is false if any `%var%` had no entry in env.
pub fn interpolate(text: &str, env: &HashMap<String, String>) -> (String, bool) {
    let mut fully_resolved = true;
    let result = VAR_REF.replace_all(text, |caps: &regex::Captures| {
        let name = caps[1].to_lowercase();
        match env.get(&name) {
            Some(val) => val.clone(),
            None => {
                fully_resolved = false;
                caps[0].to_string()
            }
        }
    });
    (result.into_owned(), fully_resolved)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Evaluate a `$ifthen` condition against the current variable environment.
///
/// Returns `Some(true/false)` when fully resolved, `None` when unknown.
/// Unknown triggers the optimistic policy upstream: all branches are kept.
///
/// `tag` is the `.xxx` suffix from the token (e.g. `"set"`, `"not"`, `"exist"`,
/// or an arbitrary structural label like `"prod"`).
/// When `tag` is a recognised semantic keyword it controls evaluation mode;
/// otherwise evaluation mode is inferred from the condition string itself
/// (e.g. `"set myvar"`, `"not set myvar"`, `"exist file.gms"`).
pub fn evaluate_condition(
    raw_condition: &str,
    tag: &str,
    env: &HashMap<String, String>,
    loc: &SourceLocation,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    // Determine (mode, operand) — either from the explicit tag or from the
    // condition string prefix (supports `set`, `not set`, `exist`, `not exist`).
    let (mode, operand): (String, &str) = if !tag.is_empty() {
        (tag.to_lowercase(), raw_condition.trim())
    } else {
        let (kw, rest) = parse_condition_prefix(raw_condition);
        (kw.to_string(), rest)
    };

    match mode.as_str() {
        // File existence — cannot be determined statically
        "exist" | "not_exist" => None,
        // Error level — unknown at static-analysis time
        "errorlevel" => None,
        // Environment variables — not tracked in our env map → unknown
        "setenv" | "not_setenv" => None,
        // Check if a compile-time variable is defined
        // setglobal / setlocal are aliases; our env doesn't distinguish scope
        "set" | "setglobal" | "setlocal" => {
            let varname = operand.to_lowercase();
            let (expanded, _) = interpolate(&varname, env);
            Some(env.contains_key(&expanded))
        }
        // Check if a compile-time variable is NOT defined
        "not" | "not_setglobal" | "not_setlocal" => {
            let varname = operand.to_lowercase();
            let (expanded, _) = interpolate(&varname, env);
            Some(!env.contains_key(&expanded))
        }
        // sameas(a, b) — case-insensitive string equality after interpolation
        "sameas" => eval_sameas(operand, false, env),
        "not_sameas" => eval_sameas(operand, true, env),
        // Plain expression
        _ => {
            let (expanded, fully_resolved) = interpolate(operand, env);
            if !fully_resolved {
                return None;
            }
            eval_expr(&expanded, loc, diagnostics)
        }
    }
}

fn eval_sameas(operands: &str, negate: bool, env: &HashMap<String, String>) -> Option<bool> {
    let (a_raw, b_raw) = operands.split_once(',')?;
    let (a, a_ok) = interpolate(a_raw.trim(), env);
    let (b, b_ok) = interpolate(b_raw.trim(), env);
    if !a_ok || !b_ok {
        return None;
    }
    let result = a.trim().to_lowercase() == b.trim().to_lowercase();
    Some(if negate { !result } else { result })
}

/// Extract a condition-type keyword prefix from a raw condition string.
///
/// Returns `(keyword, remainder)` where keyword is one of:
/// `"set"`, `"not"`, `"exist"`, `"not_exist"`, `"sameas"`, `"not_sameas"`,
/// `"errorlevel"`, `"setenv"`, `"not_setenv"`, `"setglobal"`, `"not_setglobal"`,
/// `"setlocal"`, `"not_setlocal"`, or `""` (plain expression).
pub fn parse_condition_prefix(condition: &str) -> (&'static str, &str) {
    let trimmed = condition.trim();
    let lower = trimmed.to_lowercase();

    // Longer "not X" forms first to avoid partial matches
    if lower.starts_with("not set ")      { return ("not",          trimmed[8..].trim()); }
    if lower.starts_with("not exist ")    { return ("not_exist",    trimmed[10..].trim()); }
    if lower.starts_with("not setenv ")   { return ("not_setenv",   trimmed[11..].trim()); }
    if lower.starts_with("not setglobal ") { return ("not_setglobal", trimmed[14..].trim()); }
    if lower.starts_with("not setlocal ") { return ("not_setlocal",  trimmed[13..].trim()); }
    if lower.starts_with("not sameas(") {
        // "not sameas(a, b)" → ("not_sameas", "a, b")
        let inner = trimmed[11..].trim_end_matches(')');
        return ("not_sameas", inner.trim());
    }

    if lower.starts_with("set ")       { return ("set",       trimmed[4..].trim()); }
    if lower.starts_with("exist ")     { return ("exist",     trimmed[6..].trim()); }
    if lower.starts_with("setenv ")    { return ("setenv",    trimmed[7..].trim()); }
    if lower.starts_with("setglobal ") { return ("setglobal", trimmed[10..].trim()); }
    if lower.starts_with("setlocal ")  { return ("setlocal",  trimmed[9..].trim()); }
    if lower.starts_with("errorlevel") { return ("errorlevel", trimmed[10..].trim()); }
    if lower.starts_with("sameas(") {
        // "sameas(a, b)" → ("sameas", "a, b")
        let inner = trimmed[7..].trim_end_matches(')');
        return ("sameas", inner.trim());
    }

    ("", trimmed)
}

// ---------------------------------------------------------------------------
// Expression evaluator — recursive descent over a tiny grammar
//
//   cond  ::= or_expr
//   or    ::= and (OR and)*
//   and   ::= not  (AND not)*
//   not   ::= NOT not | comparison
//   comp  ::= '(' cond ')' | value OP value | value
//   value ::= NUM | STR | WORD
//   OP    ::= '==' | '!=' | '<>' | '<' | '<=' | '>' | '>='
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum ExprToken {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Op(String),
    Num(f64),
    Str(String),
    Word(String),
}

static EXPR_TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?P<LPAREN>\()|(?P<RPAREN>\))|(?P<AND>\band\b)|(?P<OR>\bor\b)|(?P<NOT>\bnot\b)|(?P<OP>==|!=|<>|<=|>=|<|>)|(?P<NUM>-?\d+(?:\.\d+)?)|(?P<STR>"[^"]*"|'[^']*')|(?P<WORD>[^\s=!<>()]+)"#
    ).unwrap()
});

fn tokenise_expr(text: &str) -> Vec<ExprToken> {
    EXPR_TOKENS
        .captures_iter(text)
        .filter_map(|caps| {
            if caps.name("LPAREN").is_some() { return Some(ExprToken::LParen); }
            if caps.name("RPAREN").is_some() { return Some(ExprToken::RParen); }
            if caps.name("AND").is_some()    { return Some(ExprToken::And); }
            if caps.name("OR").is_some()     { return Some(ExprToken::Or); }
            if caps.name("NOT").is_some()    { return Some(ExprToken::Not); }
            if let Some(m) = caps.name("OP")  { return Some(ExprToken::Op(m.as_str().to_string())); }
            if let Some(m) = caps.name("NUM") {
                return m.as_str().parse::<f64>().ok().map(ExprToken::Num);
            }
            if let Some(m) = caps.name("STR") {
                let s = m.as_str();
                return Some(ExprToken::Str(s[1..s.len()-1].to_string()));
            }
            if let Some(m) = caps.name("WORD") {
                let s = m.as_str();
                if let Ok(n) = s.parse::<f64>() {
                    return Some(ExprToken::Num(n));
                }
                return Some(ExprToken::Word(s.to_string()));
            }
            None
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Value type for the evaluator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Value {
    Num(f64),
    Str(String),
}

impl Value {
    fn as_str_lower(&self) -> String {
        match self {
            Value::Num(n) => n.to_string(),
            Value::Str(s) => s.trim().to_lowercase(),
        }
    }
}

// ---------------------------------------------------------------------------
// Recursive descent parser
// ---------------------------------------------------------------------------

struct EvalError(String);

fn eval_expr(expr: &str, loc: &SourceLocation, diagnostics: &mut Vec<Diagnostic>) -> Option<bool> {
    let tokens = tokenise_expr(expr);
    if tokens.is_empty() {
        return None;
    }
    match parse_or(&tokens, 0) {
        Ok((result, _)) => result,
        Err(EvalError(msg)) => {
            diagnostics.push(Diagnostic::new(
                "D010",
                format!("Cannot evaluate condition '{expr}': {msg}"),
                loc.clone(),
                Severity::Info,
            ));
            None
        }
    }
}

fn parse_or(tokens: &[ExprToken], mut pos: usize) -> Result<(Option<bool>, usize), EvalError> {
    let (mut left, new_pos) = parse_and(tokens, pos)?;
    pos = new_pos;
    while pos < tokens.len() {
        if !matches!(tokens[pos], ExprToken::Or) { break; }
        pos += 1;
        let (right, new_pos) = parse_and(tokens, pos)?;
        pos = new_pos;
        left = or_tri(left, right);
    }
    Ok((left, pos))
}

fn parse_and(tokens: &[ExprToken], mut pos: usize) -> Result<(Option<bool>, usize), EvalError> {
    let (mut left, new_pos) = parse_not(tokens, pos)?;
    pos = new_pos;
    while pos < tokens.len() {
        if !matches!(tokens[pos], ExprToken::And) { break; }
        pos += 1;
        let (right, new_pos) = parse_not(tokens, pos)?;
        pos = new_pos;
        left = and_tri(left, right);
    }
    Ok((left, pos))
}

fn parse_not(tokens: &[ExprToken], pos: usize) -> Result<(Option<bool>, usize), EvalError> {
    if pos < tokens.len() && matches!(tokens[pos], ExprToken::Not) {
        let (val, new_pos) = parse_not(tokens, pos + 1)?;
        return Ok((val.map(|v| !v), new_pos));
    }
    parse_comparison(tokens, pos)
}

fn parse_comparison(tokens: &[ExprToken], pos: usize) -> Result<(Option<bool>, usize), EvalError> {
    // Parenthesised sub-expression
    if pos < tokens.len() && matches!(tokens[pos], ExprToken::LParen) {
        let (val, new_pos) = parse_or(tokens, pos + 1)?;
        if new_pos >= tokens.len() || !matches!(tokens[new_pos], ExprToken::RParen) {
            return Err(EvalError("missing ')'".into()));
        }
        return Ok((val, new_pos + 1));
    }

    let (lhs, mut pos) = parse_value(tokens, pos)?;

    // value OP value
    if pos < tokens.len() {
        if let ExprToken::Op(op) = &tokens[pos] {
            let op = op.clone();
            let (rhs, new_pos) = parse_value(tokens, pos + 1)?;
            pos = new_pos;
            return Ok((compare(&lhs, &op, &rhs), pos));
        }
    }

    // bare value: truthy if non-empty / non-zero
    let result = match &lhs {
        None => None,
        Some(Value::Num(n)) => Some(*n != 0.0),
        Some(Value::Str(s)) => Some(!s.trim().is_empty()),
    };
    Ok((result, pos))
}

fn parse_value(
    tokens: &[ExprToken],
    pos: usize,
) -> Result<(Option<Value>, usize), EvalError> {
    if pos >= tokens.len() {
        return Err(EvalError("unexpected end of condition".into()));
    }
    match &tokens[pos] {
        ExprToken::Num(n) => Ok((Some(Value::Num(*n)), pos + 1)),
        ExprToken::Str(s) => Ok((Some(Value::Str(s.clone())), pos + 1)),
        ExprToken::Word(w) => Ok((Some(Value::Str(w.clone())), pos + 1)),
        other => Err(EvalError(format!("unexpected token {other:?}"))),
    }
}

fn compare(lhs: &Option<Value>, op: &str, rhs: &Option<Value>) -> Option<bool> {
    let (Some(l), Some(r)) = (lhs, rhs) else { return None; };

    // Numeric comparison when both sides are numbers
    if let (Value::Num(ln), Value::Num(rn)) = (l, r) {
        return Some(match op {
            "==" => ln == rn,
            "!=" | "<>" => ln != rn,
            "<"  => ln <  rn,
            "<=" => ln <= rn,
            ">"  => ln >  rn,
            ">=" => ln >= rn,
            _    => return None,
        });
    }

    // String comparison (case-insensitive, matching GAMS behaviour)
    let ls = l.as_str_lower();
    let rs = r.as_str_lower();
    Some(match op {
        "==" => ls == rs,
        "!=" | "<>" => ls != rs,
        "<"  => ls <  rs,
        "<=" => ls <= rs,
        ">"  => ls >  rs,
        ">=" => ls >= rs,
        _    => return None,
    })
}

// ---------------------------------------------------------------------------
// Three-valued logic helpers
// ---------------------------------------------------------------------------

fn or_tri(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn and_tri(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}
