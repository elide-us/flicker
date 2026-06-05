//! flicker-worker — a small, generic worker pool for off-main-thread
//! calculating tasks.
//!
//! The engine has a growing set of work that should not block a frame: mesh
//! derivation (the first user), and later nav-surface generation, streaming,
//! and physics. [`WorkerPool`] runs all of it on a fixed set of background
//! threads.
//!
//! # Why closures, not a job enum
//!
//! A job is a `Box<dyn FnOnce() + Send>` — a self-contained unit of work that
//! **captures its own inputs and routes its own output**. A mesh job, for
//! example, captures an `Arc` to the read-only source data and a `Sender` for
//! its result, does the work, and sends the result on that channel. The pool
//! only *runs* jobs; it never names a task's input or result type, so one
//! pool serves every kind of task without a central job/result enum that
//! grows with each new caller. Result routing (including best-effort /
//! generation tagging) is the caller's concern, kept out of the pool.
//!
//! # Lifecycle
//!
//! Threads are spawned in [`WorkerPool::new`] and live until the pool is
//! dropped; [`Drop`] closes the job queue (so each worker's `recv` returns
//! `Err` and the thread exits) and joins every worker, so no thread outlives
//! the pool.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// A self-contained unit of work. It captures everything it needs and routes
/// its own output; the pool just calls it on a worker thread.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed-size pool of worker threads draining a shared job queue.
///
/// Submit work with [`submit`](WorkerPool::submit). Jobs run on whichever
/// worker is free; there is no ordering guarantee between jobs. Dropping the
/// pool shuts the workers down cleanly (see the module docs).
pub struct WorkerPool {
    /// `Option` so [`Drop`] can take the sender, closing the queue. `None`
    /// only during shutdown.
    job_tx: Option<Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    /// Create a pool with `threads` workers (clamped to at least 1).
    #[must_use]
    pub fn new(threads: usize) -> Self {
        let threads = threads.max(1);
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        // std `Receiver` is single-consumer; share it across workers behind a
        // mutex. Each worker locks only long enough to pull one job, then
        // releases the lock before running it, so the work itself never
        // serialises on the queue lock.
        let job_rx = Arc::new(Mutex::new(job_rx));
        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            let job_rx = Arc::clone(&job_rx);
            workers.push(thread::spawn(move || loop {
                let job = {
                    let rx = job_rx.lock().expect("worker job queue poisoned");
                    rx.recv()
                };
                match job {
                    Ok(job) => job(),
                    Err(_) => break, // sender dropped → pool is shutting down
                }
            }));
        }
        Self {
            job_tx: Some(job_tx),
            workers,
        }
    }

    /// A pool sized to the machine, leaving one core for the main/render
    /// thread: `max(1, available_parallelism - 1)`.
    #[must_use]
    pub fn with_default_size() -> Self {
        let n = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self::new(n.saturating_sub(1).max(1))
    }

    /// Submit a job to run on a worker thread. The job captures its own
    /// inputs and routes its own output. Silently dropped if the pool has
    /// already begun shutting down.
    pub fn submit<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if let Some(tx) = &self.job_tx {
            // Send only fails if every worker has exited, which only happens
            // during shutdown — dropping the job is correct then.
            let _ = tx.send(Box::new(job));
        }
    }

    /// Number of worker threads in the pool.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        // Close the queue first: each worker's `recv` returns `Err` and the
        // thread breaks out of its loop. Then join, so no worker outlives the
        // pool (and any in-flight job finishes before we return).
        self.job_tx.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn runs_every_submitted_job() {
        let pool = WorkerPool::new(4);
        let (tx, rx) = mpsc::channel();
        for i in 0..200u32 {
            let tx = tx.clone();
            pool.submit(move || {
                tx.send(i * i).expect("result receiver alive");
            });
        }
        drop(tx); // last sender closes once every job's clone is also dropped
        let mut got: Vec<u32> = rx.iter().collect();
        got.sort_unstable();
        let want: Vec<u32> = (0..200u32).map(|i| i * i).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn new_clamps_to_at_least_one_thread() {
        assert_eq!(WorkerPool::new(0).thread_count(), 1);
        assert_eq!(WorkerPool::new(3).thread_count(), 3);
    }

    #[test]
    fn default_size_has_at_least_one_thread() {
        assert!(WorkerPool::with_default_size().thread_count() >= 1);
    }

    #[test]
    fn drop_joins_without_hanging() {
        // A job runs, then the pool is dropped: Drop must close the queue and
        // join the (idle, blocked-on-recv) workers without deadlocking.
        let pool = WorkerPool::new(2);
        let (tx, rx) = mpsc::channel();
        pool.submit(move || tx.send(42u32).unwrap());
        assert_eq!(rx.recv().unwrap(), 42);
        drop(pool);
    }
}
