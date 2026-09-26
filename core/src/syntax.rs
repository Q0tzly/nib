//! Syntax trees with tree-sitter. Grammars are WebAssembly modules that
//! plugins provide; parsing and highlighting stay in the core because every
//! plugin shares them and they run on every edit and frame.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, Range as TsRange, TextProvider,
    Tree, WasmStore,
};
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
        /// Sources by name.
        queries: BTreeMap<String, String>,
    },
    Loaded {
        language: Language,
        highlights: Option<Query>,
        /// Run after every parse, so compiled with the grammar.
        injections: Option<Query>,
        /// The other queries, compiled when first run.
        queries: BTreeMap<String, QueryState>,
    },
    Failed(String),
}

enum QueryState {
    Source(String),
    Ready(Query),
    /// Reported once; runs as a query without matches afterwards.
    Failed,
}

/// A syntax node as the plugin API hands it out: a value, since tree-sitter
/// nodes borrow their tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: u64,
    pub kind: String,
    pub named: bool,
    pub range: Range<usize>,
}

/// The syntax of one buffer.
pub(crate) struct BufferSyntax {
    pub language: usize,
    /// `None` until parsed, and after changes too big to apply as an edit.
    pub tree: Option<Tree>,
    /// The text changed since `tree` was parsed.
    pub dirty: bool,
    /// Where the text changed since the last parse.
    edited: Option<Range<usize>>,
    injections: Injections,
    /// The first parse leaves injections for later, so a file just opened
    /// shows the colors of its own language sooner.
    injections_pending: bool,
}

/// How deep injections nest: Rust, the Markdown of its doc comments, the
/// Rust of their code blocks, and the Markdown of those doc comments.
const MAX_DEPTH: usize = 4;

/// A language injected into another, such as the code blocks of Markdown
/// or the doc comments of Rust, parsed from only its ranges of the text.
/// Positions in its tree count from the start of the buffer.
struct Layer {
    key: LayerKey,
    language: usize,
    /// Sorted and apart; never empty.
    ranges: Vec<TsRange>,
    tree: Tree,
    /// An edit reached into `ranges` since the layer was parsed.
    touched: bool,
    injections: Injections,
}

/// The languages injected into a tree.
#[derive(Default)]
struct Injections {
    /// What the injections query found, in order. Kept to look again only
    /// where the tree changed: the query over a whole tree of a few thousand
    /// lines takes milliseconds.
    found: Vec<Injection>,
    layers: Vec<Layer>,
}

/// One match of an injections query.
struct Injection {
    language: usize,
    /// The pattern, if all its matches of the same language make one layer
    /// (`injection.combined`).
    combined: Option<usize>,
    /// Sorted and apart; never empty.
    ranges: Vec<TsRange>,
}

impl Injection {
    fn span(&self) -> (usize, usize) {
        let last = self.ranges.last().expect("never empty");
        (self.ranges[0].start_byte, last.end_byte)
    }
}

/// Where to look for injections: a tree, which covers `host` (all of the
/// text if empty), and the regions to look in (everywhere if `None`).
struct Found<'a> {
    tree: &'a Tree,
    host: &'a [TsRange],
    regions: Option<&'a [Range<usize>]>,
}

/// Which layer after a parse is which before it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LayerKey {
    /// A layer of one match, by where it starts.
    At(usize, usize),
    /// The layer of all the matches of a combined pattern and language,
    /// whose first match can come and go.
    Combined(usize, usize),
}

/// The layers that `found` makes: one for each match, but one for all the
/// matches of a combined pattern and language.
fn group(found: &[Injection]) -> Vec<(LayerKey, usize, Vec<TsRange>)> {
    let mut layers = Vec::new();
    let mut combined: Vec<(LayerKey, usize, Vec<TsRange>)> = Vec::new();
    for injection in found {
        let (language, ranges) = (injection.language, injection.ranges.iter().copied());
        match injection.combined {
            None => {
                let key = LayerKey::At(language, injection.ranges[0].start_byte);
                layers.push((key, language, ranges.collect()));
            }
            Some(pattern) => {
                let key = LayerKey::Combined(pattern, language);
                match combined.iter_mut().find(|(k, _, _)| *k == key) {
                    Some((_, _, all)) => all.extend(ranges),
                    None => combined.push((key, language, ranges.collect())),
                }
            }
        }
    }
    for (key, language, mut ranges) in combined {
        // tree-sitter wants them in order and apart.
        ranges.sort_by_key(|r| r.start_byte);
        let mut end = 0;
        ranges.retain(|r| {
            let apart = r.start_byte >= end;
            end = end.max(r.end_byte);
            apart
        });
        layers.push((key, language, ranges));
    }
    layers
}

