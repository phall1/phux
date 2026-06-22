use std::process::ExitCode;

use super::registry::SatelliteEntry;

pub(super) fn print_satellites_json(entries: &[SatelliteEntry]) -> ExitCode {
    let satellites: Vec<_> = entries.iter().map(satellite_json).collect();
    print_doc(&serde_json::json!({
        "schema_version": 1,
        "satellites": satellites
    }))
}

pub(super) fn print_satellite_json(key: &str, entry: &SatelliteEntry) -> ExitCode {
    print_doc(&serde_json::json!({
        "schema_version": 1,
        key: satellite_json(entry)
    }))
}

fn satellite_json(entry: &SatelliteEntry) -> serde_json::Value {
    serde_json::json!({
        "name": entry.name,
        "endpoint": entry.endpoint,
        "enabled": entry.enabled,
    })
}

fn print_doc(doc: &serde_json::Value) -> ExitCode {
    match serde_json::to_string_pretty(doc) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux: could not render satellite JSON: {err}");
            ExitCode::FAILURE
        }
    }
}
