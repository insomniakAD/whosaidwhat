//! Background pipeline worker: keeps `process_recording` off the detection
//! thread (the known limitation flagged in docs/00 §3 and main.rs).
//!
//! Shape: one dedicated worker thread draining an unbounded FIFO channel.
//! Jobs are minutes long (whisper on an hour of audio), arrive at most once
//! per meeting, and must run sequentially anyway — whisper.cpp/Metal and the
//! diarizer should not run twice concurrently on one machine, and SQLite's
//! WAL allows exactly one writer at a time. A thread + `std::sync::mpsc` is
//! the whole requirement; anything fancier is overhead.
//!
//! Concurrency discipline: the worker OWNS its DB connection, router, and
//! tokio runtime (constructed by `init` inside the thread), so nothing is
//! shared with the detection thread but the channel. The detection loop's
//! writes (meeting row creation) and the worker's writes (segments/summaries)
//! interleave safely under WAL + the 5 s busy_timeout set in db::Store::open.
//!
//! Dependency-free (std only) so it runs under the bare-rustc harness
//! (BUILD_LOG D-006).

use std::sync::mpsc::{channel, Sender};
use std::thread::JoinHandle;

/// One saved recording awaiting processing. Carries everything the pipeline
/// needs so the worker never touches detection-side state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub meeting_id: String,
    pub system_wav: std::path::PathBuf,
    pub mic_wav: Option<std::path::PathBuf>,
}

/// Handle owned by the detection thread. Dropping it closes the channel; the
/// worker finishes the jobs already queued, then exits (join happens in
/// `Drop`, so process shutdown never truncates a summary mid-write).
pub struct PipelineWorker {
    tx: Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl PipelineWorker {
    /// Spawn the worker. `init` runs once ON the worker thread and builds its
    /// private state (DB connection, router, runtime — none of which need to
    /// be `Send` past this boundary); `process` runs per job, in FIFO order.
    pub fn spawn<S, I, F>(init: I, mut process: F) -> std::io::Result<Self>
    where
        S: 'static,
        I: FnOnce() -> S + Send + 'static,
        F: FnMut(&mut S, Job) + Send + 'static,
    {
        let (tx, rx) = channel::<Job>();
        let handle = std::thread::Builder::new().name("wsw-pipeline".into()).spawn(move || {
            let mut state = init();
            // `for` over a Receiver blocks until a job arrives and ends when
            // every Sender is dropped — the natural drain-then-exit loop.
            for job in rx {
                process(&mut state, job);
            }
        })?;
        Ok(PipelineWorker { tx: Some(tx), handle: Some(handle) })
    }

    /// Queue a job; returns immediately. `false` only if the worker thread
    /// died (its receiver is gone) — the caller should log and surface that,
    /// since recordings would otherwise silently pile up unprocessed.
    pub fn submit(&self, job: Job) -> bool {
        match &self.tx {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        }
    }
}

impl Drop for PipelineWorker {
    fn drop(&mut self) {
        // Close the channel first (or `for job in rx` never ends), then wait
        // for in-flight + queued jobs to finish.
        drop(self.tx.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn job(id: &str) -> Job {
        Job { meeting_id: id.into(), system_wav: "/tmp/x.system.wav".into(), mic_wav: None }
    }

    #[test]
    fn processes_jobs_fifo_with_thread_local_state() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let worker = PipelineWorker::spawn(
            // init state proves per-thread construction works (a counter here;
            // a Store/Runtime in main.rs).
            || 0usize,
            move |count, job| {
                *count += 1;
                seen2.lock().unwrap().push(format!("{}#{count}", job.meeting_id));
            },
        )
        .unwrap();
        assert!(worker.submit(job("a")));
        assert!(worker.submit(job("b")));
        assert!(worker.submit(job("c")));
        drop(worker); // joins: all queued jobs must have run
        assert_eq!(*seen.lock().unwrap(), vec!["a#1", "b#2", "c#3"]);
    }

    #[test]
    fn submit_does_not_block_on_a_busy_worker() {
        // The worker parks on the first job until we release it; meanwhile the
        // "detection thread" must be able to queue more jobs instantly.
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let gate2 = gate.clone();
        let processed = Arc::new(AtomicUsize::new(0));
        let processed2 = processed.clone();

        let worker = PipelineWorker::spawn(
            || (),
            move |_, _| {
                let (lock, cvar) = &*gate2;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cvar.wait(open).unwrap();
                }
                processed2.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();

        let t0 = std::time::Instant::now();
        for i in 0..100 {
            assert!(worker.submit(job(&format!("m{i}"))));
        }
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(500),
            "submit must be queue-and-return, not wait-for-processing"
        );
        assert_eq!(processed.load(Ordering::SeqCst), 0, "worker still gated");

        let (lock, cvar) = &*gate;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        drop(worker); // drains the queue before joining
        assert_eq!(processed.load(Ordering::SeqCst), 100);
    }

    #[test]
    fn drop_with_empty_queue_exits_cleanly() {
        let worker = PipelineWorker::spawn(|| (), |_, _: Job| {}).unwrap();
        drop(worker);
    }
}
