//! The template language (ADR 004): `{name}` or `{name:args}`, with escapes `\{ \} \| \\`.
//! A template is parsed into nodes, then evaluated. Arguments are evaluated before their function
//! runs, so calls nest: `{base64:{input}}`.

use std::fmt::Write as _;

use base64::Engine as _;
use laqta_http::Response;
use laqta_ports::{BoxFuture, PortError};
use serde_json::Value;

use super::xml;
use super::{Env, Input, SxcuError};

/// A parsed template: literal text and function calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    Text(String),
    Call { name: String, args: Vec<Vec<Node>> },
}

/// Parses a template. A backslash escapes `{`, `}`, `|` and `\`; before any other character it is
/// literal. A `}` or `|` outside a call is literal too.
pub(super) fn parse(src: &str) -> Result<Vec<Node>, SxcuError> {
    let chars: Vec<char> = src.chars().collect();
    let mut parser = Parser { chars: &chars, pos: 0 };
    let (nodes, _) = parser.sequence(false)?;
    Ok(nodes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    End,
    Pipe,
    Close,
}

struct Parser<'s> {
    chars: &'s [char],
    pos: usize,
}

fn flush(nodes: &mut Vec<Node>, text: &mut String) {
    if !text.is_empty() {
        nodes.push(Node::Text(std::mem::take(text)));
    }
}

impl Parser<'_> {
    /// Reads nodes until the end of input, or, inside a call, until an unescaped `|` or `}`.
    fn sequence(&mut self, in_call: bool) -> Result<(Vec<Node>, Stop), SxcuError> {
        let mut nodes = Vec::new();
        let mut text = String::new();
        loop {
            let Some(&c) = self.chars.get(self.pos) else {
                flush(&mut nodes, &mut text);
                return Ok((nodes, Stop::End));
            };
            self.pos += 1;
            match c {
                '\\' => match self.chars.get(self.pos) {
                    Some(&escaped @ ('{' | '}' | '|' | '\\')) => {
                        text.push(escaped);
                        self.pos += 1;
                    }
                    _ => text.push('\\'),
                },
                '{' => {
                    flush(&mut nodes, &mut text);
                    nodes.push(self.call()?);
                }
                '|' if in_call => {
                    flush(&mut nodes, &mut text);
                    return Ok((nodes, Stop::Pipe));
                }
                '}' if in_call => {
                    flush(&mut nodes, &mut text);
                    return Ok((nodes, Stop::Close));
                }
                c => text.push(c),
            }
        }
    }

    /// Reads a call. The `{` is already consumed.
    fn call(&mut self) -> Result<Node, SxcuError> {
        let mut name = String::new();
        loop {
            match self.chars.get(self.pos) {
                None => return Err(SxcuError::Syntax(format!("unclosed `{{{}`", name.trim()))),
                Some('}') => {
                    self.pos += 1;
                    return Ok(Node::Call { name: normalize(&name), args: Vec::new() });
                }
                Some(':') => {
                    self.pos += 1;
                    break;
                }
                Some('{') => return Err(SxcuError::Syntax(format!("`{{` inside the function name `{}`", name.trim()))),
                Some(&c) => {
                    name.push(c);
                    self.pos += 1;
                }
            }
        }
        let mut args = Vec::new();
        loop {
            let (nodes, stop) = self.sequence(true)?;
            args.push(nodes);
            match stop {
                Stop::Pipe => {}
                Stop::Close => break,
                Stop::End => return Err(SxcuError::Syntax(format!("unclosed `{{{}:`", name.trim()))),
            }
        }
        Ok(Node::Call { name: normalize(&name), args })
    }
}

fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// How a function's result is embedded. Only top-level calls in a `Data` body are escaped, so
/// the text stays valid JSON or XML; nested arguments are never escaped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Escape {
    Plain,
    Json,
    Xml,
}

impl Escape {
    fn apply(self, value: String) -> String {
        match self {
            Escape::Plain => value,
            Escape::Json => json_escape(&value),
            Escape::Xml => xml_escape(&value),
        }
    }
}

