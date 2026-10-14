//! Background track analysis (`docs/re/ops/analyse-track.md`): a queue served by worker threads
//! (rekordbox runs about seven in parallel). Each worker runs `mb_library::analyse_track`, which
//! decodes and analyses without touching the database; the app hands every result to the editor
//! thread as `Command::ApplyAnalysis`, which writes the files and rows.

use mb_core::DbId;
use mb_library::{AnalysisSettings, TrackAnalysis, analyse_track_with, stored_grid};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

/// One track to analyse.
#[derive(Debug, Clone)]
pub struct Job {
    pub content: DbId,
    pub audio: PathBuf,
    /// `FileNameL`, written into PPTH.
    pub file_name: String,
    pub title: String,
    /// The current `.DAT` and `djmdContent.BPM`: the grid key and phrase fall back to when
    /// high precision finds no beats.
    pub stored_dat: Option<PathBuf>,
    pub stored_bpm: Option<i64>,
}

/// A finished analysis.
pub struct Finished {
    pub content: DbId,
    pub title: String,
    pub result: Result<Box<TrackAnalysis>, String>,
}

/// What the status bar shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// Titles of the tracks being analysed.
    pub running: Vec<String>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    running: Vec<(DbId, String)>,
    done: usize,
    total: usize,
    /// Bumped by `cancel`: results of an older generation are dropped.
    generation: u64,
    shutdown: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct AnalysisQueue {
    shared: Shared,
    rx: Receiver<(u64, Finished)>,
}

impl AnalysisQueue {
    /// Starts `threads` workers (at least one).
    pub fn new(ctx: &egui::Context, settings: AnalysisSettings, threads: usize) -> Self {
        let shared: Shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let (tx, rx) = channel();
        for _ in 0..threads.max(1) {
            let (shared, tx, ctx) = (shared.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || worker(&shared, &tx, &ctx, settings));
        }
        Self { shared, rx }
    }

    /// Worker threads for this machine: one core is left to the UI; rekordbox uses about 7.
    pub fn default_threads() -> usize {
        std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).clamp(1, 7)
    }

    /// Adds jobs at the end; tracks already queued or running are skipped. Returns how many were
    /// added.
    pub fn enqueue(&self, jobs: Vec<Job>) -> usize {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap();
        let mut added = 0;
        for job in jobs {
            let known = s.queue.iter().any(|j| j.content == job.content) || s.running.iter().any(|(id, _)| *id == job.content);
            if !known {
                s.queue.push_back(job);
                s.total += 1;
                added += 1;
            }
        }
        cv.notify_all();
        added
    }

    /// Moves a queued track to the front (the selected track goes first).
    pub fn prioritize(&self, content: &DbId) {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        if let Some(i) = s.queue.iter().position(|j| j.content == *content)
            && i > 0
            && let Some(job) = s.queue.remove(i)
        {
            s.queue.push_front(job);
        }
    }

    /// Drops the queue; running analyses finish, but their results are discarded.
    pub fn cancel(&self) {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.queue.clear();
        s.generation += 1;
        s.done = 0;
        s.total = 0;
    }

    /// `None` when idle.
    pub fn progress(&self) -> Option<Progress> {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        if s.queue.is_empty() && s.running.is_empty() {
            s.done = 0;
            s.total = 0;
            return None;
        }
        Some(Progress { done: s.done, total: s.total, running: s.running.iter().map(|(_, t)| t.clone()).collect() })
    }

    /// Results finished since the last call (current generation only).
    pub fn poll(&self) -> Vec<Finished> {
        let generation = self.shared.0.lock().unwrap().generation;
        self.rx.try_iter().filter(|(g, _)| *g == generation).map(|(_, f)| f).collect()
    }
}

impl Drop for AnalysisQueue {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.shutdown = true;
        s.queue.clear();
        cv.notify_all();
    }
}

fn worker(shared: &Shared, tx: &Sender<(u64, Finished)>, ctx: &egui::Context, settings: AnalysisSettings) {
    let (lock, cv) = &**shared;
    loop {
        let (job, generation) = {
            let mut s = lock.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(job) = s.queue.pop_front() {
                    s.running.push((job.content.clone(), job.title.clone()));
                    break (job, s.generation);
                }
                s = cv.wait(s).unwrap();
            }
        };
        let stored = settings.high_precision.and(job.stored_dat.as_deref()).and_then(|dat| stored_grid(dat, job.stored_bpm));
        let result =
            analyse_track_with(&job.audio, &job.file_name, &settings, stored.as_ref()).map(Box::new).map_err(|e| e.to_string());
        {
            let mut s = lock.lock().unwrap();
            if let Some(i) = s.running.iter().position(|(id, _)| *id == job.content) {
                s.running.remove(i);
            }
            if s.generation == generation {
                s.done += 1;
            }
        }
        let _ = tx.send((generation, Finished { content: job.content, title: job.title, result }));
        ctx.request_repaint();
    }
}
