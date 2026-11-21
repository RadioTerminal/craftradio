/// Why a device could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// No device with this name exists.
    #[error("audio device not found: {0}")]
    NotFound(String),
    /// There is no default device (or no audio host at all).
    #[error("no default audio device available")]
    NoDevice,
    /// The device offers no usable configuration.
    #[error("unsupported audio configuration: {0}")]
    Unsupported(String),
    /// The backend refused to build or start the stream.
    #[error("audio stream failed: {0}")]
    Stream(String),
    /// The audio source could not be recovered from the old stream for `reopen`.
    #[error(
        "audio source was lost with the previous stream; \
         re-create the Output with a new source"
    )]
    SourceLost,
    /// A resampler could not be built or run.
    #[error("resampler error: {0}")]
    Resample(String),
}

/// Something that happened to a device after it was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// The device went away (unplugged, daemon stopped). Call `Output::reopen`.
    Lost {
        /// Name of the lost device.
        name: String,
    },
}
