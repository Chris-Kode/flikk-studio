//! From an [`FxCall`] to what the browser loads: the document after
//! `{{name}}` substitution, wrapped when it is a fragment, and the per-frame
//! state (`window.koo` and the `:root` custom properties).

use koo_core::fx::{FxAsset, ParamKind, ParamValue};
use koo_core::services::FxCall;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::hash::{DefaultHasher, Hash, Hasher};

const HEAD: &str = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n\
<style>html, body { margin: 0; width: 100%; height: 100%; overflow: hidden; background: transparent; }</style>\n\
</head><body>\n";
const TAIL: &str = "\n</body></html>\n";

/// A call, resolved for the browser.
pub struct Prepared {
    /// The page to load.
    pub doc: String,
    /// Hash of `doc`: tabs that loaded it only need to seek.
    pub doc_key: u64,
    /// Hash of everything but the time: params, duration, canvas.
    pub state_key: u64,
    /// `{ time, duration, width, height, params, vars }` for `__koo.frame`.
    pub state: Value,
    /// Lines in front of the author's code, to map error lines back.
    pub header_lines: usize,
    pub canvas: (u32, u32),
}

pub fn prepare(call: &FxCall<'_>) -> Prepared {
    let code = substitute(&call.asset.code, call.asset, call.values);
    let fragment = !is_document(&code);
    let doc = if fragment {
        format!("{HEAD}{code}{TAIL}")
    } else {
        code
    };
    let header_lines = if fragment { HEAD.lines().count() } else { 0 };
    let (params, vars) = params(call.asset, call.values);
    let (w, h) = call.canvas;
    let vars = format!(
        "{vars}--duration: {}; --width: {w}; --height: {h};",
        num(call.duration)
    );
    let mut state = json!({
        "duration": call.duration,
        "width": w,
        "height": h,
        "params": params,
        "vars": vars,
    });
    let state_key = hash(&state.to_string());
    state["time"] = json!(call.time.max(0.0));
    Prepared {
        doc_key: hash(&doc),
        doc,
        state_key,
        state,
        header_lines,
        canvas: call.canvas,
    }
}

fn is_document(code: &str) -> bool {
    let head: String = code
        .trim_start()
        .chars()
        .take(15)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype") || head.starts_with("<html")
}

pub fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `{{name}}` → the value: text HTML-escaped, images as `file://` URLs.
/// Unknown names stay as written.
fn substitute(code: &str, asset: &FxAsset, values: &BTreeMap<String, ParamValue>) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            rest = &rest[open..];
            break;
        };
        let name = after[..close].trim();
        match asset.param(name).zip(values.get(name)) {
            Some((spec, value)) => out.push_str(&inline(&spec.kind, value)),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

fn inline(kind: &ParamKind, value: &ParamValue) -> String {
    match (kind, value) {
        (ParamKind::Image, ParamValue::Text(path)) => escape(&file_url(path)),
        (ParamKind::Color, ParamValue::List(c)) => rgba(c),
        (ParamKind::Point, ParamValue::List(p)) if p.len() == 2 => {
            format!("{},{}", num(p[0]), num(1.0 - p[1]))
        }
        (_, ParamValue::Text(t)) => escape(t),
        (_, ParamValue::Number(v)) => num(*v),
        (_, ParamValue::Bool(b)) => b.to_string(),
        (_, ParamValue::List(l)) => l.iter().map(|v| num(*v)).collect::<Vec<_>>().join(","),
    }
}

/// `window.koo.params` and the `:root` declarations.
fn params(asset: &FxAsset, values: &BTreeMap<String, ParamValue>) -> (Map<String, Value>, String) {
    let mut params = Map::new();
    let mut vars = String::new();
    for spec in &asset.params {
        let value = values.get(&spec.name).unwrap_or(&spec.default);
        let name = &spec.name;
        let (js, css) = match (&spec.kind, value) {
            (ParamKind::Color, ParamValue::List(c)) => {
                let s = rgba(c);
                (json!(s), s)
            }
            (ParamKind::Point, ParamValue::List(p)) if p.len() == 2 => {
                let (x, y) = (p[0], 1.0 - p[1]);
                let _ = write!(vars, "--{name}-x: {}; --{name}-y: {}; ", num(x), num(y));
                params.insert(name.clone(), json!([x, y]));
                continue;
            }
            (ParamKind::Image, ParamValue::Text(path)) => {
                let url = file_url(path);
                let css = if url.is_empty() {
                    "none".to_owned()
                } else {
                    format!("url({})", css_string(&url))
                };
                (json!(url), css)
            }
            (_, ParamValue::Number(v)) => (json!(v), num(*v)),
            (_, ParamValue::Bool(b)) => (json!(b), if *b { "1" } else { "0" }.to_owned()),
            (_, ParamValue::Text(t)) => (json!(t), css_string(t)),
            (_, ParamValue::List(l)) => (
                json!(l),
                l.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "),
            ),
        };
        let _ = write!(vars, "--{name}: {css}; ");
        params.insert(name.clone(), js);
    }
    (params, vars)
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn rgba(c: &[f64]) -> String {
    let at = |i: usize| c.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
    format!(
        "rgba({}, {}, {}, {})",
        (at(0) * 255.0).round(),
        (at(1) * 255.0).round(),
        (at(2) * 255.0).round(),
        num(at(3))
    )
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn css_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\a "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A local path as a `file://` URL; URLs pass through, empty stays empty.
pub fn file_url(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path.contains("://") || path.starts_with("data:") {
        return path.to_owned();
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.into());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
