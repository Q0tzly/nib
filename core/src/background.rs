//! Work done on background threads for plugins, such as reading programs'
//! output and listing files. The threads queue messages in an inbox, which
//! wakes the main loop.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::process::Stream;

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
    Files {
        job: u32,
        paths: Vec<String>,
        done: bool,
    },
}

/// Called from background threads after they queue a message, so the
/// frontend can wake its main loop.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub(crate) struct Inbox {
    messages: Mutex<VecDeque<Message>>,
    waker: Mutex<Option<Waker>>,
}

impl Inbox {
    pub fn push(&self, message: Message) {
        self.messages.lock().expect("inbox lock").push_back(message);
        if let Some(waker) = &*self.waker.lock().expect("waker lock") {
            waker();
        }
    }

    pub fn take(&self) -> Vec<Message> {
        self.messages
            .lock()
            .expect("inbox lock")
            .drain(..)
            .collect()
    }

    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.waker.lock().expect("waker lock") = waker;
    }
}
