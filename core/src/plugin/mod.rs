//! Loads WebAssembly plugins and calls into them with time and memory limits.

mod api;
mod manifest;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;

use crate::Error;
use crate::config::Settings;
use crate::editor::{CORE_COMMANDS, Editor, State};
use crate::events::Event;
use crate::input::KeyEvent;
use api::bindings;
use api::bindings::exports::nib::plugin::guest::KeyResult;

pub type PluginId = usize;

/// Reads the name of the plugin in `dir` from its manifest.
pub fn plugin_name(dir: &Path) -> Result<String, Error> {
    Ok(manifest::read(&dir.join("plugin.toml"))?.name)
}

/// The version of `nib:plugin` this host implements.
pub const API_VERSION: &str = "0.1";

/// How often the epoch advances during a plugin call. Timeouts are
/// accurate to about one tick.
const EPOCH_TICK: Duration = Duration::from_millis(10);
/// A plugin that fails this many times within `CRASH_WINDOW` is disabled.
const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const STDERR_CAPACITY: usize = 64 * 1024;
/// At most this many events are delivered in one go, so plugins that keep
/// answering each other's events cannot hang the editor.
const MAX_EVENTS: usize = 1000;

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
            call_timeout: Settings::default().plugin_timeout,
            init_timeout: Settings::default().plugin_init_timeout,
            memory_limit: Settings::default().plugin_memory,
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
    /// Has code to run; otherwise it only provides data, such as languages.
    pub has_code: bool,
    /// A call taking longer is stopped.
    pub timeout: Duration,
}

/// Where a plugin's manifest and code come from.
enum Source<'a> {
    Dir(&'a Path),
    /// Built into the editor: the manifest and the other files by path.
    Bytes {
        manifest: &'a str,
        files: &'a [(&'a str, &'a [u8])],
    },
}

impl Source<'_> {
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Source::Dir(dir) => match std::fs::read(dir.join(path)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(format!("{path}: {err}")),
            },
            Source::Bytes { files, .. } => Ok(files
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, bytes)| bytes.to_vec())),
        }
    }
}

#[derive(Default)]
pub(crate) struct Plugins {
    pub(crate) options: PluginOptions,
    /// Created when the first plugin is loaded, so starting without plugins
    /// costs nothing.
    runtime: Option<Runtime>,
    entries: Vec<Plugin>,
    /// Plugins that failed in a call made from another plugin, with the
    /// reason. They are restarted once the outermost call returns.
    failures: Vec<(PluginId, String)>,
}

struct Runtime {
    engine: Engine,
    linker: Linker<PluginData>,
    ticker: Ticker,
}

/// Advances the engine's epoch while a plugin call runs, and sleeps
/// otherwise, so an idle editor never wakes up for it.
struct Ticker {
    /// Calls in progress; nested calls count too.
    calls: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: thread::Thread,
}

impl Ticker {
    fn start(engine: Engine) -> std::io::Result<Self> {
        let calls = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_calls, thread_stop) = (calls.clone(), stop.clone());
        let handle = thread::Builder::new()
            .name("nib-epoch".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    if thread_calls.load(Ordering::Acquire) > 0 {
                        thread::sleep(EPOCH_TICK);
                        engine.increment_epoch();
                    } else {
                        // An unpark that comes first makes this return at once.
                        thread::park();
                    }
                }
            })?;
        Ok(Self {
            calls,
            stop,
            thread: handle.thread().clone(),
        })
    }

    fn begin(&self) {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            self.thread.unpark();
        }
    }

    fn end(&self) {
        self.calls.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }
}

struct Plugin {
    name: String,
    version: String,
    /// `None` for plugins built into the editor.
    dir: Option<PathBuf>,
    /// `None` for plugins that only provide data, such as languages.
    component: Option<Component>,
    /// `[settings]` from its `plugins/<name>.toml`, as JSON.
    config: String,
    limits: Limits,
    /// The kinds of events it gets.
    subscriptions: Vec<String>,
    /// Out of `Plugins` while a call runs: calling it again then would
    /// re-enter it.
    instance: Option<Instance>,
    in_call: bool,
    enabled: bool,
    crashes: Vec<Instant>,
    slow_calls: u32,
    last_error: Option<String>,
}

/// Limits for one plugin: its own from `plugins/<name>.toml`, or the
/// defaults.
#[derive(Clone, Copy)]
struct Limits {
    call: Duration,
    init: Duration,
    memory: usize,
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
    /// The other plugins, present only during a call, so it can call them.
    plugins: Option<Plugins>,
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

