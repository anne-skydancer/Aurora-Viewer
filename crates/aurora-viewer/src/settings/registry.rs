//! A reflective, typed view of every setting by its dotted name
//! (`draw_distance`, `camera.pull_back`, `colors.tag_me`): name, kind,
//! current value, default, description, and the typed `set` / `reset` that
//! the Debug settings window drives (and, later, the quick preferences
//! editor). Aurora's counterpart of LLControlGroup's name → variable map
//! (indra/llxml/llcontrol.cpp), which LLFloaterSettingsDebug walks.
//!
//! No derive macro and no table by hand: `Settings` and its nested structs
//! already derive serde, so the registry flattens `serde_json::to_value` of
//! the settings and of `Settings::default()`. A field added anywhere in the
//! settings is a row at once. A change goes back the same way: the tree
//! patched at the name, read again as a `Settings` (serde's typing refuses
//! a wrong type, a value out of a `u8`, an unknown enum variant), with the
//! `#[serde(skip)]` runtime fields carried over (`restore_runtime`, whose
//! list a test checks against the build-time scan).
//!
//! Descriptions come from the `///` doc comments, scanned by build.rs into
//! `settings_docs.rs` (`scan`): the comment of a field is its description,
//! as the Legacy Viewer shows a setting's comment. Those comments name the
//! Legacy setting the field ports ("LanguageIsPublic"), so a search by the
//! Legacy name finds it, as LLFloaterSettingsDebug::onUpdateFilter searches
//! the comments.

use super::Settings;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One field of a struct, as build.rs scanned it.
#[derive(Debug)]
pub struct Field {
    /// Its name in the file (`#[serde(rename)]` applied).
    pub json: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    pub rust: &'static str,
    /// Its Rust type, as written.
    pub ty: &'static str,
    pub doc: &'static str,
    /// Not in the saved file (`skip`, `skip_serializing`).
    pub skipped: bool,
    /// `#[serde(skip)]`: a value of the running viewer the file never holds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub runtime: bool,
}

/// One struct of the crate, as build.rs scanned it.
#[derive(Debug)]
pub struct Struct {
    pub name: &'static str,
    pub fields: &'static [Field],
}

mod scan {
    use super::{Field, Struct};
    pub static STRUCTS: &[Struct] = include!(concat!(env!("OUT_DIR"), "/settings_docs.rs"));
}

/// Account data, never listed.
const HIDDEN: &[&str] = &["username", "mfa_hash", "remember_password"];

/// Listed, never edited here: a path, a login address, something the viewer
/// sets itself.
const READ_ONLY: &[&str] = &[
    "custom_login_uri",
    "cache_location",
    "chat_log_location",
    "gpu_adapter",
    "settings_version",
    "graphics_saved_active",
];

/// What the file never holds and a read of the patched tree would reset:
/// `restore_runtime` carries them over. `Struct.field`, as the scan names
/// them; the test `runtime_fields_are_all_restored` keeps the list current.
#[cfg(test)]
const RESTORED: &[&str] = &[
    "Settings.debug",
    "Settings.hover_height_account",
    "Settings.newer_file",
    "Settings.photo_held",
    "Settings.new_profile",
    "Settings.camera_limits_restored",
    "BuildSettings.select_probes",
    "RadarSettings.alert_sounds",
    "Overlays.land_owners",
    "Overlays.hide_particles",
];

/// The values of the running viewer that the settings structs carry
/// without saving them, put back into a `Settings` read from the tree.
fn restore_runtime(fresh: &mut Settings, old: &Settings) {
    fresh.debug = old.debug;
    fresh.hover_height_account = old.hover_height_account;
    fresh.newer_file = old.newer_file.clone();
    fresh.photo_held = old.photo_held.clone();
    fresh.new_profile = old.new_profile;
    fresh.camera_limits_restored = old.camera_limits_restored;
    fresh.build.select_probes = old.build.select_probes;
    fresh.radar.alert_sounds = old.radar.alert_sounds;
    fresh.world_overlays.land_owners = old.world_overlays.land_owners;
    fresh.world_overlays.hide_particles = old.world_overlays.hide_particles;
}