/// Where to look for injections again after parsing `old` into `new`:
/// where the syntax changed, and where the text did, since an edit inside
/// a comment changes no syntax but the text an injection covers.
fn changed_regions(old: &Tree, new: &Tree, edited: Option<Range<usize>>) -> Vec<Range<usize>> {
    let mut regions: Vec<Range<usize>> = old
        .changed_ranges(new)
        .map(|r| r.start_byte..r.end_byte)
        .chain(edited)
        // Matches that only touch the edges count too.
        .map(|r| r.start.saturating_sub(1)..r.end + 1)
        .collect();
    regions.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for region in regions {
        match merged.last_mut() {
            Some(last) if region.start <= last.end => last.end = last.end.max(region.end),
            _ => merged.push(region),
        }
    }
    merged
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
        queries: BTreeMap<String, String>,
    ) {
        let entry = Entry {
            name: name.to_string(),
            file_types,
            state: EntryState::Pending { grammar, queries },
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
            mut queries,
        } = std::mem::replace(&mut self.list[id].state, EntryState::Failed(String::new()))
        else {
            unreachable!("checked above");
        };
        let name = self.list[id].name.clone();
        let highlights = queries.remove("highlights");
        let injections = queries.remove("injections");
        let loaded = self.load(
            &name,
            &grammar,
            highlights.as_deref(),
            injections.as_deref(),
        );
        self.list[id].state = match loaded {
            Ok((language, highlights, injections)) => EntryState::Loaded {
                language,
                highlights,
                injections,
                queries: queries
                    .into_iter()
                    .map(|(name, source)| (name, QueryState::Source(source)))
                    .collect(),
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
        injections: Option<&str>,
    ) -> Result<(Language, Option<Query>, Option<Query>), String> {
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
        let compile = |name: &str, source: Option<&str>| {
            source
                .map(|source| Query::new(&language, source).map_err(|err| format!("{name}: {err}")))
                .transpose()
        };
        let highlights = compile("highlights", highlights)?;
        let injections = compile("injections", injections)?;
        Ok((language, highlights, injections))
    }

    pub fn name(&self, id: usize) -> &str {
        &self.list[id].name
    }

    /// The language for a file, by its extension.
    pub fn for_path(&self, path: &Path) -> Option<usize> {
        let extension = path.extension()?.to_str()?;
        self.list
            .iter()
            .position(|e| e.file_types.iter().any(|t| t == extension))
    }

    /// The language an injection names, by its name or a file type, as in
    /// the info string of a Markdown code block.
    fn for_injection(&self, name: &str) -> Option<usize> {
        let name = name.to_ascii_lowercase();
        self.list
            .iter()
            .position(|e| e.name == name)
            .or_else(|| self.list.iter().position(|e| e.file_types.contains(&name)))
    }

    /// Parses the syntax of a buffer: its tree, reusing the old one for the
    /// parts that did not change, and the languages injected into it. Loads
    /// grammars first if needed.
    pub fn parse(&mut self, syntax: &mut BufferSyntax, text: &Rope) -> Result<(), String> {
        let old = syntax.tree.take();
        syntax.tree = self.parse_in(syntax.language, text, &[], old.as_ref())?;
        syntax.dirty = false;
        let edited = syntax.edited.take();
        if syntax.injections_pending {
            return Ok(());
        }
        match &syntax.tree {
            Some(tree) => {
                let regions = old.map(|old| changed_regions(&old, tree, edited.clone()));
                let (language, injections) = (syntax.language, &mut syntax.injections);
                let found = Found {
                    tree,
                    host: &[],
                    regions: regions.as_deref(),
                };
                self.inject(language, found, text, injections, edited.as_ref(), 1);
            }
            None => syntax.injections = Injections::default(),
        }
        Ok(())
    }

    /// Finds and parses the injections the first parse left for later.
    /// Returns whether there were any.
    pub fn inject_pending(&mut self, syntax: &mut BufferSyntax, text: &Rope) -> bool {
        if !syntax.injections_pending {
            return false;
        }
        syntax.injections_pending = false;
        let Some(tree) = &syntax.tree else {
            return false;
        };
        let found = Found {
            tree,
            host: &[],
            regions: None,
        };
        let (language, injections) = (syntax.language, &mut syntax.injections);
        self.inject(language, found, text, injections, None, 1);
        !injections.layers.is_empty()
    }

    /// Parses only `ranges` of `text`, or all of it if there are none.
    fn parse_in(
        &mut self,
        id: usize,
        text: &Rope,
        ranges: &[TsRange],
        old: Option<&Tree>,
    ) -> Result<Option<Tree>, String> {
        self.ensure_loaded(id)?;
        let EntryState::Loaded { language, .. } = &self.list[id].state else {
            unreachable!("loaded above");
        };
        if self.parser.set_language(language).is_err()
            || self.parser.set_included_ranges(ranges).is_err()
        {
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

    /// Brings the injections of a tree of `language` up to date: looks for
    /// them again where the tree changed, keeps the layers whose text did
    /// not change, and parses the others again from their old trees.
    fn inject(
        &mut self,
        language: usize,
        found: Found,
        text: &Rope,
        injections: &mut Injections,
        edited: Option<&Range<usize>>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        self.find_injections(language, &found, text, &mut injections.found);
        let mut old: HashMap<LayerKey, Layer> = std::mem::take(&mut injections.layers)
            .into_iter()
            .map(|layer| {
                // Where it starts moved along with the edits.
                let key = match layer.key {
                    LayerKey::At(language, _) => LayerKey::At(language, layer.ranges[0].start_byte),
                    key => key,
                };
                (key, layer)
            })
            .collect();
        for (key, language, ranges) in group(&injections.found) {
            let old = old.remove(&key);
            let unchanged = old
                .as_ref()
                .is_some_and(|layer| !layer.touched && same_bytes(&layer.ranges, &ranges));
            if unchanged && let Some(mut layer) = old {
                layer.key = key;
                layer.ranges = ranges;
                injections.layers.push(layer);
                continue;
            }
            let (old_tree, mut inner) = match old {
                Some(mut layer) => {
                    mark_range_changes(&mut layer.tree, &layer.ranges, &ranges);
                    (Some(layer.tree), layer.injections)
                }
                None => (None, Injections::default()),
            };
            // A grammar that fails to load was reported when a file of its
            // own type was opened, or will be; here it just shows as text.
            let Ok(Some(tree)) = self.parse_in(language, text, &ranges, old_tree.as_ref()) else {
                continue;
            };
            let regions = old_tree.map(|old| changed_regions(&old, &tree, edited.cloned()));
            let found = Found {
                tree: &tree,
                host: &ranges,
                regions: regions.as_deref(),
            };
            self.inject(language, found, text, &mut inner, edited, depth + 1);
            injections.layers.push(Layer {
                key,
                language,
                ranges,
                tree,
                touched: false,
                injections: inner,
            });
        }
    }

    /// Runs the `injections` query of `language` over the regions of
    /// `found`, and puts what it finds there in place of what `injections`
    /// had. Follows tree-sitter's conventions: the captures
    /// `@injection.content` and `@injection.language`, and the settings
    /// `injection.language`, `injection.combined`, and
    /// `injection.include-children`.
    fn find_injections(
        &self,
        language: usize,
        found: &Found,
        text: &Rope,
        injections: &mut Vec<Injection>,
    ) {
        let EntryState::Loaded {
            injections: Some(query),
            ..
        } = &self.list[language].state
        else {
            injections.clear();
            return;
        };
        let Some(content) = query.capture_index_for_name("injection.content") else {
            injections.clear();
            return;
        };
        let named = query.capture_index_for_name("injection.language");
        let everywhere = 0..usize::MAX;
        let mut new = Vec::new();
        let mut cursor = QueryCursor::new();
        for region in found.regions.unwrap_or(std::slice::from_ref(&everywhere)) {
            cursor.set_byte_range(region.clone());
            let mut matches = cursor.matches(query, found.tree.root_node(), node_text(text));
            while let Some(m) = matches.next() {
                let settings = query.property_settings(m.pattern_index);
                let setting = |key: &str| settings.iter().find(|p| &*p.key == key);
                let name = match m.captures().iter().find(|c| Some(c.index) == named) {
                    Some(c) => text.byte_slice(c.node.byte_range()).to_string(),
                    None => match setting("injection.language").and_then(|p| p.value.as_deref()) {
                        Some(name) => name.to_string(),
                        None => continue,
                    },
                };
                // "rust,ignore" and "python title=x" name their language
                // first.
                let name = name
                    .trim()
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .unwrap_or_default();
                let Some(injected) = self.for_injection(name) else {
                    continue;
                };
                let include_children = setting("injection.include-children").is_some();
                let ranges: Vec<TsRange> = m
                    .captures()
                    .iter()
                    .filter(|c| c.index == content)
                    .flat_map(|c| content_ranges(c.node, include_children, found.host))
                    .filter(|r| r.start_byte < r.end_byte)
                    .collect();
                if !ranges.is_empty() {
                    new.push(Injection {
                        language: injected,
                        combined: setting("injection.combined").map(|_| m.pattern_index),
                        ranges,
                    });
                }
            }
        }
        match found.regions {
            None => *injections = new,
            Some(regions) => {
                // A match can reach into two regions.
                new.sort_by_key(|i| i.ranges[0].start_byte);
                new.dedup_by(|a, b| a.language == b.language && same_bytes(&a.ranges, &b.ranges));
                injections.retain(|old| {
                    let (start, end) = old.span();
                    !regions.iter().any(|r| r.start <= end && start <= r.end)
                        && !new.iter().any(|n| {
                            let (s, e) = n.span();
                            s < end && start < e
                        })
                });
                injections.append(&mut new);
                injections.sort_by_key(|i| i.ranges[0].start_byte);
            }
        }
    }

    /// The highlight style of each byte in `range`, or `None` for plain text.
    /// Injected languages paint over the language around them.
    pub fn highlight(
        &self,
        theme: &Theme,
        syntax: &BufferSyntax,
        text: &Rope,
        range: Range<usize>,
    ) -> Vec<Option<Style>> {
        let mut styles = vec![None; range.len()];
        if let Some(tree) = &syntax.tree {
            self.paint_layer(theme, syntax.language, tree, &[], text, &range, &mut styles);
            self.paint_layers(theme, &syntax.injections.layers, text, &range, &mut styles);
        }
        styles
    }

    fn paint_layers(
        &self,
        theme: &Theme,
        layers: &[Layer],
        text: &Rope,
        range: &Range<usize>,
        styles: &mut [Option<Style>],
    ) {
        for layer in layers {
            // Injected layers lie inside their host's ranges, so ones out of
            // sight hide their own injections too.
            if clip(&layer.ranges, range).is_empty() {
                continue;
            }
            let ranges = &layer.ranges;
            self.paint_layer(
                theme,
                layer.language,
                &layer.tree,
                ranges,
                text,
                range,
                styles,
            );
            self.paint_layers(theme, &layer.injections.layers, text, range, styles);
        }
    }

    /// Paints the highlights of one tree over `styles`, which covers
    /// `range`, but only within `ranges` if there are any.
    #[allow(clippy::too_many_arguments)]
    fn paint_layer(
        &self,
        theme: &Theme,
        language: usize,
        tree: &Tree,
        ranges: &[TsRange],
        text: &Rope,
        range: &Range<usize>,
        styles: &mut [Option<Style>],
    ) {
        let EntryState::Loaded {
            highlights: Some(highlights),
            ..
        } = &self.list[language].state
        else {
            return;
        };
        // Looked up per frame, so theme changes show at once; queries have
        // a few dozen captures.
        let capture_styles: Vec<Option<Style>> = highlights
            .capture_names()
            .iter()
            .map(|name| theme.style(name))
            .collect();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut spans = Vec::new();
        let mut captures = cursor.captures(highlights, tree.root_node(), node_text(text));
        while let Some((found, index)) = captures.next() {
            let capture = found.captures()[*index];
            if let Some(style) = capture_styles[capture.index as usize] {
                spans.push((capture.node.byte_range(), found.pattern_index, style));
            }
        }
        let mask = (!ranges.is_empty()).then(|| clip(ranges, range));
        paint(styles, range.start, spans, mask);
    }

    /// Where `capture` of the query `name` matched in `range`, one range per
    /// match, sorted and without repeats. Compiles the query the first time;
    /// the error comes back once, and later runs find nothing.
    pub fn captures(
        &mut self,
        language: usize,
        name: &str,
        capture: &str,
        tree: &Tree,
        text: &Rope,
        range: Range<usize>,
    ) -> Result<Vec<Range<usize>>, String> {
        let entry = &mut self.list[language];
        let EntryState::Loaded {
            language, queries, ..
        } = &mut entry.state
        else {
            return Ok(Vec::new());
        };
        let Some(state) = queries.get_mut(name) else {
            return Ok(Vec::new());
        };
        if let QueryState::Source(source) = state {
            match Query::new(language, source) {
                Ok(query) => *state = QueryState::Ready(query),
                Err(err) => {
                    *state = QueryState::Failed;
                    return Err(format!("{} {name}: {err}", entry.name));
                }
            }
        }
        let QueryState::Ready(query) = state else {
            return Ok(Vec::new());
        };
        let Some(index) = query.capture_index_for_name(capture) else {
            return Ok(Vec::new());
        };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range);
        let mut found = Vec::new();
        let mut matches = cursor.matches(query, tree.root_node(), node_text(text));
        while let Some(m) = matches.next() {
            let span = m
                .captures()
                .iter()
                .filter(|c| c.index == index)
                .map(|c| c.node.byte_range())
                .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end));
            found.extend(span);
        }
        found.sort_by_key(|r| (r.start, r.end));
        found.dedup();
        Ok(found)
    }
}

