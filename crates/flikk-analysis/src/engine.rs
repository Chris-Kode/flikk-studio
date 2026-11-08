//! What the analysis engines that make things on threads of their own
//! (depth maps, optical flow, camera paths, subject and person masks)
//! share.

use flikk_os::sync::panic_detail;
use std::any::Any;

/// "`what` stopped unexpectedly: …" with what the panic said, or
/// "`what` stopped unexpectedly." when it said nothing: a caught panic as
/// the masks report it, a sentence the user reads.
pub(crate) fn stopped_sentence(what: &str, payload: &(dyn Any + Send)) -> String {
    match panic_detail(payload) {
        Some(said) => format!("{what} stopped unexpectedly: {said}"),
        None => format!("{what} stopped unexpectedly."),
    }
}
