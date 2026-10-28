//! WPF-compatible launcher-state.json. The caller supplies an isolated profile.
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub fn default_launcher_state() -> Value {
    json!({"schemaVersion":1,"quickSearchShortcut":"Control+Alt+Space","theme":"dark",
        "launcherDisplayMode":"compact","launchOnStartup":false,
        "searchEngines":[
            {"id":"google","name":"Google","template":"https://www.google.com/search?q=%s","enabled":true,"builtIn":true,"order":0},
            {"id":"baidu","name":"百度","template":"https://www.baidu.com/s?wd=%s","enabled":true,"builtIn":true,"order":1},
            {"id":"bilibili","name":"Bilibili","template":"https://search.bilibili.com/all?keyword=%s","enabled":true,"builtIn":true,"order":2}],
        "defaultSearchEngineId":"google","everythingEnabled":false,"everythingEsPath":"",
        "websites":[],"appSearchMemory":[]})
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid launcher state")
}
fn len(text: &str) -> usize {
    text.encode_utf16().count()
}
fn text(value: &Value, key: &str, min: usize, max: usize) -> bool {
    value[key]
        .as_str()
        .is_some_and(|s| (min..=max).contains(&len(s)))
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn http_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
        && !authority.is_empty()
        && !authority.contains('@')
        && !authority
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        && !s.chars().any(char::is_control)
}
pub fn is_valid_hotkey(s: &str) -> bool {
    crate::hotkey::Binding::parse(s).is_some()
}

/// Same FormKD/lowercase/letters-and-digits normalization used by SearchCore.
pub fn normalize_query(query: &str) -> String {
    use windows_sys::Win32::Globalization::{
        u_charType, u_tolower, NormalizationKD, NormalizeString,
    };
    let source: Vec<u16> = query.encode_utf16().collect();
    if source.is_empty() {
        return String::new();
    }
    let count = unsafe {
        NormalizeString(
            NormalizationKD,
            source.as_ptr(),
            source.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    let mut output = vec![0u16; count.unsigned_abs() as usize];
    let written = unsafe {
        NormalizeString(
            NormalizationKD,
            source.as_ptr(),
            source.len() as i32,
            output.as_mut_ptr(),
            output.len() as i32,
        )
    };
    let normalized = if written > 0 {
        String::from_utf16_lossy(&output[..written as usize])
    } else {
        query.to_string()
    };
    normalized
        .chars()
        .filter_map(|c| char::from_u32(unsafe { u_tolower(c as i32) } as u32))
        .filter(|c| matches!(unsafe { u_charType(*c as i32) }, 1..=5 | 9))
        .collect()
}

pub fn valid_launcher_state(state: &Value) -> bool {
    if state["schemaVersion"] != 1
        || !state["quickSearchShortcut"]
            .as_str()
            .is_some_and(is_valid_hotkey)
        || !matches!(state["theme"].as_str(), Some("light" | "dark" | "system"))
        || !matches!(
            state["launcherDisplayMode"].as_str(),
            Some("compact" | "expanded")
        )
        || !state["launchOnStartup"].is_boolean()
        || !state["everythingEnabled"].is_boolean()
    {
        return false;
    }
    let Some(path) = state["everythingEsPath"].as_str() else {
        return false;
    };
    if len(path) > 1000
        || (!path.is_empty()
            && (!Path::new(path).is_absolute()
                || !Path::new(path)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("es.exe"))))
    {
        return false;
    }
    let Some(engines) = state["searchEngines"].as_array() else {
        return false;
    };
    if !(1..=64).contains(&engines.len()) {
        return false;
    }
    let mut ids = HashSet::new();
    for engine in engines {
        let Some(id) = engine["id"].as_str() else {
            return false;
        };
        let Some(template) = engine["template"].as_str() else {
            return false;
        };
        if !identifier(id)
            || !ids.insert(id)
            || !text(engine, "name", 1, 80)
            || len(template) > 1000
            || template.matches("%s").count() != 1
            || !http_url(&template.replace("%s", "webtools"))
            || !engine["enabled"].is_boolean()
            || !engine["builtIn"].is_boolean()
            || !engine["order"]
                .as_i64()
                .is_some_and(|n| (0..=i32::MAX as i64).contains(&n))
        {
            return false;
        }
    }
    let Some(default) = state["defaultSearchEngineId"].as_str() else {
        return false;
    };
    if !identifier(default)
        || !engines
            .iter()
            .any(|e| e["id"] == default && e["enabled"] == true)
    {
        return false;
    }
    let Some(websites) = state["websites"].as_array() else {
        return false;
    };
    if websites.len() > 2000 {
        return false;
    }
    for website in websites {
        if !text(website, "id", 1, 128)
            || website["id"].as_str().unwrap().trim().is_empty()
            || !text(website, "name", 1, 256)
            || !text(website, "description", 0, 2000)
            || !text(website, "url", 1, 2048)
            || !http_url(website["url"].as_str().unwrap())
        {
            return false;
        }
        let Some(folders) = website["folderIds"].as_array() else {
            return false;
        };
        if folders.len() > 128
            || !folders
                .iter()
                .all(|f| f.as_str().is_some_and(|s| len(s) <= 128))
        {
            return false;
        }
    }
    let Some(memory) = state["appSearchMemory"].as_array() else {
        return false;
    };
    memory.len() <= 100
        && memory.iter().all(|entry| {
            let Some(query) = entry["query"].as_str() else {
                return false;
            };
            text(entry, "query", 1, 128)
                && normalize_query(query) == query
                && entry["appId"].as_str().is_some_and(valid_app_id)
                && entry["lastUsedAt"].as_i64().is_some_and(|t| t > 0)
        })
}
fn valid_app_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|c| c.is_ascii_hexdigit())
}

