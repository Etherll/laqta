//! Workspace tasks: `check-deps` (the CI dependency guard, ADR 024), `bench-png`, `measure-idle`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, PackageId};
use serde::Deserialize;

mod measure;

#[derive(Debug, Deserialize)]
struct Policy {
    targets: Vec<String>,
    #[serde(default)]
    edges: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    allow: BTreeMap<String, Vec<String>>,
    deny: Deny,
    #[serde(default)]
    placement: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    source_rule: Vec<SourceRule>,
}

#[derive(Debug, Deserialize)]
struct Deny {
    crates: Vec<String>,
    forbid: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SourceRule {
    path: String,
    forbid_imports: Vec<String>,
}

/// One broken rule. `kind` is used by the self-test.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Violation {
    kind: &'static str,
    message: String,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let result = match args.first().map(String::as_str) {
        Some("check-deps") if args.get(1).map(String::as_str) == Some("--self-test") => self_test(&root),
        Some("check-deps") => report(check_deps(&root, false)),
        Some("bench-png") => run(Command::new("cargo").current_dir(&root).args(["run", "--release", "-p", "laqta-imaging", "--example", "png_bench"])),
        Some("measure-idle") => measure::measure_idle(&root),
        _ => {
            eprintln!("usage: cargo xtask <check-deps [--self-test] | bench-png | measure-idle>");
            Err("unknown command".into())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(Path::to_path_buf).unwrap_or(manifest)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("command failed: {status}")) }
}

fn report(result: Result<Vec<Violation>, String>) -> Result<(), String> {
    let violations = result?;
    if violations.is_empty() {
        println!("check-deps: ok");
        return Ok(());
    }
    for v in &violations {
        eprintln!("[{}] {}", v.kind, v.message);
    }
    Err(format!("{} dependency-policy violation(s)", violations.len()))
}

fn metadata(root: &Path, target: Option<&str>, offline: bool) -> Result<Metadata, String> {
    let mut cmd = MetadataCommand::new();
    cmd.manifest_path(root.join("Cargo.toml"));
    let mut extra = Vec::new();
    if let Some(t) = target {
        extra.push("--filter-platform".to_string());
        extra.push(t.to_string());
    }
    if offline {
        extra.push("--offline".to_string());
    }
    cmd.other_options(extra);
    cmd.exec().map_err(|e| format!("cargo metadata failed: {e}"))
}

