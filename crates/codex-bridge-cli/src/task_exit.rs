//! Helpers for handling background task exits in the foreground CLI runtime.

use anyhow::{bail, Context, Result};

/// Convert a background task result into a fatal CLI error.
pub fn background_task_exit_error(component: &str, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => bail!("{component} stopped unexpectedly"),
        Err(error) => Err(error).with_context(|| format!("{component} stopped")),
    }
}