fn node_text<'a>(text: &'a Rope) -> impl TextProvider<&'a [u8]> + 'a {
    |node: Node| {
        text.byte_slice(node.byte_range())
            .chunks()
            .map(str::as_bytes)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn info(node: Node) -> NodeInfo {
    NodeInfo {
        id: node.id() as u64,
        kind: node.kind().to_string(),
        named: node.is_named(),
        range: node.byte_range(),
    }
}

/// The smallest node covering `range`, or the smallest named one.
pub(crate) fn node_at(tree: &Tree, range: Range<usize>, named: bool) -> Option<NodeInfo> {
    let root = tree.root_node();
    let node = if named {
        root.named_descendant_for_byte_range(range.start, range.end)
    } else {
        root.descendant_for_byte_range(range.start, range.end)
    };
    node.map(info)
}

pub(crate) fn parent(tree: &Tree, of: &NodeInfo) -> Option<NodeInfo> {
    find(tree, of)?.parent().map(info)
}

pub(crate) fn children(tree: &Tree, of: &NodeInfo) -> Vec<NodeInfo> {
    let Some(node) = find(tree, of) else {
        return Vec::new();
    };
    let mut cursor = node.walk();
    node.children(&mut cursor).map(info).collect()
}

/// Finds a node handed out earlier. Nodes with its range cover the smallest
/// node there, so it is among that node's ancestors if it still exists.
fn find<'t>(tree: &'t Tree, of: &NodeInfo) -> Option<Node<'t>> {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(of.range.start, of.range.end)?;
    loop {
        if node.id() as u64 == of.id {
            return (node.byte_range() == of.range).then_some(node);
        }
        node = node.parent()?;
    }
}

