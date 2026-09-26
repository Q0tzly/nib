//! Plugins from elsewhere (docs/plugin-install.md): packing them into
//! `.nib.tar.gz` archives, and adding, updating, and removing them. No
//! plugin code runs here.

use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use nib_core::{API_VERSION, PluginManifest, read_manifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SUFFIX: &str = ".nib.tar.gz";
/// The most an archive may unpack to.
const MAX_SIZE: u64 = 100 << 20;

/// Where installed plugins live: `<data>/installed/<name>/`, and the
/// record of them, `<data>/installed.toml`.
pub struct Store {
    pub data: PathBuf,
}

/// An installed plugin, as `installed.toml` keeps it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Record {
    pub name: String,
    /// As it was given to `add`.
    pub source: String,
    /// The archive fetched.
    pub url: String,
    /// The release's tag, for GitHub.
    pub tag: Option<String>,
    pub version: String,
    pub sha256: String,
    /// The capabilities agreed to.
    pub capabilities: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Records {
    #[serde(default, rename = "plugin")]
    plugins: Vec<Record>,
}

impl Store {
    pub fn dir(&self, name: &str) -> PathBuf {
        self.data.join("installed").join(name)
    }

    fn record_file(&self) -> PathBuf {
        self.data.join("installed.toml")
    }

    pub fn records(&self) -> Result<Vec<Record>, String> {
        let file = self.record_file();
        match fs::read_to_string(&file) {
            Ok(text) => toml::from_str::<Records>(&text)
                .map(|r| r.plugins)
                .map_err(|err| format!("{}: {err}", file.display())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(format!("{}: {err}", file.display())),
        }
    }

    fn save(&self, plugins: Vec<Record>) -> Result<(), String> {
        let text = toml::to_string(&Records { plugins }).map_err(|err| err.to_string())?;
        let file = self.record_file();
        fs::create_dir_all(&self.data).map_err(|err| format!("{}: {err}", self.data.display()))?;
        fs::write(&file, text).map_err(|err| format!("{}: {err}", file.display()))
    }
}

/// `nib plugin pack <dir>`: writes `<name>-<version>.nib.tar.gz` into `out`.
pub fn pack(dir: &Path, out: &Path) -> Result<PathBuf, String> {
    let manifest = read_manifest(dir).map_err(|err| err.to_string())?;
    let file = out.join(format!("{}-{}{SUFFIX}", manifest.name, manifest.version));
    let fail = |err: io::Error| format!("{}: {err}", file.display());
    let encoder = GzEncoder::new(File::create(&file).map_err(fail)?, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let mut files = Vec::new();
    collect(dir, dir, &mut files).map_err(|err| format!("{}: {err}", dir.display()))?;
    files.sort();
    for (name, path) in files {
        archive.append_path_with_name(&path, &name).map_err(fail)?;
    }
    archive
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(fail)?;
    Ok(file)
}

/// The files under `dir`, with their paths relative to `root` using `/`.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            let relative = path.strip_prefix(root).expect("under root");
            let name: Vec<_> = relative.iter().map(|p| p.to_string_lossy()).collect();
            files.push((name.join("/"), path));
        }
    }
    Ok(())
}

/// Unpacks `archive` into `dest`. Only plain files and directories inside
/// `dest` are allowed, and no more than `MAX_SIZE` in all.
pub fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    let fail = |err: io::Error| format!("{}: {err}", archive.display());
    let file = File::open(archive).map_err(fail)?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let mut total = 0;
    for entry in tar.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?.into_owned();
        let inside = path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !inside {
            return Err(format!("{} is outside the plugin", path.display()));
        }
        match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Directory => {}
            _ => return Err(format!("{}: only files and directories", path.display())),
        }
        total += entry.header().size().map_err(fail)?;
        if total > MAX_SIZE {
            return Err(format!("it unpacks to more than {} MiB", MAX_SIZE >> 20));
        }
        entry.unpack_in(dest).map_err(fail)?;
    }
    Ok(())
}

/// Where a plugin comes from, as given to `add`.
#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    GitHub {
        owner: String,
        repo: String,
        /// A release's tag; otherwise the latest release.
        tag: Option<String>,
    },
    Url(String),
    /// An archive on this machine, as when trying one's own build.
    File(PathBuf),
}

