//! `min_phux_version` gate: compare a plugin manifest's declared floor
//! against the running phux version at manifest load time.
//!
//! Every consumer that loads a manifest — `phux plugin link`, `phux
//! plugin install`, the best-effort [`super::load_enabled_manifests`]
//! batch used by the TUI/server, and the action runtime — routes through
//! [`super::load_plugin_manifest`], so enforcing here covers link time
//! and load time with one check.

use super::PluginManifestError;

/// The phux version plugin manifests are gated against.
///
/// All workspace crates share the single `[workspace.package]` version,
/// so this crate's own package version is the `phux` binary's version.
pub const CURRENT_PHUX_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Reject a manifest whose `min_phux_version` is newer than `current`.
///
/// The error names both versions so the user can tell at a glance
/// whether to upgrade phux or pin an older plugin.
pub(super) fn enforce_min_phux_version(
    plugin_id: &str,
    min_phux_version: &str,
    current: &str,
) -> Result<(), PluginManifestError> {
    let min = parse_version(min_phux_version).ok_or_else(|| {
        PluginManifestError::Invalid(format!(
            "plugin {plugin_id} declares malformed min_phux_version \
             {min_phux_version:?} (expected a dotted numeric version like \"0.1.0\")"
        ))
    })?;
    let have = parse_version(current).ok_or_else(|| {
        PluginManifestError::Invalid(format!(
            "current phux version {current:?} is not a dotted numeric version"
        ))
    })?;
    if min > have {
        return Err(PluginManifestError::Invalid(format!(
            "plugin {plugin_id} requires phux >= {min_phux_version}, \
             but this is phux {current}"
        )));
    }
    Ok(())
}

/// Parse `"X"`, `"X.Y"`, or `"X.Y.Z"` into a comparable triple, with
/// missing components treated as zero. Anything else — empty parts,
/// non-digits, more than three components, pre-release suffixes —
/// returns `None`.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = [0_u64; 3];
    let mut count = 0;
    for part in text.trim().split('.') {
        if count == parts.len() || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        parts[count] = part.parse().ok()?;
        count += 1;
    }
    (count > 0).then_some((parts[0], parts[1], parts[2]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::{enforce_min_phux_version, parse_version};

    #[test]
    fn parses_one_two_and_three_component_versions() {
        assert_eq!(parse_version("1"), Some((1, 0, 0)));
        assert_eq!(parse_version("0.2"), Some((0, 2, 0)));
        assert_eq!(parse_version("0.0.3"), Some((0, 0, 3)));
        assert_eq!(parse_version(" 1.2.3 "), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_malformed_versions() {
        for bad in ["", ".", "1.", ".1", "1.2.3.4", "abc", "1.x", "1.2-rc1"] {
            assert_eq!(parse_version(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn accepts_equal_and_older_minimums() {
        enforce_min_phux_version("p", "0.0.3", "0.0.3").unwrap();
        enforce_min_phux_version("p", "0.0.2", "0.0.3").unwrap();
        enforce_min_phux_version("p", "0", "0.0.3").unwrap();
    }

    #[test]
    fn rejects_newer_minimum_naming_both_versions() {
        let err = enforce_min_phux_version("example.future", "9.9.9", "0.0.3").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("example.future"), "{message}");
        assert!(message.contains("9.9.9"), "{message}");
        assert!(message.contains("0.0.3"), "{message}");
    }

    #[test]
    fn rejects_malformed_minimum_with_clear_error() {
        let err = enforce_min_phux_version("example.bad", "not-a-version", "0.0.3").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("malformed min_phux_version"), "{message}");
        assert!(message.contains("example.bad"), "{message}");
    }
}
