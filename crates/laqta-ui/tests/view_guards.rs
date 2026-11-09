//! Input handlers in the GPUI views must not block the UI thread.
//!
//! A pointer, key or scroll handler that waits on the compute pool (`in_pool`, `block_on`) or on
//! the disk (`std::fs`) freezes the window: on a 280 Hz mouse, a full-canvas render per move kept
//! the editor "Not Responding" (2026-11-08). This test scans each view's source for the closures
//! passed to input listeners, follows `this.<fn>(` / `self.<fn>(` calls one level into the same
//! file, and fails on a blocking call. It stands in for type-level enforcement until R1 A2
//! removes `in_pool` from `laqta-ui` entirely.

use std::path::Path;

const LISTENERS: &[&str] = &[
    "on_mouse_move(",
    "on_any_mouse_down(",
    "on_any_mouse_up(",
    "on_mouse_down(",
    "on_mouse_up(",
    "on_key_down(",
    "on_scroll_wheel(",
    "on_drag_move(",
    "capture_any_mouse_up(",
    "capture_any_mouse_down(",
];

const BLOCKING: &[&str] = &["in_pool(", "block_on(", "std::fs::"];

/// Known exceptions: (file, fn, reason). The scan does not follow calls past an allowed fn.
/// Each one is an R1 A2 item: a user-requested save, not per-event work.
const ALLOWED: &[(&str, &str, &str)] = &[
    ("editor_window.rs", "save", "Ctrl+S / Enter: encodes with in_pool, writes in background_spawn; R1 A2"),
    ("editor_window.rs", "save_as", "Ctrl+Shift+S: as save; R1 A2"),
];

fn allowed(file: &str, name: &str) -> bool {
    ALLOWED.iter().any(|(f, w, _)| *f == file && *w == name)
}

/// The text from `open` (an index just past a `(` or `{`) to its matching close, exclusive.
fn balanced(src: &str, open: usize) -> &str {
    let bytes = src.as_bytes();
    let (o, c) = (bytes[open - 1], if bytes[open - 1] == b'(' { b')' } else { b'}' });
    let mut depth = 1usize;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if b == o {
            depth += 1;
        } else if b == c {
            depth -= 1;
            if depth == 0 {
                return &src[open..i];
            }
        }
        i += 1;
    }
    &src[open..]
}

/// Bodies of every closure argument passed to an input listener.
fn handler_bodies(src: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    for l in LISTENERS {
        let mut from = 0;
        while let Some(at) = src[from..].find(l) {
            let open = from + at + l.len();
            out.push((l.trim_end_matches('(').to_string(), balanced(src, open)));
            from = open;
        }
    }
    out
}

/// Names called as `this.name(` or `self.name(` in `body`.
fn called(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    for recv in ["this.", "self."] {
        let mut from = 0;
        while let Some(at) = body[from..].find(recv) {
            let start = from + at + recv.len();
            let name: String = body[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let after = start + name.len();
            if !name.is_empty() && body[after..].starts_with('(') && !names.contains(&name) {
                names.push(name);
            }
            from = start;
        }
    }
    names
}

/// The body of `fn name(` in `src`, if defined there.
fn fn_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let at = src.find(&format!("fn {name}(")).or_else(|| src.find(&format!("fn {name}<")))?;
    let brace = src[at..].find('{')? + at + 1;
    Some(balanced(src, brace))
}

/// How far calls are followed from a handler (handler → fn → fn …).
const DEPTH: usize = 4;

fn violations(file: &str, src: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut check = |chain: &str, body: &str| {
        let last = chain.rsplit(" → ").next().unwrap_or(chain);
        for tok in BLOCKING {
            if body.contains(tok) && !allowed(file, last) {
                found.push(format!("{file}: {chain}: {tok}"));
            }
        }
    };
    for (listener, body) in handler_bodies(src) {
        check(&listener, body);
        let mut seen: Vec<String> = Vec::new();
        let mut frontier: Vec<(String, &str)> = vec![(listener.clone(), body)];
        for _ in 0..DEPTH {
            let mut next = Vec::new();
            for (chain, b) in &frontier {
                for name in called(b) {
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name.clone());
                    if allowed(file, &name) {
                        continue;
                    }
                    if let Some(fb) = fn_body(src, &name) {
                        let chain = format!("{chain} → {name}");
                        check(&chain, fb);
                        next.push((chain, fb));
                    }
                }
            }
            frontier = next;
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn input_handlers_do_not_block() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut all = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.map(|e| e.path()).unwrap_or_default();
        if path.extension().is_some_and(|e| e == "rs") {
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            all.extend(violations(&file, &src));
        }
    }
    assert!(all.is_empty(), "input handlers block the UI thread:\n{}", all.join("\n"));
}

#[test]
fn scanner_catches_the_original_editor_freeze() {
    // The shape of editor_window.rs before the fix: the move handler called `input`, which waited
    // on the pool.
    let src = r#"
        root.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
            this.input(EditorInput::PointerMove { x, y }, window, cx);
        }))
        fn input(&mut self, input: EditorInput) {
            self.run(move |e| e.handle(input));
        }
        fn run(&mut self) { let out = in_pool(&self.spawner, f); }
    "#;
    assert_eq!(violations("editor_window.rs", src), vec!["editor_window.rs: on_mouse_move → input → run: in_pool(".to_string()]);
}