fn check_deps(root: &Path, offline: bool) -> Result<Vec<Violation>, String> {
    let text = std::fs::read_to_string(root.join("deps-policy.toml")).map_err(|e| format!("deps-policy.toml: {e}"))?;
    let policy: Policy = toml::from_str(&text).map_err(|e| format!("deps-policy.toml: {e}"))?;
    let meta = metadata(root, None, offline)?;
    let mut out = BTreeSet::new();

    let members: Vec<&cargo_metadata::Package> = meta.workspace_packages();
    let ws_names: BTreeSet<String> = members.iter().map(|p| p.name.to_string()).collect();

    for pkg in &members {
        let name = pkg.name.to_string();
        let normal: Vec<&cargo_metadata::Dependency> = pkg.dependencies.iter().filter(|d| d.kind == DependencyKind::Normal).collect();

        // edges
        if name != "xtask" {
            match policy.edges.get(&name) {
                None => {
                    out.insert(Violation { kind: "edges", message: format!("{name}: workspace crate has no row in [edges]") });
                }
                Some(allowed) if !allowed.iter().any(|a| a == "*") => {
                    for d in pkg.dependencies.iter().filter(|d| ws_names.contains(&d.name) && d.kind != DependencyKind::Development) {
                        if !allowed.contains(&d.name) {
                            out.insert(Violation { kind: "edges", message: format!("{name} -> {}: edge not in [edges]", d.name) });
                        }
                    }
                }
                Some(_) => {}
            }
        }
        // allow
        if let Some(allowed) = policy.allow.get(&name) {
            for d in normal.iter().filter(|d| !ws_names.contains(&d.name)) {
                if !allowed.contains(&d.name) {
                    out.insert(Violation { kind: "allow", message: format!("{name} -> {}: not on {name}'s allow-list", d.name) });
                }
            }
        }
        // placement
        for d in &normal {
            if let Some(places) = policy.placement.get(&d.name) {
                if !places.contains(&name) {
                    out.insert(Violation { kind: "placement", message: format!("{name} -> {}: {} is allowed only in {places:?}", d.name, d.name) });
                }
            }
        }
    }

    // deny: transitive normal dependencies, per target
    for target in &policy.targets {
        let meta = metadata(root, Some(target), offline)?;
        let resolve = meta.resolve.as_ref().ok_or("cargo metadata returned no resolve graph")?;
        let names: HashMap<&PackageId, String> = meta.packages.iter().map(|p| (&p.id, p.name.to_string())).collect();
        let nodes: HashMap<&PackageId, &cargo_metadata::Node> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
        for start in meta.workspace_packages().iter().filter(|p| policy.deny.crates.contains(&p.name.to_string())) {
            let mut parent: HashMap<&PackageId, &PackageId> = HashMap::new();
            let mut queue = VecDeque::from([&start.id]);
            let mut seen = BTreeSet::from([&start.id]);
            while let Some(id) = queue.pop_front() {
                let Some(node) = nodes.get(id) else { continue };
                for dep in &node.deps {
                    if !dep.dep_kinds.iter().any(|k| k.kind == DependencyKind::Normal) {
                        continue;
                    }
                    if !seen.insert(&dep.pkg) {
                        continue;
                    }
                    parent.insert(&dep.pkg, id);
                    let dep_name = names.get(&dep.pkg).cloned().unwrap_or_default();
                    if policy.deny.forbid.contains(&dep_name) {
                        let mut path = vec![dep_name.clone()];
                        let mut cur = id;
                        loop {
                            path.push(names.get(cur).cloned().unwrap_or_default());
                            match parent.get(cur) {
                                Some(p) => cur = p,
                                None => break,
                            }
                        }
                        path.reverse();
                        out.insert(Violation { kind: "deny", message: format!("[{target}] {} reaches forbidden {dep_name}: {}", start.name, path.join(" -> ")) });
                        continue;
                    }
                    queue.push_back(&dep.pkg);
                }
            }
        }
    }

    // source rules
    for rule in &policy.source_rule {
        let dir = root.join(&rule.path);
        for file in rs_files(&dir) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            for name in &rule.forbid_imports {
                if mentions(&text, name) {
                    let rel = file.strip_prefix(root).unwrap_or(&file).display().to_string();
                    out.insert(Violation { kind: "source_rule", message: format!("{rel}: imports forbidden `{name}`") });
                }
            }
        }
    }
    Ok(out.into_iter().collect())
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(rs_files(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

/// True if `text` contains `use <name>` or `<name>::` where `name` is a whole identifier.
fn mentions(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    for (i, _) in text.match_indices(name) {
        let before = text[..i].chars().next_back();
        let after = &text[i + name.len()..];
        if before.is_some_and(is_ident) || after.chars().next().is_some_and(is_ident) {
            continue;
        }
        if after.starts_with("::") {
            return true;
        }
        let head = text[..i].trim_end();
        if head.ends_with("use") && !head[..head.len() - 3].chars().next_back().is_some_and(is_ident) && head.len() < text[..i].len() {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Self-test: plant one violation of each kind in a copy of the workspace and expect it to fail.

fn copy_workspace(root: &Path, dest: &Path) -> Result<(), String> {
    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let name = e.file_name();
            if name == "target" || name == ".git" || name == ".claude" {
                continue;
            }
            let (s, d) = (e.path(), dst.join(&name));
            if e.file_type()?.is_dir() { copy_dir(&s, &d)? } else { std::fs::copy(&s, &d).map(|_| ())? }
        }
        Ok(())
    }
    for item in ["Cargo.toml", "Cargo.lock", "deps-policy.toml"] {
        std::fs::copy(root.join(item), dest.join(item)).map_err(|e| format!("{item}: {e}"))?;
    }
    for dir in ["crates", "apps", "xtask"] {
        copy_dir(&root.join(dir), &dest.join(dir)).map_err(|e| format!("{dir}: {e}"))?;
    }
    Ok(())
}

fn add_dep(manifest: &Path, line: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let new = if text.contains("\n[dependencies]\n") {
        text.replacen("\n[dependencies]\n", &format!("\n[dependencies]\n{line}\n"), 1)
    } else {
        format!("{text}\n[dependencies]\n{line}\n")
    };
    std::fs::write(manifest, new).map_err(|e| e.to_string())
}

fn self_test(root: &Path) -> Result<(), String> {
    type Plant = fn(&Path) -> Result<(), String>;
    let cases: [(&str, Plant); 5] = [
        ("edges", |r| add_dep(&r.join("crates/laqta-ipc/Cargo.toml"), "laqta-ports.workspace = true")),
        ("allow", |r| add_dep(&r.join("crates/laqta-imaging/Cargo.toml"), "bitflags.workspace = true")),
        ("deny", |r| {
            add_dep(&r.join("crates/laqta-ports/Cargo.toml"), "interprocess.workspace = true")?;
            let p = r.join("deps-policy.toml");
            let t = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
            std::fs::write(&p, t.replace(r#""laqta-ports"   = ["thiserror", "jiff"]"#, r#""laqta-ports"   = ["thiserror", "jiff", "interprocess"]"#)).map_err(|e| e.to_string())
        }),
        ("placement", |r| add_dep(&r.join("crates/laqta-ipc/Cargo.toml"), "gpui.workspace = true")),
        ("source_rule", |r| {
            let dir = r.join("crates/laqta-ui/src/presenters");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("planted.rs"), "use gpui::Div;\n").map_err(|e| e.to_string())
        }),
    ];
    let baseline = check_deps(root, true)?;
    if !baseline.is_empty() {
        return Err(format!("self-test needs a clean workspace first: {baseline:?}"));
    }
    let mut failures = Vec::new();
    for (kind, plant) in cases {
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        copy_workspace(root, tmp.path())?;
        plant(tmp.path())?;
        let found = check_deps(tmp.path(), true)?;
        if found.iter().any(|v| v.kind == kind) {
            println!("self-test: planted {kind} violation caught");
        } else {
            failures.push(format!("planted {kind} violation not caught (found {found:?})"));
        }
    }
    if failures.is_empty() {
        println!("check-deps self-test: ok");
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::mentions;

    #[test]
    fn mention_detection() {
        assert!(mentions("use gpui::Div;", "gpui"));
        assert!(mentions("let x = gpui::px(1.0);", "gpui"));
        assert!(mentions("use gpui;", "gpui"));
        assert!(!mentions("use gpui_component::Button;", "gpui"));
        assert!(!mentions("// my_gpui::x", "gpui"));
        assert!(mentions("use laqta_library::Item;", "laqta_library"));
    }
}