impl Source {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.ends_with(SUFFIX) {
            if text.starts_with("https://") {
                return Ok(Source::Url(text.into()));
            }
            if text.contains("://") {
                return Err(format!("{text}: only https URLs"));
            }
            return Ok(Source::File(text.into()));
        }
        let rest = text.strip_prefix("https://").unwrap_or(text);
        let rest = rest.strip_prefix("github.com/").unwrap_or(rest);
        let (path, tag) = match rest.split_once('@') {
            Some((path, tag)) => (path, Some(tag.to_string())),
            None => (rest, None),
        };
        let usage = || {
            format!("{text}: expected owner/repo, github.com/owner/repo, or a URL of a *{SUFFIX}")
        };
        let (owner, repo) = path
            .trim_end_matches('/')
            .split_once('/')
            .ok_or_else(usage)?;
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return Err(usage());
        }
        Ok(Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            tag,
        })
    }

    /// Whether `update` follows it: a GitHub repository with no tag given.
    fn follows_releases(&self) -> bool {
        matches!(self, Source::GitHub { tag: None, .. })
    }
}

/// An archive ready to install, and where it came from.
struct Fetched {
    archive: PathBuf,
    url: String,
    tag: Option<String>,
}

/// Downloads what `source` points at into `work`.
fn fetch(source: &Source, work: &Path) -> Result<Fetched, String> {
    let (url, tag) = match source {
        Source::File(path) => {
            return Ok(Fetched {
                archive: path.clone(),
                url: path.display().to_string(),
                tag: None,
            });
        }
        Source::Url(url) => (url.clone(), None),
        Source::GitHub { owner, repo, tag } => {
            let release = match tag {
                Some(tag) => format!("tags/{tag}"),
                None => "latest".into(),
            };
            let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/{release}");
            let response = work.join("release.json");
            curl(&api, &response)?;
            let text = fs::read_to_string(&response).map_err(|err| err.to_string())?;
            let release: Value =
                serde_json::from_str(&text).map_err(|err| format!("{api}: {err}"))?;
            let (url, tag) =
                release_asset(&release).map_err(|err| format!("{owner}/{repo}: {err}"))?;
            (url, Some(tag))
        }
    };
    let archive = work.join(format!("plugin{SUFFIX}"));
    curl(&url, &archive)?;
    Ok(Fetched { archive, url, tag })
}

/// The URL of the release's one plugin archive, and the release's tag.
fn release_asset(release: &Value) -> Result<(String, String), String> {
    let tag = release["tag_name"].as_str().unwrap_or_default().to_string();
    let archives: Vec<&str> = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|asset| asset["name"].as_str().is_some_and(|n| n.ends_with(SUFFIX)))
        .filter_map(|asset| asset["browser_download_url"].as_str())
        .collect();
    match archives[..] {
        [url] => Ok((url.to_string(), tag)),
        [] => Err(format!("release {tag} has no *{SUFFIX} file")),
        _ => Err(format!("release {tag} has more than one *{SUFFIX} file")),
    }
}

