//! Every file the app pulls off the network, in one list.
//!
//! Before this there were two: the GGUF paste box owned a handle, a `.part` and
//! a byte count, and Studio's Wan installer owned the same three again plus a
//! queue it drained one file at a time. Two copies of the same state machine
//! means two places to get cancel-sweeps wrong, and neither could show the
//! other's transfers — so a screen could only report the download it started.
//!
//! What this adds over the pair it replaces: [`LIMIT`] files at once instead of
//! strictly one, a named bar per file rather than one aggregate percentage, and
//! [`Action::Promote`] to pull a queued file to the front. The engine underneath
//! is unchanged — [`crate::model_download`] still streams a single URL, and this
//! module is the thing that runs several of them and remembers which is which.
//!
//! Deliberately not here: resume (needs a `Range` request and a `.part` size
//! check — worth doing the first time a 10 GB transfer actually dies), speed and
//! ETA, and hash verification.

use crate::model_download::{self, Progress};
use crate::Message;
use iced::Task;
use std::path::{Path, PathBuf};

/// How many transfers run at once. Three, because a single stream rarely fills
/// a domestic line and ten would only make each one slower and the list
/// unreadable. Not a setting until someone's link says it should be.
pub const LIMIT: usize = 3;

/// Who asked for the file, which is how a finished job finds its follow-up:
/// a GGUF becomes the selected local model, a Wan file makes Studio re-ask
/// ComfyUI what it can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    LocalModel,
    MediaModel,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed(String),
}

pub struct Job {
    pub id: u64,
    /// What the row is called — the file name, since that is what the model
    /// card quoted and what ComfyUI will look for.
    pub label: String,
    pub tag: Tag,
    url: String,
    dir: PathBuf,
    file: String,
    pub received: u64,
    /// The size the caller knew before the press, replaced by the transfer's
    /// own `Content-Length` once there is one.
    pub total: Option<u64>,
    pub status: Status,
    /// Aborts the transfer. Dropping the stream is the whole of cancel — it
    /// drops mid-write, so [`sweep`] has to remove the `.part` by hand.
    handle: Option<iced::task::Handle>,
}

impl Job {
    /// `0.0`–`1.0` for this file alone. A job with no known length reports 0
    /// rather than guessing; the byte count beside the bar still moves.
    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(total) if total > 0 => (self.received as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn active(&self) -> bool {
        matches!(self.status, Status::Queued | Status::Running)
    }

    fn part(&self) -> PathBuf {
        model_download::part_path(&self.dir, &self.file)
    }
}

/// A row's buttons, as messages. One enum rather than three variants on
/// `Message` so an embedded panel can be handed a single mapper.
#[derive(Debug, Clone)]
pub enum Action {
    Cancel(u64),
    /// Move a queued file to the front of the queue. Reprioritising while it is
    /// already running would mean restarting it, which is not what the press
    /// means, so a running job ignores this.
    Promote(u64),
    Retry(u64),
    /// Forget everything finished or failed. Nothing running is touched.
    Clear,
}

#[derive(Default)]
pub struct Downloads {
    pub jobs: Vec<Job>,
    next_id: u64,
}

/// What a finished job hands back so its caller can do the follow-up.
pub struct Finished {
    pub tag: Tag,
    pub path: String,
}

impl Downloads {
    /// Queue a file, unless the same destination is already queued or running —
    /// two streams writing one `.part` is a corrupt file, and the second press
    /// of a button is almost always an impatient one rather than a new request.
    pub fn enqueue(&mut self, tag: Tag, url: String, dir: PathBuf, file: String) -> Option<u64> {
        if self.jobs.iter().any(|j| j.active() && j.dir == dir && j.file == file) {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(Job {
            id,
            label: file.clone(),
            tag,
            url,
            dir,
            file,
            received: 0,
            total: None,
            status: Status::Queued,
            handle: None,
        });
        Some(id)
    }

    /// Give the job an expected size before a byte of it arrives, so the bar
    /// reads properly from the first tick instead of sitting at zero until the
    /// server's `Content-Length` shows up.
    pub fn expect_bytes(&mut self, id: u64, bytes: Option<u64>) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.total = bytes;
        }
    }

    pub fn running(&self) -> usize {
        self.jobs.iter().filter(|j| j.status == Status::Running).count()
    }

    pub fn active(&self) -> usize {
        self.jobs.iter().filter(|j| j.active()).count()
    }