/// Paints `spans` into `styles`, which covers bytes from `offset`, but
/// only within `mask` if there is one. Inner spans win over the ones
/// around them; for the same range, the pattern written first in the query
/// wins, as in tree-sitter's highlighting.
fn paint(
    styles: &mut [Option<Style>],
    offset: usize,
    mut spans: Vec<(Range<usize>, usize, Style)>,
    mask: Option<Vec<Range<usize>>>,
) {
    spans.sort_by_key(|(range, pattern, _)| (range.start, std::cmp::Reverse(range.end), *pattern));
    let whole = offset..offset + styles.len();
    let mask = mask.as_deref().unwrap_or(std::slice::from_ref(&whole));
    let mut last: Option<Range<usize>> = None;
    for (range, _, style) in spans {
        if last.as_ref() == Some(&range) {
            continue;
        }
        let first = mask.partition_point(|m| m.end <= range.start);
        for part in &mask[first..] {
            if part.start >= range.end {
                break;
            }
            let start = range
                .start
                .max(part.start)
                .saturating_sub(offset)
                .min(styles.len());
            let end = range
                .end
                .min(part.end)
                .saturating_sub(offset)
                .min(styles.len());
            for slot in &mut styles[start..end] {
                *slot = Some(style);
            }
        }
        last = Some(range);
    }
}

