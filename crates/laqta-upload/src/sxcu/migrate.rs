//! Load-time migration of ShareX 12 text. A `$name:args$` marker becomes a `{name:args}` call,
//! and `$regex:N,G$` becomes `{regex:<RegexList[N-1]>|G}`. The old syntax had no braces, so
//! the literal text around the markers is escaped.

use super::SxcuError;

/// The ShareX 12 marker names. Each one maps to the template function of the same name.
const LEGACY_NAMES: [&str; 7] = ["json", "xml", "regex", "random", "response", "input", "filename"];

/// True when `text` holds a ShareX 12 marker such as `$json:data.link$` or `$input$`.
pub(super) fn has_marker(text: &str) -> bool {
    LEGACY_NAMES.iter().any(|name| text.contains(&format!("${name}$")) || text.contains(&format!("${name}:")))
}

/// Converts ShareX 12 text to the current template syntax.
pub(super) fn convert(text: &str, regexes: &[String]) -> Result<String, SxcuError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$'
            && let Some((call, next)) = marker(&chars, i, regexes)?
        {
            out.push_str(&call);
            i = next;
            continue;
        }
        match chars[i] {
            c @ ('\\' | '{' | '}') => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        i += 1;
    }
    Ok(out)
}

/// Reads the marker that starts at `chars[start]` (a `$`). Returns the call and the index after
/// the closing `$`, or `None` when no legacy marker starts here.
fn marker(chars: &[char], start: usize, regexes: &[String]) -> Result<Option<(String, usize)>, SxcuError> {
    let name_start = start + 1;
    let name_end = chars[name_start..].iter().position(|c| !c.is_ascii_lowercase()).map_or(chars.len(), |p| name_start + p);
    let name: String = chars[name_start..name_end].iter().collect();
    if !LEGACY_NAMES.contains(&name.as_str()) {
        return Ok(None);
    }
    match chars.get(name_end) {
        Some('$') => Ok(Some((format!("{{{name}}}"), name_end + 1))),
        Some(':') => {
            let Some(offset) = chars[name_end + 1..].iter().position(|&c| c == '$') else {
                return Ok(None);
            };
            let close = name_end + 1 + offset;
            let args: String = chars[name_end + 1..close].iter().collect();
            Ok(Some((call(&name, &args, regexes)?, close + 1)))
        }
        _ => Ok(None),
    }
}

fn call(name: &str, args: &str, regexes: &[String]) -> Result<String, SxcuError> {
    match name {
        "regex" => {
            let (index, group) = args.split_once(',').unwrap_or((args, ""));
            let pattern = index
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| regexes.get(i))
                .ok_or_else(|| SxcuError::Syntax(format!("`$regex:{args}$` has no RegexList entry")))?;
            let group = group.trim();
            let group = if group.is_empty() { String::new() } else { format!("|{group}") };
            Ok(format!("{{regex:{}{group}}}", escape_pattern(pattern)))
        }
        // `|` separates the options.
        "random" => Ok(format!("{{random:{}}}", escape_braces(args))),
        _ => Ok(format!("{{{name}:{}}}", escape_braces(args))),
    }
}

/// Escapes `\`, `{` and `}`, so the text reads as literal under the template syntax.
fn escape_braces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '{' | '}') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escapes a regex pattern as an argument, so its `|` alternations stay inside one argument.
fn escape_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for c in pattern.chars() {
        if matches!(c, '\\' | '{' | '}' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