/// What a setting holds, and so which editor it takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// An integer of the Rust type's range.
    Integer {
        min: f64,
        max: f64,
    },
    Float,
    Text,
    /// An enum: its variants, as serde names them.
    Choice(Vec<String>),
    /// An `Rgba`: red, green, blue, alpha, 0 to 255.
    Color,
    /// An array of this many numbers (a camera offset…).
    Vector(usize),
    /// An unset `Option`: nothing to edit.
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Editable,
    ReadOnly,
}

/// One setting.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub value: Value,
    pub default: Value,
    pub description: &'static str,
    pub access: Access,
}

impl Entry {
    /// Differs from its default: the Legacy window's `*` column.
    pub fn modified(&self) -> bool {
        self.value != self.default
    }

    /// The Rust type of the setting (`f32`, `u8`…), empty when unknown.
    pub fn ty(&self) -> &'static str {
        info_of(&self.name).map(|f| f.ty).unwrap_or("")
    }
}

/// The settings as they are now, sorted by name.
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    pub fn of(settings: &Settings) -> Registry {
        let current = serde_json::to_value(settings).unwrap_or(Value::Null);
        let defaults = default_tree();
        let mut entries = Vec::new();
        flatten(&current, defaults, "", &mut entries);
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Registry { entries }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The entries whose name or description contains `query`, case
    /// insensitively (LLFloaterSettingsDebug::onUpdateFilter).
    pub fn matching<'a>(&'a self, query: &str) -> impl Iterator<Item = &'a Entry> + 'a {
        let query = query.trim().to_lowercase();
        self.entries()
            .iter()
            .filter(move |e| query.is_empty() || e.name.to_lowercase().contains(&query) || e.description.to_lowercase().contains(&query))
    }

    /// Sets `name` to `value` in `settings`, typed by serde: the error says
    /// why a value is refused (a wrong type, out of range, an unknown
    /// variant), in serde's words. A read-only, hidden or unknown name is
    /// refused too.
    pub fn set(settings: &mut Settings, name: &str, value: Value) -> Result<(), String> {
        if access_of(name) != Access::Editable {
            return Err(format!("{name} is read-only"));
        }
        let mut tree = serde_json::to_value(&*settings).map_err(|e| e.to_string())?;
        if !patch(&mut tree, name, value) {
            return Err(format!("unknown setting {name}"));
        }
        let mut fresh: Settings = serde_json::from_value(tree).map_err(|e| e.to_string())?;
        restore_runtime(&mut fresh, settings);
        *settings = fresh.sanitized();
        Ok(())
    }

    /// Back to the default (LLControlVariable::resetToDefault).
    pub fn reset(settings: &mut Settings, name: &str) -> Result<(), String> {
        let Some(default) = leaf(default_tree(), name).cloned() else {
            return Err(format!("unknown setting {name}"));
        };
        Self::set(settings, name, default)
    }
}

fn default_tree() -> &'static Value {
    static TREE: OnceLock<Value> = OnceLock::new();
    TREE.get_or_init(|| serde_json::to_value(Settings::default()).unwrap_or(Value::Null))
}

fn access_of(name: &str) -> Access {
    let first = name.split('.').next().unwrap_or(name);
    let last = name.rsplit('.').next().unwrap_or(name);
    if HIDDEN.contains(&first) {
        return Access::ReadOnly;
    }
    if READ_ONLY.contains(&first) {
        return Access::ReadOnly;
    }
    if ["_location", "_dir", "_folder", "_path"].iter().any(|s| last.ends_with(s)) {
        return Access::ReadOnly;
    }
    Access::Editable
}

/// The scanned field of `name`, through the structs the tree walks.
fn info_of(name: &str) -> Option<&'static Field> {
    let mut tree = default_tree();
    let mut structure = root_struct();
    let mut segments = name.split('.').peekable();
    while let Some(segment) = segments.next() {
        let field = structure?.fields.iter().find(|f| f.json == segment)?;
        tree = tree.get(segment)?;
        if segments.peek().is_none() {
            return Some(field);
        }
        structure = struct_for(field.ty, tree);
    }
    None
}

fn root_struct() -> Option<&'static Struct> {
    static ROOT: OnceLock<Option<&'static Struct>> = OnceLock::new();
    *ROOT.get_or_init(|| struct_for("Settings", default_tree()))
}