fn normalize_state(mut state: Value) -> Value {
    state["searchEngines"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|e| e["order"].as_i64().unwrap());
    state["appSearchMemory"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|e| std::cmp::Reverse(e["lastUsedAt"].as_i64().unwrap()));
    for website in state["websites"].as_array_mut().unwrap() {
        let mut seen = HashSet::new();
        website["folderIds"]
            .as_array_mut()
            .unwrap()
            .retain(|id| seen.insert(id.clone()));
    }
    state
}

pub struct LauncherStateStore {
    path: Option<PathBuf>,
    state: Value,
}
impl LauncherStateStore {
    pub fn memory() -> Self {
        Self {
            path: None,
            state: default_launcher_state(),
        }
    }
    pub fn load(root: &Path) -> io::Result<Self> {
        let path = root.join("launcher-state.json");
        let mut store = Self {
            path: Some(path.clone()),
            state: default_launcher_state(),
        };
        if path.exists() {
            if let Ok(state) = read_state(&path) {
                store.state = normalize_state(state);
                return Ok(store);
            }
            if let Ok(state) = read_state(&path.with_extension("json.bak")) {
                store.commit(state)?;
                return Ok(store);
            }
            fs::copy(
                &path,
                path.with_extension(format!("json.corrupt-{}", crate::new_request_id())),
            )?;
        } else if let Ok(legacy) = read_json(&root.join("nook-data.json"), 32 * 1024 * 1024) {
            if legacy["version"] == 2 {
                let mut candidate = default_launcher_state();
                for key in SETTINGS {
                    if *key == "searchEngines" {
                        continue;
                    }
                    if let Some(v) = legacy["settings"].get(*key) {
                        if (candidate[*key].is_string() && v.is_string())
                            || (candidate[*key].is_boolean() && v.is_boolean())
                        {
                            candidate[*key] = v.clone();
                        }
                    }
                }
                if let Some(engines) = legacy["settings"]["searchEngines"]
                    .as_array()
                    .filter(|rows| (1..=64).contains(&rows.len()))
                {
                    candidate["searchEngines"]=json!(engines.iter().enumerate().map(|(index,e)|json!({
                        "id":e["id"].as_str().unwrap_or(""),"name":e["name"].as_str().unwrap_or(""),
                        "template":e["template"].as_str().unwrap_or(""),"enabled":e["enabled"]!=false,
                        "builtIn":e["builtIn"]==true,"order":e["order"].as_i64().unwrap_or(index as i64).max(0)
                    })).collect::<Vec<_>>());
                }
                if !candidate["searchEngines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["id"] == candidate["defaultSearchEngineId"] && e["enabled"] == true)
                {
                    if let Some(engine) = candidate["searchEngines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|e| e["enabled"] == true)
                    {
                        candidate["defaultSearchEngineId"] = engine["id"].clone();
                    }
                }
                if let Some(rows) = legacy["webEntries"].as_array() {
                    candidate["websites"]=json!(rows.iter().take(2001).map(|row|json!({
                        "id":row["id"].as_str().unwrap_or(""),"name":row["name"].as_str().unwrap_or(""),
                        "url":row["url"].as_str().unwrap_or(""),"description":row["description"].as_str().unwrap_or(""),
                        "folderIds":row["folderIds"].as_array().map(|ids|ids.iter().filter_map(Value::as_str).take(128).collect::<Vec<_>>()).unwrap_or_default()
                    })).collect::<Vec<_>>());
                }
                if let Some(entries) = legacy["appSearchMemory"].as_object() {
                    let mut memory: Vec<Value> = entries
                        .iter()
                        .filter_map(|(query, e)| {
                            let app_id = e.get("appId")?.as_str()?;
                            let timestamp = e.get("lastUsedAt")?.as_i64()?;
                            Some(json!({"query":query,"appId":app_id,"lastUsedAt":timestamp}))
                        })
                        .collect();
                    memory
                        .sort_by_key(|e| std::cmp::Reverse(e["lastUsedAt"].as_i64().unwrap_or(0)));
                    memory.truncate(100);
                    candidate["appSearchMemory"] = json!(memory);
                }
                if valid_launcher_state(&candidate) {
                    store.state = candidate;
                }
            }
        }
        store.commit(store.state.clone())?;
        Ok(store)
    }
    pub fn snapshot(&self) -> Value {
        self.state.clone()
    }
    pub fn preview_settings(&self, update: &Value) -> io::Result<Value> {
        if !update.is_object() {
            return Err(invalid());
        }
        let mut candidate = self.snapshot();
        for key in SETTINGS {
            if let Some(value) = update.get(*key) {
                if !value.is_null() {
                    candidate[*key] = value.clone();
                }
            }
        }
        if !valid_launcher_state(&candidate) {
            return Err(invalid());
        }
        Ok(normalize_state(candidate))
    }
    pub fn apply_settings(&mut self, update: &Value) -> io::Result<Value> {
        let candidate = self.preview_settings(update)?;
        self.commit(candidate)?;
        Ok(self.snapshot())
    }
    pub fn replace_websites(&mut self, websites: &Value) -> io::Result<usize> {
        let mut candidate = self.snapshot();
        candidate["websites"] = websites.clone();
        self.commit(candidate)?;
        Ok(self.state["websites"].as_array().unwrap().len())
    }
    pub fn remembered(&self, query: &str) -> Option<String> {
        let query = normalize_query(query);
        self.state["appSearchMemory"]
            .as_array()?
            .iter()
            .find(|e| e["query"] == query)?["appId"]
            .as_str()
            .map(str::to_string)
    }
    pub fn remember(&mut self, query: &str, app_id: &str) -> io::Result<()> {
        let normalized = normalize_query(query);
        if query.trim().is_empty()
            || len(query) > 128
            || normalized.is_empty()
            || !valid_app_id(app_id)
        {
            return Err(invalid());
        }
        let mut candidate = self.snapshot();
        let rows = candidate["appSearchMemory"].as_array_mut().unwrap();
        rows.retain(|e| e["query"] != normalized);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        rows.push(json!({"query":normalized,"appId":app_id,"lastUsedAt":now}));
        rows.sort_by_key(|e| std::cmp::Reverse(e["lastUsedAt"].as_i64().unwrap()));
        rows.truncate(100);
        self.commit(candidate)
    }
    fn commit(&mut self, candidate: Value) -> io::Result<()> {
        if !valid_launcher_state(&candidate) {
            return Err(invalid());
        }
        let candidate = normalize_state(candidate);
        if let Some(path) = &self.path {
            persist(path, &serde_json::to_vec_pretty(&candidate)?)?;
        }
        self.state = candidate;
        Ok(())
    }
}
const SETTINGS: &[&str] = &[
    "quickSearchShortcut",
    "theme",
    "launcherDisplayMode",
    "launchOnStartup",
    "searchEngines",
    "defaultSearchEngineId",
    "everythingEnabled",
    "everythingEsPath",
];
fn read_json(path: &Path, max: u64) -> io::Result<Value> {
    if fs::metadata(path)?.len() > max {
        return Err(invalid());
    }
    let bytes = fs::read(path)?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn read_state(path: &Path) -> io::Result<Value> {
    let state = read_json(path, 8 * 1024 * 1024)?;
    if valid_launcher_state(&state) {
        Ok(state)
    } else {
        Err(invalid())
    }
}
pub(crate) fn persist(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("json.{}.tmp", crate::new_request_id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            fs::copy(path, path.with_extension("json.bak"))?;
        }
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
