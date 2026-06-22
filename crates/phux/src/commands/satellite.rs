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
            json,
        } => run_add(NewSatellite::new(name, endpoint, !disabled), *json),
        SatelliteAction::Remove { name, json } => run_remove(name, *json),
    }
}

fn run_list(json: bool) -> ExitCode {
    match load_registry() {
        Ok(entries) if json => print_satellites_json(&entries),
        Ok(entries) => {
            for entry in entries {
                let state = if entry.enabled { "enabled" } else { "disabled" };
                println!("{} {} ({state})", entry.name, entry.endpoint);
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
            let state = if entry.enabled { "enabled" } else { "disabled" };
            println!("satellite {} {} ({state})", entry.name, entry.endpoint);
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
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
