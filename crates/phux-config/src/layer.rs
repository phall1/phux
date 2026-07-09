//! Layered config resolution and merge (ADR-0039).
//!
//! A config file may declare a top-level `extends = ["path-or-name"]`
//! array. The effective config is an ordered stack — embedded
//! `default.toml` <- extended layers (depth-first, in listed order) <-
//! the declaring file — folded with the same recursive table merge the
//! two-layer scheme used, plus one addition: a key ending in `-append`
//! whose value is an array appends to (rather than replaces) the array
//! under the base key.
//!
//! Resolution is bounded ([`MAX_EXTENDS_DEPTH`]) and acyclic; a layer
//! reachable via two branches (diamond) is merged once, at its first
//! position. Every failure names the offending layer file.

#![allow(
    clippy::redundant_pub_crate,
    reason = "private module: items are crate-internal on purpose; plain `pub` would trip unreachable_pub"
)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{ConfigError, byte_offset_to_line_col};

/// Maximum `extends` nesting below the root config file.
///
/// The root file's layers sit at depth 1; a file at depth
/// `MAX_EXTENDS_DEPTH` may not declare `extends`. Deep enough for
/// user <- distro <- distro-base stacks with room to spare; small
/// enough that a runaway include graph fails fast.
pub const MAX_EXTENDS_DEPTH: usize = 4;

const EXTENDS_KEY: &str = "extends";
const APPEND_SUFFIX: &str = "-append";

/// Parse `input` as a plain TOML table, mapping errors to
/// [`ConfigError::Parse`] with `line:col` pointing into `input`.
pub(crate) fn parse_table(input: &str, path: &Path) -> Result<toml::Table, ConfigError> {
    toml::from_str(input).map_err(|e| {
        let (line, col) = e
            .span()
            .map_or((1, 1), |r| byte_offset_to_line_col(input, r.start));
        ConfigError::Parse {
            path: path.to_path_buf(),
            line,
            col,
            message: e.message().to_owned(),
        }
    })
}

/// Resolve the ordered layer stack rooted at `user_input` / `path`.
///
/// Returns `(layer path, table)` pairs in merge order: extended layers
/// first (depth-first, in listed order), the root file last. Each
/// table has its `extends` key consumed.
pub(crate) fn resolve_user_stack(
    user_input: &str,
    path: &Path,
) -> Result<Vec<(PathBuf, toml::Table)>, ConfigError> {
    let root = parse_table(user_input, path)?;
    let mut out = Vec::new();
    // The root is on the chain from the start, so a layer that extends
    // the user's own config file is reported as a cycle.
    let mut visiting = vec![canonical(path)];
    let mut seen = HashSet::new();
    push_layer(path, root, 0, &mut visiting, &mut seen, &mut out)?;
    Ok(out)
}

/// Depth-first post-order walk: resolve `table`'s `extends` chain into
/// `out`, then push `table` itself.
fn push_layer(
    path: &Path,
    mut table: toml::Table,
    depth: usize,
    visiting: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<(PathBuf, toml::Table)>,
) -> Result<(), ConfigError> {
    if let Some(value) = table.remove(EXTENDS_KEY) {
        if depth >= MAX_EXTENDS_DEPTH {
            return Err(ConfigError::Layer {
                path: path.to_path_buf(),
                message: format!(
                    "`extends` nesting exceeds the maximum depth of {MAX_EXTENDS_DEPTH}"
                ),
            });
        }
        for entry in extends_entries(value, path)? {
            let layer_path = resolve_entry(&entry, path);
            let layer_canon = canonical(&layer_path);
            if visiting.contains(&layer_canon) {
                return Err(ConfigError::LayerCycle {
                    layer: layer_path,
                    referenced_from: path.to_path_buf(),
                });
            }
            if !seen.insert(layer_canon.clone()) {
                // Diamond: already merged via another branch. First
                // position wins (ADR-0039).
                continue;
            }
            let contents =
                std::fs::read_to_string(&layer_path).map_err(|source| ConfigError::LayerRead {
                    layer: layer_path.clone(),
                    referenced_from: path.to_path_buf(),
                    source,
                })?;
            let layer_table = parse_table(&contents, &layer_path)?;
            visiting.push(layer_canon);
            push_layer(&layer_path, layer_table, depth + 1, visiting, seen, out)?;
            visiting.pop();
        }
    }
    out.push((path.to_path_buf(), table));
    Ok(())
}