/// The scanned struct of a field typed `ty` whose value is `object`:
/// among the structs of that simple name (several are called `Settings`),
/// the one whose fields match the object's keys best.
fn struct_for(ty: &str, object: &Value) -> Option<&'static Struct> {
    let keys = object.as_object()?;
    let simple = simple_type(ty);
    scan::STRUCTS
        .iter()
        .filter(|s| s.name == simple)
        .map(|s| (s.fields.iter().filter(|f| !f.skipped && keys.contains_key(f.json)).count(), s))
        .filter(|(overlap, _)| *overlap > 0)
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, s)| s)
}

/// `Option<crate::camera::CameraSettings>` → `CameraSettings`.
fn simple_type(ty: &str) -> &str {
    let inner = ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')).unwrap_or(ty);
    inner.rsplit("::").next().unwrap_or(inner).trim()
}

fn is_map_type(ty: &str) -> bool {
    ty.contains("HashMap<") || ty.contains("BTreeMap<")
}

fn identifier_like(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_lowercase()) && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The leaves of `current` under `prefix`, with their defaults.
fn flatten(current: &Value, defaults: &Value, prefix: &str, out: &mut Vec<Entry>) {
    let Some(object) = current.as_object() else {
        return;
    };
    let structure = if prefix.is_empty() {
        root_struct()
    } else {
        info_of(prefix).and_then(|f| struct_for(f.ty, current))
    };
    for (key, value) in object {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let field = structure.and_then(|s| s.fields.iter().find(|f| f.json == key));
        let ty = field.map(|f| f.ty).unwrap_or("");
        let default = defaults.get(key).unwrap_or(&Value::Null);
        match value {
            Value::Object(inner) => {
                // a map (by account, by name) is not a list of settings
                if is_map_type(ty) || !inner.keys().all(|k| identifier_like(k)) {
                    continue;
                }
                flatten(value, default, &name, out);
            }
            Value::Array(items) => {
                if items.is_empty() || !items.iter().all(Value::is_number) {
                    continue;
                }
                let kind = if simple_type(ty) == "Rgba" && items.len() == 4 {
                    Kind::Color
                } else {
                    Kind::Vector(items.len())
                };
                out.push(entry(name, kind, value, default, field));
            }
            Value::Null => out.push(entry(name, Kind::Null, value, default, field)),
            Value::Bool(_) => out.push(entry(name, Kind::Bool, value, default, field)),
            Value::Number(n) => {
                let kind = if n.is_f64() || ty.starts_with('f') {
                    Kind::Float
                } else {
                    let (min, max) = integer_range(ty);
                    Kind::Integer { min, max }
                };
                out.push(entry(name, kind, value, default, field));
            }
            Value::String(_) => {
                let kind = match choices_of(&name, ty) {
                    Some(variants) => Kind::Choice(variants),
                    None => Kind::Text,
                };
                out.push(entry(name, kind, value, default, field));
            }
        }
    }
}

fn entry(name: String, kind: Kind, value: &Value, default: &Value, field: Option<&'static Field>) -> Entry {
    let access = if kind == Kind::Null { Access::ReadOnly } else { access_of(&name) };
    Entry {
        name,
        kind,
        value: value.clone(),
        default: default.clone(),
        description: field.map(|f| f.doc).unwrap_or(""),
        access,
    }
}

fn integer_range(ty: &str) -> (f64, f64) {
    match simple_type(ty) {
        "u8" => (0.0, f64::from(u8::MAX)),
        "u16" => (0.0, f64::from(u16::MAX)),
        "u32" => (0.0, f64::from(u32::MAX)),
        "u64" | "usize" => (0.0, u64::MAX as f64),
        "i8" => (f64::from(i8::MIN), f64::from(i8::MAX)),
        "i16" => (f64::from(i16::MIN), f64::from(i16::MAX)),
        "i32" => (f64::from(i32::MIN), f64::from(i32::MAX)),
        _ => (i64::MIN as f64, i64::MAX as f64),
    }
}

