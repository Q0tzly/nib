//! Loads WebAssembly plugins and calls into them with time and memory limits.

mod api;
mod manifest;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;

use crate::Error;
use crate::editor::{Editor, State};
use crate::input::KeyEvent;
use api::bindings;
use api::bindings::exports::nib::plugin::guest::KeyResult;

pub type PluginId = usize;

/// The version of `nib:plugin` this host implements.
pub const API_VERSION: &str = "0.1";

/// How often the epoch advances. Timeouts are accurate to about one tick.
const EPOCH_TICK: Duration = Duration::from_millis(10);
/// A plugin that fails this many times within `CRASH_WINDOW` is disabled.
const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const STDERR_CAPACITY: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct PluginOptions {
    /// Where compiled plugins are cached. `None` compiles on every load.
    pub cache_dir: Option<PathBuf>,
    /// A call taking longer is counted as slow.
    pub warn_after: Duration,
    /// A call taking longer is stopped.
    pub call_timeout: Duration,
    pub init_timeout: Duration,
    /// Maximum size of a plugin's linear memory, in bytes.
    pub memory_limit: usize,
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            warn_after: Duration::from_millis(16),
            call_timeout: Duration::from_secs(1),
            init_timeout: Duration::from_secs(5),
            memory_limit: 256 << 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// Calls that took longer than `PluginOptions::warn_after`.
    pub slow_calls: u32,
    /// Why the plugin last failed.
    pub last_error: Option<String>,
    /// Loaded from a directory, so it can be reloaded from disk.
    pub reloadable: bool,
}

/// Where a plugin's manifest and code come from.
enum Source<'a> {
    Dir(&'a Path),
    /// Built into the editor.
    Bytes {
        manifest: &'a str,
        wasm: &'a [u8],
    },
}

#[derive(Default)]
pub(crate) struct Plugins {
    options: PluginOptions,
    /// Created when the first plugin is loaded, so starting without plugins
    /// costs nothing.
    runtime: Option<Runtime>,
    entries: Vec<Plugin>,
}

struct Runtime {
    engine: Engine,
    linker: Linker<PluginData>,
    stop_ticker: Arc<AtomicBool>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop_ticker.store(true, Ordering::Relaxed);
    }
}

struct Plugin {
    name: String,
    version: String,
    /// `None` for plugins built into the editor.
    dir: Option<PathBuf>,
    component: Component,
    config: String,
    instance: Option<Instance>,
    enabled: bool,
    crashes: Vec<Instant>,
    slow_calls: u32,
    last_error: Option<String>,
}

struct Instance {
    store: Store<PluginData>,
    bindings: bindings::Plugin,
    stderr: MemoryOutputPipe,
}

/// The data of a plugin's store.
pub(crate) struct PluginData {
    /// The editor state, present only during a call.
    state: Option<State>,
    plugin: PluginId,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl Runtime {
    fn new(options: &PluginOptions) -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        if let Some(dir) = &options.cache_dir {
            let mut cache = CacheConfig::new();
            cache.with_directory(dir);
            config.cache(Some(Cache::new(cache)?));
        }
        let engine = Engine::new(&config)?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Plugin::add_to_linker::<PluginData, HasSelf<PluginData>>(&mut linker, |data| {
            data
        })?;

        let stop_ticker = Arc::new(AtomicBool::new(false));
        let (ticker_engine, stop) = (engine.clone(), stop_ticker.clone());
        thread::Builder::new()
            .name("nib-epoch".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    thread::sleep(EPOCH_TICK);
                    ticker_engine.increment_epoch();
                }
            })?;

        Ok(Self {
            engine,
            linker,
            stop_ticker,
        })
    }
}

impl Editor {
    /// Takes effect for plugins loaded afterwards.
    pub fn set_plugin_options(&mut self, options: PluginOptions) {
        self.plugins.options = options;
    }

    /// Loads the plugin in `dir` and calls its `init` with its table from
    /// config.toml.
    pub fn load_plugin(&mut self, dir: &Path) -> Result<(), Error> {
        self.add_plugin(Source::Dir(dir))
    }

    /// Loads a plugin built into the editor.
    pub fn load_builtin_plugin(&mut self, manifest: &str, wasm: &[u8]) -> Result<(), Error> {
        self.add_plugin(Source::Bytes { manifest, wasm })
    }

    fn add_plugin(&mut self, source: Source) -> Result<(), Error> {
        let (manifest, component) = self.compile(&source)?;
        if self.plugins.entries.iter().any(|p| p.name == manifest.name) {
            return Err(Error::Plugin(format!("{}: already loaded", manifest.name)));
        }
        let id = self.plugins.entries.len();
        self.plugins.entries.push(Plugin {
            name: manifest.name.clone(),
            version: manifest.version,
            dir: match source {
                Source::Dir(dir) => Some(dir.to_path_buf()),
                Source::Bytes { .. } => None,
            },
            component,
            config: self.plugin_config(&manifest.name).to_string(),
            instance: None,
            enabled: true,
            crashes: Vec::new(),
            slow_calls: 0,
            last_error: None,
        });
        if let Err(message) = self.start_plugin(id) {
            self.plugins.entries.pop();
            return Err(Error::Plugin(format!("{}: {message}", manifest.name)));
        }
        Ok(())
    }

