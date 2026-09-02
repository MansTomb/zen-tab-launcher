use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const MAX_FAVICON_BYTES: usize = 512 * 1024;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    targets: HashMap<String, Target>,
    #[serde(default = "default_window_class")]
    zen_window_class: String,
}

#[derive(Clone, Deserialize)]
struct Target {
    name: String,
    url: String,
    #[serde(default)]
    favicon: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "default_match")]
    r#match: MatchMode,
    #[serde(default = "default_open")]
    open: OpenMode,
    #[serde(default)]
    container: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MatchMode {
    Exact,
    Origin,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OpenMode {
    Focus,
    ReuseOrCreate,
    AlwaysNew,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tab {
    id: String,
    title: String,
    url: String,
    #[serde(default)]
    fav_icon_url: Option<String>,
}

fn default_window_class() -> String {
    "zen".to_owned()
}

fn default_match() -> MatchMode {
    MatchMode::Origin
}

fn default_open() -> OpenMode {
    OpenMode::ReuseOrCreate
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

fn config_path() -> Result<PathBuf> {
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".config"));
    Ok(root.join("zen-tab-launcher/config.json"))
}

fn applications_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("ZEN_TAB_APPLICATIONS_DIR") {
        return Ok(PathBuf::from(path));
    }
    let root = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".local/share"));
    Ok(root.join("applications"))
}

fn cache_dir() -> Result<PathBuf> {
    let root = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".cache"));
    Ok(root.join("zen-tab-launcher/favicons"))
}

fn runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let identity = env::var_os("HOME").unwrap_or_default();
            let digest = hex_digest(identity.as_encoded_bytes());
            PathBuf::from(format!("/tmp/zen-tab-launcher-{}", &digest[..12]))
        })
        .join("zen-tab-launcher")
}

fn load_config() -> Result<Config> {
    let path = config_path()?;
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let config: Config = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    if config.zen_window_class.trim().is_empty() {
        return Err("zenWindowClass must not be empty".to_owned());
    }
    for (id, target) in &config.targets {
        if id.is_empty()
            || !id.chars().all(|value| {
                value.is_ascii_lowercase() || value.is_ascii_digit() || ".-_".contains(value)
            })
        {
            return Err(format!("invalid target id: {id}"));
        }
        if target.name.trim().is_empty() {
            return Err(format!("target {id} needs a non-empty name"));
        }
        parse_web_url(&target.url)
            .map_err(|_| format!("target {id} needs an http or https URL"))?;
        if let Some(favicon) = &target.favicon {
            parse_web_url(favicon)
                .map_err(|_| format!("target {id} favicon must be an http or https URL"))?;
        }
        if target
            .container
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(format!("target {id} container must not be empty"));
        }
    }
    Ok(())
}

fn parse_web_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are supported".to_owned());
    }
    Ok(url)
}

struct LauncherEntries {
    directory: PathBuf,
    executable: PathBuf,
}

impl LauncherEntries {
    fn installed() -> Result<Self> {
        Ok(Self {
            directory: applications_dir()?,
            executable: env::current_exe().map_err(|error| error.to_string())?,
        })
    }

    fn sync_targets(&self, config: &Config, favicons: &FaviconCache) -> Result<()> {
        let mut desired = HashMap::new();
        for (id, target) in &config.targets {
            let host = parse_web_url(&target.url)?
                .host_str()
                .unwrap_or_default()
                .to_owned();
            let mut keywords = target.aliases.clone();
            keywords.push(target.url.clone());
            keywords.push(host);
            let icon = target
                .favicon
                .as_deref()
                .and_then(|source| favicons.resolve(source).ok().flatten())
                .unwrap_or_else(|| "zen-browser".to_owned());
            desired.insert(
                format!("zen-tab-target-{id}.desktop"),
                self.entry(&target.name, &keywords, &["open", id], &icon),
            );
        }
        self.reconcile("zen-tab-target-", desired)
    }

    fn sync_tabs(&self, tabs: &[Tab], favicons: &FaviconCache) -> Result<()> {
        let mut desired = HashMap::new();
        for tab in tabs {
            let Ok(url) = parse_web_url(&tab.url) else {
                continue;
            };
            if tab.id.is_empty() || tab.title.trim().is_empty() {
                continue;
            }
            let digest = hex_digest(tab.id.as_bytes());
            let icon = tab
                .fav_icon_url
                .as_deref()
                .and_then(|source| favicons.resolve(source).ok().flatten())
                .unwrap_or_else(|| "zen-browser".to_owned());
            desired.insert(
                format!("zen-tab-live-{}.desktop", &digest[..16]),
                self.entry(
                    &tab.title,
                    &[
                        tab.url.clone(),
                        url.host_str().unwrap_or_default().to_owned(),
                    ],
                    &["focus-tab", &tab.id, &tab.url],
                    &icon,
                ),
            );
        }
        self.reconcile("zen-tab-live-", desired)
    }

