mod json;
mod registry;

use std::process::ExitCode;

use crate::commands::SatelliteAction;
use json::{print_satellite_json, print_satellites_json};
use registry::{NewSatellite, add_or_update, find_entry, load_registry, remove_entry};

pub(crate) fn run_satellite(action: &SatelliteAction) -> ExitCode {
    match action {
        SatelliteAction::List { json } => run_list(*json),
        SatelliteAction::Add {
            name,
            endpoint,
            disabled,
            token_file,
            cert_fingerprint,
            json,
        } => run_add(
            NewSatellite::new(
                name,
                endpoint,
                !disabled,
                token_file.as_deref(),
                cert_fingerprint.as_deref(),
            ),
            *json,
        ),
        SatelliteAction::Remove { name, json } => run_remove(name, *json),
    }
}

fn run_list(json: bool) -> ExitCode {
    match load_registry() {
        Ok(entries) if json => print_satellites_json(&entries),
        Ok(entries) => {
            for entry in entries {
                println!("{}", describe(&entry));
            }
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

fn run_add(new: Result<NewSatellite, String>, json: bool) -> ExitCode {
    let satellite = match new {
        Ok(satellite) => satellite,
        Err(err) => return fail(&err),
    };
    match add_or_update(&satellite) {
        Ok(entry) if json => print_satellite_json("satellite", &entry),
        Ok(entry) => {
            println!("satellite {}", describe(&entry));
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

/// One human-readable line per entry. Auth material (ADR-0038) is shown by
/// reference only: the token-file *path* and the certificate fingerprint are
/// displayable, the token bytes never are (and are never read here).
fn describe(entry: &registry::SatelliteEntry) -> String {
    use std::fmt::Write as _;

    let state = if entry.enabled { "enabled" } else { "disabled" };
    let mut line = format!("{} {} ({state})", entry.name, entry.endpoint);
    if let Some(token_file) = &entry.token_file {
        let _ = write!(line, " token-file={}", token_file.display());
    }
    if let Some(fingerprint) = &entry.cert_fingerprint {
        let _ = write!(line, " cert-fingerprint={fingerprint}");
    }
    line
}

fn run_remove(name: &str, json: bool) -> ExitCode {
    let entry = match find_entry(name) {
        Ok(entry) => entry,
        Err(err) => return fail(&err),
    };
    match remove_entry(entry.index) {
        Ok(()) if json => print_satellite_json("removed", &entry),
        Ok(()) => {
            println!("removed {}", entry.name);
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("phux: {message}");
    ExitCode::FAILURE
}
