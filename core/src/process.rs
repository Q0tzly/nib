//! Programs plugins start. Their output is read on background threads and
//! queued in an inbox, which wakes the main loop.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::plugin::PluginId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Message {
    Output {
        id: u32,
        stream: Stream,
        data: Vec<u8>,
    },
    Exit {
        id: u32,
        code: Option<i32>,
    },
}

/// Called from background threads after they queue a message, so the
/// frontend can wake its main loop.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Messages from background threads.
#[derive(Default)]
pub(crate) struct Inbox {
    messages: Mutex<VecDeque<Message>>,
    waker: Mutex<Option<Waker>>,
}

impl Inbox {
    fn push(&self, message: Message) {
        self.messages.lock().expect("inbox lock").push_back(message);
        if let Some(waker) = &*self.waker.lock().expect("waker lock") {
            waker();
        }
    }
}

struct Process {
    id: u32,
    owner: PluginId,
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
}

#[derive(Default)]
pub(crate) struct Processes {
    list: Vec<Process>,
    last_id: u32,
    inbox: Arc<Inbox>,
}

impl Processes {
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.inbox.waker.lock().expect("waker lock") = waker;
    }

    pub fn spawn(
        &mut self,
        owner: PluginId,
        command: &str,
        args: &[String],
        cwd: Option<PathBuf>,
    ) -> Result<u32, String> {
        let mut builder = Command::new(command);
        builder
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            builder.current_dir(cwd);
        }
        let mut child = builder.spawn().map_err(|err| format!("{command}: {err}"))?;
        self.last_id += 1;
        let id = self.last_id;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let child = Arc::new(Mutex::new(child));

        let inbox = self.inbox.clone();
        let errors = thread::Builder::new()
            .name(format!("nib-process-{id}-stderr"))
            .spawn(move || read(id, Stream::Stderr, stderr, &inbox))
            .map_err(|err| err.to_string())?;
        let (inbox, waited) = (self.inbox.clone(), child.clone());
        thread::Builder::new()
            .name(format!("nib-process-{id}"))
            .spawn(move || {
                read(id, Stream::Stdout, stdout, &inbox);
                // All output comes before the exit.
                let _ = errors.join();
                let code = wait(&waited);
                inbox.push(Message::Exit { id, code });
            })
            .map_err(|err| err.to_string())?;
        self.list.push(Process {
            id,
            owner,
            child,
            stdin,
        });
        Ok(id)
    }

    fn get(&mut self, id: u32, owner: PluginId) -> Option<&mut Process> {
        self.list
            .iter_mut()
            .find(|p| p.id == id && p.owner == owner)
    }

    pub fn write(&mut self, id: u32, owner: PluginId, data: &[u8]) -> Result<(), String> {
        let process = self.get(id, owner).ok_or("the program has ended")?;
        let stdin = process.stdin.as_mut().ok_or("its input is closed")?;
        stdin
            .write_all(data)
            .and_then(|()| stdin.flush())
            .map_err(|err| err.to_string())
    }

    pub fn close_stdin(&mut self, id: u32, owner: PluginId) {
        if let Some(process) = self.get(id, owner) {
            process.stdin = None;
        }
    }

    /// Kills the program. Its exit still arrives as a message.
    pub fn kill(&mut self, id: u32, owner: PluginId) {
        if let Some(process) = self.get(id, owner) {
            let _ = process.child.lock().expect("child lock").kill();
        }
    }

    /// Kills the program and forgets it: nothing more arrives from it.
    pub fn forget(&mut self, id: u32, owner: PluginId) {
        self.kill(id, owner);
        self.list.retain(|p| !(p.id == id && p.owner == owner));
    }

    pub fn remove_owner(&mut self, owner: PluginId) {
        for process in self.list.iter().filter(|p| p.owner == owner) {
            let _ = process.child.lock().expect("child lock").kill();
        }
        self.list.retain(|p| p.owner != owner);
    }

    /// The queued messages of programs still known, with their owners.
    /// Programs that exited are forgotten.
    pub fn take_messages(&mut self) -> Vec<(PluginId, Message)> {
        let messages: Vec<Message> = self
            .inbox
            .messages
            .lock()
            .expect("inbox lock")
            .drain(..)
            .collect();
        let mut owned = Vec::new();
        for message in messages {
            let id = match &message {
                Message::Output { id, .. } | Message::Exit { id, .. } => *id,
            };
            let Some(process) = self.list.iter().find(|p| p.id == id) else {
                continue;
            };
            let owner = process.owner;
            if matches!(message, Message::Exit { .. }) {
                self.list.retain(|p| p.id != id);
            }
            owned.push((owner, message));
        }
        owned
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        for process in &self.list {
            let _ = process.child.lock().expect("child lock").kill();
        }
    }
}

fn read(id: u32, stream: Stream, mut reader: impl Read, inbox: &Inbox) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => inbox.push(Message::Output {
                id,
                stream,
                data: buffer[..n].to_vec(),
            }),
        }
    }
}

/// Waits for the program to exit after its output closed. The lock is only
/// held to check, so the main thread can still kill it meanwhile.
fn wait(child: &Mutex<Child>) -> Option<i32> {
    let mut delay = Duration::from_millis(1);
    loop {
        match child.lock().expect("child lock").try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {}
            Err(_) => return None,
        }
        thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_millis(200));
    }
}