/// What a template is evaluated against: the input, the prompts and randomness, and the HTTP
/// response when the template is a response template.
pub(super) struct Ctx<'a> {
    pub(super) input: &'a Input<'a>,
    pub(super) env: &'a Env<'a>,
    pub(super) response: Option<&'a Response>,
}

/// Parses and evaluates `src`.
pub(super) async fn render(ctx: &Ctx<'_>, src: &str, escape: Escape) -> Result<String, SxcuError> {
    let nodes = parse(src)?;
    eval(&nodes, ctx, escape).await
}

fn eval<'a>(nodes: &'a [Node], ctx: &'a Ctx<'a>, escape: Escape) -> BoxFuture<'a, Result<String, SxcuError>> {
    Box::pin(async move {
        let mut out = String::new();
        for node in nodes {
            match node {
                Node::Text(text) => out.push_str(text),
                Node::Call { name, args } => {
                    let mut values = Vec::with_capacity(args.len());
                    for arg in args {
                        values.push(eval(arg, ctx, Escape::Plain).await?);
                    }
                    let value = call(name, &values, ctx).await?;
                    out.push_str(&escape.apply(value));
                }
            }
        }
        Ok(out)
    })
}

async fn call(name: &str, args: &[String], ctx: &Ctx<'_>) -> Result<String, SxcuError> {
    match name {
        "input" => {
            no_args(name, args)?;
            Ok(ctx.input.text.unwrap_or_default().to_string())
        }
        "filename" => {
            no_args(name, args)?;
            Ok(ctx.input.file_name.to_string())
        }
        "random" => {
            if args.is_empty() {
                return Err(SxcuError::Syntax("`{random}` needs at least one option".into()));
            }
            let index = (ctx.env.rng.next_u64() % args.len() as u64) as usize;
            Ok(args[index].clone())
        }
        "select" => {
            if args.is_empty() {
                return Err(SxcuError::Syntax("`{select}` needs at least one option".into()));
            }
            let choice = ctx.env.prompt.choose("Select", args).await.map_err(port_error)?;
            args.get(choice)
                .cloned()
                .ok_or_else(|| SxcuError::Syntax(format!("`{{select}}` answered option {choice} of {}", args.len())))
        }
        "inputbox" => {
            let (title, default) = title_and_rest(args);
            ctx.env.prompt.ask_text(&title, &default).await.map_err(port_error)
        }
        "outputbox" => {
            let (title, text) = title_and_rest(args);
            ctx.env.prompt.show_text(&title, &text).await.map_err(port_error)?;
            Ok(String::new())
        }
        "base64" => {
            if args.is_empty() {
                return Err(SxcuError::Syntax("`{base64}` needs an argument".into()));
            }
            Ok(base64::engine::general_purpose::STANDARD.encode(args.join("|")))
        }
        "response" | "responseurl" | "header" | "json" | "xml" | "regex" => {
            let Some(response) = ctx.response else {
                return Err(SxcuError::Syntax(format!("`{{{name}}}` is only valid in a response template")));
            };
            respond(name, args, response)
        }
        other => Err(SxcuError::Syntax(format!("unknown function `{{{other}}}`"))),
    }
}

/// The functions that read the response.
fn respond(name: &str, args: &[String], response: &Response) -> Result<String, SxcuError> {
    match name {
        "response" => {
            no_args(name, args)?;
            Ok(response.text())
        }
        "responseurl" => {
            no_args(name, args)?;
            Ok(response.url.clone())
        }
        "header" => {
            let [header] = args else {
                return Err(SxcuError::Syntax("`{header}` takes one header name".into()));
            };
            Ok(response.header(header).unwrap_or_default().to_string())
        }
        "json" => json(args, response),
        "xml" => {
            let [path] = args else {
                return Err(SxcuError::Syntax("`{xml}` takes one XPath".into()));
            };
            xml::select(&response.text(), path)
        }
        _ => regex(args, &response.text()),
    }
}

