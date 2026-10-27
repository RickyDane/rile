//! Extension seam. Built-in features implement [`Extension`]; a WASM plugin
//! host (planned) will wrap guest plugins behind the same trait.

use std::path::PathBuf;

/// A command contributed to the context menu / command palette.
#[derive(Debug, Clone)]
pub struct ExtensionCommand {
    pub id: String,
    pub label: String,
}

pub trait Extension: Send + Sync {
    fn id(&self) -> &str;
    /// Commands applicable to the given selection.
    fn commands(&self, _selection: &[PathBuf]) -> Vec<ExtensionCommand> {
        Vec::new()
    }
    fn run_command(&self, _command_id: &str, _selection: &[PathBuf]) -> anyhow::Result<()> {
        Ok(())
    }
}
