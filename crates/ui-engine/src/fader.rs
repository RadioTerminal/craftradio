//! Fader edits of the mixer.

use radio_engine::Command;

use crate::EngineSource;

impl EngineSource {
    /// Sends a fader change. It does not wait for the mixer.
    pub(crate) fn fader(&mut self, cmd: Command) -> Result<usize, String> {
        self.playout.send(cmd).map_err(|e| e.to_string())?;
        Ok(1)
    }
}
