//! File lists for plugins, made on background threads with the `ignore`
//! crate, so .gitignore is honored as ripgrep honors it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::background::{Inbox, Message};
use crate::plugin::PluginId;

/// Paths sent in one message.
const BATCH: usize = 1000;

struct Job {
    id: u32,
    owner: PluginId,
    cancelled: Arc<AtomicBool>,
}

pub(crate) struct FileJobs {
    jobs: Vec<Job>,
    last_id: u32,
    inbox: Arc<Inbox>,
}

impl FileJobs {
    pub fn new(inbox: Arc<Inbox>) -> Self {
        Self {
            jobs: Vec::new(),
            last_id: 0,
            inbox,
        }
    }

    /// Starts listing the files under `dir`, and returns the job's id.
    pub fn list(&mut self, owner: PluginId, dir: PathBuf) -> Result<u32, String> {
        if !dir.is_dir() {
            return Err(format!("{}: not a directory", dir.display()));
        }
        self.last_id += 1;
        let id = self.last_id;
        let cancelled = Arc::new(AtomicBool::new(false));
        let (inbox, stop) = (self.inbox.clone(), cancelled.clone());
        thread::Builder::new()
            .name(format!("nib-files-{id}"))
            .spawn(move || walk(id, &dir, &inbox, &stop))
            .map_err(|err| err.to_string())?;
        self.jobs.push(Job {
            id,
            owner,
            cancelled,
        });
        Ok(id)
    }

    /// Stops the job; nothing more arrives from it.
    pub fn cancel(&mut self, id: u32, owner: PluginId) {
        self.jobs.retain(|job| {
            let this = job.id == id && job.owner == owner;
            if this {
                job.cancelled.store(true, Ordering::Relaxed);
            }
            !this
        });
    }

    pub fn remove_owner(&mut self, owner: PluginId) {
        self.jobs.retain(|job| {
            if job.owner == owner {
                job.cancelled.store(true, Ordering::Relaxed);
            }
            job.owner != owner
        });
    }

    /// The plugin a message of job `id` is for, if it is still wanted. A
    /// finished job is forgotten.
    pub fn owner(&mut self, id: u32, done: bool) -> Option<PluginId> {
        let owner = self.jobs.iter().find(|job| job.id == id)?.owner;
        if done {
            self.jobs.retain(|job| job.id != id);
        }
        Some(owner)
    }
}

fn walk(id: u32, dir: &Path, inbox: &Inbox, cancelled: &AtomicBool) {
    let walker = ignore::WalkBuilder::new(dir)
        // .gitignore counts outside a repository too, as the docs say.
        .require_git(false)
        .build();
    let mut paths = Vec::new();
    for entry in walker.flatten() {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(dir) else {
            continue;
        };
        paths.push(relative.to_string_lossy().replace('\\', "/"));
        if paths.len() == BATCH {
            let batch = std::mem::take(&mut paths);
            inbox.push(Message::Files {
                job: id,
                paths: batch,
                done: false,
            });
        }
    }
    inbox.push(Message::Files {
        job: id,
        paths,
        done: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_files_but_not_ignored_or_hidden_ones() {
        let dir = std::env::temp_dir().join(format!("nib-{}-files", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        fs::write(dir.join("src/a.rs"), "").unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("target/out"), "").unwrap();
        fs::write(dir.join(".hidden"), "").unwrap();

        let inbox = Inbox::default();
        walk(7, &dir, &inbox, &AtomicBool::new(false));
        let mut listed = Vec::new();
        let messages = inbox.take();
        for message in &messages {
            let Message::Files { job, paths, .. } = message else {
                panic!("{message:?}");
            };
            assert_eq!(*job, 7);
            listed.extend(paths.iter().cloned());
        }
        assert!(matches!(
            messages.last(),
            Some(Message::Files { done: true, .. })
        ));
        listed.sort();
        assert_eq!(listed, ["b.txt", "src/a.rs"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
