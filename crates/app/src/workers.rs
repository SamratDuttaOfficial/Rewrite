//! Background work for the UI. A [`Lane`] is one thread that runs only the most recent job given to it,
//! and hands the result back on the UI thread. Debounced calls ([`later`]) and messages from other
//! threads ([`post`]) also land on the UI thread.

use gtk4::glib;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

type Job = Box<dyn FnOnce() + Send>;

struct Slot {
    job: Mutex<Option<Job>>,
    wake: Condvar,
    /// A job is running or waiting.
    active: AtomicBool,
}

#[derive(Clone)]
pub struct Lane {
    slot: Arc<Slot>,
}

impl Lane {
    pub fn new(name: &str) -> Lane {
        let slot = Arc::new(Slot { job: Mutex::new(None), wake: Condvar::new(), active: AtomicBool::new(false) });
        let s = slot.clone();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || loop {
                let job = {
                    let mut g = s.job.lock().unwrap();
                    while g.is_none() {
                        g = s.wake.wait(g).unwrap();
                    }
                    g.take().unwrap()
                };
                job();
                let g = s.job.lock().unwrap();
                s.active.store(g.is_some(), Ordering::SeqCst);
            })
            .expect("a worker thread");
        Lane { slot }
    }

    /// Run `job` on this lane, replacing a job that has not started yet; `done` gets its result (or the
    /// panic message) on the UI thread.
    pub fn submit<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(Result<R, String>) + 'static,
    ) {
        let done = glib::thread_guard::ThreadGuard::new(done);
        let wrapped: Job = Box::new(move || {
            let res = catch_unwind(AssertUnwindSafe(job)).map_err(panic_text);
            glib::MainContext::default().invoke(move || (done.into_inner())(res));
        });
        let mut g = self.slot.job.lock().unwrap();
        *g = Some(wrapped);
        self.slot.active.store(true, Ordering::SeqCst);
        self.slot.wake.notify_one();
    }

    /// Run a job whose result nobody waits for.
    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        self.submit(job, |_| {});
    }

    /// A handle a job can poll from its own thread (to yield to other lanes).
    pub fn activity(&self) -> LaneActivity {
        LaneActivity(self.slot.clone())
    }
}

#[derive(Clone)]
pub struct LaneActivity(Arc<Slot>);

impl LaneActivity {
    pub fn active(&self) -> bool {
        self.0.active.load(Ordering::SeqCst)
    }
}

pub fn panic_text(p: Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "an internal error".to_string()
    }
}

/// Run `f` on the UI thread (from any thread).
pub fn post(f: impl FnOnce() + Send + 'static) {
    glib::MainContext::default().invoke(f);
}

thread_local! {
    static PENDING: RefCell<HashMap<String, glib::SourceId>> = RefCell::new(HashMap::new());
}

/// Debounce: run `f` after `ms`, cancelling an earlier call with the same name.
pub fn later(name: &str, ms: u32, f: impl FnOnce() + 'static) {
    cancel(name);
    let key = name.to_string();
    let k2 = key.clone();
    let id = glib::timeout_add_local_once(std::time::Duration::from_millis(ms as u64), move || {
        PENDING.with(|p| p.borrow_mut().remove(&k2));
        f();
    });
    PENDING.with(|p| p.borrow_mut().insert(key, id));
}

pub fn cancel(name: &str) {
    if let Some(id) = PENDING.with(|p| p.borrow_mut().remove(name)) {
        id.remove();
    }
}

/// A flag a long job checks to stop early.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// The flag itself, for libraries that take an AtomicBool.
    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
    pub fn same(&self, other: &Cancel) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
