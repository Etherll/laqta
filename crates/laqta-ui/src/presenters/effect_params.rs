//! An effect's parameters as form fields, so the effects window can show a control for each one
//! instead of raw JSON. The fields come from the effect's serialized form, so new effects and
//! parameters need no change here. Values the form can't show (long matrices, unset options) stay
//! editable as JSON.

use laqta_domain::color::Rgba;
use laqta_domain::effects::Effect;
use serde_json::{Map, Number, Value};

/// One step on the way from the effect's parameter object to a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Name(String),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// `integer` fields round to whole numbers.
    Number {
        value: f64,
        integer: bool,
        step: f64,
    },
    Toggle(bool),
    Color(Rgba),
    Choice {
        options: &'static [&'static str],
        selected: usize,
    },
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub path: Vec<Key>,
    pub label: String,
    pub kind: FieldKind,
}

/// A new value for one field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Number(f64),
    Bool(bool),
    Color(Rgba),
    Text(String),
}

/// The form for an effect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub fields: Vec<Field>,
    /// Some parameters have no field; they can only be edited as JSON.
    pub json_only: bool,
}

/// Unit-variant enums inside effect parameters, by their serialized names.
const CHOICES: [&[&str]; 5] = [
    &["Red", "Green", "Blue"],
    &["TopLeft", "TopCenter", "TopRight", "MiddleLeft", "MiddleCenter", "MiddleRight", "BottomLeft", "BottomCenter", "BottomRight"],
    &["Outside", "Inside"],
    &["Rectangle", "Ellipse", "Triangle"],
    &["Grow", "Crop"],
];

