//! Background track analysis (`docs/re/ops/analyse-track.md`): a queue served by worker threads
//! (rekordbox runs about seven in parallel). Each worker runs `mb_library::analyse_track`, which
//! decodes and analyses without touching the database; the app hands every result to the editor
//! thread as `Command::ApplyAnalysis`, which writes the files and rows.

use mb_core::DbId;
use mb_library::{AnalysisSettings, TrackAnalysis, analyse_track_with, stored_grid};
use std::collections::{HashSet, VecDeque};
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
    /// The user asked for it: a missing audio file is reported (automatic jobs skip it quietly).
    pub explicit: bool,
}

/// The result of a job whose audio file is missing (checked by the worker, not the UI thread).
pub const MISSING_FILE: &str = "the audio file is missing";

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
    /// Tracks queued or running (`queue` and `running` by ID), so queueing a whole collection
    /// stays linear.
    known: HashSet<DbId>,
    done: usize,
    total: usize,
    /// Bumped by `cancel`: results of an older generation are dropped.
    generation: u64,
    shutdown: bool,
}

impl State {
    /// Queues `job` unless its track is already queued or running.
    fn push(&mut self, job: Job) -> bool {
        if !self.known.insert(job.content.clone()) {
            return false;
        }
        self.queue.push_back(job);
        self.total += 1;
        true
    }

    /// Drops the queue; running jobs stay known until they finish.
    fn clear_queue(&mut self) {
        for job in self.queue.drain(..) {
            self.known.remove(&job.content);
        }
    }

    fn prioritize(&mut self, content: &DbId) {
        if !self.known.contains(content) || self.queue.front().is_some_and(|j| j.content == *content) {
            return;
        }
        if let Some(i) = self.queue.iter().position(|j| j.content == *content)
            && let Some(job) = self.queue.remove(i)
        {
            self.queue.push_front(job);
        }
    }
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
            let (shared, tx, ctx, settings) = (shared.clone(), tx.clone(), ctx.clone(), settings.clone());
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
            added += usize::from(s.push(job));
        }
        cv.notify_all();
        added
    }

    /// Moves a queued track to the front (the selected track goes first). Called every frame:
    /// cheap when the track is not queued or already first.
    pub fn prioritize(&self, content: &DbId) {
        self.shared.0.lock().unwrap().prioritize(content);
    }

    /// Analyses are queued or running.
    pub fn busy(&self) -> bool {
        !self.shared.0.lock().unwrap().known.is_empty()
    }

    /// Drops the queue; running analyses finish, but their results are discarded.
    pub fn cancel(&self) {
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.clear_queue();
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
        s.clear_queue();
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
        // Missing files are skipped (the app used to check them on the UI thread, one stat per
        // track of the collection); only explicit jobs report them.
        let present = job.audio.is_file();
        let result = if present {
            let stored = settings.high_precision.and(job.stored_dat.as_deref()).and_then(|dat| stored_grid(dat, job.stored_bpm));
            analyse_track_with(&job.audio, &job.file_name, &settings, stored.as_ref()).map(Box::new).map_err(|e| e.to_string())
        } else {
            Err(MISSING_FILE.to_owned())
        };
        {
            let mut s = lock.lock().unwrap();
            if let Some(i) = s.running.iter().position(|(id, _)| *id == job.content) {
                s.running.remove(i);
            }
            s.known.remove(&job.content);
            if s.generation == generation {
                s.done += 1;
            }
        }
        if present || job.explicit {
            let _ = tx.send((generation, Finished { content: job.content, title: job.title, result }));
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str) -> Job {
        Job {
            content: DbId(id.into()),
            audio: PathBuf::from(format!("/nonexistent/{id}.mp3")),
            file_name: format!("{id}.mp3"),
            title: id.into(),
            stored_dat: None,
            stored_bpm: None,
            explicit: false,
        }
    }

    #[test]
    fn queued_and_running_tracks_are_not_queued_twice() {
        let mut s = State::default();
        assert!(s.push(job("1")));
        assert!(s.push(job("2")));
        assert!(!s.push(job("1")), "already queued");
        // "1" starts running, as a worker takes it.
        let running = s.queue.pop_front().unwrap();
        s.running.push((running.content.clone(), running.title));
        assert!(!s.push(job("1")), "running");
        assert_eq!((s.queue.len(), s.total), (1, 2));
        // Cancelling drops the queue but not the running track.
        s.clear_queue();
        assert!(s.push(job("2")));
        assert!(!s.push(job("1")));
    }

    #[test]
    fn prioritize_moves_a_queued_track_first() {
        let mut s = State::default();
        for id in ["1", "2", "3"] {
            s.push(job(id));
        }
        s.prioritize(&DbId("3".into()));
        s.prioritize(&DbId("9".into()));
        let order: Vec<&str> = s.queue.iter().map(|j| j.content.0.as_str()).collect();
        assert_eq!(order, ["3", "1", "2"]);
    }

    #[test]
    fn missing_files_finish_quietly_unless_asked_for() {
        let ctx = egui::Context::default();
        let queue = AnalysisQueue::new(&ctx, AnalysisSettings::default(), 1);
        let asked = Job { explicit: true, ..job("asked") };
        assert_eq!(queue.enqueue(vec![job("auto"), asked, job("auto")]), 2);
        let start = std::time::Instant::now();
        let mut finished = Vec::new();
        while queue.busy() || finished.is_empty() {
            assert!(start.elapsed().as_secs() < 10, "queue did not drain");
            finished.extend(queue.poll());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        finished.extend(queue.poll());
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].content, DbId("asked".into()));
        assert_eq!(finished[0].result.as_ref().err().map(String::as_str), Some(MISSING_FILE));
    }
}