        let ticker = Ticker::start(engine.clone())?;
        Ok(Self {
            engine,
            linker,
            ticker,
        })
    }
}

impl Editor {
    /// Takes effect for plugins loaded afterwards.
    pub fn set_plugin_options(&mut self, options: PluginOptions) {
        self.plugins.options = options;
    }

    /// Where compiled plugins are cached. Takes effect for plugins loaded
    /// afterwards.
    pub fn set_plugin_cache_dir(&mut self, dir: Option<PathBuf>) {
        self.state_mut().languages.cache_dir = dir.clone();
        self.plugins.options.cache_dir = dir;
    }

    /// Loads the plugin in `dir` and calls its `init` with its table from
    /// config.toml.
    pub fn load_plugin(&mut self, dir: &Path) -> Result<(), Error> {
        self.add_plugin(Source::Dir(dir))
    }

    /// Loads a plugin built into the editor from its manifest and its other
    /// files by path, such as `plugin.wasm`.
    pub fn load_builtin_plugin(
        &mut self,
        manifest: &str,
        files: &[(&str, &[u8])],
    ) -> Result<(), Error> {
        self.add_plugin(Source::Bytes { manifest, files })
    }

    fn add_plugin(&mut self, source: Source) -> Result<(), Error> {
        let (manifest, component) = self.compile(&source)?;
        if self.plugins.entries.iter().any(|p| p.name == manifest.name) {
            return Err(Error::Plugin(format!("{}: already loaded", manifest.name)));
        }
        self.add_languages(&manifest, &source)
            .map_err(|err| Error::Plugin(format!("{}: {err}", manifest.name)))?;
        let settings = self.plugin_config(&manifest.name);
        let options = &self.plugins.options;
        let limits = Limits {
            call: settings.timeout.unwrap_or(options.call_timeout),
            init: settings.init_timeout.unwrap_or(options.init_timeout),
            memory: settings.memory.unwrap_or(options.memory_limit),
        };
        let id = self.plugins.entries.len();
        self.plugins.entries.push(Plugin {
            name: manifest.name.clone(),
            version: manifest.version,
            dir: match source {
                Source::Dir(dir) => Some(dir.to_path_buf()),
                Source::Bytes { .. } => None,
            },
            component,
            config: settings.settings,
            limits,
            subscriptions: manifest.events,
            instance: None,
            in_call: false,
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
    /// Registers the languages a plugin provides, and gives them to open
    /// buffers of their file types.
    fn add_languages(
        &mut self,
        manifest: &manifest::Manifest,
        source: &Source,
    ) -> Result<(), String> {
        for language in &manifest.languages {
            let grammar = source
                .read(&language.grammar)?
                .ok_or_else(|| format!("{} is missing", language.grammar))?;
            let mut queries = BTreeMap::new();
            for (name, path) in &language.queries {
                let bytes = source
                    .read(path)?
                    .ok_or_else(|| format!("{path} is missing"))?;
                let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
                queries.insert(name.clone(), text);
            }
            self.state_mut().languages.add(
                &language.name,
                language.file_types.clone(),
                grammar,
                queries,
            );
        }
        if !manifest.languages.is_empty() {
            self.state_mut().attach_syntax();
        }
        Ok(())
    }

    /// Reads and checks the manifest, then compiles the component, if the
    /// plugin has code.
    fn compile(
        &mut self,
        source: &Source,
    ) -> Result<(manifest::Manifest, Option<Component>), Error> {
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
        let Some(wasm) = source.read("plugin.wasm").map_err(fail)? else {
            // Data only, such as a language.
            return Ok((manifest, None));
        };
        if self.plugins.runtime.is_none() {
            let runtime = Runtime::new(&self.plugins.options).map_err(|err| {
                Error::Plugin(format!("starting the plugin runtime failed: {err}"))
            })?;
            self.plugins.runtime = Some(runtime);
        }
        let engine = &self.plugins.runtime.as_ref().expect("created above").engine;
        let component = Component::new(engine, &wasm).map_err(|err| fail(format!("{err:#}")))?;
        Ok((manifest, Some(component)))
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
        self.add_languages(&manifest, &Source::Dir(&dir))?;
        let plugin = &mut self.plugins.entries[id];
        plugin.version = manifest.version;
        plugin.subscriptions = manifest.events;
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
                has_code: p.component.is_some(),
                timeout: p.limits.call,
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
        let Some(component) = self.plugins.entries[id].component.clone() else {
            // Data only: nothing runs.
            return Ok(());
        };
        let runtime = self.plugins.runtime.as_ref().expect("runtime exists");
        let plugin = &mut self.plugins.entries[id];
        let limits = plugin.limits;

        let stderr = MemoryOutputPipe::new(STDERR_CAPACITY);
        let data = PluginData {
            state: None,
            plugins: None,
            plugin: id,
            wasi: WasiCtx::builder().stderr(stderr.clone()).build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(limits.memory).build(),
        };
        let mut store = Store::new(&runtime.engine, data);
        store.limiter(|data| &mut data.limits);
        store.set_epoch_deadline(ticks(limits.init));
        let bindings = bindings::Plugin::instantiate(&mut store, &component, &runtime.linker)
            .map_err(|err| format!("{err:#}"))?;
        plugin.instance = Some(Instance {
            store,
            bindings,
            stderr,
        });

        let config = plugin.config.clone();
        let timeout = limits.init;
        let result = self.invoke(id, timeout, |bindings, store| {
            bindings.nib_plugin_guest().call_init(store, &config)
        });
        let result = match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(message),
            Err(err) => Err(describe_failure(&self.plugins.entries[id], &err)),
        };
        if let Err(message) = &result {
            self.plugins.entries[id].last_error = Some(message.clone());
            self.stop_plugin(id);
        }
        self.handle_nested_failures();
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
        let timeout = self.plugins.entries[id].limits.call;
        let result = self.invoke(id, timeout, f);
        let value = match result {
            Ok(value) => Some(value),
            Err(err) => {
                let reason = describe_failure(&self.plugins.entries[id], &err);
                self.plugin_failed(id, reason);
                None
            }
        };
        self.handle_nested_failures();
        value
    }

    fn invoke<R>(
        &mut self,
        id: PluginId,
        timeout: Duration,
        f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
    ) -> wasmtime::Result<R> {
        call_in(&mut self.plugins, &mut self.state, id, timeout, f)
    }

    /// Restarts or disables the plugins that failed in calls from other
    /// plugins.
    fn handle_nested_failures(&mut self) {
        while !self.plugins.failures.is_empty() {
            let (id, reason) = self.plugins.failures.remove(0);
            self.plugin_failed(id, reason);
        }
    }

    fn plugin_failed(&mut self, id: PluginId, reason: String) {
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

    /// Calls a command a plugin registered, or a core command.
    pub fn call_command(&mut self, name: &str, args: &str) -> Result<String, String> {
        let command = self
            .state()
            .commands
            .iter()
            .find(|command| command.name == name)
            .cloned();
        let result = match command {
            Some(command) => self
                .call_plugin(command.owner, |bindings, store| {
                    bindings
                        .nib_plugin_guest()
                        .call_run_command(store, &command.short, args)
                })
                .unwrap_or_else(|| Err(format!("{name}: the plugin is not running"))),
            None => self.state_mut().run_command(name, args),
        };
        self.deliver_events();
        result
    }

    /// Every command, core and registered, with its description.
    pub fn commands(&self) -> Vec<(String, String)> {
        let core = CORE_COMMANDS
            .iter()
            .map(|&(name, description)| (name.to_string(), description.to_string()));
        let registered = self
            .state()
            .commands
            .iter()
            .map(|command| (command.name.clone(), command.description.clone()));
        core.chain(registered).collect()
    }

    /// Delivers the queued events, and the ones plugins emit meanwhile, in
    /// order.
    pub fn deliver_events(&mut self) -> bool {
        let mut delivered = 0;
        while let Some((target, event)) = self.state_mut().pop_event() {
            if delivered == MAX_EVENTS {
                let state = self.state_mut();
                let dropped = 1 + state.events.len();
                state.events.clear();
                state.message = Some(format!(
                    "dropped {dropped} events: plugins sent more than {MAX_EVENTS} at once"
                ));
                break;
            }
            delivered += 1;
            let receivers: Vec<PluginId> = match target {
                Some(id) => vec![id],
                None => (0..self.plugins.entries.len())
                    .filter(|&id| {
                        self.plugins.entries[id]
                            .subscriptions
                            .iter()
                            .any(|kind| kind == event.kind())
                    })
                    .collect(),
            };
            for id in receivers {
                self.call_plugin(id, |bindings, store| {
                    bindings
                        .nib_plugin_guest()
                        .call_on_event(store, &api::wit_event(&event))
                });
            }
        }
        delivered > 0
    }

    /// When the next timer is due.
    pub fn next_timer(&self) -> Option<Instant> {
        self.state().timers.iter().map(|timer| timer.due).min()
    }

    /// Sends the timers that are due to their plugins.
    pub fn run_timers(&mut self) {
        let due = self.state_mut().take_due_timers(Instant::now());
        for timer in due {
            self.state_mut()
                .push_event(Some(timer.owner), Event::Timer(timer.id));
        }
        self.after_plugins_ran();
    }
}

/// Lends the editor state and the other plugins to plugin `id`'s store for
/// the duration of `f`. Its instance is out of `plugins` meanwhile, so a
/// call back into it finds it busy.
fn call_in<R>(
    plugins: &mut Plugins,
    state: &mut Option<State>,
    id: PluginId,
    timeout: Duration,
    f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
) -> wasmtime::Result<R> {
    let mut instance = plugins.entries[id]
        .instance
        .take()
        .ok_or_else(|| wasmtime::Error::msg("plugin is not running"))?;
    plugins.entries[id].in_call = true;
    let warn_after = plugins.options.warn_after;
    plugins
        .runtime
        .as_ref()
        .expect("runtime exists")
        .ticker
        .begin();

    let data = instance.store.data_mut();
    data.state = state.take();
    data.plugins = Some(std::mem::take(plugins));
    instance.store.set_epoch_deadline(ticks(timeout));
    let started = Instant::now();
    let result = f(&instance.bindings, &mut instance.store);
    let elapsed = started.elapsed();
    let data = instance.store.data_mut();
    *state = data.state.take();
    *plugins = data.plugins.take().expect("plugins come back after a call");

    plugins
        .runtime
        .as_ref()
        .expect("runtime exists")
        .ticker
        .end();
    let plugin = &mut plugins.entries[id];
    plugin.in_call = false;
    if elapsed > warn_after {
        plugin.slow_calls += 1;
    }
    plugin.instance = Some(instance);
    result
}

impl PluginData {
    /// Calls `command` of plugin `owner` from within this plugin's call,
    /// passing on what this plugin was lent.
    pub(crate) fn call_plugin_command(
        &mut self,
        owner: PluginId,
        name: &str,
        short: &str,
        args: &str,
    ) -> wasmtime::Result<Result<String, String>> {
        let plugins = self
            .plugins
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("plugins are only reachable during a call"))?;
        let plugin = &plugins.entries[owner];
        if plugin.in_call {
            return Ok(Err(format!(
                "{name}: {} is in a call already and cannot be called back",
                plugin.name
            )));
        }
        if plugin.instance.is_none() {
            return Ok(Err(format!("{name}: {} is not running", plugin.name)));
        }
        let timeout = plugin.limits.call;
        let result = call_in(
            plugins,
            &mut self.state,
            owner,
            timeout,
            |bindings, store| {
                bindings
                    .nib_plugin_guest()
                    .call_run_command(store, short, args)
            },
        );
        match result {
            Ok(result) => Ok(result),
            Err(err) => {
                let plugins = self.plugins.as_mut().expect("still lent");
                let plugin = &mut plugins.entries[owner];
                let reason = describe_failure(plugin, &err);
                let message = format!("{name}: {} failed: {reason}", plugin.name);
                plugin.instance = None;
                plugins.failures.push((owner, reason));
                Ok(Err(message))
            }
        }
    }

    /// The name of this plugin, for names it registers or emits.
    pub(crate) fn plugin_name(&self) -> wasmtime::Result<&str> {
        let plugins = self
            .plugins
            .as_ref()
            .ok_or_else(|| wasmtime::Error::msg("plugins are only reachable during a call"))?;
        Ok(&plugins.entries[self.plugin].name)
    }
}

fn describe_failure(plugin: &Plugin, err: &wasmtime::Error) -> String {
    if let Some(Trap::Interrupt) = err.downcast_ref::<Trap>() {
        return "it took too long".into();
    }
    // A Rust plugin prints its panic message to stderr before trapping.
    let panic = plugin
        .instance
        .as_ref()
        .and_then(|instance| panic_message(&instance.stderr.contents()));
    panic.unwrap_or_else(|| format!("{err}"))
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