/// "TopLeft" → "Top left", for choice labels.
pub fn choice_label(option: &str) -> String {
    let mut out = String::new();
    for (i, c) in option.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// "in_black" → "In black".
fn humanize(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The effect's parameter object: `{"category":…,"effect":{"Variant":{…}}}` → the inner object.
/// Unit variants serialize as a bare string and have none.
fn params(root: &Value) -> Option<&Map<String, Value>> {
    root.get("effect")?.as_object()?.values().next()?.as_object()
}

fn params_mut(root: &mut Value) -> Option<&mut Map<String, Value>> {
    root.get_mut("effect")?.as_object_mut()?.values_mut().next()?.as_object_mut()
}

fn as_color(v: &Value) -> Option<Rgba> {
    let o = v.as_object()?;
    if o.len() != 4 {
        return None;
    }
    let ch = |k: &str| o.get(k)?.as_u64().and_then(|n| u8::try_from(n).ok());
    Some(Rgba { r: ch("r")?, g: ch("g")?, b: ch("b")?, a: ch("a")? })
}

/// Parameters on a 0..1 scale (strengths, opacities, offsets); they step by 0.05.
const UNIT_KEYS: [&str; 12] =
    ["strength", "alpha", "add", "clip", "grain", "balance", "temperature", "tint", "opacity", "shadows", "highlights", "amount"];

/// How far one click of the stepper moves a number. Ranges live in the effect docs, not the data,
/// so this goes by the parameter name and then by the size of the value.
fn step_for(key: &str, value: f64, integer: bool) -> f64 {
    if integer {
        1.0
    } else if UNIT_KEYS.contains(&key) {
        0.05
    } else if matches!(key, "stops" | "gamma") {
        0.1
    } else if value != 0.0 && value.abs() <= 2.0 {
        0.05
    } else {
        1.0
    }
}

fn number_field(key: &str, n: &Number) -> FieldKind {
    let value = n.as_f64().unwrap_or(0.0);
    let integer = !n.is_f64();
    FieldKind::Number { value, integer, step: step_for(key, value, integer) }
}

/// Labels for the items of a short array: coordinates, colour channels, or numbers.
fn item_labels(key: &str, len: usize) -> Vec<String> {
    match len {
        2 if key.contains("size") => vec!["width".into(), "height".into()],
        2 => vec!["X".into(), "Y".into()],
        3 if matches!(key, "shadows" | "midtones" | "highlights") => vec!["red".into(), "green".into(), "blue".into()],
        n => (1..=n).map(|i| i.to_string()).collect(),
    }
}

fn walk(value: &Value, path: Vec<Key>, label: String, key: &str, form: &mut Form) {
    let kind = match value {
        Value::Number(n) => number_field(key, n),
        Value::Bool(b) => FieldKind::Toggle(*b),
        Value::String(s) => match CHOICES.iter().find(|set| set.contains(&s.as_str())) {
            Some(options) => FieldKind::Choice { options, selected: options.iter().position(|o| o == s).unwrap_or(0) },
            None => FieldKind::Text(s.clone()),
        },
        Value::Object(o) => {
            if let Some(c) = as_color(value) {
                FieldKind::Color(c)
            } else {
                for (k, v) in o {
                    // A nested struct's fields read as their own names; an enum payload keeps the
                    // parent's label in front ("Size mode: Percentage").
                    let child = if k.chars().next().is_some_and(char::is_uppercase) {
                        format!("{label}: {}", choice_label(k).to_lowercase())
                    } else {
                        humanize(k)
                    };
                    let mut p = path.clone();
                    p.push(Key::Name(k.clone()));
                    walk(v, p, child, k, form);
                }
                return;
            }
        }
        Value::Array(items) if items.len() <= 4 => {
            let labels = item_labels(key, items.len());
            for (i, (v, l)) in items.iter().zip(labels).enumerate() {
                let mut p = path.clone();
                p.push(Key::Index(i));
                walk(v, p, format!("{label} {l}"), key, form);
            }
            return;
        }
        // Long arrays (matrices) and unset options.
        Value::Array(_) | Value::Null => {
            form.json_only = true;
            return;
        }
    };
    form.fields.push(Field { path, label, kind });
}

/// The fields for an effect's parameters, in declaration order.
pub fn form(effect: &Effect) -> Form {
    let mut form = Form::default();
    let Ok(root) = serde_json::to_value(effect) else { return form };
    if let Some(p) = params(&root) {
        for (k, v) in p {
            walk(v, vec![Key::Name(k.clone())], humanize(k), k, &mut form);
        }
    }
    form
}

fn to_json(value: FieldValue, old: &Value) -> Value {
    match value {
        FieldValue::Number(n) => {
            // A negative value for an unsigned parameter fails to deserialize, so `set` rejects it.
            if old.is_u64() || old.is_i64() {
                if n < 0.0 { Value::from(n.round() as i64) } else { Value::from(n.round() as u64) }
            } else {
                Number::from_f64(n).map(Value::Number).unwrap_or(Value::from(0))
            }
        }
        FieldValue::Bool(b) => Value::Bool(b),
        FieldValue::Color(c) => serde_json::json!({ "r": c.r, "g": c.g, "b": c.b, "a": c.a }),
        FieldValue::Text(s) => Value::String(s),
    }
}

/// The effect with one field changed, or `None` when the value doesn't fit the parameter (such
/// as 300 for a value that must fit a byte).
pub fn set(effect: &Effect, path: &[Key], value: FieldValue) -> Option<Effect> {
    let mut root = serde_json::to_value(effect).ok()?;
    let (first, rest) = path.split_first()?;
    let Key::Name(first) = first else { return None };
    let mut slot = params_mut(&mut root)?.get_mut(first)?;
    for k in rest {
        slot = match k {
            Key::Name(n) => slot.get_mut(n)?,
            Key::Index(i) => slot.get_mut(*i)?,
        };
    }
    *slot = to_json(value, slot);
    serde_json::from_value(root).ok()
}

/// The effect as editable JSON, for parameters the form can't show.
pub fn to_text(effect: &Effect) -> String {
    serde_json::to_string_pretty(effect).unwrap_or_default()
}

pub fn from_text(text: &str) -> Result<Effect, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// `#RRGGBB`, or `#RRGGBBAA` when not opaque.
pub fn hex(c: Rgba) -> String {
    if c.a == 255 { format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b) } else { format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a) }
}

