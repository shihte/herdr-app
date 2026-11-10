//! SiaoDesk: interface language. Every plain text element (a `&'static str`, `String` or
//! `SharedString` child) goes through `tr`, which swaps a whole string for its translation from a
//! dictionary file. Terminal content and text fields are drawn another way and are never touched.
//!
//! Dictionary: `$SIAO_LOCALE`, else `$HERDR_APP_HOME/locale.json`, else
//! `~/.config/herdr-app/locale.json`. A JSON object `{ "English": "中文", ... }`; a key with `{}`
//! is a pattern: each `{}` matches any text, which goes into the `{}`s of the value in order
//! (`"{} agents": "{} 個代理"`). Edits are picked up within two seconds.
//! `SIAO_I18N_MISSING=<file>` appends every string shown without a translation, once each.

use crate::SharedString;
use collections::HashMap;
use parking_lot::Mutex;
use std::{
    io::Write,
    path::PathBuf,
    sync::OnceLock,
    time::{Instant, SystemTime},
};

struct Dict {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    checked: Instant,
    exact: HashMap<String, SharedString>,
    patterns: Vec<(Vec<String>, Vec<String>)>,
    cache: HashMap<SharedString, Option<SharedString>>,
    missing: Option<(PathBuf, collections::HashSet<String>)>,
}

fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SIAO_LOCALE") {
        return Some(p.into());
    }
    if let Some(h) = std::env::var_os("HERDR_APP_HOME") {
        return Some(PathBuf::from(h).join("locale.json"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("herdr-app").join("locale.json"))
}

impl Dict {
    fn new() -> Self {
        let mut d = Dict {
            path: path(),
            modified: None,
            checked: Instant::now(),
            exact: HashMap::default(),
            patterns: Vec::new(),
            cache: HashMap::default(),
            missing: std::env::var_os("SIAO_I18N_MISSING").map(|p| (p.into(), Default::default())),
        };
        d.load();
        d
    }

    fn load(&mut self) {
        self.exact.clear();
        self.patterns.clear();
        self.cache.clear();
        let Some(path) = &self.path else { return };
        self.modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        for (k, v) in map {
            let Some(v) = v.as_str() else { continue };
            if v.is_empty() || k.starts_with("//") {
                continue;
            }
            if k.contains("{}") {
                let from = k.split("{}").map(str::to_owned).collect();
                let to = v.split("{}").map(str::to_owned).collect();
                self.patterns.push((from, to));
            } else {
                self.exact.insert(k, SharedString::from(v.to_owned()));
            }
        }
        // Longer patterns first: the most specific one wins.
        self.patterns.sort_by_key(|(from, _)| std::cmp::Reverse(from.iter().map(String::len).sum::<usize>()));
    }

    fn refresh(&mut self) {
        if self.checked.elapsed().as_secs() < 2 {
            return;
        }
        self.checked = Instant::now();
        let now = self.path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        if now != self.modified {
            self.load();
        }
    }

    fn lookup(&self, s: &str) -> Option<SharedString> {
        let trimmed = s.trim();
        if trimmed.is_empty() || !trimmed.bytes().any(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        if let Some(t) = self.exact.get(s) {
            return Some(t.clone());
        }
        if trimmed.len() != s.len() {
            if let Some(t) = self.exact.get(trimmed) {
                let (lead, tail) = (&s[..s.len() - s.trim_start().len()], &s[s.trim_end().len()..]);
                return Some(SharedString::from(format!("{lead}{t}{tail}")));
            }
        }
        for (from, to) in &self.patterns {
            if let Some(caps) = matches(from, s) {
                let mut out = String::new();
                for (i, part) in to.iter().enumerate() {
                    out.push_str(part);
                    if i + 1 < to.len() {
                        out.push_str(caps.get(i).map(String::as_str).unwrap_or(""));
                    }
                }
                return Some(SharedString::from(out));
            }
        }
        None
    }
}

/// `from` = literal pieces around each `{}`; returns the text each `{}` matched (shortest first).
fn matches(from: &[String], s: &str) -> Option<Vec<String>> {
    let first = from.first()?;
    let mut rest = s.strip_prefix(first.as_str())?;
    let mut caps = Vec::new();
    for (i, piece) in from.iter().enumerate().skip(1) {
        let last = i + 1 == from.len();
        let at = if last {
            if piece.is_empty() { Some(rest.len()) } else if rest.ends_with(piece.as_str()) { Some(rest.len() - piece.len()) } else { None }
        } else if piece.is_empty() {
            None
        } else {
            rest.find(piece.as_str())
        }?;
        if at == 0 {
            return None;
        }
        caps.push(rest[..at].to_owned());
        rest = &rest[at + piece.len()..];
    }
    rest.is_empty().then_some(caps)
}

static DICT: OnceLock<Mutex<Dict>> = OnceLock::new();

/// The translation of `s`, or `s` itself.
pub fn tr(s: &SharedString) -> SharedString {
    let mut d = DICT.get_or_init(|| Mutex::new(Dict::new())).lock();
    d.refresh();
    if let Some(hit) = d.cache.get(s) {
        return hit.clone().unwrap_or_else(|| s.clone());
    }
    let found = d.lookup(s);
    if found.is_none() {
        if let Some((file, seen)) = &mut d.missing {
            let key = s.to_string();
            if key.bytes().any(|b| b.is_ascii_alphabetic()) && seen.insert(key.clone()) {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&*file) {
                    let _ = writeln!(f, "{}", serde_json::Value::String(key));
                }
            }
        }
    }
    if d.cache.len() > 20_000 {
        d.cache.clear();
    }
    d.cache.insert(s.clone(), found.clone());
    found.unwrap_or_else(|| s.clone())
}

/// For a `&'static str` element.
pub fn tr_static(s: &'static str) -> SharedString {
    tr(&SharedString::new_static(s))
}