/// The byte ranges of `ranges` within `range`.
fn clip(ranges: &[TsRange], range: &Range<usize>) -> Vec<Range<usize>> {
    let first = ranges.partition_point(|r| r.end_byte <= range.start);
    ranges[first..]
        .iter()
        .take_while(|r| r.start_byte < range.end)
        .map(|r| r.start_byte.max(range.start)..r.end_byte.min(range.end))
        .filter(|r| !r.is_empty())
        .collect()
}

/// Tells `tree` about the text that joined or left its ranges without an
/// edit to it, such as a comment made a doc comment: tree-sitter reuses
/// what no edit reached, so it would not read that text otherwise. Each
/// such range becomes an edit that replaces it with itself, from the end
/// of the range before it: text after the end of the tree reaches no node
/// otherwise.
fn mark_range_changes(tree: &mut Tree, old: &[TsRange], new: &[TsRange]) {
    let key = |r: &TsRange| (r.start_byte, r.end_byte);
    let (mut i, mut j) = (0, 0);
    let mut before: Option<(usize, Point)> = None;
    while i < old.len() || j < new.len() {
        let changed = match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if key(a) == key(b) => {
                i += 1;
                j += 1;
                before = Some((a.end_byte, a.end_point));
                continue;
            }
            (Some(a), Some(b)) if a.start_byte <= b.start_byte => {
                i += 1;
                a
            }
            (Some(a), None) => {
                i += 1;
                a
            }
            (_, Some(b)) => {
                j += 1;
                b
            }
            (None, None) => unreachable!("checked by the loop"),
        };
        let (start_byte, start_position) = before
            .filter(|(byte, _)| *byte <= changed.start_byte)
            .unwrap_or((changed.start_byte, changed.start_point));
        tree.edit(&InputEdit {
            start_byte,
            old_end_byte: changed.end_byte,
            new_end_byte: changed.end_byte,
            start_position,
            old_end_position: changed.end_point,
            new_end_position: changed.end_point,
        });
    }
}

