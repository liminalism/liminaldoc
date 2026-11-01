#[derive(Debug, Clone)]
pub enum ViewerEvent {
    Wake,
    FatalBackgroundError(String),
    /// Device discovery finished off the UI thread; surface creation stays on it.
    GpuPrepared(Result<(), String>),
}
