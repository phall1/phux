#![allow(clippy::expect_used, reason = "tests")]

use std::path::PathBuf;

use phux_config::{SatelliteConfigEntry, parse_str};

fn path() -> PathBuf {
    PathBuf::from("config.toml")
}

#[test]
fn satellite_registry_entries_parse() {
    let input = r#"
[[satellites]]
name = "devbox"
endpoint = "ssh://devbox"

[[satellites]]
name = "lab"
endpoint = "quic://lab.example:8788"
enabled = false
"#;
    let cfg = parse_str(input, &path()).expect("satellite registry parses");

    assert_eq!(
        cfg.satellites,
        vec![
            SatelliteConfigEntry {
                name: "devbox".to_owned(),
                endpoint: "ssh://devbox".to_owned(),
                enabled: true,
            },
            SatelliteConfigEntry {
                name: "lab".to_owned(),
                endpoint: "quic://lab.example:8788".to_owned(),
                enabled: false,
            },
        ]
    );
}