    fn clear_live(&self) -> Result<()> {
        self.reconcile("zen-tab-live-", HashMap::new())
    }

    fn clear_all(&self) -> Result<()> {
        self.clear_live()?;
        self.reconcile("zen-tab-target-", HashMap::new())
    }

    fn entry(&self, name: &str, keywords: &[String], arguments: &[&str], icon: &str) -> String {
        let mut command = vec![exec_quote(&self.executable.to_string_lossy())];
        command.extend(arguments.iter().map(|argument| exec_quote(argument)));
        let keywords = keywords
            .iter()
            .filter(|value| !value.is_empty())
            .map(|value| desktop_value(value).replace(';', " "))
            .collect::<Vec<_>>()
            .join(";");
        format!(
            "[Desktop Entry]\nType=Application\nName={}\nKeywords={keywords};\nExec={}\nIcon={}\nTerminal=false\nCategories=Network;WebBrowser;\nStartupNotify=false\nX-Zen-Tab-Launcher=true\n",
            desktop_value(name),
            command.join(" "),
            desktop_value(icon)
        )
    }

    fn reconcile(&self, prefix: &str, desired: HashMap<String, String>) -> Result<()> {
        fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(&self.directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.starts_with(prefix) && name.ends_with(".desktop") && !desired.contains_key(name)
            {
                fs::remove_file(path).map_err(|error| error.to_string())?;
            }
        }
        for (name, content) in desired {
            let destination = self.directory.join(&name);
            if fs::read_to_string(&destination).is_ok_and(|current| current == content) {
                continue;
            }
            atomic_write(&destination, content.as_bytes(), 0o644)?;
        }
        Ok(())
    }
}

struct FaviconCache {
    directory: PathBuf,
}

impl FaviconCache {
    fn installed() -> Result<Self> {
        Ok(Self {
            directory: cache_dir()?,
        })
    }

    fn resolve(&self, source: &str) -> Result<Option<String>> {
        let digest = hex_digest(source.as_bytes());
        if let Some(existing) = self.existing(&digest)? {
            return Ok(Some(existing.to_string_lossy().into_owned()));
        }
        let Some((extension, content)) = self.read(source)? else {
            return Ok(None);
        };
        if content.is_empty() || content.len() > MAX_FAVICON_BYTES {
            return Ok(None);
        }
        fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        let destination = self.directory.join(format!("{digest}.{extension}"));
        if !matches!(fs::read(&destination), Ok(existing) if existing == content) {
            atomic_write(&destination, &content, 0o600)?;
        }
        Ok(Some(destination.to_string_lossy().into_owned()))
    }

    fn existing(&self, digest: &str) -> Result<Option<PathBuf>> {
        if !self.directory.exists() {
            return Ok(None);
        }
        for entry in fs::read_dir(&self.directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value == digest)
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn read(&self, source: &str) -> Result<Option<(&'static str, Vec<u8>)>> {
        if let Some(data) = source.strip_prefix("data:") {
            let (metadata, encoded) = data
                .split_once(',')
                .ok_or_else(|| "invalid favicon data URL".to_owned())?;
            if !metadata.split(';').any(|field| field == "base64") {
                return Ok(None);
            }
            let content = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| error.to_string())?;
            return Ok(image_extension(&content).map(|extension| (extension, content)));
        }
        parse_web_url(source)?;
        let output = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--max-time",
                "2",
                "--max-filesize",
                &MAX_FAVICON_BYTES.to_string(),
                source,
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() || output.stdout.len() > MAX_FAVICON_BYTES {
            return Ok(None);
        }
        Ok(image_extension(&output.stdout).map(|extension| (extension, output.stdout)))
    }
}

fn image_extension(content: &[u8]) -> Option<&'static str> {
    if content.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if content.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a") {
        Some("gif")
    } else if content.starts_with(&[0, 0, 1, 0]) {
        Some("ico")
    } else if content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| "destination has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".zen-tab-{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn desktop_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(['\n', '\r'], " ")
        .trim()
        .to_owned()
}

