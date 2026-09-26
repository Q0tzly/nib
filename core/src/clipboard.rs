//! The system clipboard, which the frontend provides, so the core stays
//! free of platform code.

/// Reads and writes the clipboard for plugins.
pub trait Clipboard: Send {
    fn get(&mut self) -> Result<String, String>;
    fn set(&mut self, text: &str) -> Result<(), String>;
}

/// A clipboard inside the editor, for when the frontend gives none, as in
/// tests.
#[derive(Default)]
pub(crate) struct Internal(String);

impl Clipboard for Internal {
    fn get(&mut self) -> Result<String, String> {
        Ok(self.0.clone())
    }

    fn set(&mut self, text: &str) -> Result<(), String> {
        self.0 = text.to_string();
        Ok(())
    }
}
