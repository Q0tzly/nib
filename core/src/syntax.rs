//! Syntax trees with tree-sitter. Grammars are WebAssembly modules that
//! plugins provide; parsing and highlighting stay in the core because every
//! plugin shares them and they run on every edit and frame.

use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, Tree, WasmStore};
use wasmtime::{Cache, CacheConfig, Config, Engine};

use crate::grid::Style;
use crate::ui::Theme;

pub(crate) struct Languages {
    /// Created with the first grammar. Unlike the plugin engine, it has no
    /// epoch interruption, which tree-sitter's stores do not expect.
    engine: Option<Engine>,
    parser: Parser,
    list: Vec<Entry>,
    pub cache_dir: Option<PathBuf>,
}

struct Entry {
    name: String,
    file_types: Vec<String>,
    state: EntryState,
}

/// Grammars load the first time a file of their type is shown, so languages
/// that are not used cost nothing at startup.
enum EntryState {
    Pending {
        grammar: Vec<u8>,
        highlights: Option<String>,
    },
    Loaded {
        language: Language,
        highlights: Option<Highlights>,
    },
    Failed(String),
}

struct Highlights {
    query: Query,
}

/// The syntax of one buffer.
pub(crate) struct BufferSyntax {
    pub language: usize,
    /// `None` until parsed, and after changes too big to apply as an edit.
    pub tree: Option<Tree>,
    /// The text changed since `tree` was parsed.
    pub dirty: bool,
}

impl Default for Languages {
    fn default() -> Self {
        Self {
            engine: None,
            parser: Parser::new(),
            list: Vec::new(),
            cache_dir: None,
        }
    }
}

impl Languages {
    /// Registers a language, replacing one of the same name. Its grammar is
    /// loaded when first needed.
    pub fn add(
        &mut self,
        name: &str,
        file_types: Vec<String>,
        grammar: Vec<u8>,
        highlights: Option<String>,
    ) {
        let entry = Entry {
            name: name.to_string(),
            file_types,
            state: EntryState::Pending {
                grammar,
                highlights,
            },
        };
        match self.list.iter().position(|e| e.name == name) {
            Some(i) => self.list[i] = entry,
            None => self.list.push(entry),
        }
    }

    fn ensure_loaded(&mut self, id: usize) -> Result<(), String> {
        match &self.list[id].state {
            EntryState::Loaded { .. } => return Ok(()),
            EntryState::Failed(err) => return Err(err.clone()),
            EntryState::Pending { .. } => {}
        }
        let EntryState::Pending {
            grammar,
            highlights,
        } = std::mem::replace(&mut self.list[id].state, EntryState::Failed(String::new()))
        else {
            unreachable!("checked above");
        };
        let name = self.list[id].name.clone();
        let loaded = self.load(&name, &grammar, highlights.as_deref());
        self.list[id].state = match loaded {
            Ok((language, highlights)) => EntryState::Loaded {
                language,
                highlights,
            },
            Err(err) => EntryState::Failed(format!("{name}: {err}")),
        };
        match &self.list[id].state {
            EntryState::Failed(err) => Err(err.clone()),
            _ => Ok(()),
        }
    }

    fn load(
        &mut self,
        name: &str,
        grammar: &[u8],
        highlights: Option<&str>,
    ) -> Result<(Language, Option<Highlights>), String> {
        if self.engine.is_none() {
            let mut config = Config::new();
            if let Some(dir) = &self.cache_dir {
                let mut cache = CacheConfig::new();
                cache.with_directory(dir);
                config.cache(Some(Cache::new(cache).map_err(|err| err.to_string())?));
            }
            self.engine = Some(Engine::new(&config).map_err(|err| err.to_string())?);
        }
        let engine = self.engine.as_ref().expect("created above");
        let mut store = match self.parser.take_wasm_store() {
            Some(store) => store,
            None => WasmStore::new(engine).map_err(|err| err.to_string())?,
        };
        let loaded = store.load_language(name, grammar);
        self.parser
            .set_wasm_store(store)
            .map_err(|err| err.to_string())?;
        let language = loaded.map_err(|err| err.to_string())?;
        let highlights = highlights
            .map(|source| {
                let query =
                    Query::new(&language, source).map_err(|err| format!("highlights: {err}"))?;
                Ok::<_, String>(Highlights { query })
            })
            .transpose()?;
        Ok((language, highlights))
    }

    /// The language for a file, by its extension.
    pub fn for_path(&self, path: &Path) -> Option<usize> {
        let extension = path.extension()?.to_str()?;
        self.list
            .iter()
            .position(|e| e.file_types.iter().any(|t| t == extension))
    }

