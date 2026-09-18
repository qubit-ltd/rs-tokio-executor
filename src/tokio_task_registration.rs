use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

pub(crate) struct TaskRegistration {
    finished: AtomicBool,
}

impl TaskRegistration {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            finished: AtomicBool::new(false),
        })
    }

    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

pub(crate) fn key(marker: &Arc<TaskRegistration>) -> usize {
    Arc::as_ptr(marker) as usize
}