/// `{json:path}` reads the response. `{json:input|path}` reads an explicit JSON `input`.
fn json(args: &[String], response: &Response) -> Result<String, SxcuError> {
    let (document, path) = match args {
        [] => return Err(SxcuError::Syntax("`{json}` needs a path".into())),
        [path] => (response_json(response)?, path.clone()),
        [first, rest @ ..] => match serde_json::from_str::<Value>(first.trim()) {
            Ok(document) => (document, rest.join("|")),
            Err(_) => (response_json(response)?, args.join("|")),
        },
    };
    let path = normalize_path(path.trim());
    let parsed = serde_json_path::JsonPath::parse(&path).map_err(|e| SxcuError::Syntax(format!("JSON path `{path}`: {e}")))?;
    Ok(match parsed.query(&document).first() {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    })
}

fn response_json(response: &Response) -> Result<Value, SxcuError> {
    serde_json::from_slice(&response.body).map_err(|e| SxcuError::Json(format!("response is not JSON: {e}")))
}

/// Accepts the RFC 9535 form (`$.data.link`, `$[0]`) and ShareX's dotted form (`data.link`,
/// `files[0].url`).
fn normalize_path(path: &str) -> String {
    if path.starts_with('$') {
        path.to_string()
    } else if path.starts_with('[') {
        format!("${path}")
    } else {
        format!("$.{path}")
    }
}

/// `{regex:pattern|group}`. The last argument is the group: an index, 0 for the whole match, or a
/// name. Without one, group 1 is used, or the whole match when the pattern has no groups. Earlier
/// arguments rejoin into the pattern, so an unescaped `|` still works.
fn regex(args: &[String], text: &str) -> Result<String, SxcuError> {
    let (pattern, group) = match args {
        [] => return Err(SxcuError::Syntax("`{regex}` needs a pattern".into())),
        [pattern] => (pattern.clone(), None),
        [.., group] => (args[..args.len() - 1].join("|"), Some(group.trim().to_string())),
    };
    let re = fancy_regex::Regex::new(&pattern).map_err(|e| SxcuError::Syntax(format!("regex `{pattern}`: {e}")))?;
    let captures = re.captures(text).map_err(|e| SxcuError::Syntax(format!("regex `{pattern}`: {e}")))?;
    let Some(captures) = captures else {
        return Ok(String::new());
    };
    let found = match group.as_deref() {
        None => captures.get(if captures.len() > 1 { 1 } else { 0 }),
        Some(group) => match group.parse::<usize>() {
            Ok(index) => {
                if index >= captures.len() {
                    return Err(SxcuError::Syntax(format!("regex `{pattern}` has no group {index}")));
                }
                captures.get(index)
            }
            Err(_) => {
                if !re.capture_names().flatten().any(|name| name == group) {
                    return Err(SxcuError::Syntax(format!("regex `{pattern}` has no group named `{group}`")));
                }
                captures.name(group)
            }
        },
    };
    Ok(found.map(|m| m.as_str().to_string()).unwrap_or_default())
}

fn no_args(name: &str, args: &[String]) -> Result<(), SxcuError> {
    if args.is_empty() { Ok(()) } else { Err(SxcuError::Syntax(format!("`{{{name}}}` takes no arguments"))) }
}

/// The title and the rest of the arguments, rejoined with `|`.
fn title_and_rest(args: &[String]) -> (String, String) {
    match args.split_first() {
        Some((title, rest)) => (title.clone(), rest.join("|")),
        None => (String::new(), String::new()),
    }
}

pub(super) fn port_error(error: PortError) -> SxcuError {
    match error {
        PortError::Cancelled => SxcuError::Cancelled,
        PortError::Unsupported(message) | PortError::Failed(message) => SxcuError::Unsupported(message),
        PortError::PermissionDenied => SxcuError::Unsupported("permission denied".into()),
        other => SxcuError::Unsupported(other.to_string()),
    }
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}
