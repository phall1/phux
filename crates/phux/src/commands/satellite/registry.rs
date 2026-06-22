use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use phux_config::SatelliteConfigEntry;
use phux_config::loader as config_loader;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

#[derive(Debug, Clone)]
pub(super) struct SatelliteEntry {
    pub(super) index: usize,
    pub(super) name: String,
    pub(super) endpoint: String,
    pub(super) enabled: bool,
}

#[derive(Debug, Clone)]
pub(super) struct NewSatellite {
    name: String,
    endpoint: String,
    enabled: bool,
}

impl NewSatellite {
    pub(super) fn new(name: &str, endpoint: &str, enabled: bool) -> Result<Self, String> {
        Ok(Self {
            name: registry_name(name)?,
            endpoint: registry_endpoint(endpoint)?,
            enabled,
        })
    }
}

pub(super) fn load_registry() -> Result<Vec<SatelliteEntry>, String> {
    let cfg = config_loader::load().map_err(|err| err.to_string())?;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::with_capacity(cfg.satellites.len());
    for (index, satellite) in cfg.satellites.into_iter().enumerate() {
        if !seen.insert(satellite.name.clone()) {
            return Err(format!("duplicate satellite name {:?}", satellite.name));
        }
        entries.push(entry_from_config(index, satellite));
    }
    Ok(entries)
}

pub(super) fn find_entry(name: &str) -> Result<SatelliteEntry, String> {
    load_registry()?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| format!("satellite {name:?} is not registered"))
}

pub(super) fn add_or_update(new: &NewSatellite) -> Result<SatelliteEntry, String> {
    let config_path = config_loader::config_path();
    reject_symlinked_config(&config_path)?;
    let mut doc = read_config_document(&config_path)?;
    let mut updated = false;
    for entry in load_registry()? {
        if entry.name == new.name {
            update_entry(&mut doc, entry.index, new)?;
            updated = true;
            break;
        }
    }
    if !updated {
        push_entry(&mut doc, new)?;
    }
    write_config_document(&config_path, &doc)?;
    Ok(SatelliteEntry {
        index: 0,
        name: new.name.clone(),
        endpoint: new.endpoint.clone(),
        enabled: new.enabled,
    })
}

pub(super) fn remove_entry(index: usize) -> Result<(), String> {
    let config_path = config_loader::config_path();
    reject_symlinked_config(&config_path)?;
    let mut doc = read_config_document(&config_path)?;
    let satellites = satellite_tables_mut(&mut doc)?;
    if index >= satellites.len() {
        return Err(format!("satellite registry index {index} disappeared"));
    }
    satellites.remove(index);
    write_config_document(&config_path, &doc)
}

fn entry_from_config(index: usize, satellite: SatelliteConfigEntry) -> SatelliteEntry {
    SatelliteEntry {
        index,
        name: satellite.name,
        endpoint: satellite.endpoint,
        enabled: satellite.enabled,
    }
}

fn registry_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains(':') {
        Err("satellite name must be non-empty and must not contain '/' or ':'".to_owned())
    } else {
        Ok(trimmed.to_owned())
    }
}

fn registry_endpoint(endpoint: &str) -> Result<String, String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() || !trimmed.contains("://") {
        Err("satellite endpoint must be a URI such as ssh://devbox".to_owned())
    } else {
        Ok(trimmed.to_owned())
    }
}

fn read_config_document(config_path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(config_path) {
        Ok(input) => input
            .parse::<DocumentMut>()
            .map_err(|err| format!("could not parse {}: {err}", config_path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(err) => Err(format!("could not read {}: {err}", config_path.display())),
    }
}

fn write_config_document(config_path: &Path, doc: &DocumentMut) -> Result<(), String> {
    reject_symlinked_config(config_path)?;
    let parent = config_path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", config_path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no file name", config_path.display()))?;
    let tmp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        temp_nonce()
    ));
    let write_result = write_temp_file(&tmp_path, doc.to_string().as_bytes())
        .and_then(|()| std::fs::rename(&tmp_path, config_path).map_err(|err| err.to_string()));
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("could not write {}: {err}", config_path.display()));
    }
    Ok(())
}

fn push_entry(doc: &mut DocumentMut, new: &NewSatellite) -> Result<(), String> {
    satellite_tables_mut(doc)?.push(entry_table(new));
    Ok(())
}

fn update_entry(doc: &mut DocumentMut, index: usize, new: &NewSatellite) -> Result<(), String> {
    let table = satellite_table_mut(doc, index)?;
    table.insert("name", value(&new.name));
    table.insert("endpoint", value(&new.endpoint));
    table.insert("enabled", value(new.enabled));
    Ok(())
}

fn entry_table(new: &NewSatellite) -> Table {
    let mut table = Table::new();
    table.insert("name", value(&new.name));
    table.insert("endpoint", value(&new.endpoint));
    table.insert("enabled", value(new.enabled));
    table
}

fn satellite_table_mut(doc: &mut DocumentMut, index: usize) -> Result<&mut Table, String> {
    satellite_tables_mut(doc)?
        .get_mut(index)
        .ok_or_else(|| format!("satellite registry index {index} disappeared"))
}

fn satellite_tables_mut(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables, String> {
    let item = doc
        .as_table_mut()
        .entry("satellites")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
    item.as_array_of_tables_mut()
        .ok_or_else(|| "`satellites` must be an array of tables".to_owned())
}

fn reject_symlinked_config(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{} must not be a symlink", path.display()))
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("could not inspect {}: {err}", path.display())),
    }
}

fn temp_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn write_temp_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| err.to_string())?;
    file.write_all(bytes).map_err(|err| err.to_string())?;
    file.sync_all().map_err(|err| err.to_string())
}
