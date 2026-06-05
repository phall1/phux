use std::process::ExitCode;

use phux_config::loader as config_loader;

use crate::commands::ConfigAction;

/// `phux config <action>` (phux-ijp). Entirely client-local: inspects
/// and scaffolds the on-disk config without contacting a server.
pub(crate) fn run_config(action: &ConfigAction) -> ExitCode {
    match action {
        ConfigAction::Path => {
            println!("{}", config_loader::config_path().display());
            ExitCode::SUCCESS
        }
        ConfigAction::Init { force } => {
            let path = config_loader::config_path();
            match phux_config::scaffold::write_reference_config(&path, *force) {
                Ok(phux_config::scaffold::ScaffoldOutcome::Wrote(p)) => {
                    println!("wrote {}", p.display());
                    ExitCode::SUCCESS
                }
                Ok(phux_config::scaffold::ScaffoldOutcome::Skipped(p)) => {
                    eprintln!(
                        "phux: {} already exists; refusing to overwrite (use --force)",
                        p.display()
                    );
                    ExitCode::FAILURE
                }
                Err(err) => {
                    eprintln!("phux: could not write config: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        ConfigAction::Show { default } => {
            // `--default` echoes the embedded defaults verbatim, comments
            // and all — the annotated source of truth. Plain `show`
            // renders the effective merged document (defaults + the user's
            // overrides) as canonical TOML.
            if *default {
                print!("{}", phux_config::DEFAULT_CONFIG_TOML);
                return ExitCode::SUCCESS;
            }
            let path = config_loader::config_path();
            let user_input = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => {
                    eprintln!("phux: could not read {}: {err}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let merged = match phux_config::merged_config_table(&user_input, &path) {
                Ok(table) => table,
                Err(err) => {
                    eprintln!("phux: {err}");
                    return ExitCode::FAILURE;
                }
            };
            match toml::to_string(&merged) {
                Ok(rendered) => {
                    print!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("phux: could not render config: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
