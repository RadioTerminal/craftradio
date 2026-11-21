//! The Icecast settings of the station.

use radio_ui_egui::view::IcecastView;

use crate::EngineSource;

impl EngineSource {
    pub(crate) fn set_icecast(&mut self, _ice: IcecastView) -> Result<usize, String> {
        Err("Icecast settings are not in this build yet".into())
    }
}