/// The variants of the enum at `name`, None for a free text. Found once
/// per name by reading the default tree with an impossible string there:
/// serde's error names the variants ("unknown variant `…`, expected one of
/// `Aurora`, `Classic`"). A `String` field is never probed.
fn choices_of(name: &str, ty: &str) -> Option<Vec<String>> {
    if simple_type(ty) == "String" {
        return None;
    }
    static CHOICES: OnceLock<parking_lot::Mutex<HashMap<String, Option<Vec<String>>>>> = OnceLock::new();
    let cache = CHOICES.get_or_init(Default::default);
    if let Some(known) = cache.lock().get(name) {
        return known.clone();
    }
    let mut tree = default_tree().clone();
    let found = if patch(&mut tree, name, Value::String("\u{1}aurora-probe".into())) {
        serde_json::from_value::<Settings>(tree)
            .err()
            .and_then(|e| variants_in(&e.to_string()))
    } else {
        None
    };
    cache.lock().insert(name.to_owned(), found.clone());
    found
}

/// The backticked names after "expected" in serde's unknown-variant message:
/// "expected one of `A`, `B`, `C`", "expected `A` or `B`", "expected `A`".
fn variants_in(message: &str) -> Option<Vec<String>> {
    let (_, after) = message.split_once("expected")?;
    let variants: Vec<String> = after
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .filter(|v| !v.is_empty())
        .collect();
    (!variants.is_empty()).then_some(variants)
}

fn leaf<'a>(tree: &'a Value, name: &str) -> Option<&'a Value> {
    name.split('.').try_fold(tree, |node, segment| node.get(segment))
}

/// Replaces the value at `name`, which must exist; false otherwise.
fn patch(tree: &mut Value, name: &str, value: Value) -> bool {
    let mut node = tree;
    let mut segments = name.split('.').peekable();
    while let Some(segment) = segments.next() {
        let Some(child) = node.get_mut(segment) else {
            return false;
        };
        if segments.peek().is_none() {
            *child = value;
            return true;
        }
        node = child;
    }
    false
}

/// A typed value from the text of an editor, by kind: what `set` is given.
pub fn parse(kind: &Kind, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match kind {
        Kind::Bool => match text.to_lowercase().as_str() {
            "true" | "1" => Ok(Value::Bool(true)),
            "false" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("{text:?} is not true or false")),
        },
        Kind::Integer { .. } => text
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| text.parse::<u64>().map(Value::from))
            .map_err(|e| e.to_string()),
        Kind::Float => text.parse::<f64>().map_err(|e| e.to_string()).and_then(|f| {
            serde_json::Number::from_f64(f)
                .map(Value::Number)
                .ok_or_else(|| "not a finite number".to_owned())
        }),
        Kind::Text | Kind::Choice(_) => Ok(Value::String(text.to_owned())),
        Kind::Color | Kind::Vector(_) => {
            let parts: Result<Vec<Value>, String> = text
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(|s| parse(&Kind::Integer { min: 0.0, max: 0.0 }, s).or_else(|_| parse(&Kind::Float, s)))
                .collect();
            parts.map(Value::Array)
        }
        Kind::Null => Err("nothing to set".to_owned()),
    }
}

/// The text an editor shows for `value`: floats through `f32` when the
/// field is one (serde writes a `f32` 0.1 as 0.10000000149011612).
pub fn display(value: &Value, ty: &str) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_text(n, ty),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(|v| display(v, ty)).collect::<Vec<_>>().join(", "),
        Value::Object(_) => value.to_string(),
    }
}

