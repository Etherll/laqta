//! The directory indexer: a folder tree as text, JSON, XML or HTML.

use std::path::Path;

use laqta_ports::AppError;
use serde::Serialize;

use super::common::io_error;
use crate::Ports;
use crate::files::{self, WalkEntry};
use crate::task::compute;

/// The format of an index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    Html,
    Json,
    Text,
    Xml,
}

/// What an index lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexOptions {
    /// How many levels below the folder to list: `Some(1)` lists its direct children, `None` lists everything.
    pub max_depth: Option<u32>,
    /// Whether hidden entries (a leading dot, or the hidden attribute on Windows) are listed.
    pub include_hidden: bool,
    /// Whether files are listed. Folders are always listed.
    pub include_files: bool,
    /// Whether file sizes in bytes are written.
    pub show_sizes: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions { max_depth: None, include_hidden: false, include_files: true, show_sizes: true }
    }
}

/// Indexes the folder `dir` in `format`. The entries are sorted with folders first, then by name. The result is one
/// document: text is an indented tree, JSON a nested object, XML a nested element tree, HTML a nested list.
pub async fn index_folder(ports: &Ports, dir: &Path, format: IndexFormat, options: IndexOptions) -> Result<String, AppError> {
    let spawner = ports.spawner.as_ref();
    let entries = files::walk(spawner, dir.to_path_buf(), options.max_depth, options.include_hidden).await.map_err(io_error)?;
    let title = dir.file_name().map_or_else(|| dir.display().to_string(), |name| name.to_string_lossy().into_owned());
    compute(spawner, move || render(&title, &entries, format, options)).await
}

/// One listed entry, with its children when it is a folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Node {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Node>,
}

fn render(title: &str, entries: &[WalkEntry], format: IndexFormat, options: IndexOptions) -> Result<String, AppError> {
    let nodes = build_tree(entries, options);
    match format {
        IndexFormat::Text => Ok(text(title, &nodes)),
        IndexFormat::Json => {
            let root = Node { name: title.to_owned(), kind: "folder", size: None, children: nodes };
            serde_json::to_string_pretty(&root).map_err(|error| AppError::Io(error.to_string()))
        }
        IndexFormat::Xml => Ok(xml(title, &nodes)),
        IndexFormat::Html => html(title, &nodes),
    }
}

/// Rebuilds the tree from the walk. A walk lists each folder's contents right after it, so the depth tells where a
/// level ends.
fn build_tree(entries: &[WalkEntry], options: IndexOptions) -> Vec<Node> {
    let mut at = 0;
    let mut nodes = Vec::new();
    take_level(entries, &mut at, 0, options, &mut nodes);
    nodes
}

fn take_level(entries: &[WalkEntry], at: &mut usize, depth: u32, options: IndexOptions, out: &mut Vec<Node>) {
    while let Some(entry) = entries.get(*at) {
        if entry.depth < depth {
            return;
        }
        *at += 1;
        if !entry.is_dir && !options.include_files {
            continue;
        }
        let mut node = Node {
            name: entry.name.clone(),
            kind: if entry.is_dir { "folder" } else { "file" },
            size: (!entry.is_dir && options.show_sizes).then_some(entry.size),
            children: Vec::new(),
        };
        if entry.is_dir {
            take_level(entries, at, depth + 1, options, &mut node.children);
        }
        out.push(node);
    }
}

fn text(title: &str, nodes: &[Node]) -> String {
    let mut out = format!("{title}/\n");
    for node in nodes {
        text_line(node, 1, &mut out);
    }
    out
}

fn text_line(node: &Node, depth: usize, out: &mut String) {
    let label = match (node.kind, node.size) {
        ("folder", _) => format!("{}/", node.name),
        (_, Some(size)) => format!("{} ({size} bytes)", node.name),
        _ => node.name.clone(),
    };
    out.push_str(&format!("{}{label}\n", "  ".repeat(depth)));
    for child in &node.children {
        text_line(child, depth + 1, out);
    }
}

