use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::Error;
use crate::change::{ChangeSet, Edit};
use crate::grapheme;
use crate::history::{History, UndoMode};
use crate::search;
use crate::selection::{Range, Selection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// What a change did to a buffer.
#[derive(Debug)]
pub struct Change {
    /// The change sets applied to the text, in order.
    pub changes: Vec<ChangeSet>,
    /// The selection of the view that made the change, after the change.
    pub selection: Selection,
}

/// A text buffer. Line endings are normalized to LF in memory and restored
/// on save, so offsets never point between `\r` and `\n`.
pub struct Buffer {
    text: Rope,
    version: u64,
    line_ending: LineEnding,
    path: Option<PathBuf>,
    history: History,
    saved_state: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::with_text("")
    }
}

impl Buffer {
    /// Creates a buffer from `text`. If the first line ends with CRLF, the
    /// buffer uses CRLF and every CRLF in `text` becomes LF.
    pub fn with_text(text: &str) -> Self {
        let line_ending = match text.find('\n') {
            Some(i) if text[..i].ends_with('\r') => LineEnding::Crlf,
            _ => LineEnding::Lf,
        };
        let text = match line_ending {
            LineEnding::Lf => Rope::from_str(text),
            LineEnding::Crlf => Rope::from_str(&text.replace("\r\n", "\n")),
        };
        Self {
            text,
            version: 0,
            line_ending,
            path: None,
            history: History::default(),
            saved_state: 0,
        }
    }

