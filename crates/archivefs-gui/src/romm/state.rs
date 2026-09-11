use crate::*;

/// One RomM operation in flight.
///
/// `generation` is what makes stale traffic harmless: a progress event or a result
/// carries the generation it was started with, and anything that does not match the
/// current one is discarded. Without it, cancelling an import and starting another
/// would let the first one's late progress overwrite the second's.
pub(crate) struct RunningRommOperation {
    pub(crate) generation: u64,
    pub(crate) operation: RommOperation,
    pub(crate) cancellation: Arc<AtomicBool>,
    pub(crate) receiver: Receiver<(u64, Result<RommOperationOutcome, String>)>,
    pub(crate) progress_receiver: Receiver<(u64, RommProgressEvent)>,
    pub(crate) progress: Option<RommProgress>,
    pub(crate) cancellation_requested: bool,
}

