//! One background job at a time, where only the newest request made while
//! it runs is worth starting next.

/// The job's state. A request is kept only while a job runs, and a newer
/// one replaces it.
#[derive(Default)]
pub enum Latest<T> {
    #[default]
    Idle,
    Running,
    /// Running, with the newest request asked for meanwhile.
    RunningWithNext(T),
}

impl<T> Latest<T> {
    /// While a job runs, keeps `request` to start after it (in place of
    /// any kept before) and returns `None`. While idle, hands `request`
    /// back for the caller to start, calling [`Self::start`].
    pub fn queue(&mut self, request: T) -> Option<T> {
        match self {
            Latest::Idle => Some(request),
            Latest::Running | Latest::RunningWithNext(_) => {
                *self = Latest::RunningWithNext(request);
                None
            }
        }
    }

    /// A job started. Whatever request was kept stays kept.
    pub fn start(&mut self) {
        if let Latest::Idle = self {
            *self = Latest::Running;
        }
    }

    /// The running job ended: idle again, handing over the request kept
    /// meanwhile, if any.
    pub fn finish(&mut self) -> Option<T> {
        match std::mem::take(self) {
            Latest::RunningWithNext(next) => Some(next),
            Latest::Idle | Latest::Running => None,
        }
    }

    /// Forgets the request kept meanwhile; a running job keeps running.
    pub fn drop_next(&mut self) {
        if let Latest::RunningWithNext(_) = self {
            *self = Latest::Running;
        }
    }
}