fn number_text(n: &serde_json::Number, ty: &str) -> String {
    if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
        if ty.contains("f64") {
            f.to_string()
        } else {
            (f as f32).to_string()
        }
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every top-level field of `Settings` in the file is a setting of the
    /// registry, a hidden one, or on this list of the maps and lists it
    /// leaves out: a new field of another shape fails here, to be sorted.
    #[test]
    fn every_root_field_is_listed_hidden_or_left_out_on_purpose() {
        const LEFT_OUT: &[&str] = &[
            // structs holding only lists and maps (by account, by name)
            "ao",
            "gestures",
            "keybinds",
            "particle_filters",
            "trusted_sites",
            "world_context_menus",
            // lists and maps
            "bridge",
            "camera_shots",
            "friends_only",
            "graphics_saved",
            "grids",
            // (`legacy_toolbars`, saved as `toolbar` only when not empty, is
            // absent from the default tree: a map by account when present)
            "movements",
            "render_exceptions",
        ];
        let registry = Registry::of(&Settings::default());
        let mut unsorted = Vec::new();
        for key in default_tree().as_object().expect("settings are an object").keys() {
            let listed = registry
                .entries()
                .iter()
                .any(|e| e.name == *key || e.name.starts_with(&format!("{key}.")));
            if !listed && !HIDDEN.contains(&key.as_str()) && !LEFT_OUT.contains(&key.as_str()) {
                unsorted.push(key.clone());
            }
        }
        assert!(unsorted.is_empty(), "settings neither listed nor sorted out: {unsorted:?}");
        for key in LEFT_OUT {
            assert!(
                default_tree().get(key).is_some() && !registry.entries().iter().any(|e| e.name.starts_with(&format!("{key}."))),
                "{key} is listed or gone: take it off LEFT_OUT"
            );
        }
    }

    /// The `#[serde(skip)]` fields of the structs the registry walks are
    /// exactly those `restore_runtime` carries over.
    #[test]
    fn runtime_fields_are_all_restored() {
        fn walk(tree: &Value, structure: Option<&'static Struct>, found: &mut Vec<String>) {
            let Some(structure) = structure else {
                return;
            };
            for field in structure.fields {
                if field.runtime {
                    found.push(format!("{}.{}", structure.name, field.rust));
                }
                if let Some(child) = tree.get(field.json).filter(|v| v.is_object())
                    && !is_map_type(field.ty)
                {
                    walk(child, struct_for(field.ty, child), found);
                }
            }
        }
        let mut found = Vec::new();
        walk(default_tree(), root_struct(), &mut found);
        found.sort();
        let mut expected: Vec<String> = RESTORED.iter().map(|s| (*s).to_owned()).collect();
        expected.sort();
        assert_eq!(found, expected, "update RESTORED and restore_runtime");
    }

    #[test]
    fn values_round_trip_by_kind() {
        let mut s = Settings::default();
        Registry::set(&mut s, "draw_distance", parse(&Kind::Float, "192.5").expect("a float")).expect("set");
        assert_eq!(s.draw_distance, 192.5);
        Registry::set(&mut s, "show_huds", parse(&Kind::Bool, "false").expect("a bool")).expect("set");
        assert!(!s.show_huds);
        Registry::set(
            &mut s,
            "fps_limit",
            parse(&Kind::Integer { min: 0.0, max: 255.0 }, "77").expect("an int"),
        )
        .expect("set");
        assert_eq!(s.fps_limit, 77);
        Registry::set(&mut s, "colors.tag_me", parse(&Kind::Color, "1, 2, 3, 4").expect("a colour")).expect("set");
        assert_eq!(s.colors.tag_me, [1, 2, 3, 4]);
        Registry::set(&mut s, "language", parse(&Kind::Text, "fr").expect("a text")).expect("set");
        assert_eq!(s.language, "fr");
        let registry = Registry::of(&s);
        let entry = registry.get("colors.tag_me").expect("listed");
        assert_eq!(entry.kind, Kind::Color);
        assert!(entry.modified());
        assert_eq!(display(&entry.value, entry.ty()), "1, 2, 3, 4");
        assert_eq!(
            registry.get("draw_distance").map(|e| display(&e.value, e.ty())).as_deref(),
            Some("192.5")
        );
    }

    #[test]
    fn bad_values_are_refused_and_change_nothing() {
        let mut s = Settings::default();
        let before = serde_json::to_value(&s).expect("json");
        assert!(Registry::set(&mut s, "time_of_day", Value::from(300)).is_err(), "300 is no u8");
        assert!(Registry::set(&mut s, "draw_distance", Value::String("far".into())).is_err());
        assert!(Registry::set(&mut s, "chat_style", Value::String("Nope".into())).is_err());
        assert!(Registry::set(&mut s, "no.such.setting", Value::Bool(true)).is_err());
        assert!(Registry::set(&mut s, "username", Value::String("x".into())).is_err(), "hidden");
        assert!(
            Registry::set(&mut s, "cache_location", Value::String("x".into())).is_err(),
            "read-only"
        );
        assert!(parse(&Kind::Integer { min: 0.0, max: 1.0 }, "abc").is_err());
        assert!(parse(&Kind::Bool, "maybe").is_err());
        assert_eq!(serde_json::to_value(&s).expect("json"), before);
    }

    #[test]
    fn reset_restores_the_default_and_clears_the_mark() {
        let mut s = Settings::default();
        let default = s.draw_distance;
        Registry::set(&mut s, "draw_distance", Value::from(64.0)).expect("set");
        assert!(Registry::of(&s).get("draw_distance").expect("listed").modified());
        Registry::reset(&mut s, "draw_distance").expect("reset");
        assert_eq!(s.draw_distance, default);
        assert!(!Registry::of(&s).get("draw_distance").expect("listed").modified());
    }

    #[test]
    fn runtime_values_survive_a_change() {
        let mut s = Settings::default();
        s.world_overlays.hide_particles = true;
        s.build.select_probes = true;
        s.radar.alert_sounds = true;
        Registry::set(&mut s, "show_huds", Value::Bool(false)).expect("set");
        assert!(s.world_overlays.hide_particles && s.build.select_probes && s.radar.alert_sounds);
    }

    #[test]
    fn hidden_names_are_absent_and_paths_read_only() {
        let registry = Registry::of(&Settings::default());
        for hidden in HIDDEN {
            assert!(registry.get(hidden).is_none(), "{hidden} listed");
        }
        assert_eq!(registry.get("cache_location").expect("listed").access, Access::ReadOnly);
        assert_eq!(registry.get("draw_distance").expect("listed").access, Access::Editable);
    }

    #[test]
    fn kinds_follow_the_rust_types() {
        let registry = Registry::of(&Settings::default());
        assert_eq!(registry.get("show_huds").expect("listed").kind, Kind::Bool);
        assert_eq!(registry.get("draw_distance").expect("listed").kind, Kind::Float);
        assert_eq!(
            registry.get("time_of_day").expect("listed").kind,
            Kind::Integer { min: 0.0, max: 255.0 }
        );
        assert_eq!(registry.get("language").expect("listed").kind, Kind::Text);
        let chat_style = registry.get("chat_style").expect("listed");
        let Kind::Choice(variants) = &chat_style.kind else {
            panic!("chat_style is an enum: {:?}", chat_style.kind);
        };
        assert!(
            variants.len() >= 2 && variants.iter().any(|v| chat_style.value == Value::String(v.clone())),
            "{variants:?}"
        );
    }

    #[test]
    fn descriptions_come_from_the_doc_comments_and_name_the_legacy_setting() {
        let registry = Registry::of(&Settings::default());
        let language = registry.get("language_is_public").expect("listed");
        assert!(language.description.contains("LanguageIsPublic"), "{:?}", language.description);
        assert!(registry.matching("languageispublic").any(|e| e.name == "language_is_public"));
        let nested = registry
            .entries()
            .iter()
            .filter(|e| e.name.starts_with("camera.") && !e.description.is_empty())
            .count();
        assert!(nested > 0, "nested structs keep their doc comments");
        let documented = registry.entries().iter().filter(|e| !e.description.is_empty()).count();
        assert!(
            documented * 2 > registry.entries().len(),
            "{documented} of {} described",
            registry.entries().len()
        );
    }

    #[test]
    fn serde_variant_messages_are_read() {
        assert_eq!(
            variants_in("unknown variant `x`, expected one of `Aurora`, `Classic`, `Plain`"),
            Some(vec!["Aurora".into(), "Classic".into(), "Plain".into()])
        );
        assert_eq!(
            variants_in("unknown variant `x`, expected `On` or `Off`"),
            Some(vec!["On".into(), "Off".into()])
        );
        assert_eq!(variants_in("unknown variant `x`, expected `Only`"), Some(vec!["Only".into()]));
        assert_eq!(variants_in("invalid type: string, found integer"), None);
    }

    #[test]
    fn floats_display_through_their_type() {
        let tenth = serde_json::to_value(0.1f32).expect("json");
        assert_eq!(display(&tenth, "f32"), "0.1");
        assert_eq!(display(&Value::from(3), "u8"), "3");
        assert_eq!(display(&Value::Array(vec![Value::from(1), Value::from(2)]), "Rgba"), "1, 2");
    }
}