fn exec_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
    )
}

fn hex_digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

struct NativeHost {
    config: Config,
    entries: Arc<LauncherEntries>,
    favicons: FaviconCache,
    output: Arc<Mutex<io::Stdout>>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
}

impl NativeHost {
    fn new() -> Result<Self> {
        Ok(Self {
            config: load_config()?,
            entries: Arc::new(LauncherEntries::installed()?),
            favicons: FaviconCache::installed()?,
            output: Arc::new(Mutex::new(io::stdout())),
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn run(&self) -> Result<()> {
        let directory = runtime_dir();
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let socket_path = directory.join("bridge.sock");
        if socket_path.exists() {
            fs::remove_file(&socket_path).map_err(|error| error.to_string())?;
        }
        self.entries.clear_live()?;
        self.entries.sync_targets(&self.config, &self.favicons)?;
        let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        let output = Arc::clone(&self.output);
        let pending = Arc::clone(&self.pending);
        let window_class = self.config.zen_window_class.clone();
        thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                let output = Arc::clone(&output);
                let pending = Arc::clone(&pending);
                let window_class = window_class.clone();
                thread::spawn(move || serve_client(connection, output, pending, &window_class));
            }
        });
        let mut input = io::stdin().lock();
        while let Some(message) = read_native_message(&mut input)? {
            self.handle_extension_message(message)?;
        }
        self.entries.clear_live()?;
        let _ = fs::remove_file(socket_path);
        Ok(())
    }