fn xml(title: &str, nodes: &[Node]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!("<index name=\"{}\">\n", escape(title)));
    for node in nodes {
        xml_element(node, 1, &mut out);
    }
    out.push_str("</index>\n");
    out
}

fn xml_element(node: &Node, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let tag = node.kind;
    let size = node.size.map(|bytes| format!(" size=\"{bytes}\"")).unwrap_or_default();
    let attributes = format!(" name=\"{}\"{size}", escape(&node.name));
    if node.children.is_empty() {
        out.push_str(&format!("{indent}<{tag}{attributes}/>\n"));
        return;
    }
    out.push_str(&format!("{indent}<{tag}{attributes}>\n"));
    for child in &node.children {
        xml_element(child, depth + 1, out);
    }
    out.push_str(&format!("{indent}</{tag}>\n"));
}

fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&apos;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

/// The HTML page: a title and a nested list. It has no styling.
const PAGE: &str = "<!doctype html>
<html>
<head>
<meta charset=\"utf-8\">
<title>{{ title }}</title>
</head>
<body>
<ul>
{%- for node in nodes recursive %}
<li>{{ node.name }}{% if node.type == \"folder\" %}/{% endif %}{% if node.size is number %} ({{ node.size }} bytes){% endif %}
{%- if node.children %}
<ul>{{ loop(node.children) }}</ul>
{%- endif %}</li>
{%- endfor %}
</ul>
</body>
</html>
";

#[derive(Serialize)]
struct Page<'a> {
    title: &'a str,
    nodes: &'a [Node],
}

fn html(title: &str, nodes: &[Node]) -> Result<String, AppError> {
    let mut env = minijinja::Environment::new();
    env.set_auto_escape_callback(|_: &str| minijinja::AutoEscape::Html);
    env.add_template("index.html", PAGE).map_err(template_error)?;
    let template = env.get_template("index.html").map_err(template_error)?;
    template.render(minijinja::value::Serde(Page { title, nodes })).map_err(template_error)
}

fn template_error(error: minijinja::Error) -> AppError {
    AppError::Io(format!("index page: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn entry(name: &str, depth: u32, is_dir: bool, size: u64) -> WalkEntry {
        WalkEntry { name: name.to_owned(), depth, is_dir, size }
    }

    #[test]
    fn the_tree_follows_the_depths() {
        let entries = [entry("a", 0, true, 0), entry("inner.txt", 1, false, 3), entry("b.txt", 0, false, 2)];
        let nodes = build_tree(&entries, IndexOptions::default());
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].children.len(), 1);
        assert_eq!(nodes[0].children[0].size, Some(3));
        assert_eq!(nodes[1].name, "b.txt");
    }

    #[test]
    fn options_drop_files_and_sizes() {
        let entries = [entry("a", 0, true, 0), entry("x.txt", 1, false, 3)];
        let no_files = IndexOptions { include_files: false, ..IndexOptions::default() };
        let nodes = build_tree(&entries, no_files);
        assert!(nodes[0].children.is_empty());

        let no_sizes = IndexOptions { show_sizes: false, ..IndexOptions::default() };
        assert_eq!(build_tree(&entries, no_sizes)[0].children[0].size, None);
    }

    #[test]
    fn xml_escapes_names() {
        let nodes = vec![Node { name: "a&<b>\"'.txt".to_owned(), kind: "file", size: Some(1), children: Vec::new() }];
        let xml = xml("t", &nodes);
        assert!(xml.contains("name=\"a&amp;&lt;b&gt;&quot;&apos;.txt\" size=\"1\"/>"), "{xml}");
    }

    #[test]
    fn html_escapes_names_and_title() {
        let nodes = vec![Node { name: "<script>".to_owned(), kind: "file", size: None, children: Vec::new() }];
        let page = html("a & b", &nodes).unwrap();
        assert!(page.contains("<title>a &amp; b</title>"), "{page}");
        assert!(page.contains("<li>&lt;script&gt;</li>"), "{page}");
    }
}