/// Validate the `extends` value: an array of strings.
fn extends_entries(value: toml::Value, path: &Path) -> Result<Vec<String>, ConfigError> {
    let err = || ConfigError::Layer {
        path: path.to_path_buf(),
        message: "`extends` must be an array of strings (layer paths or names)".to_owned(),
    };
    let toml::Value::Array(items) = value else {
        return Err(err());
    };
    items
        .into_iter()
        .map(|item| match item {
            toml::Value::String(s) => Ok(s),
            _ => Err(err()),
        })
        .collect()
}

/// Map one `extends` entry to a layer path (ADR-0039): absolute paths
/// pass through; anything with a path separator or a `.toml` suffix is
/// relative to the declaring file's directory; a bare name `n` means
/// `layers/n.toml` beside the declaring file.
fn resolve_entry(entry: &str, declaring: &Path) -> PathBuf {
    let candidate = Path::new(entry);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    let base = declaring.parent().unwrap_or_else(|| Path::new(""));
    let has_toml_suffix = candidate
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if entry.contains(std::path::MAIN_SEPARATOR) || entry.contains('/') || has_toml_suffix {
        base.join(candidate)
    } else {
        base.join("layers").join(format!("{entry}.toml"))
    }
}

/// Canonical identity for cycle / diamond detection. Falls back to the
/// lexical path when canonicalization fails (e.g. the root path names
/// no real file, as in pure-string parses); the read step reports the
/// real error for missing layers.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Recursively merge `overlay` (from the layer file at `layer`) into
/// `base`.
///
/// Tables merge per key; any other value type — including arrays —
/// replaces wholesale. A key `x-append` holding an array appends its
/// elements to `base`'s `x` (creating it when absent) instead of
/// replacing. Misuse — appending to a non-array, a non-array append
/// value, or `x` and `x-append` in the same overlay table — is an
/// error naming `layer`.
pub(crate) fn merge_layer(
    mut base: toml::Table,
    overlay: toml::Table,
    layer: &Path,
) -> Result<toml::Table, ConfigError> {
    let layer_err = |message: String| ConfigError::Layer {
        path: layer.to_path_buf(),
        message,
    };

    // Split plain keys from `-append` directives; plain keys apply
    // first so append order is deterministic regardless of key order.
    let mut appends: Vec<(String, toml::Value)> = Vec::new();
    let mut plain = toml::Table::new();
    for (key, value) in overlay {
        match key.strip_suffix(APPEND_SUFFIX) {
            Some(target) if !target.is_empty() => appends.push((target.to_owned(), value)),
            _ => {
                plain.insert(key, value);
            }
        }
    }
    for (target, _) in &appends {
        if plain.contains_key(target) {
            return Err(layer_err(format!(
                "both `{target}` and `{target}{APPEND_SUFFIX}` are set in the same layer; \
                 use one (`{target}` replaces, `{target}{APPEND_SUFFIX}` appends)"
            )));
        }
    }

    for (key, value) in plain {
        match (base.remove(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => {
                base.insert(key, toml::Value::Table(merge_layer(b, o, layer)?));
            }
            (_, toml::Value::Table(o)) => {
                // No base table to merge into, but the overlay table
                // may still carry nested `-append` directives (e.g.
                // `[[hooks.<name>-append]]` when the base defines no
                // hooks at all); normalize them against an empty base
                // so directive keys never leak into the final table.
                base.insert(
                    key,
                    toml::Value::Table(merge_layer(toml::Table::new(), o, layer)?),
                );
            }
            (_, v) => {
                base.insert(key, v);
            }
        }
    }

    for (target, value) in appends {
        let toml::Value::Array(mut additions) = value else {
            return Err(layer_err(format!(
                "`{target}{APPEND_SUFFIX}` must be an array (it appends to the array `{target}`)"
            )));
        };
        match base.remove(&target) {
            None => {
                base.insert(target, toml::Value::Array(additions));
            }
            Some(toml::Value::Array(mut existing)) => {
                existing.append(&mut additions);
                base.insert(target, toml::Value::Array(existing));
            }
            Some(_) => {
                return Err(layer_err(format!(
                    "`{target}{APPEND_SUFFIX}` targets `{target}`, which is not an array"
                )));
            }
        }
    }

    Ok(base)
}
