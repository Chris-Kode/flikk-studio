use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Short form keeps chat references and logs readable.
                write!(f, "{}", &self.0.simple().to_string()[..8])
            }
        }
    )*};
}

id_type!(
    ProjectId,
    MediaId,
    BinId,
    TrackId,
    ClipId,
    MarkerId,
    ChangeSetId,
    ChangeItemId,
    SpeakerId,
    RuleId,
    FactId,
    JobId,
    // One effect on a clip's stack (video or audio), unique within the clip.
    EffectId,
    // A project FX asset (shader or HTML program).
    FxId,
    // One mask on a clip.
    MaskId,
    // A program or preset in the user's library.
    LibraryId,
    // A file or folder attached to a chat for the agent to read.
    AttachmentId,
);
