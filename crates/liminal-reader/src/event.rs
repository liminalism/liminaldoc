#[derive(Debug, Clone)]
pub enum ViewerEvent {
    Wake,
    FatalBackgroundError(String),
    Processing(crate::processing::ProcessingUpdate),
    /// Device discovery finished off the UI thread; surface creation stays on it.
    GpuPrepared(Result<(), String>),
}