fn same_bytes(a: &[TsRange], b: &[TsRange]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| (a.start_byte, a.end_byte) == (b.start_byte, b.end_byte))
}

/// The text of an injection's `node`: without its named children unless
/// `include_children`, and within `host` if it is not empty. Unnamed
/// children, such as the `(` in a Markdown code block, are text like the
/// rest.
fn content_ranges(node: Node, include_children: bool, host: &[TsRange]) -> Vec<TsRange> {
    let mut ranges = Vec::new();
    if include_children {
        ranges.push(node.range());
    } else {
        let mut start = (node.start_byte(), node.start_position());
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() > start.0 {
                ranges.push(ts_range(
                    start,
                    (child.start_byte(), child.start_position()),
                ));
            }
            if child.end_byte() > start.0 {
                start = (child.end_byte(), child.end_position());
            }
        }
        if node.end_byte() > start.0 {
            ranges.push(ts_range(start, (node.end_byte(), node.end_position())));
        }
    }
    if host.is_empty() {
        return ranges;
    }
    let mut within = Vec::new();
    for range in ranges {
        let first = host.partition_point(|h| h.end_byte <= range.start_byte);
        for h in host[first..]
            .iter()
            .take_while(|h| h.start_byte < range.end_byte)
        {
            let start = if h.start_byte > range.start_byte {
                (h.start_byte, h.start_point)
            } else {
                (range.start_byte, range.start_point)
            };
            let end = if h.end_byte < range.end_byte {
                (h.end_byte, h.end_point)
            } else {
                (range.end_byte, range.end_point)
            };
            if start.0 < end.0 {
                within.push(ts_range(start, end));
            }
        }
    }
    within
}