    /// Parses `text`, reusing `old` for the parts that did not change. Loads
    /// the grammar first if needed.
    pub fn parse(
        &mut self,
        id: usize,
        text: &Rope,
        old: Option<&Tree>,
    ) -> Result<Option<Tree>, String> {
        self.ensure_loaded(id)?;
        let EntryState::Loaded { language, .. } = &self.list[id].state else {
            unreachable!("loaded above");
        };
        if self.parser.set_language(language).is_err() {
            return Ok(None);
        }
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= text.len_bytes() {
                return &[];
            }
            let (chunk, start, _, _) = text.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        };
        Ok(self.parser.parse_with_options(&mut read, old, None))
    }

    /// The highlight style of each byte in `range`, or `None` for plain text.
    pub fn highlight(
        &self,
        theme: &Theme,
        language: usize,
        tree: &Tree,
        text: &Rope,
        range: Range<usize>,
    ) -> Vec<Option<Style>> {
        let mut styles = vec![None; range.len()];
        let EntryState::Loaded {
            highlights: Some(highlights),
            ..
        } = &self.list[language].state
        else {
            return styles;
        };
        // Looked up per frame, so theme changes show at once; queries have
        // a few dozen captures.
        let capture_styles: Vec<Option<Style>> = highlights
            .query
            .capture_names()
            .iter()
            .map(|name| theme.style(name))
            .collect();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let node_text = |node: tree_sitter::Node| {
            text.byte_slice(node.byte_range())
                .chunks()
                .map(str::as_bytes)
                .collect::<Vec<_>>()
                .into_iter()
        };
        let mut spans = Vec::new();
        let mut captures = cursor.captures(&highlights.query, tree.root_node(), node_text);
        while let Some((found, index)) = captures.next() {
            let capture = found.captures()[*index];
            if let Some(style) = capture_styles[capture.index as usize] {
                spans.push((capture.node.byte_range(), found.pattern_index, style));
            }
        }
        paint(&mut styles, range.start, spans);
        styles
    }
}

/// Paints `spans` into `styles`, which covers bytes from `offset`. Inner
/// spans win over the ones around them; for the same range, the pattern
/// written first in the query wins, as in tree-sitter's highlighting.
fn paint(
    styles: &mut [Option<Style>],
    offset: usize,
    mut spans: Vec<(Range<usize>, usize, Style)>,
) {
    spans.sort_by_key(|(range, pattern, _)| (range.start, std::cmp::Reverse(range.end), *pattern));
    let mut last: Option<Range<usize>> = None;
    for (range, _, style) in spans {
        if last.as_ref() == Some(&range) {
            continue;
        }
        let start = range.start.saturating_sub(offset).min(styles.len());
        let end = range.end.saturating_sub(offset).min(styles.len());
        for slot in &mut styles[start..end] {
            *slot = Some(style);
        }
        last = Some(range);
    }
}

impl BufferSyntax {
    pub fn new(language: usize) -> Self {
        Self {
            language,
            tree: None,
            dirty: true,
        }
    }

    /// Tells the tree that `old` became `new` between `start` and the ends,
    /// so the next parse only redoes what changed.
    pub fn edit(&mut self, old: &Rope, new: &Rope, start: usize, old_end: usize, new_end: usize) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&InputEdit {
                start_byte: start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: point(old, start),
                old_end_position: point(old, old_end),
                new_end_position: point(new, new_end),
            });
        }
        self.dirty = true;
    }

    /// Drops the tree when an edit cannot be described, e.g. after undo.
    pub fn invalidate(&mut self) {
        self.tree = None;
        self.dirty = true;
    }
}

fn point(text: &Rope, byte: usize) -> Point {
    let row = text.byte_to_line(byte);
    Point {
        row,
        column: byte - text.line_to_byte(row),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;

    fn style(n: u8) -> Style {
        Style {
            fg: Color::Indexed(n),
            ..Style::default()
        }
    }

    #[test]
    fn inner_spans_win_and_the_first_pattern_wins_ties() {
        let mut styles = vec![None; 10];
        paint(
            &mut styles,
            100,
            vec![
                (102..108, 0, style(1)),
                (104..105, 3, style(2)),
                (100..101, 2, style(3)),
                (100..101, 1, style(4)),
            ],
        );
        let expected = [4, 0, 1, 1, 2, 1, 1, 1, 0, 0];
        for (i, n) in expected.into_iter().enumerate() {
            assert_eq!(styles[i], (n > 0).then(|| style(n)), "byte {i}");
        }
    }
}
