//! The `{xml:…}` XPath subset (network research §2.6), evaluated on roxmltree. Supported: absolute
//! and `//` paths over element names or `*`, 1-based `[n]` positions, and a final `@attr` or
//! `text()`. Anything else is `Unsupported`.

use std::collections::HashSet;

use roxmltree::{Document, Node};

use super::SxcuError;

#[derive(Debug, PartialEq, Eq)]
enum Test {
    Element(String),
    Text,
    Attribute(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Step {
    /// `//` rather than `/`.
    descendant: bool,
    test: Test,
    /// 1-based position among the matching children of each parent.
    position: Option<usize>,
}

/// The string value of the first node the path selects, or "" when nothing matches. An element
/// gives all its descendant text.
pub(super) fn select(xml: &str, path: &str) -> Result<String, SxcuError> {
    let steps = parse(path)?;
    let document = Document::parse(xml).map_err(|e| SxcuError::Syntax(format!("response is not XML: {e}")))?;
    let mut nodes = vec![document.root()];
    for step in &steps {
        match &step.test {
            Test::Element(name) => nodes = child_elements(&nodes, step, name),
            Test::Text | Test::Attribute(_) => {
                // `parse` allows these only as the last step.
                return Ok(terminal(&nodes, step).into_iter().next().unwrap_or_default());
            }
        }
    }
    Ok(nodes.first().map(string_value).unwrap_or_default())
}

fn parse(path: &str) -> Result<Vec<Step>, SxcuError> {
    let mut rest = path.trim();
    if rest.is_empty() {
        return Err(SxcuError::Syntax("empty XPath".into()));
    }
    let mut steps: Vec<Step> = Vec::new();
    while !rest.is_empty() {
        let descendant = if let Some(tail) = rest.strip_prefix("//") {
            rest = tail;
            true
        } else if let Some(tail) = rest.strip_prefix('/') {
            rest = tail;
            false
        } else if steps.is_empty() {
            // In `.sxcu` XPath, a relative path starts at the document.
            false
        } else {
            return Err(SxcuError::Syntax(format!("expected `/` in XPath `{path}`")));
        };
        let end = rest.find('/').unwrap_or(rest.len());
        let token = &rest[..end];
        rest = &rest[end..];
        steps.push(step(token, descendant, path)?);
    }
    if steps[..steps.len() - 1].iter().any(|s| !matches!(s.test, Test::Element(_))) {
        return Err(SxcuError::Unsupported(format!("`text()` and `@attr` must be the last step in `{path}`")));
    }
    Ok(steps)
}

fn step(token: &str, descendant: bool, path: &str) -> Result<Step, SxcuError> {
    if token.is_empty() {
        return Err(SxcuError::Syntax(format!("empty step in XPath `{path}`")));
    }
    if token == "text()" {
        return Ok(Step { descendant, test: Test::Text, position: None });
    }
    if let Some(attribute) = token.strip_prefix('@') {
        check_name(attribute)?;
        return Ok(Step { descendant, test: Test::Attribute(attribute.to_string()), position: None });
    }
    let (name, position) = match token.split_once('[') {
        None => (token, None),
        Some((name, predicate)) => {
            let digits = predicate.strip_suffix(']').ok_or_else(|| SxcuError::Unsupported(format!("XPath predicate `[{predicate}`")))?;
            let position: usize = digits
                .trim()
                .parse()
                .map_err(|_| SxcuError::Unsupported(format!("XPath predicate `[{digits}]`; only positions are supported")))?;
            if position == 0 {
                return Err(SxcuError::Syntax("XPath positions start at 1".into()));
            }
            (name, Some(position))
        }
    };
    if name != "*" {
        check_name(name)?;
    }
    Ok(Step { descendant, test: Test::Element(name.to_string()), position })
}

fn check_name(name: &str) -> Result<(), SxcuError> {
    let starts_ok = name.starts_with(|c: char| c.is_alphabetic() || c == '_');
    if starts_ok && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')) {
        Ok(())
    } else {
        Err(SxcuError::Unsupported(format!("XPath step `{name}`")))
    }
}

/// The nodes a step starts from: the node itself, or itself and its descendants for `//`.
fn bases<'a, 'i>(node: Node<'a, 'i>, descendant: bool) -> Vec<Node<'a, 'i>> {
    if descendant { node.descendants().collect() } else { vec![node] }
}

fn matches_name(node: &Node<'_, '_>, name: &str) -> bool {
    node.is_element() && (name == "*" || node.tag_name().name() == name)
}

fn child_elements<'a, 'i>(context: &[Node<'a, 'i>], step: &Step, name: &str) -> Vec<Node<'a, 'i>> {
    let mut found = Vec::new();
    for &node in context {
        for base in bases(node, step.descendant) {
            let matching: Vec<Node<'a, 'i>> = base.children().filter(|child| matches_name(child, name)).collect();
            match step.position {
                Some(position) => found.extend(matching.get(position - 1).copied()),
                None => found.extend(matching),
            }
        }
    }
    document_order(found)
}

/// Text or attribute values selected by the last step.
fn terminal(context: &[Node<'_, '_>], step: &Step) -> Vec<String> {
    let mut values = Vec::new();
    for &node in context {
        for base in bases(node, step.descendant) {
            match &step.test {
                Test::Text => values.extend(base.children().filter(|c| c.is_text()).filter_map(|c| c.text()).map(str::to_string)),
                Test::Attribute(name) => values.extend(base.attributes().filter(|a| a.name() == name).map(|a| a.value().to_string())),
                Test::Element(_) => {}
            }
        }
    }
    values
}

/// The XPath string value: the descendant text nodes, in order. `Node::text` is not used because
/// it returns the first child for an element and the body of a comment.
fn string_value(node: &Node<'_, '_>) -> String {
    node.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect()
}

/// Removes duplicates and sorts into document order (roxmltree numbers nodes in document order).
fn document_order<'a, 'i>(mut nodes: Vec<Node<'a, 'i>>) -> Vec<Node<'a, 'i>> {
    let mut seen = HashSet::new();
    nodes.retain(|node| seen.insert(node.id()));
    nodes.sort_by_key(|node| node.id().get());
    nodes
}
