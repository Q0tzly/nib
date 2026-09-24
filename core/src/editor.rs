use std::path::PathBuf;

use crate::Error;
use crate::buffer::Buffer;
use crate::input::KeyEvent;
use crate::view::View;

pub struct Editor {
    buffers: Vec<Buffer>,
    view: View,
    width: u16,
    height: u16,
    quit: bool,
}

impl Default for Editor {
    /// Starts with an empty buffer that has no path.
    fn default() -> Self {
        Self {
            buffers: vec![Buffer::default()],
            view: View::new(0),
            width: 0,
            height: 0,
            quit: false,
        }
    }
}

impl Editor {
    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let buffer = Buffer::open(path)?;
        let scratch = &self.buffers[0];
        if self.buffers.len() == 1
            && scratch.path().is_none()
            && scratch.is_empty()
            && !scratch.is_modified()
        {
            self.buffers[0] = buffer;
            self.view = View::new(0);
        } else {
            self.buffers.push(buffer);
            self.view = View::new(self.buffers.len() - 1);
        }
        Ok(())
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn view_mut(&mut self) -> &mut View {
        &mut self.view
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.view.buffer]
    }

    /// No plugin handles keys yet, so only the emergency keys work.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key == KeyEvent::ctrl('q') {
            self.quit = true;
        }
    }

    /// Keys that work only while no plugin takes input, so the editor can
    /// always be left even if the keymap plugin is broken.
    pub fn emergency_keys_hint(&self) -> &'static str {
        "Ctrl-q: quit"
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    #[cfg(test)]
    pub(crate) fn with_text(text: &str) -> Self {
        Self {
            buffers: vec![Buffer::with_text(text)],
            ..Self::default()
        }
    }
}