    pub fn by_tag(&self, tag: Tag) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(move |j| j.tag == tag)
    }

    /// Start queued jobs until [`LIMIT`] are in flight. Idempotent and cheap —
    /// called after every enqueue and every terminal progress, which is what
    /// keeps the pipe full without anything tracking whose turn it is.
    pub fn pump(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        let mut running = self.running();
        for job in self.jobs.iter_mut() {
            if running >= LIMIT {
                break;
            }
            if job.status != Status::Queued {
                continue;
            }
            let id = job.id;
            let (task, handle) = Task::stream(model_download::download(
                job.url.clone(),
                job.dir.clone(),
                job.file.clone(),
            ))
            .map(move |p| Message::Download(id, p))
            .abortable();
            job.handle = Some(handle);
            job.status = Status::Running;
            running += 1;
            tasks.push(task);
        }
        Task::batch(tasks)
    }

    /// Fold one tick in. Returns the file that just landed, if this was the
    /// message that landed it — the caller owns what happens next, because
    /// "point the settings at it" and "ask ComfyUI again" are not this module's
    /// business.
    pub fn progressed(&mut self, id: u64, progress: Progress) -> Option<Finished> {
        let job = self.jobs.iter_mut().find(|j| j.id == id)?;
        match progress {
            Progress::Downloading { received, total } => {
                job.received = received;
                job.total = total.or(job.total);
                None
            }
            Progress::Done(path) => {
                job.status = Status::Done;
                job.handle = None;
                // The bar should read full even when nothing ever sent a
                // length, so the row does not end at 0% having succeeded.
                job.total = job.total.or(Some(job.received.max(1)));
                job.received = job.total.unwrap_or(job.received);
                Some(Finished { tag: job.tag, path })
            }
            Progress::Failed(e) => {
                job.status = Status::Failed(e);
                job.handle = None;
                sweep(&job.part());
                None
            }
        }
    }

    pub fn act(&mut self, action: Action) {
        match action {
            Action::Cancel(id) => {
                if let Some(i) = self.jobs.iter().position(|j| j.id == id) {
                    let mut job = self.jobs.remove(i);
                    if let Some(handle) = job.handle.take() {
                        handle.abort();
                    }
                    sweep(&job.part());
                }
            }
            Action::Promote(id) => {
                let Some(from) =
                    self.jobs.iter().position(|j| j.id == id && j.status == Status::Queued)
                else {
                    return;
                };
                // In front of the other queued files, behind the running ones:
                // a running job cannot be jumped without restarting it.
                let to = self.jobs.iter().position(|j| j.status == Status::Queued).unwrap_or(from);
                let job = self.jobs.remove(from);
                self.jobs.insert(to, job);
            }
            Action::Retry(id) => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                    if !job.active() {
                        job.status = Status::Queued;
                        job.received = 0;
                    }
                }
            }
            Action::Clear => self.jobs.retain(|j| j.active()),
        }
    }
}

/// A cancelled or failed transfer is dropped mid-write, so its half-file is
/// ours to remove — gigabytes of nothing otherwise.
fn sweep(part: &Path) {
    let _ = std::fs::remove_file(part);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(n: usize) -> Downloads {
        let mut d = Downloads::default();
        for i in 0..n {
            d.enqueue(
                Tag::MediaModel,
                format!("https://example.invalid/{i}"),
                PathBuf::from("/tmp/agp-test"),
                format!("f{i}.safetensors"),
            );
        }
        d
    }

    /// The whole point of the module: the second and third file start without
    /// waiting for the first, and the fourth waits for a slot.
    #[test]
    fn three_run_and_the_rest_wait() {
        let mut d = queue(5);
        let _ = d.pump();
        assert_eq!(d.running(), LIMIT);
        assert_eq!(d.active(), 5);
        assert_eq!(d.jobs[3].status, Status::Queued);
    }

    #[test]
    fn a_finished_file_frees_a_slot_and_names_its_owner() {
        let mut d = queue(4);
        let _ = d.pump();
        let done = d.progressed(1, Progress::Done("/tmp/f0".into())).expect("finished");
        assert_eq!(done.tag, Tag::MediaModel);
        assert_eq!(d.running(), LIMIT - 1);
        let _ = d.pump();
        assert_eq!(d.running(), LIMIT, "the queued file took the freed slot");
    }

    #[test]
    fn promote_jumps_the_queue_but_not_the_running_files() {
        let mut d = queue(5);
        let _ = d.pump();
        let last = d.jobs[4].id;
        d.act(Action::Promote(last));
        assert_eq!(d.jobs[LIMIT].id, last, "it sits first among the queued");
        assert_eq!(d.running(), LIMIT, "nothing running was disturbed");

        let head = d.jobs[0].id;
        d.act(Action::Promote(head));
        assert_eq!(d.jobs[0].id, head, "a running file ignores the press");
    }

    #[test]
    fn the_same_file_twice_is_one_job() {
        let mut d = queue(1);
        let again = d.enqueue(
            Tag::MediaModel,
            "https://example.invalid/0".into(),
            PathBuf::from("/tmp/agp-test"),
            "f0.safetensors".into(),
        );
        assert!(again.is_none());
        assert_eq!(d.jobs.len(), 1);
    }

    #[test]
    fn clear_keeps_what_is_still_going() {
        let mut d = queue(3);
        let _ = d.pump();
        d.progressed(1, Progress::Done("/tmp/f0".into()));
        d.progressed(2, Progress::Failed("nope".into()));
        d.act(Action::Clear);
        assert_eq!(d.jobs.len(), 1);
        assert_eq!(d.jobs[0].status, Status::Running);
    }

    #[test]
    fn a_failure_can_be_retried_from_zero() {
        let mut d = queue(1);
        let _ = d.pump();
        d.progressed(1, Progress::Downloading { received: 99, total: Some(400) });
        d.progressed(1, Progress::Failed("reset by peer".into()));
        d.act(Action::Retry(1));
        assert_eq!(d.jobs[0].status, Status::Queued);
        assert_eq!(d.jobs[0].received, 0);
    }

    #[test]
    fn a_finished_job_reads_full_even_without_a_content_length() {
        let mut d = queue(1);
        let _ = d.pump();
        d.progressed(1, Progress::Downloading { received: 512, total: None });
        d.progressed(1, Progress::Done("/tmp/f0".into()));
        assert_eq!(d.jobs[0].fraction(), 1.0);
    }
}