    /// Reads and checks the manifest, then compiles the component.
    fn compile(&mut self, source: &Source) -> Result<(manifest::Manifest, Component), Error> {
        let manifest = match source {
            Source::Dir(dir) => manifest::read(&dir.join("plugin.toml"))?,
            Source::Bytes { manifest, .. } => manifest::parse(manifest, "built-in plugin")?,
        };
        let fail = |message: String| Error::Plugin(format!("{}: {message}", manifest.name));
        if manifest.api != API_VERSION {
            return Err(fail(format!(
                "needs API {}, but nib provides {API_VERSION}",
                manifest.api
            )));
        }
        if self.plugins.runtime.is_none() {
            let runtime = Runtime::new(&self.plugins.options).map_err(|err| {
                Error::Plugin(format!("starting the plugin runtime failed: {err}"))
            })?;
            self.plugins.runtime = Some(runtime);
        }
        let engine = &self.plugins.runtime.as_ref().expect("created above").engine;
        let component = match source {
            Source::Dir(dir) => Component::from_file(engine, dir.join("plugin.wasm")),
            Source::Bytes { wasm, .. } => Component::new(engine, wasm),
        }
        .map_err(|err| fail(format!("{err:#}")))?;
        Ok((manifest, component))
    }

    /// Stops the plugin, forgets its failures, and starts it again.
    pub(crate) fn restart_plugin(&mut self, id: PluginId) -> Result<(), String> {
        self.stop_plugin(id);
        let plugin = &mut self.plugins.entries[id];
        plugin.crashes.clear();
        plugin.enabled = true;
        let result = self.start_plugin(id);
        if result.is_err() {
            self.plugins.entries[id].enabled = false;
        }
        result
    }

    pub(crate) fn disable_plugin(&mut self, id: PluginId) {
        self.stop_plugin(id);
        self.plugins.entries[id].enabled = false;
    }

    /// Compiles the plugin again from its directory and restarts it.
    pub(crate) fn reload_plugin(&mut self, id: PluginId) -> Result<(), String> {
        let plugin = &self.plugins.entries[id];
        let Some(dir) = plugin.dir.clone() else {
            return Err("it is built in and has no files to reload".into());
        };
        let name = plugin.name.clone();
        let (manifest, component) = self
            .compile(&Source::Dir(&dir))
            .map_err(|err| err.to_string())?;
        if manifest.name != name {
            return Err(format!("its name changed to {}", manifest.name));
        }
        let plugin = &mut self.plugins.entries[id];
        plugin.version = manifest.version;
        plugin.component = component;
        self.restart_plugin(id)
    }