/// Parses `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the `#` is optional).
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    match s.len() {
        3 => {
            let n = |i: usize| u8::from_str_radix(s.get(i..i + 1)?, 16).ok().map(|v| v * 17);
            Some(Rgba { r: n(0)?, g: n(1)?, b: n(2)?, a: 255 })
        }
        6 => Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: 255 }),
        8 => Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: byte(6)? }),
        _ => None,
    }
}

/// The unit after a number, from its parameter name: pixels for sizes and offsets, degrees for
/// angles.
pub fn unit(field: &Field) -> Option<&'static str> {
    let key = field.path.iter().rev().find_map(|k| match k {
        Key::Name(n) => Some(n.as_str()),
        Key::Index(_) => None,
    })?;
    if !matches!(field.kind, FieldKind::Number { .. }) {
        return None;
    }
    if key == "angle" || key.ends_with("_angle") {
        Some("°")
    } else if ["size", "width", "height", "radius", "padding", "margin", "offset", "thickness", "spacing"]
        .iter()
        .any(|w| key == *w || key.ends_with(&format!("_{w}")) || key.starts_with(&format!("{w}_")))
    {
        Some("px")
    } else {
        None
    }
}

/// The name the window shows. Some effects keep ShareX's identifiers ("BackgroundImage"); those
/// read as words here.
pub fn display_name(effect: &Effect) -> String {
    match effect.name() {
        "TextEx" => "Text (advanced)".into(),
        n if !n.contains(' ') && n.chars().skip(1).any(char::is_uppercase) => {
            let words = choice_label(n);
            let mut c = words.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        n => n.into(),
    }
}

/// The effect's category name and a short line saying how many parameters it has.
pub fn summary(effect: &Effect) -> String {
    let category = match effect {
        Effect::Adjust(_) => "Adjustment",
        Effect::Draw(_) => "Drawing",
        Effect::Filter(_) => "Filter",
        Effect::Manip(_) => "Manipulation",
    };
    let n = form(effect).fields.len();
    match n {
        0 => category.to_string(),
        1 => format!("{category} · 1 setting"),
        n => format!("{category} · {n} settings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use laqta_domain::effects::{Adjust, Draw, Filter, Manip};

    #[test]
    fn number_and_color_fields() {
        let e = Effect::Adjust(Adjust::Colorize { color: Rgba::rgb(1, 2, 3), value: 0.5 });
        let f = form(&e);
        assert!(!f.json_only);
        assert_eq!(f.fields.len(), 2);
        assert!(f.fields.iter().any(|x| x.label == "Color" && x.kind == FieldKind::Color(Rgba::rgb(1, 2, 3))));
        assert!(f.fields.iter().any(|x| x.label == "Value" && matches!(x.kind, FieldKind::Number { integer: false, .. })));
    }

    #[test]
    fn set_writes_back_and_rejects_values_that_dont_fit() {
        let e = Effect::Adjust(Adjust::Threshold { level: 10 });
        let path = [Key::Name("level".into())];
        assert_eq!(set(&e, &path, FieldValue::Number(99.4)), Some(Effect::Adjust(Adjust::Threshold { level: 99 })));
        assert_eq!(set(&e, &path, FieldValue::Number(300.0)), None);
        let f = form(&e);
        assert!(matches!(f.fields[0].kind, FieldKind::Number { integer: true, .. }));
        assert_eq!(set(&e, &path, FieldValue::Number(-1.0)), None);
    }

    #[test]
    fn choices_and_arrays() {
        let e = Effect::Adjust(Adjust::ChannelSwap {
            red: laqta_domain::effects::adjust::Channel::Blue,
            green: laqta_domain::effects::adjust::Channel::Green,
            blue: laqta_domain::effects::adjust::Channel::Red,
        });
        let f = form(&e);
        assert_eq!(f.fields[0].kind, FieldKind::Choice { options: CHOICES[0], selected: 2 });
        let changed = set(&e, &f.fields[0].path, FieldValue::Text("Red".into())).unwrap();
        assert!(matches!(changed, Effect::Adjust(Adjust::ChannelSwap { red: laqta_domain::effects::adjust::Channel::Red, .. })));

        let line = Effect::Draw(Draw::DrawLine { from: (0, 0), to: (5, 5), width: 1, color: Rgba::BLACK });
        let f = form(&line);
        assert_eq!(
            set(&line, &f.fields[0].path, FieldValue::Number(-3.0)).map(|e| form(&e).fields[0].kind.clone()),
            Some(FieldKind::Number { value: -3.0, integer: true, step: 1.0 })
        );
        assert!(f.fields.iter().any(|x| x.label == "From X"));
        assert!(f.fields.iter().any(|x| x.label == "To Y"));
    }

    #[test]
    fn every_default_effect_has_a_form_that_writes_back() {
        let all = Adjust::defaults()
            .into_iter()
            .map(Effect::Adjust)
            .chain(Draw::defaults().into_iter().map(Effect::Draw))
            .chain(Filter::defaults().into_iter().map(Effect::Filter))
            .chain(Manip::defaults().into_iter().map(Effect::Manip));
        for e in all {
            for f in form(&e).fields {
                let v = match &f.kind {
                    FieldKind::Number { value, .. } => FieldValue::Number(*value),
                    FieldKind::Toggle(b) => FieldValue::Bool(*b),
                    FieldKind::Color(c) => FieldValue::Color(*c),
                    FieldKind::Choice { options, selected } => FieldValue::Text(options[*selected].to_string()),
                    FieldKind::Text(s) => FieldValue::Text(s.clone()),
                };
                assert_eq!(set(&e, &f.path, v).as_ref(), Some(&e), "{} {}", e.name(), f.label);
            }
        }
    }

    #[test]
    fn units_come_from_names() {
        let f = form(&Effect::Draw(Draw::Border { kind: laqta_domain::effects::draw::BorderKind::Outside, size: 4, color: Rgba::BLACK, dash: false }));
        assert_eq!(unit(&f.fields[1]), Some("px"));
        assert_eq!(unit(&f.fields[0]), None);
        let f = form(&Effect::Adjust(Adjust::Hue { angle: 0.0 }));
        assert_eq!(unit(&f.fields[0]), Some("°"));
    }

    #[test]
    fn steps_follow_the_parameter() {
        assert_eq!(step_for("value", 0.0, false), 1.0);
        assert_eq!(step_for("strength", 0.0, false), 0.05);
        assert_eq!(step_for("stops", 0.5, false), 0.1);
        assert_eq!(step_for("radius", 8.0, true), 1.0);
    }

    #[test]
    fn matrices_are_json_only() {
        let f = form(&Effect::Adjust(Adjust::MatrixColor { matrix: [0.0; 20] }));
        assert!(f.json_only && f.fields.is_empty());
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("#F0a"), Some(Rgba::rgb(0xFF, 0x00, 0xAA)));
        assert_eq!(parse_hex(&hex(Rgba { r: 1, g: 2, b: 3, a: 4 })), Some(Rgba { r: 1, g: 2, b: 3, a: 4 }));
        assert_eq!(hex(Rgba::rgb(255, 0, 16)), "#FF0010");
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(choice_label("TopLeft"), "Top left");
        assert_eq!(display_name(&Effect::Manip(Manip::defaults().into_iter().find(|m| m.name() == "AutoCrop").unwrap())), "Auto crop");
        assert_eq!(display_name(&Effect::Adjust(Adjust::BlackWhite)), "Black & White");
        let _ = Manip::defaults();
    }
}
