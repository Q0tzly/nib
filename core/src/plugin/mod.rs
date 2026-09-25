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
    component: Component,
    config: String,
    instance: Option<Instance>,
    enabled: bool,
    crashes: Vec<Instant>,
    slow_calls: u32,
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

    /// Loads the plugin in `dir` and calls its `init` with `config`, a JSON
    /// object.
    pub fn load_plugin(&mut self, dir: &Path, config: &str) -> Result<(), Error> {
        let manifest = manifest::read(&dir.join("plugin.toml"))?;
        let fail = |message: String| Error::Plugin(format!("{}: {message}", manifest.name));
        if manifest.api != API_VERSION {
            return Err(fail(format!(
                "needs API {}, but nib provides {API_VERSION}",
                manifest.api
            )));
        }
        if self.plugins.entries.iter().any(|p| p.name == manifest.name) {
            return Err(fail("already loaded".into()));
        }

        if self.plugins.runtime.is_none() {
            let runtime = Runtime::new(&self.plugins.options).map_err(|err| {
                Error::Plugin(format!("starting the plugin runtime failed: {err}"))
            })?;
            self.plugins.runtime = Some(runtime);
        }
        let engine = &self.plugins.runtime.as_ref().expect("created above").engine;
        let component = Component::from_file(engine, dir.join("plugin.wasm"))
            .map_err(|err| fail(format!("{err:#}")))?;

        let id = self.plugins.entries.len();
        self.plugins.entries.push(Plugin {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            component,
            config: config.into(),
            instance: None,
            enabled: true,
            crashes: Vec::new(),
            slow_calls: 0,
        });
        if let Err(message) = self.start_plugin(id) {
            self.plugins.entries.pop();
            return Err(fail(message));
        }
        Ok(())
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
            })
            .collect()
    }

    /// Restarts every plugin, including disabled ones, forgetting their past
    /// failures.
    pub(crate) fn restart_plugins(&mut self) {
        let mut failures = Vec::new();
        for id in 0..self.plugins.entries.len() {
            self.stop_plugin(id);
            let plugin = &mut self.plugins.entries[id];
            plugin.crashes.clear();
            plugin.enabled = true;
            if let Err(err) = self.start_plugin(id) {
                let plugin = &mut self.plugins.entries[id];
                plugin.enabled = false;
                failures.push(format!("{}: {err}", plugin.name));
            }
        }
        let message = match (self.plugins.entries.is_empty(), failures.is_empty()) {
            (true, _) => "no plugins are loaded".to_string(),
            (false, true) => "plugins restarted".to_string(),
            (false, false) => format!("restarting failed: {}", failures.join("; ")),
        };
        self.state_mut().message = Some(message);
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
        if result.is_err() {
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
        self.state_mut().layers.retain(|&layer| layer != id);
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
