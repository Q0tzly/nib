//! The OS clipboard, given to the core for plugins.

pub struct System(arboard::Clipboard);

impl System {
    pub fn new() -> Option<Self> {
        arboard::Clipboard::new().ok().map(Self)
    }
}

impl nib_core::Clipboard for System {
    fn get(&mut self) -> Result<String, String> {
        match self.0.get_text() {
            Ok(text) => Ok(text),
            // Empty, or holding something other than text, such as an image.
            Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn set(&mut self, text: &str) -> Result<(), String> {
        self.0.set_text(text).map_err(|err| err.to_string())
    }
}