    /// Restarts every plugin, including disabled ones, forgetting their past
    /// failures.
    pub(crate) fn restart_plugins(&mut self) {
        let mut failures = Vec::new();
        for id in 0..self.plugins.entries.len() {
            if let Err(err) = self.restart_plugin(id) {
                failures.push(format!("{}: {err}", self.plugins.entries[id].name));
            }
        }
        let message = match (self.plugins.entries.is_empty(), failures.is_empty()) {
            (true, _) => "no plugins are loaded".to_string(),
            (false, true) => "plugins restarted".to_string(),
            (false, false) => format!("restarting failed: {}", failures.join("; ")),
        };
        self.state_mut().message = Some(message);
    }

    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .entries
            .iter()
            .map(|p| PluginInfo {
                name: p.name.clone(),
                version: p.version.clone(),
                enabled: p.enabled,
                slow_calls: p.slow_calls,
                last_error: p.last_error.clone(),
                reloadable: p.dir.is_some(),
            })
            .collect()
    }

    /// Returns whether the plugin handled the key. A failing plugin counts
    /// as having handled it, so the key does not fall through.
    pub(crate) fn plugin_handle_key(&mut self, id: PluginId, key: KeyEvent) -> bool {
        let key = api::key_event(key);
        let result = self.call_plugin(id, |bindings, store| {
            bindings.nib_plugin_guest().call_handle_key(store, key)
        });
        !matches!(result, Some(KeyResult::Pass))
    }

    /// Instantiates the plugin and calls `init`. On failure the plugin is
    /// left stopped.
    fn start_plugin(&mut self, id: PluginId) -> Result<(), String> {
        let runtime = self.plugins.runtime.as_ref().expect("runtime exists");
        let options = &self.plugins.options;
        let plugin = &mut self.plugins.entries[id];

        let stderr = MemoryOutputPipe::new(STDERR_CAPACITY);
        let data = PluginData {
            state: None,
            plugin: id,
            wasi: WasiCtx::builder().stderr(stderr.clone()).build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(options.memory_limit)
                .build(),
        };
        let mut store = Store::new(&runtime.engine, data);
        store.limiter(|data| &mut data.limits);
        store.set_epoch_deadline(ticks(options.init_timeout));
        let bindings =
            bindings::Plugin::instantiate(&mut store, &plugin.component, &runtime.linker)
                .map_err(|err| format!("{err:#}"))?;
        plugin.instance = Some(Instance {
            store,
            bindings,
            stderr,
        });

        let config = plugin.config.clone();
        let timeout = options.init_timeout;
        let result = self.invoke(id, timeout, |bindings, store| {
            bindings.nib_plugin_guest().call_init(store, &config)
        });
        let result = match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(message),
            Err(err) => Err(self.describe_failure(id, &err)),
        };
        if let Err(message) = &result {
            self.plugins.entries[id].last_error = Some(message.clone());
            self.stop_plugin(id);
        }
        result
    }

    /// Calls into a running plugin, handling a failure by restarting or
    /// disabling it. Returns `None` if the plugin is not running or failed.
    fn call_plugin<R>(
        &mut self,
        id: PluginId,
        f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        // A stopped plugin has nothing to call and has not failed again.
        self.plugins.entries[id].instance.as_ref()?;
        let timeout = self.plugins.options.call_timeout;
        match self.invoke(id, timeout, f) {
            Ok(value) => Some(value),
            Err(err) => {
                self.plugin_failed(id, &err);
                None
            }
        }
    }

    /// Lends the editor state to the plugin's store for the duration of `f`.
    fn invoke<R>(
        &mut self,
        id: PluginId,
        timeout: Duration,
        f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
    ) -> wasmtime::Result<R> {
        let warn_after = self.plugins.options.warn_after;
        let plugin = &mut self.plugins.entries[id];
        let instance = plugin
            .instance
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("plugin is not running"))?;

        instance.store.data_mut().state = self.state.take();
        instance.store.set_epoch_deadline(ticks(timeout));
        let started = Instant::now();
        let result = f(&instance.bindings, &mut instance.store);
        let elapsed = started.elapsed();
        self.state = instance.store.data_mut().state.take();

        if elapsed > warn_after {
            plugin.slow_calls += 1;
        }
        result
    }

    fn plugin_failed(&mut self, id: PluginId, err: &wasmtime::Error) {
        let reason = self.describe_failure(id, err);
        self.stop_plugin(id);

        let plugin = &mut self.plugins.entries[id];
        let now = Instant::now();
        plugin
            .crashes
            .retain(|&time| now.duration_since(time) < CRASH_WINDOW);
        plugin.crashes.push(now);
        plugin.last_error = Some(reason.clone());
        let name = plugin.name.clone();

        let message = if plugin.crashes.len() >= MAX_CRASHES {
            plugin.enabled = false;
            format!("plugin {name} disabled after repeated errors: {reason}")
        } else {
            match self.start_plugin(id) {
                Ok(()) => format!("plugin {name} restarted after an error: {reason}"),
                Err(err) => {
                    self.plugins.entries[id].enabled = false;
                    format!("plugin {name} disabled, restarting failed: {err}")
                }
            }
        };
        self.state_mut().message = Some(message);
    }

    /// Drops the instance and everything the plugin put into the editor.
    fn stop_plugin(&mut self, id: PluginId) {
        self.plugins.entries[id].instance = None;
        self.state_mut().remove_plugin_parts(id);
    }

    fn describe_failure(&self, id: PluginId, err: &wasmtime::Error) -> String {
        if let Some(Trap::Interrupt) = err.downcast_ref::<Trap>() {
            return "it took too long".into();
        }
        // A Rust plugin prints its panic message to stderr before trapping.
        let panic = self.plugins.entries[id]
            .instance
            .as_ref()
            .and_then(|instance| panic_message(&instance.stderr.contents()));
        panic.unwrap_or_else(|| format!("{err}"))
    }
}

/// Finds the message in Rust's "thread '...' panicked at file:line:col:"
/// output, which is on the line after that header.
fn panic_message(stderr: &[u8]) -> Option<String> {
    let stderr = String::from_utf8_lossy(stderr);
    let mut lines = stderr.lines();
    lines.find(|line| line.contains("panicked at"))?;
    lines.next().map(|line| format!("panicked: {line}"))
}

/// Epoch ticks covering at least `duration`.
fn ticks(duration: Duration) -> u64 {
    (duration.as_millis() / EPOCH_TICK.as_millis()) as u64 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_panic_message() {
        let stderr = b"thread '<unnamed>' panicked at src/lib.rs:36:30:\nasked to panic\nnote: run with `RUST_BACKTRACE=1`\n";
        assert_eq!(
            panic_message(stderr).as_deref(),
            Some("panicked: asked to panic")
        );
        assert_eq!(panic_message(b"just output\n"), None);
    }

    #[test]
    fn ticks_cover_the_duration() {
        assert_eq!(ticks(Duration::from_millis(0)), 1);
        assert_eq!(ticks(Duration::from_millis(100)), 11);
    }
}
