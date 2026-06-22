use clap::Subcommand;

/// `phux config <action>` — local config inspection and scaffolding.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
    /// Write a commented starter config to the canonical path.
    ///
    /// The file is the shipped defaults, fully commented out: inert until
    /// you uncomment a line, so the binary's defaults stay authoritative.
    /// Refuses to overwrite an existing config unless `--force`.
    Init {
        /// Overwrite an existing config file instead of refusing.
        #[arg(long)]
        force: bool,
    },

    /// Print the resolved config path. Pure path math — prints the path
    /// whether or not the file exists.
    Path,

    /// Print the effective config (shipped defaults + your overrides) as
    /// TOML. With `--default`, print the shipped defaults verbatim
    /// instead, ignoring any user config.
    Show {
        /// Show the shipped defaults verbatim, not the merged result.
        #[arg(long)]
        default: bool,
    },

    /// List plugin manifests declared by `[[plugins]]`.
    Plugins {
        /// Emit a stable JSON document instead of human text.
        #[arg(long)]
        json: bool,
    },

    /// List agent states declared by configured plugin manifests.
    Agents {
        /// Emit a stable JSON document instead of human text.
        #[arg(long)]
        json: bool,
    },

    /// Execute one action declared by a configured plugin manifest.
    Run {
        /// Configured plugin id.
        plugin: String,

        /// Plugin-local action id.
        action: String,

        /// Give up after this many seconds. Omit to wait indefinitely.
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,

        /// Override the action cwd. Relative paths resolve under plugin root.
        #[arg(long, value_name = "PATH")]
        cwd: Option<std::path::PathBuf>,

        /// Emit the structured action result as JSON.
        #[arg(long)]
        json: bool,
    },
}