fn ts_range(start: (usize, Point), end: (usize, Point)) -> TsRange {
    TsRange {
        start_byte: start.0,
        start_point: start.1,
        end_byte: end.0,
        end_point: end.1,
    }
}

impl BufferSyntax {
    pub fn new(language: usize) -> Self {
        Self {
            language,
            tree: None,
            dirty: true,
            edited: None,
            injections: Injections::default(),
            injections_pending: true,
        }
    }

    /// Tells the tree that `old` became `new` between `start` and the ends,
    /// so the next parse only redoes what changed.
    pub fn edit(&mut self, old: &Rope, new: &Rope, start: usize, old_end: usize, new_end: usize) {
        let edit = InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(old, start),
            old_end_position: point(old, old_end),
            new_end_position: point(new, new_end),
        };
        if let Some(tree) = &mut self.tree {
            tree.edit(&edit);
        }
        edit_injections(&mut self.injections, &edit);
        self.edited = Some(match self.edited.take() {
            Some(edited) => {
                let (edited_start, edited_end) =
                    (shift(edited.start, &edit), shift(edited.end, &edit));
                edited_start.min(start)..edited_end.max(new_end)
            }
            None => start..new_end,
        });
        self.dirty = true;
    }

    /// Drops the trees when an edit cannot be described, e.g. after undo.
    pub fn invalidate(&mut self) {
        self.tree = None;
        self.edited = None;
        self.injections = Injections::default();
        self.dirty = true;
    }
}

/// Moves what was found and the layers' trees along with an edit.
fn edit_injections(injections: &mut Injections, edit: &InputEdit) {
    for injection in &mut injections.found {
        injection
            .ranges
            .iter_mut()
            .for_each(|r| shift_range(r, edit));
    }
    for layer in &mut injections.layers {
        layer.tree.edit(edit);
        // Touching counts: typing at the end of a range may extend it.
        layer.touched |= layer
            .ranges
            .iter()
            .any(|r| edit.start_byte <= r.end_byte && r.start_byte <= edit.old_end_byte);
        layer.ranges.iter_mut().for_each(|r| shift_range(r, edit));
        edit_injections(&mut layer.injections, edit);
    }
}

/// Where `byte` is after `edit`. Bytes inside the edited text move to its
/// start.
fn shift(byte: usize, edit: &InputEdit) -> usize {
    if byte >= edit.old_end_byte {
        byte - edit.old_end_byte + edit.new_end_byte
    } else {
        byte.min(edit.start_byte)
    }
}

fn shift_range(range: &mut TsRange, edit: &InputEdit) {
    for (byte, point) in [
        (&mut range.start_byte, &mut range.start_point),
        (&mut range.end_byte, &mut range.end_point),
    ] {
        if *byte >= edit.old_end_byte {
            if point.row == edit.old_end_position.row {
                point.column =
                    point.column - edit.old_end_position.column + edit.new_end_position.column;
            }
            point.row = point.row - edit.old_end_position.row + edit.new_end_position.row;
        } else if *byte > edit.start_byte {
            *point = edit.start_position;
        }
        *byte = shift(*byte, edit);
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
            None,
        );
        let expected = [4, 0, 1, 1, 2, 1, 1, 1, 0, 0];
        for (i, n) in expected.into_iter().enumerate() {
            assert_eq!(styles[i], (n > 0).then(|| style(n)), "byte {i}");
        }
    }
}