fn curl(url: &str, out: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--output"])
        .arg(out)
        .arg(url)
        .status()
        .map_err(|err| format!("running curl failed: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("downloading {url} failed"))
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Asks the user a yes-or-no question on the terminal. No answer, as when
/// input is not a terminal, is no.
pub fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
    answer.trim().eq_ignore_ascii_case("y")
}

/// What a plugin is and may do, for the user to agree to.
fn describe(manifest: &PluginManifest, source: &str) -> String {
    let mut text = format!("{} {} from {source}\n", manifest.name, manifest.version);
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    text += &format!("  capabilities: {}\n", list(&manifest.capabilities));
    text += &format!("  events: {}\n", list(&manifest.events));
    if !manifest.languages.is_empty() {
        text += &format!("  languages: {}\n", manifest.languages.join(", "));
    }
    text
}

/// A directory to work in under the store, removed when dropped.
struct Work(PathBuf);

impl Work {
    fn new(store: &Store) -> Result<Self, String> {
        let dir = store
            .data
            .join("installed")
            .join(format!(".work-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("plugin"))
            .map_err(|err| format!("{}: {err}", dir.display()))?;
        Ok(Self(dir))
    }

    fn plugin(&self) -> PathBuf {
        self.0.join("plugin")
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Downloads, unpacks, and checks the plugin at `source`.
fn prepare(source: &Source, work: &Work) -> Result<(Fetched, PluginManifest), String> {
    let fetched = fetch(source, &work.0)?;
    unpack(&fetched.archive, &work.plugin())?;
    let manifest = read_manifest(&work.plugin()).map_err(|err| err.to_string())?;
    if manifest.api != API_VERSION {
        return Err(format!(
            "{} is for nib's plugin API {}, but this nib has {API_VERSION}",
            manifest.name, manifest.api
        ));
    }
    Ok((fetched, manifest))
}

/// Puts the unpacked plugin in its place, replacing an older one.
fn put_in_place(store: &Store, work: &Work, name: &str) -> Result<(), String> {
    let dir = store.dir(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    fs::rename(work.plugin(), &dir).map_err(|err| format!("{}: {err}", dir.display()))
}

/// `nib plugin add`. `builtin` names the plugins built into nib, which an
/// installed one may not share a name with. Returns the plugin's name, or
/// `None` when the user said no.
pub fn add(
    store: &Store,
    text: &str,
    builtin: &[&str],
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Option<String>, String> {
    let source = Source::parse(text)?;
    let work = Work::new(store)?;
    let (fetched, manifest) = prepare(&source, &work)?;
    let name = &manifest.name;
    if builtin.contains(&name.as_str()) {
        return Err(format!("{name} is the name of a plugin built into nib"));
    }
    let mut records = store.records()?;
    if let Some(other) = records.iter().find(|r| &r.name == name && r.source != text) {
        return Err(format!(
            "{name} is installed already, from {}; remove it first",
            other.source
        ));
    }
    let question = format!("{}Install it?", describe(&manifest, text));
    if !confirm(&question) {
        return Ok(None);
    }
    put_in_place(store, &work, name)?;
    records.retain(|r| &r.name != name);
    records.push(Record {
        name: name.clone(),
        source: text.into(),
        sha256: sha256(&fetched.archive)?,
        url: fetched.url,
        tag: fetched.tag,
        version: manifest.version.clone(),
        capabilities: manifest.capabilities.clone(),
    });
    records.sort_by(|a, b| a.name.cmp(&b.name));
    store.save(records)?;
    Ok(Some(name.clone()))
}

/// What `update` did to one plugin.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Updated {
        from: String,
        to: String,
    },
    UpToDate,
    /// Installed from a tag or an archive, so it stays as it is.
    Pinned,
    /// It asks for more capabilities, and the user said no.
    Declined,
}

/// `nib plugin update` for one installed plugin. The user is asked again
/// only when the new version wants capabilities not agreed to before.
pub fn update(
    store: &Store,
    name: &str,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Outcome, String> {
    let records = store.records()?;
    let record = records
        .iter()
        .find(|r| r.name == name)
        .cloned()
        .ok_or_else(|| format!("{name} is not installed"))?;
    let source = Source::parse(&record.source)?;
    if !source.follows_releases() {
        return Ok(Outcome::Pinned);
    }
    update_from(store, record, &source, confirm)
}

/// Updates `record`'s plugin from `source`.
fn update_from(
    store: &Store,
    record: Record,
    source: &Source,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Outcome, String> {
    let name = record.name.as_str();
    let mut records = store.records()?;
    let work = Work::new(store)?;
    let (fetched, manifest) = prepare(source, &work)?;
    if manifest.name != name {
        return Err(format!(
            "{} now holds a plugin named {}",
            record.source, manifest.name
        ));
    }
    let sha256 = sha256(&fetched.archive)?;
    if sha256 == record.sha256 {
        return Ok(Outcome::UpToDate);
    }
    let added: Vec<String> = manifest
        .capabilities
        .iter()
        .filter(|c| !record.capabilities.contains(c))
        .cloned()
        .collect();
    if !added.is_empty() {
        let question = format!(
            "{}It now also wants: {}. Update it?",
            describe(&manifest, &record.source),
            added.join(", ")
        );
        if !confirm(&question) {
            return Ok(Outcome::Declined);
        }
    }
    put_in_place(store, &work, name)?;
    for r in &mut records {
        if r.name == name {
            r.url = fetched.url.clone();
            r.tag = fetched.tag.clone();
            r.version = manifest.version.clone();
            r.sha256 = sha256.clone();
            r.capabilities = manifest.capabilities.clone();
        }
    }
    store.save(records)?;
    Ok(Outcome::Updated {
        from: record.version,
        to: manifest.version,
    })
}

/// `nib plugin remove`: the installed files and the record go; settings and
/// data the plugin kept stay.
pub fn remove(store: &Store, name: &str) -> Result<(), String> {
    let mut records = store.records()?;
    if !records.iter().any(|r| r.name == name) {
        return Err(format!("{name} is not installed"));
    }
    let dir = store.dir(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    records.retain(|r| r.name != name);
    store.save(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A plugin directory named `name` asking for `capabilities`.
    fn plugin(root: &Path, name: &str, version: &str, capabilities: &str, api: &str) -> PathBuf {
        let dir = root.join(format!("src-{name}-{version}"));
        fs::create_dir_all(dir.join("queries")).unwrap();
        fs::write(
            dir.join("plugin.toml"),
            format!(
                "name = \"{name}\"\nversion = \"{version}\"\napi = \"{api}\"\ncapabilities = [{capabilities}]\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("plugin.wasm"), b"\0asm").unwrap();
        fs::write(dir.join("queries/a.scm"), "(x)").unwrap();
        dir
    }

    fn packed(root: &Path, name: &str, version: &str, capabilities: &str) -> String {
        let dir = plugin(root, name, version, capabilities, API_VERSION);
        pack(&dir, root).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn packs_and_unpacks() {
        let temp = Temp::new("pack");
        let archive = packed(&temp.0, "foo", "0.1.0", "");
        assert!(archive.ends_with("foo-0.1.0.nib.tar.gz"));
        let dest = temp.0.join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack(Path::new(&archive), &dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("queries/a.scm")).unwrap(),
            "(x)"
        );
        assert_eq!(read_manifest(&dest).unwrap().name, "foo");
    }

    /// An archive with one entry, written as is.
    fn raw_archive(path: &Path, name: &[u8], kind: tar::EntryType) {
        let mut header = tar::Header::new_gnu();
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        header.set_entry_type(kind);
        header.set_size(1);
        if kind == tar::EntryType::Symlink {
            header.set_size(0);
            header.set_link_name("/etc/passwd").unwrap();
        }
        header.set_cksum();
        let encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let data: &[u8] = if kind == tar::EntryType::Symlink {
            b""
        } else {
            b"x"
        };
        archive.append(&header, data).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn unpacking_refuses_what_leaves_the_plugin() {
        let temp = Temp::new("unsafe");
        let dest = temp.0.join("out");
        fs::create_dir_all(&dest).unwrap();
        let archive = temp.0.join("bad.nib.tar.gz");
        raw_archive(&archive, b"../escape", tar::EntryType::Regular);
        assert!(
            unpack(&archive, &dest)
                .unwrap_err()
                .contains("outside the plugin")
        );
        raw_archive(&archive, b"link", tar::EntryType::Symlink);
        assert!(unpack(&archive, &dest).unwrap_err().contains("only files"));
        assert!(!temp.0.join("escape").exists());
    }

    #[test]
    fn sources_parse() {
        let github = |owner: &str, repo: &str, tag: Option<&str>| Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            tag: tag.map(String::from),
        };
        assert_eq!(Source::parse("a/b"), Ok(github("a", "b", None)));
        assert_eq!(
            Source::parse("github.com/a/b@v1"),
            Ok(github("a", "b", Some("v1")))
        );
        assert_eq!(
            Source::parse("https://github.com/a/b/"),
            Ok(github("a", "b", None))
        );
        assert_eq!(
            Source::parse("https://x.org/f.nib.tar.gz"),
            Ok(Source::Url("https://x.org/f.nib.tar.gz".into()))
        );
        assert!(Source::parse("http://x.org/f.nib.tar.gz").is_err());
        assert!(Source::parse("nothing").is_err());
    }

    #[test]
    fn releases_have_one_archive() {
        let release = serde_json::json!({"tag_name": "v1", "assets": [
            {"name": "notes.txt", "browser_download_url": "https://x/notes.txt"},
            {"name": "foo-1.nib.tar.gz", "browser_download_url": "https://x/foo-1.nib.tar.gz"},
        ]});
        assert_eq!(
            release_asset(&release),
            Ok(("https://x/foo-1.nib.tar.gz".into(), "v1".into()))
        );
        let none = serde_json::json!({"tag_name": "v2", "assets": []});
        assert!(
            release_asset(&none)
                .unwrap_err()
                .contains("no *.nib.tar.gz")
        );
    }

    #[test]
    fn adds_after_asking_and_removes() {
        let temp = Temp::new("add");
        let store = Store {
            data: temp.0.join("data"),
        };
        let archive = packed(&temp.0, "foo", "0.1.0", "\"clipboard\"");
        let mut asked = String::new();
        let name = add(&store, &archive, &["helix"], &mut |q| {
            asked = q.to_string();
            true
        })
        .unwrap();
        assert_eq!(name.as_deref(), Some("foo"));
        assert!(asked.contains("capabilities: clipboard"), "{asked}");
        assert!(store.dir("foo").join("plugin.wasm").is_file());
        let records = store.records().unwrap();
        assert_eq!(records[0].version, "0.1.0");
        assert_eq!(records[0].capabilities, ["clipboard"]);
        assert_eq!(records[0].sha256, sha256(Path::new(&archive)).unwrap());
        // Pinned to a file, so update leaves it.
        assert_eq!(update(&store, "foo", &mut |_| true), Ok(Outcome::Pinned));

        remove(&store, "foo").unwrap();
        assert!(!store.dir("foo").exists());
        assert!(store.records().unwrap().is_empty());
    }

    #[test]
    fn updates_ask_again_only_for_new_capabilities() {
        let temp = Temp::new("update");
        let store = Store {
            data: temp.0.join("data"),
        };
        let first = packed(&temp.0, "foo", "0.1.0", "\"clipboard\"");
        add(&store, &first, &[], &mut |_| true).unwrap();
        let record = || store.records().unwrap()[0].clone();
        let from = |path: &str| Source::File(path.into());

        let same = from(&first);
        assert_eq!(
            update_from(&store, record(), &same, &mut |_| panic!()),
            Ok(Outcome::UpToDate)
        );
        // Fewer capabilities: no question.
        let second = packed(&temp.0, "foo", "0.2.0", "");
        let updated = update_from(&store, record(), &from(&second), &mut |_| panic!());
        assert_eq!(
            updated,
            Ok(Outcome::Updated {
                from: "0.1.0".into(),
                to: "0.2.0".into()
            })
        );
        assert!(record().capabilities.is_empty());
        // More: asked, and no keeps the old one.
        let third = packed(&temp.0, "foo", "0.3.0", "\"process\"");
        let mut asked = String::new();
        let declined = update_from(&store, record(), &from(&third), &mut |q| {
            asked = q.to_string();
            false
        });
        assert_eq!(declined, Ok(Outcome::Declined));
        assert!(asked.contains("now also wants: process"), "{asked}");
        assert_eq!(record().version, "0.2.0");
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let temp = Temp::new("refuse");
        let store = Store {
            data: temp.0.join("data"),
        };
        let yes = &mut |_: &str| true;
        let helix = packed(&temp.0, "helix", "9.0.0", "");
        assert!(
            add(&store, &helix, &["helix"], yes)
                .unwrap_err()
                .contains("built into nib")
        );
        let dir = plugin(&temp.0, "old", "0.1.0", "", "0.1");
        let old = pack(&dir, &temp.0).unwrap();
        let err = add(&store, &old.to_string_lossy(), &[], yes).unwrap_err();
        assert!(err.contains("plugin API 0.1"), "{err}");
        // Saying no installs nothing.
        let foo = packed(&temp.0, "foo", "0.1.0", "");
        assert_eq!(add(&store, &foo, &[], &mut |_| false), Ok(None));
        assert!(!store.dir("foo").exists());
    }
}
