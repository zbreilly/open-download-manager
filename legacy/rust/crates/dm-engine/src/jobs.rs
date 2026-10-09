//! Persistent job list: survives restarts, drives the queue.
//!
//! Store format (one job per line, tab-separated):
//! `id \t state \t url \t output-path`

use crate::queue::{self, JobEvent};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Paused,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobState::Pending => "pending",
            JobState::Running => "running",
            JobState::Paused => "paused",
            JobState::Done => "done",
            JobState::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(JobState::Pending),
            "running" => Some(JobState::Running),
            "paused" => Some(JobState::Paused),
            "done" => Some(JobState::Done),
            "failed" => Some(JobState::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRecord {
    pub id: u64,
    pub state: JobState,
    pub url: String,
    pub out: PathBuf,
    /// persisted progress (set on completion; optional fields 5/6 per line)
    pub received: u64,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed job line: {0}")]
    Malformed(String),
}

pub struct JobStore {
    path: PathBuf,
    jobs: Vec<JobRecord>,
    next_id: u64,
}

impl JobStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut jobs = Vec::new();
        let mut next_id = 1u64;
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() < 4 {
                    return Err(StoreError::Malformed(line.to_string()));
                }
                let id: u64 = parts[0].parse().map_err(|_| StoreError::Malformed(line.to_string()))?;
                let state = JobState::parse(parts[1])
                    .ok_or_else(|| StoreError::Malformed(line.to_string()))?;
                // optional fields 5/6: received, total (backward compatible)
                let received: u64 =
                    parts.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
                let total: Option<u64> =
                    parts.get(5).and_then(|v| v.parse().ok());
                jobs.push(JobRecord {
                    id,
                    state,
                    url: parts[2].to_string(),
                    out: PathBuf::from(parts[3]),
                    received,
                    total,
                });
                next_id = next_id.max(id + 1);
            }
        }
        Ok(Self { path: path.to_path_buf(), jobs, next_id })
    }

    fn persist(&self) -> Result<(), StoreError> {
        let mut text = String::new();
        for j in &self.jobs {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                j.id,
                j.state.as_str(),
                j.url,
                j.out.display(),
                j.received,
                j.total.map(|t| t.to_string()).unwrap_or_else(|| "-".into())
            ));
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn add(&mut self, url: &str, out: &Path) -> Result<u64, StoreError> {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(JobRecord {
            id,
            state: JobState::Pending,
            url: url.to_string(),
            out: out.to_path_buf(),
            received: 0,
            total: None,
        });
        self.persist()?;
        Ok(id)
    }

    pub fn set_progress(
        &mut self,
        id: u64,
        received: u64,
        total: Option<u64>,
    ) -> Result<(), StoreError> {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.received = received;
            j.total = total;
        }
        self.persist()
    }

    pub fn remove(&mut self, id: u64) -> Result<(), StoreError> {
        self.jobs.retain(|j| j.id != id);
        self.persist()
    }

    pub fn requeue_failed(&mut self) -> usize {
        let mut n = 0;
        for j in &mut self.jobs {
            if j.state == JobState::Failed {
                j.state = JobState::Pending;
                n += 1;
            }
        }
        if n > 0 {
            let _ = self.persist();
        }
        n
    }

    pub fn list(&self) -> Vec<JobRecord> {
        self.jobs.clone()
    }

    /// Snapshot of Pending jobs (for fine-grained runners, e.g. a server).
    pub fn pending(&self) -> Vec<JobRecord> {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Pending)
            .cloned()
            .collect()
    }

    pub fn set_state(&mut self, id: u64, state: JobState) -> Result<(), StoreError> {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.state = state;
        }
        self.persist()
    }

    /// Run every Pending job through the queue, persisting state changes.
    /// Returns the number of jobs that were run.
    pub fn run_pending(
        &mut self,
        _parts: usize,
        max_parallel: usize,
        on_event: &mut dyn FnMut(JobEvent),
    ) -> Result<usize, StoreError> {
        let pending: Vec<JobRecord> = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Pending)
            .cloned()
            .collect();
        if pending.is_empty() {
            return Ok(0);
        }
        for j in &pending {
            self.set_state(j.id, JobState::Running)?;
        }
        let jobs: Vec<(String, PathBuf)> =
            pending.iter().map(|j| (j.url.clone(), j.out.clone())).collect();
        // queue events carry the job INDEX; map it back to the store id
        let queue_id_to_store_id: Vec<u64> = pending.iter().map(|j| j.id).collect();
        let store_path = self.path.clone();
        queue::run_jobs(jobs, max_parallel.max(1), &mut |ev| {
            // mirror terminal states into the store as they arrive
            let mapped = match &ev {
                JobEvent::Done { id, .. } => {
                    queue_id_to_store_id.get(*id as usize).map(|sid| (*sid, JobState::Done))
                }
                JobEvent::Failed { id, .. } => {
                    queue_id_to_store_id.get(*id as usize).map(|sid| (*sid, JobState::Failed))
                }
                _ => None,
            };
            if let Some((sid, state)) = mapped {
                let _ = Self::set_state_sync(&store_path, sid, state);
            }
            on_event(ev);
        });
        // reload on-disk truth back into memory
        let fresh = Self::open(&self.path)?;
        self.jobs = fresh.jobs;
        self.next_id = self.next_id.max(fresh.next_id);
        Ok(pending.len())
    }

    fn set_state_sync(path: &Path, id: u64, state: JobState) -> Result<(), StoreError> {
        let mut store = Self::open(path)?;
        store.set_state(id, state)?;
        Ok(())
    }
}
