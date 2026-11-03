use crate::ids::{ClipId, MarkerId, MediaId, TrackId};

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("clip {0} not found")]
    ClipNotFound(ClipId),
    #[error("track {0} not found")]
    TrackNotFound(TrackId),
    #[error("media {0} not found")]
    MediaNotFound(MediaId),
    #[error("marker {0} not found")]
    MarkerNotFound(MarkerId),
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("permission denied: {0:?} is off for this author")]
    PermissionDenied(crate::command::ToolCategory),
    #[error("the project changed since this was proposed")]
    Conflict,
    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),
}