    fn handle_extension_message(&self, message: Value) -> Result<()> {
        match message.get("type").and_then(Value::as_str) {
            Some("snapshot") => {
                let tabs: Vec<Tab> = serde_json::from_value(
                    message.get("tabs").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| error.to_string())?;
                self.entries.sync_tabs(&tabs, &self.favicons)
            }
            Some("response") => {
                let Some(request_id) = message.get("requestId").and_then(Value::as_str) else {
                    return Ok(());
                };
                if let Some(sender) = self.pending.lock().unwrap().remove(request_id) {
                    let _ = sender.send(message);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn serve_client(
    mut stream: UnixStream,
    output: Arc<Mutex<io::Stdout>>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
    window_class: &str,
) {
    let response = (|| -> Result<Value> {
        stream
            .set_read_timeout(Some(Duration::from_secs(6)))
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let mut request: Map<String, Value> =
            serde_json::from_str(&line).map_err(|error| error.to_string())?;
        let request_id = Uuid::new_v4().simple().to_string();
        request.insert("type".to_owned(), json!("request"));
        request.insert("requestId".to_owned(), json!(request_id));
        let (sender, receiver) = mpsc::channel();
        pending.lock().unwrap().insert(request_id.clone(), sender);
        let write_result =
            write_native_message(&mut *output.lock().unwrap(), &Value::Object(request));
        if let Err(error) = write_result {
            pending.lock().unwrap().remove(&request_id);
            return Err(error);
        }
        let response = match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(response) => response,
            Err(_) => {
                pending.lock().unwrap().remove(&request_id);
                return Err("Zen did not answer within 5 seconds".to_owned());
            }
        };
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            focus_zen(window_class);
        }
        Ok(response)
    })()
    .unwrap_or_else(|error| json!({"ok": false, "error": error}));
    let _ = serde_json::to_writer(&mut stream, &response);
    let _ = stream.write_all(b"\n");
}

fn read_native_message(input: &mut impl Read) -> Result<Option<Value>> {
    let mut header = [0_u8; 4];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let length = u32::from_ne_bytes(header) as usize;
    if length > 1024 * 1024 {
        return Err("native message exceeds 1 MiB".to_owned());
    }
    let mut body = vec![0; length];
    input
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn write_native_message(output: &mut impl Write, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    output
        .write_all(&(body.len() as u32).to_ne_bytes())
        .and_then(|_| output.write_all(&body))
        .and_then(|_| output.flush())
        .map_err(|error| error.to_string())
}

fn focus_zen(window_class: &str) -> bool {
    Command::new("hyprctl")
        .args([
            "dispatch",
            "focuswindow",
            &format!("class:^({window_class})$"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn socket_request(payload: &Value) -> Result<Value> {
    let path = runtime_dir().join("bridge.sock");
    let mut stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(6)))
        .map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut stream, payload).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&response).map_err(|error| error.to_string())
}

fn launch_url(url: &str, window_class: &str) -> Result<()> {
    parse_web_url(url)?;
    Command::new("zen-browser")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    for _ in 0..10 {
        if focus_zen(window_class) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn target_request(id: &str, target: &Target) -> Value {
    json!({
        "action": "open-target",
        "targetId": id,
        "url": target.url,
        "match": match target.r#match { MatchMode::Exact => "exact", MatchMode::Origin => "origin" },
        "open": match target.open {
            OpenMode::Focus => "focus",
            OpenMode::ReuseOrCreate => "reuse-or-create",
            OpenMode::AlwaysNew => "always-new",
        },
        "container": target.container,
    })
}

fn cli(args: &[String]) -> Result<()> {
    let config = load_config()?;
    let entries = LauncherEntries::installed()?;
    let favicons = FaviconCache::installed()?;
    match args {
        [command] if command == "sync-targets" => {
            entries.clear_live()?;
            entries.sync_targets(&config, &favicons)
        }
        [command] if command == "clear-entries" => entries.clear_all(),
        [command, id, url] if command == "focus-tab" => {
            parse_web_url(url)?;
            let response = socket_request(&json!({"action": "focus-tab", "tabId": id}));
            if response
                .as_ref()
                .ok()
                .and_then(|value| value.get("ok"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                Ok(())
            } else {
                launch_url(url, &config.zen_window_class)
            }
        }
        [command, id] if command == "open" => {
            let target = config
                .targets
                .get(id)
                .ok_or_else(|| format!("unknown target: {id}"))?;
            let response = socket_request(&target_request(id, target));
            if response
                .as_ref()
                .ok()
                .and_then(|value| value.get("ok"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                Ok(())
            } else if matches!(target.open, OpenMode::Focus) {
                Err(format!("no open tab matches {id}"))
            } else {
                launch_url(&target.url, &config.zen_window_class)
            }
        }
        _ => Err(
            "usage: zen-tab <focus-tab TAB_ID URL|open TARGET|sync-targets|clear-entries>"
                .to_owned(),
        ),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = if args.first().is_some_and(|value| {
        matches!(
            value.as_str(),
            "focus-tab" | "open" | "sync-targets" | "clear-entries"
        )
    }) {
        cli(&args)
    } else {
        NativeHost::new().and_then(|host| host.run())
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entries(root: &Path) -> LauncherEntries {
        LauncherEntries {
            directory: root.to_owned(),
            executable: PathBuf::from("/opt/zen tab/zen-tab"),
        }
    }

    #[test]
    fn validates_target_urls() {
        assert!(parse_web_url("https://example.com/path").is_ok());
        assert!(parse_web_url("file:///tmp/private").is_err());
    }

    #[test]
    fn live_entry_has_favicon_and_url_fallback() {
        let root = env::temp_dir().join(format!("zen-tab-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let favicon_root = root.join("icons");
        let favicon = FaviconCache {
            directory: favicon_root.clone(),
        };
        let source = "data:image/png;base64,iVBORw0KGgo=";
        test_entries(&root)
            .sync_tabs(
                &[Tab {
                    id: "42".to_owned(),
                    title: "Issue tracker".to_owned(),
                    url: "https://github.com/acme/issues".to_owned(),
                    fav_icon_url: Some(source.to_owned()),
                }],
                &favicon,
            )
            .unwrap();
        let entry = fs::read_to_string(
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .map(|value| value.path())
                .find(|path| path.extension().is_some_and(|value| value == "desktop"))
                .unwrap(),
        )
        .unwrap();
        assert!(entry.contains("Name=Issue tracker"));
        assert!(entry.contains("github.com"));
        assert!(entry.contains("\"focus-tab\" \"42\" \"https://github.com/acme/issues\""));
        assert!(entry.contains(&format!("Icon={}", favicon_root.display())));
        assert!(!entry.contains("GenericName=Zen tab"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_preserves_unowned_entries() {
        let root = env::temp_dir().join(format!("zen-tab-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("unrelated.desktop"), "keep").unwrap();
        test_entries(&root).clear_all().unwrap();
        assert_eq!(
            fs::read_to_string(root.join("unrelated.desktop")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_messages_round_trip() {
        let value = json!({"type": "response", "ok": true});
        let mut bytes = Vec::new();
        write_native_message(&mut bytes, &value).unwrap();
        assert_eq!(
            read_native_message(&mut bytes.as_slice()).unwrap(),
            Some(value)
        );
    }
}