    /// Opens the file at `path`. A missing file gives an empty buffer that
    /// will be created on save.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let mut buffer = match fs::read_to_string(&path) {
            Ok(text) => Self::with_text(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(err.into()),
        };
        buffer.path = Some(path);
        Ok(buffer)
    }

    /// Writes a temporary file next to the target and renames it over the
    /// target, so a crash while saving never leaves a truncated file.
    pub fn save(&mut self) -> Result<(), Error> {
        let path = self.path.as_ref().ok_or(Error::NoPath)?;
        // Write to the file a symlink points to, instead of replacing the link.
        let target = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let tmp = target.with_file_name(format!(".{name}.nib-{}~", std::process::id()));
        let result = self.write_file(&tmp).and_then(|()| {
            if let Ok(metadata) = fs::metadata(&target) {
                fs::set_permissions(&tmp, metadata.permissions())?;
            }
            fs::rename(&tmp, &target)
        });
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        self.saved_state = self.history.state();
        Ok(())
    }

    fn write_file(&self, path: &Path) -> io::Result<()> {
        let mut out = BufWriter::new(File::create(path)?);
        match self.line_ending {
            LineEnding::Lf => self.text.write_to(&mut out)?,
            LineEnding::Crlf => {
                for chunk in self.text.chunks() {
                    out.write_all(chunk.replace('\n', "\r\n").as_bytes())?;
                }
            }
        }
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()
    }

    pub fn save_as(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.path = Some(path.into());
        self.save()
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Increases on every change, including undo and redo.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_modified(&self) -> bool {
        self.history.state() != self.saved_state
    }

    pub fn slice(&self, start: usize, end: usize) -> Result<String, Error> {
        grapheme::check_position(&self.text, start)?;
        grapheme::check_position(&self.text, end)?;
        if end < start {
            return Err(Error::InvalidPosition(end));
        }
        Ok(self.text.byte_slice(start..end).to_string())
    }

    /// Text ending with a line break has an empty last line.
    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        (line < self.line_count()).then(|| self.text.line_to_byte(line))
    }

    pub fn line_of(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(self.text.byte_to_line(pos))
    }

    pub fn next_grapheme(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(grapheme::next_boundary(&self.text, pos))
    }

    pub fn prev_grapheme(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(grapheme::prev_boundary(&self.text, pos))
    }

    /// See [`search::find`].
    pub fn find(
        &self,
        pattern: &str,
        start: usize,
        backward: bool,
    ) -> Result<Option<(usize, usize)>, Error> {
        search::find(&self.text, pattern, start, backward)
    }

    /// See [`search::find_all`].
    pub fn find_all(
        &self,
        pattern: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<(usize, usize)>, Error> {
        search::find_all(&self.text, pattern, start, end)
    }

    /// Applies `edits` atomically: on error, nothing changes.
    ///
    /// `selection` is the current selection of the view making the change.
    /// `after` is the new selection as ranges in the changed text and the
    /// primary index; without it, `selection` is mapped through the change.
    pub fn apply(
        &mut self,
        base_version: u64,
        edits: Vec<Edit>,
        selection: &Selection,
        after: Option<(Vec<Range>, usize)>,
        mode: UndoMode,
    ) -> Result<Change, Error> {
        if base_version != self.version {
            return Err(Error::StaleVersion {
                base: base_version,
                current: self.version,
            });
        }
        let changes = ChangeSet::new(edits, &self.text)?;
        let mut text = self.text.clone();
        let inverse = changes.apply(&mut text);
        let after = match after {
            Some((ranges, primary)) => Selection::new(ranges, primary, &text)?,
            None => selection.map(&changes, &text),
        };
        if changes.is_empty() {
            return Ok(Change {
                changes: Vec::new(),
                selection: after,
            });
        }
        self.text = text;
        self.version += 1;
        self.history.record(
            changes.clone(),
            inverse,
            selection.clone(),
            after.clone(),
            mode,
        );
        Ok(Change {
            changes: vec![changes],
            selection: after,
        })
    }

    pub fn undo(&mut self) -> Option<Change> {
        let (changes, selection) = self.history.undo(&mut self.text)?;
        self.version += 1;
        Some(Change { changes, selection })
    }

    pub fn redo(&mut self) -> Option<Change> {
        let (changes, selection) = self.history.redo(&mut self.text)?;
        self.version += 1;
        Some(Change { changes, selection })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
            fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn insert(buffer: &mut Buffer, pos: usize, text: &str, mode: UndoMode) -> Change {
        let sel = Selection::point(pos);
        buffer
            .apply(
                buffer.version(),
                vec![Edit::insert(pos, text)],
                &sel,
                None,
                mode,
            )
            .unwrap()
    }

    #[test]
    fn crlf_is_normalized_and_restored() {
        let file = TempFile::new("crlf.txt", b"one\r\ntwo\r\n");
        let mut buffer = Buffer::open(&file.0).unwrap();
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        assert_eq!(buffer.text().to_string(), "one\ntwo\n");
        assert_eq!(buffer.line_count(), 3);
        insert(&mut buffer, 8, "three\n", UndoMode::NewStep);
        buffer.save().unwrap();
        assert_eq!(fs::read(&file.0).unwrap(), b"one\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn lone_cr_is_not_a_line_break() {
        let buffer = Buffer::with_text("a\rb\nc");
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_start(1), Some(4));
        assert_eq!(buffer.line_start(2), None);
    }

    #[test]
    fn missing_file_opens_empty() {
        let path = std::env::temp_dir().join(format!("nib-{}-missing.txt", std::process::id()));
        let buffer = Buffer::open(&path).unwrap();
        assert!(buffer.is_empty());
        assert_eq!(buffer.path(), Some(path.as_path()));
        assert!(!buffer.is_modified());
    }

    #[test]
    fn rejects_stale_version() {
        let mut buffer = Buffer::with_text("abc");
        insert(&mut buffer, 0, "x", UndoMode::NewStep);
        let result = buffer.apply(
            0,
            vec![Edit::insert(0, "y")],
            &Selection::point(0),
            None,
            UndoMode::NewStep,
        );
        assert!(matches!(
            result,
            Err(Error::StaleVersion {
                base: 0,
                current: 1
            })
        ));
        assert_eq!(buffer.text().to_string(), "xabc");
    }

    #[test]
    fn invalid_selection_leaves_buffer_unchanged() {
        let mut buffer = Buffer::with_text("abc");
        let result = buffer.apply(
            0,
            vec![Edit::insert(0, "あ")],
            &Selection::point(0),
            Some((vec![Range::point(1)], 0)),
            UndoMode::NewStep,
        );
        assert!(matches!(result, Err(Error::InvalidPosition(1))));
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(buffer.version(), 0);
        assert!(buffer.undo().is_none());
    }

    #[test]
    fn maps_selection_without_explicit_after() {
        let mut buffer = Buffer::with_text("abc");
        let change = insert(&mut buffer, 1, "xy", UndoMode::NewStep);
        assert_eq!(change.selection.primary(), Range::point(3));
    }

    #[test]
    fn merged_edits_undo_together() {
        let mut buffer = Buffer::with_text("");
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        insert(&mut buffer, 1, "b", UndoMode::Merge);
        insert(&mut buffer, 2, "c", UndoMode::Merge);
        insert(&mut buffer, 3, " d", UndoMode::NewStep);
        assert_eq!(buffer.text().to_string(), "abc d");

        let change = buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(change.selection.primary(), Range::point(3));
        let change = buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "");
        assert_eq!(change.changes.len(), 3);
        assert!(buffer.undo().is_none());

        buffer.redo().unwrap();
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(buffer.version(), 7);
    }

    #[test]
    fn merge_after_undo_starts_a_new_step() {
        let mut buffer = Buffer::with_text("");
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        insert(&mut buffer, 1, "b", UndoMode::NewStep);
        buffer.undo().unwrap();
        insert(&mut buffer, 1, "c", UndoMode::Merge);
        assert_eq!(buffer.text().to_string(), "ac");
        assert!(buffer.redo().is_none());
        buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "a");
    }

    #[test]
    fn tracks_modification_across_save_and_undo() {
        let file = TempFile::new("modified.txt", b"");
        let mut buffer = Buffer::open(&file.0).unwrap();
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        assert!(buffer.is_modified());
        buffer.save().unwrap();
        assert!(!buffer.is_modified());
        buffer.undo().unwrap();
        assert!(buffer.is_modified());
        buffer.redo().unwrap();
        assert!(!buffer.is_modified());
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_permissions_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let file = TempFile::new("perm.sh", b"echo hi\n");
        fs::set_permissions(&file.0, fs::Permissions::from_mode(0o750)).unwrap();
        let mut buffer = Buffer::open(&file.0).unwrap();
        insert(&mut buffer, 0, "#!/bin/sh\n", UndoMode::NewStep);
        buffer.save().unwrap();

        let metadata = fs::metadata(&file.0).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o750);
        assert_eq!(fs::read(&file.0).unwrap(), b"#!/bin/sh\necho hi\n");
        let name = file.0.file_name().unwrap().to_string_lossy().into_owned();
        let leftovers = fs::read_dir(file.0.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let entry = entry.file_name().to_string_lossy().into_owned();
                entry.contains(&name) && entry.ends_with('~')
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_through_symlinks() {
        let target = TempFile::new("target.txt", b"a");
        let link =
            TempFile(std::env::temp_dir().join(format!("nib-{}-link.txt", std::process::id())));
        std::os::unix::fs::symlink(&target.0, &link.0).unwrap();

        let mut buffer = Buffer::open(&link.0).unwrap();
        insert(&mut buffer, 1, "b", UndoMode::NewStep);
        buffer.save().unwrap();

        assert!(fs::symlink_metadata(&link.0).unwrap().is_symlink());
        assert_eq!(fs::read(&target.0).unwrap(), b"ab");
    }

    #[test]
    fn empty_edits_do_not_create_undo_steps() {
        let mut buffer = Buffer::with_text("abc");
        let change = buffer
            .apply(
                0,
                vec![],
                &Selection::point(0),
                Some((vec![Range::new(0, 2)], 0)),
                UndoMode::NewStep,
            )
            .unwrap();
        assert!(change.changes.is_empty());
        assert_eq!(change.selection.primary(), Range::new(0, 2));
        assert_eq!(buffer.version(), 0);
        assert!(buffer.undo().is_none());
    }
}
