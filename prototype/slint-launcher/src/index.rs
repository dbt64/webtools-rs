//! Local application index.
//!
//! The WPF host and the Electron manager never talk over a live search RPC: the
//! manager publishes a JSON snapshot and both UIs read it. We read the very same
//! file, so the two implementations stay consistent by construction.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Launch {
    Shortcut {
        path: String,
        arguments: String,
        working_directory: String,
    },
    Executable(String),
    Packaged(String),
    System(String),
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub source: String,
    pub launch: Launch,
    pub icon_reference: String,
    search: SearchFields,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SearchFields {
    normalized_name: String,
    normalized_aliases: Vec<String>,
    normalized_text: String,
    name_tokens: Vec<String>,
    pinyin: String,
    initials: String,
    alias_full_pinyin: Vec<String>,
    alias_initials: Vec<String>,
}

impl SearchFields {
    fn new(name: &str, aliases: &[String], extra: &str) -> Self {
        let normalized_aliases = aliases
            .iter()
            .map(|alias| normalize_search_text(alias))
            .collect::<Vec<_>>();
        let combined = format!("{name} {} {extra}", aliases.join(" "));
        let (pinyin, initials) = pinyin_keys(name);
        let alias_readings = aliases
            .iter()
            .filter(|alias| contains_cjk(alias))
            .take(16)
            .map(|alias| pinyin_keys(&trim_runes(alias, 128)))
            .collect::<Vec<_>>();
        Self {
            normalized_name: normalize_search_text(name),
            normalized_aliases,
            normalized_text: normalize_search_text(&combined),
            name_tokens: search_tokens(name),
            pinyin,
            initials,
            alias_full_pinyin: alias_readings
                .iter()
                .map(|reading| reading.0.clone())
                .collect(),
            alias_initials: alias_readings
                .into_iter()
                .map(|reading| reading.1)
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub index: usize,
    pub score: i32,
}

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub apps: Vec<AppEntry>,
    pub origin: String,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(rename = "schemaVersion", default)]
    schema_version: i32,
    #[serde(default)]
    apps: Vec<CatalogApp>,
}

fn string_or_default<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn list_or_default<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct CatalogApp {
    #[serde(default, deserialize_with = "string_or_default")]
    id: String,
    #[serde(default, deserialize_with = "string_or_default")]
    name: String,
    #[serde(default, deserialize_with = "list_or_default")]
    aliases: Vec<String>,
    #[serde(default, deserialize_with = "string_or_default")]
    source: String,
    #[serde(default)]
    target: Target,
    #[serde(
        rename = "iconReference",
        default,
        deserialize_with = "string_or_default"
    )]
    icon_reference: String,
}

#[derive(Deserialize, Default)]
struct Target {
    #[serde(default, deserialize_with = "string_or_default")]
    kind: String,
    #[serde(rename = "appId", default, deserialize_with = "string_or_default")]
    app_id: String,
    #[serde(default, deserialize_with = "string_or_default")]
    app: String,
    #[serde(
        rename = "shortcutPath",
        default,
        deserialize_with = "string_or_default"
    )]
    shortcut_path: String,
    #[serde(rename = "targetPath", default, deserialize_with = "string_or_default")]
    target_path: String,
    #[serde(default, deserialize_with = "string_or_default")]
    path: String,
    #[serde(default, deserialize_with = "string_or_default")]
    arguments: String,
    #[serde(
        rename = "workingDirectory",
        default,
        deserialize_with = "string_or_default"
    )]
    working_directory: String,
}

// ---------------------------------------------------------------- pinyin table

static READINGS: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();

fn readings() -> &'static Vec<(u32, &'static str)> {
    READINGS.get_or_init(|| {
        let text = include_str!("../assets/pinyin-map.txt");
        let mut table: Vec<(u32, &'static str)> = Vec::with_capacity(24_000);
        for line in text.lines() {
            if let Some((hex, reading)) = line.split_once('\t') {
                if let Ok(codepoint) = u32::from_str_radix(hex.trim(), 16) {
                    table.push((codepoint, reading.trim()));
                }
            }
        }
        table.sort_unstable_by_key(|entry| entry.0);
        table
    })
}

fn syllable(ch: char) -> Option<&'static str> {
    let table = readings();
    table
        .binary_search_by_key(&(ch as u32), |entry| entry.0)
        .ok()
        .map(|position| table[position].1)
}

const PHRASES: &[(&str, &[&str])] = &[
    ("重庆", &["chong", "qing"]),
    ("银行", &["yin", "hang"]),
    ("音乐", &["yin", "yue"]),
    ("长安", &["chang", "an"]),
    ("重复", &["chong", "fu"]),
];

/// Full pinyin and first letters using the phrase overrides shared with WPF.
fn pinyin_keys(text: &str) -> (String, String) {
    let mut syllables = Vec::<String>::new();
    let mut offset = 0;
    while offset < text.len() {
        if let Some((phrase, readings)) = PHRASES
            .iter()
            .find(|(phrase, _)| text[offset..].starts_with(phrase))
        {
            syllables.extend(readings.iter().map(|reading| (*reading).to_string()));
            offset += phrase.len();
            continue;
        }
        let ch = text[offset..].chars().next().unwrap();
        syllables.push(
            syllable(ch)
                .map(str::to_string)
                .unwrap_or_else(|| ch.to_string()),
        );
        offset += ch.len_utf8();
    }
    let full = syllables.concat();
    let initials = syllables
        .iter()
        .filter_map(|reading| reading.chars().next())
        .collect::<String>();
    (
        normalize_search_text(&full),
        normalize_search_text(&initials),
    )
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
    })
}

fn trim_runes(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

// ---------------------------------------------------------------------- load

struct SystemSetting {
    uri: &'static str,
    commands: &'static [&'static str],
}

// Keep the same curated Windows Settings destinations as uTools 7.8.0.
// These are ordinary searchable commands; Windows resolves the ms-settings URI
// when the user launches one.
const WINDOWS_SETTINGS: &[SystemSetting] = &[
    SystemSetting {
        uri: "ms-settings:about",
        commands: &["电脑信息"],
    },
    SystemSetting {
        uri: "ms-settings:display",
        commands: &["显示器设置"],
    },
    SystemSetting {
        uri: "ms-settings:storagesense",
        commands: &["存储设置"],
    },
    SystemSetting {
        uri: "ms-settings:quietmomentshome",
        commands: &["专注助手设置"],
    },
    SystemSetting {
        uri: "ms-settings:multitasking",
        commands: &["多任务设置"],
    },
    SystemSetting {
        uri: "ms-settings:project",
        commands: &["投影到此电脑设置"],
    },
    SystemSetting {
        uri: "ms-settings:crossdevice",
        commands: &["共享体验设置"],
    },
    SystemSetting {
        uri: "ms-settings:tabletmode",
        commands: &["平板模式设置"],
    },
    SystemSetting {
        uri: "ms-settings:taskbar",
        commands: &["任务栏设置"],
    },
    SystemSetting {
        uri: "ms-settings:notifications",
        commands: &["通知和操作设置"],
    },
    SystemSetting {
        uri: "ms-settings:remotedesktop",
        commands: &["远程桌面设置"],
    },
    SystemSetting {
        uri: "ms-settings:powersleep",
        commands: &["电源和睡眠设置"],
    },
    SystemSetting {
        uri: "ms-settings:sound",
        commands: &["声音播放设置"],
    },
    SystemSetting {
        uri: "ms-settings:workplace",
        commands: &["访问工作单位或学校"],
    },
    SystemSetting {
        uri: "ms-settings:emailandaccounts",
        commands: &["你的电子邮件和应用账户设置"],
    },
    SystemSetting {
        uri: "ms-settings:otherusers",
        commands: &["家庭成员使用此电脑设置"],
    },
    SystemSetting {
        uri: "ms-settings:signinoptions",
        commands: &["登录选项设置"],
    },
    SystemSetting {
        uri: "ms-settings:sync",
        commands: &["同步个性化设置"],
    },
    SystemSetting {
        uri: "ms-settings:yourinfo",
        commands: &["账户设置"],
    },
    SystemSetting {
        uri: "ms-settings:appsfeatures",
        commands: &["应用和功能", "程序和功能"],
    },
    SystemSetting {
        uri: "ms-settings:appsforwebsites",
        commands: &["网站选择默认应用设置"],
    },
    SystemSetting {
        uri: "ms-settings:defaultapps",
        commands: &["默认应用设置"],
    },
    SystemSetting {
        uri: "ms-settings:maps",
        commands: &["离线地图设置"],
    },
    SystemSetting {
        uri: "ms-settings:startupapps",
        commands: &["开机启动设置"],
    },
    SystemSetting {
        uri: "ms-settings:videoplayback",
        commands: &["视频播放设置"],
    },
    SystemSetting {
        uri: "ms-settings:autoplay",
        commands: &["自动播放设置"],
    },
    SystemSetting {
        uri: "ms-settings:bluetooth",
        commands: &["蓝牙或其他设备设置"],
    },
    SystemSetting {
        uri: "ms-settings:mousetouchpad",
        commands: &["鼠标设置"],
    },
    SystemSetting {
        uri: "ms-settings:devices-touchpad",
        commands: &["触摸板设置"],
    },
    SystemSetting {
        uri: "ms-settings:pen",
        commands: &["笔设置"],
    },
    SystemSetting {
        uri: "ms-settings:printers",
        commands: &["打印机或扫描仪设置"],
    },
    SystemSetting {
        uri: "ms-settings:typing",
        commands: &["输入设置"],
    },
    SystemSetting {
        uri: "ms-settings:usb",
        commands: &["USB设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-audio",
        commands: &["音频设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-closedcaptioning",
        commands: &["隐藏式字幕设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-colorfilter",
        commands: &["颜色滤镜设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-display",
        commands: &["显示设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-highcontrast",
        commands: &["高对比度设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-keyboard",
        commands: &["键盘设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-magnifier",
        commands: &["放大镜设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-mouse",
        commands: &["键盘控制鼠标设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-narrator",
        commands: &["讲述人设置"],
    },
    SystemSetting {
        uri: "ms-settings:easeofaccess-speechrecognition",
        commands: &["语音交互设置"],
    },
    SystemSetting {
        uri: "ms-settings:fonts",
        commands: &["字体设置"],
    },
    SystemSetting {
        uri: "ms-settings:datausage",
        commands: &["网络流量使用情况"],
    },
    SystemSetting {
        uri: "ms-settings:network-dialup",
        commands: &["网络拨号设置"],
    },
    SystemSetting {
        uri: "ms-settings:network-ethernet",
        commands: &["以太网设置"],
    },
    SystemSetting {
        uri: "ms-settings:network-proxy",
        commands: &["网络代理设置"],
    },
    SystemSetting {
        uri: "ms-settings:network-status",
        commands: &["网络设置"],
    },
    SystemSetting {
        uri: "ms-settings:network-vpn",
        commands: &["VPN设置"],
    },
    SystemSetting {
        uri: "ms-settings:personalization-background",
        commands: &["桌面背景设置"],
    },
    SystemSetting {
        uri: "ms-settings:personalization-colors",
        commands: &["系统颜色设置"],
    },
    SystemSetting {
        uri: "ms-settings:lockscreen",
        commands: &["锁屏界面设置"],
    },
    SystemSetting {
        uri: "ms-settings:personalization-start",
        commands: &["开始菜单设置"],
    },
    SystemSetting {
        uri: "ms-settings:themes",
        commands: &["主题设置"],
    },
    SystemSetting {
        uri: "ms-settings:privacy",
        commands: &["权限和隐私设置"],
    },
    SystemSetting {
        uri: "ms-settings:dateandtime",
        commands: &["日期和时间设置"],
    },
    SystemSetting {
        uri: "ms-settings:regionlanguage",
        commands: &["区域和语言设置"],
    },
    SystemSetting {
        uri: "ms-settings:speech",
        commands: &["语音设置"],
    },
    SystemSetting {
        uri: "ms-settings:activation",
        commands: &["系统激活设置"],
    },
    SystemSetting {
        uri: "ms-settings:backup",
        commands: &["备份设置"],
    },
    SystemSetting {
        uri: "ms-settings:findmydevice",
        commands: &["查找我的设备"],
    },
    SystemSetting {
        uri: "ms-settings:developers",
        commands: &["开发者选项设置"],
    },
    SystemSetting {
        uri: "ms-settings:recovery",
        commands: &["系统恢复设置"],
    },
    SystemSetting {
        uri: "ms-settings:troubleshoot",
        commands: &["疑难解答"],
    },
    SystemSetting {
        uri: "ms-settings:windowsdefender",
        commands: &["安全设置"],
    },
    SystemSetting {
        uri: "ms-settings:windowsupdate-action",
        commands: &["检测系统更新"],
    },
];

fn system_apps() -> Vec<CatalogApp> {
    let mut apps = [
        (
            "file-explorer",
            "文件资源管理器",
            &["File Explorer", "Explorer", "explorer.exe"][..],
        ),
        (
            "control-panel",
            "控制面板",
            &["Control Panel", "control.exe"][..],
        ),
        (
            "device-manager",
            "设备管理器",
            &["Device Manager", "devmgmt.msc"][..],
        ),
        (
            "disk-management",
            "磁盘管理",
            &["Disk Management", "diskmgmt.msc"][..],
        ),
        (
            "recycle-bin",
            "回收站",
            &["Recycle Bin", "RecycleBin", "Trash"][..],
        ),
    ]
    .into_iter()
    .map(|(id, name, aliases)| CatalogApp {
        id: format!("system:{id}"),
        name: name.into(),
        aliases: aliases.iter().map(|value| (*value).into()).collect(),
        source: "system".into(),
        target: Target {
            kind: "system".into(),
            app: id.into(),
            ..Default::default()
        },
        icon_reference: String::new(),
    })
    .collect::<Vec<_>>();
    apps.extend(WINDOWS_SETTINGS.iter().map(|setting| {
        let id = setting
            .uri
            .strip_prefix("ms-settings:")
            .unwrap_or(setting.uri);
        CatalogApp {
            id: format!("system:windows-setting:{id}"),
            name: setting.commands[0].into(),
            aliases: setting
                .commands
                .iter()
                .skip(1)
                .map(|command| (*command).into())
                .collect(),
            source: "system".into(),
            target: Target {
                kind: "system".into(),
                app: format!("windows-setting:{id}"),
                ..Default::default()
            },
            icon_reference: String::new(),
        }
    }));
    apps
}

fn append_registered_apps(apps: &mut Vec<CatalogApp>) {
    use windows_sys::Win32::System::Registry::*;
    let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\App Paths");
    for (hive, view) in [
        (HKEY_CURRENT_USER, 0),
        (HKEY_LOCAL_MACHINE, 0),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
    ] {
        unsafe {
            let mut key = std::ptr::null_mut();
            if RegOpenKeyExW(hive, path.as_ptr(), 0, KEY_READ | view, &mut key) != 0 {
                continue;
            }
            let mut number = 0;
            loop {
                let mut name = [0u16; 512];
                let mut len = name.len() as u32;
                let status = RegEnumKeyExW(
                    key,
                    number,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if status == 259 {
                    break;
                }
                number += 1;
                if status != 0 {
                    continue;
                }
                let subkey = String::from_utf16_lossy(&name[..len as usize]);
                let mut bytes = 0u32;
                let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
                let subkey_wide = wide(&subkey);
                if RegGetValueW(
                    key,
                    subkey_wide.as_ptr(),
                    std::ptr::null(),
                    flags,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut bytes,
                ) != 0
                    || bytes == 0
                    || bytes > 65536
                {
                    continue;
                }
                let mut value = vec![0u16; (bytes as usize + 1) / 2];
                if RegGetValueW(
                    key,
                    subkey_wide.as_ptr(),
                    std::ptr::null(),
                    flags,
                    std::ptr::null_mut(),
                    value.as_mut_ptr().cast(),
                    &mut bytes,
                ) != 0
                {
                    continue;
                }
                let end = value.iter().position(|ch| *ch == 0).unwrap_or(value.len());
                let executable = String::from_utf16_lossy(&value[..end])
                    .trim()
                    .trim_matches('"')
                    .to_string();
                if !executable.to_ascii_lowercase().ends_with(".exe")
                    || !std::path::Path::new(&executable).is_file()
                {
                    continue;
                }
                let name = subkey
                    .strip_suffix(".exe")
                    .or_else(|| subkey.strip_suffix(".EXE"))
                    .unwrap_or(&subkey);
                if name.is_empty() {
                    continue;
                }
                if let Some(existing) = apps.iter_mut().find(|app| {
                    let title = app.name.to_lowercase();
                    let key = name.to_lowercase();
                    title == key
                        || (key.len() >= 4 && title.ends_with(&key))
                        || app.target.app_id.eq_ignore_ascii_case(name)
                }) {
                    if !existing
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(&subkey))
                    {
                        existing.aliases.push(subkey);
                    }
                    continue;
                }
                apps.push(CatalogApp {
                    id: format!("app-path:{executable}"),
                    name: name.into(),
                    aliases: vec![subkey],
                    source: "desktop".into(),
                    target: Target {
                        kind: "executable".into(),
                        path: executable,
                        ..Default::default()
                    },
                    icon_reference: String::new(),
                });
            }
            RegCloseKey(key);
        }
    }
}

/// Enumerate the actual Windows application catalog instead of depending on
/// a snapshot previously created by the WPF host.
fn discover_windows_apps() -> Option<Snapshot> {
    use windows::Win32::{System::Com::*, UI::Shell::*};
    unsafe fn display(item: &IShellItem, kind: SIGDN) -> Option<String> {
        let value = item.GetDisplayName(kind).ok()?;
        let text = value.to_string().ok();
        CoTaskMemFree(Some(value.0.cast()));
        text.filter(|text| !text.trim().is_empty())
    }
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<Vec<CatalogApp>> {
            let folder: IShellItem =
                SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
            let items: IEnumShellItems =
                folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems)?;
            let mut apps = system_apps();
            let mut seen = std::collections::HashSet::new();
            loop {
                let mut entry = [None];
                let mut count = 0;
                items.Next(&mut entry, Some(&mut count))?;
                if count == 0 {
                    break;
                }
                let Some(item) = entry[0].take() else {
                    continue;
                };
                let (Some(name), Some(app_id)) = (
                    display(&item, SIGDN_NORMALDISPLAY),
                    display(&item, SIGDN_PARENTRELATIVEPARSING),
                ) else {
                    continue;
                };
                if !seen.insert(app_id.to_lowercase())
                    || apps
                        .iter()
                        .any(|app| app.source == "system" && app.name.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
                apps.push(CatalogApp {
                    id: format!("apps-folder:{app_id}"),
                    name,
                    aliases: Vec::new(),
                    source: "start-apps".into(),
                    target: Target {
                        kind: "packaged".into(),
                        app_id,
                        ..Default::default()
                    },
                    icon_reference: String::new(),
                });
            }
            // AppsFolder covers Start and Store applications; desktop-only links
            // are separate shell entries and must remain searchable as well.
            for id in [&FOLDERID_Desktop, &FOLDERID_PublicDesktop] {
                let Ok(path) = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) else {
                    continue;
                };
                let root = path.to_string().ok();
                CoTaskMemFree(Some(path.0.cast()));
                let Some(root) = root else { continue };
                let mut pending = vec![std::path::PathBuf::from(root)];
                while let Some(dir) = pending.pop() {
                    let Ok(entries) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let Ok(kind) = entry.file_type() else {
                            continue;
                        };
                        if kind.is_dir() && !kind.is_symlink() {
                            pending.push(path);
                            continue;
                        }
                        if !path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
                        {
                            continue;
                        }
                        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
                            continue;
                        };
                        if apps.iter().any(|app| app.name.eq_ignore_ascii_case(name)) {
                            continue;
                        }
                        let path = path.to_string_lossy().into_owned();
                        apps.push(CatalogApp {
                            id: format!("desktop:{path}"),
                            name: name.into(),
                            aliases: Vec::new(),
                            source: "desktop".into(),
                            target: Target {
                                kind: "shortcut".into(),
                                shortcut_path: path,
                                ..Default::default()
                            },
                            icon_reference: String::new(),
                        });
                    }
                }
            }
            append_registered_apps(&mut apps);
            Ok(apps)
        })();
        if initialized {
            CoUninitialize();
        }
        match result {
            Ok(apps) => Some(Snapshot {
                schema_version: 1,
                apps,
            }),
            Err(error) => {
                eprintln!("index: Windows app discovery failed: {error}");
                None
            }
        }
    }
}

impl Index {
    pub fn load() -> Index {
        if cfg!(feature = "production") {
            if let Some(snapshot) = discover_windows_apps() {
                return Index::from_snapshot(snapshot, "Windows AppsFolder");
            }
        }
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(format!("{local}\\WebTools\\app-catalog.v1.json"));
        }
        for path in candidates {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(index) = Index::from_json(&text, &path) {
                    return index;
                }
            }
        }
        if cfg!(feature = "production") {
            return Index::from_snapshot(
                Snapshot {
                    schema_version: 1,
                    apps: system_apps(),
                },
                "Windows system apps",
            );
        }
        let fixture = include_str!("../../../fixtures/app-catalog.v1.json");
        Index::from_json(fixture, "fixture").unwrap_or(Index {
            apps: Vec::new(),
            origin: "empty".into(),
        })
    }

    fn from_json(text: &str, origin: &str) -> Option<Index> {
        let snapshot: Snapshot = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("index parse failed for {origin}: {error}");
                return None;
            }
        };
        Some(Self::from_snapshot(snapshot, origin))
    }

    fn from_snapshot(snapshot: Snapshot, origin: &str) -> Index {
        let mut apps: Vec<AppEntry> = Vec::new();
        let mut launch_positions: HashMap<String, usize> = HashMap::new();
        for app in snapshot
            .apps
            .into_iter()
            .filter(|app| !app.name.trim().is_empty())
        {
            let search = SearchFields::new(&app.name, &app.aliases, "");
            let launch = match app.target.kind.as_str() {
                "packaged" if !app.target.app_id.is_empty() => {
                    Launch::Packaged(app.target.app_id.clone())
                }
                "system" if system_launch(&app.target.app).is_some() => {
                    Launch::System(app.target.app.clone())
                }
                "shortcut" => {
                    let path = if !app.target.target_path.is_empty() {
                        app.target.target_path.clone()
                    } else {
                        app.target.shortcut_path.clone()
                    };
                    if path.is_empty() {
                        Launch::None
                    } else {
                        Launch::Shortcut {
                            path,
                            arguments: app.target.arguments.clone(),
                            working_directory: app.target.working_directory.clone(),
                        }
                    }
                }
                "executable" if !app.target.path.is_empty() => {
                    Launch::Executable(app.target.path.clone())
                }
                _ => Launch::None,
            };
            let entry = AppEntry {
                id: app.id,
                name: app.name,
                aliases: app.aliases,
                source: app.source,
                launch,
                icon_reference: app.icon_reference,
                search,
            };
            if let Some(identity) = launch_identity(&entry.launch) {
                if let Some(position) = launch_positions.get(&identity).copied() {
                    merge_duplicate(&mut apps[position], entry);
                    continue;
                }
                launch_positions.insert(identity, apps.len());
            }
            apps.push(entry);
        }
        Index {
            apps,
            origin: format!("{origin} (schema {})", snapshot.schema_version),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let needle = normalize_search_text(query.trim());
        if needle.is_empty() {
            return Vec::new();
        }
        let tokens = search_tokens(query);
        let mut hits: Vec<Hit> = Vec::new();
        for (index, app) in self.apps.iter().enumerate() {
            if !launchable(app) {
                continue;
            }
            if app.source == "start-apps"
                && is_broad_vs_tool_match(&app.search.normalized_name, &needle)
            {
                continue;
            }
            if let Some(score) = score_fields(&app.search, &needle, &tokens) {
                hits.push(Hit { index, score });
            }
        }
        hits.sort_by(|left, right| {
            right.score.cmp(&left.score).then_with(|| {
                compare_names_zh(&self.apps[left.index].name, &self.apps[right.index].name)
            })
        });
        hits.truncate(limit);
        hits
    }
}

// The Start menu exposes four architecture variants beneath Visual Studio's
// VC tools folder. Keep them searchable by their own names, but don't let a
// broad match on the shared "for VS 2022" suffix crowd out the applications.
fn is_broad_vs_tool_match(name: &str, needle: &str) -> bool {
    let Some((prefix, suffix)) = name.split_once("toolscommandpromptfor") else {
        return false;
    };
    (prefix.starts_with("x64") || prefix.starts_with("x86"))
        && (prefix.ends_with("native") || prefix.ends_with("cross"))
        && suffix.starts_with("vs")
        && suffix.contains(needle)
}

fn launchable(app: &AppEntry) -> bool {
    !matches!(app.launch, Launch::None)
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn launch_identity(launch: &Launch) -> Option<String> {
    match launch {
        Launch::Packaged(app_id) => Some(format!("packaged:{}", app_id.to_lowercase())),
        Launch::System(app) => Some(format!("system:{}", app.to_lowercase())),
        Launch::Shortcut {
            path,
            arguments,
            working_directory,
        } => Some(file_launch_identity(path, arguments, working_directory)),
        Launch::Executable(path) => Some(file_launch_identity(path, "", "")),
        Launch::None => None,
    }
}

/// System application targets supported by the WPF host catalog.
pub fn system_launch(app: &str) -> Option<(&'static str, &'static str)> {
    if let Some(id) = app.strip_prefix("windows-setting:") {
        return WINDOWS_SETTINGS
            .iter()
            .find(|setting| setting.uri.strip_prefix("ms-settings:") == Some(id))
            .map(|setting| (setting.uri, ""));
    }
    match app {
        "file-explorer" => Some(("explorer.exe", "")),
        "control-panel" => Some(("control.exe", "")),
        "device-manager" => Some(("mmc.exe", "devmgmt.msc")),
        "disk-management" => Some(("mmc.exe", "diskmgmt.msc")),
        "recycle-bin" => Some(("explorer.exe", "shell:RecycleBinFolder")),
        _ => None,
    }
}

pub fn system_icon_source(app: &str) -> Option<&'static str> {
    if app.starts_with("windows-setting:") {
        Some("shell:AppsFolder\\windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel")
    } else {
        system_launch(app).map(|(path, _)| path)
    }
}

fn file_launch_identity(path: &str, arguments: &str, working_directory: &str) -> String {
    let path_key = normalize_path(path);
    let parent = path_key
        .rsplit_once('\\')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default();
    let directory_key = normalize_path(working_directory);
    let directory_key = if directory_key == parent {
        String::new()
    } else {
        directory_key
    };
    let arguments_key = arguments
        .split_whitespace()
        .filter(|argument| {
            !argument
                .to_ascii_lowercase()
                .starts_with("--source-shortcut-location=")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("file:{path_key}\u{1f}{arguments_key}\u{1f}{directory_key}")
}

fn source_priority(source: &str) -> u8 {
    match source {
        "packaged" | "start-apps" => 3,
        "desktop" => 2,
        _ => 1,
    }
}

fn normalize_search_text(value: &str) -> String {
    webtools_ipc::state::normalize_query(value)
}

fn search_tokens(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Default)]
struct MappedSearchText {
    text: String,
    // Each normalized span points back to a complete character in the title.
    spans: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
}

impl MappedSearchText {
    fn push(&mut self, text: &str, original: std::ops::Range<usize>) {
        let normalized = normalize_search_text(text);
        if normalized.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(&normalized);
        self.spans.push((start..self.text.len(), original));
    }

    fn find(&self, needle: &str) -> Option<Vec<std::ops::Range<usize>>> {
        let start = self.text.find(needle)?;
        let end = start + needle.len();
        Some(merge_highlight_ranges(
            self.spans
                .iter()
                .filter(|(normalized, _)| normalized.start < end && normalized.end > start)
                .map(|(_, original)| original.clone())
                .collect(),
        ))
    }
}

fn merge_highlight_ranges(ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

struct SearchWord {
    text: String,
    characters: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
}

fn title_words(title: &str) -> Vec<SearchWord> {
    let mut words = Vec::new();
    let mut word: Option<SearchWord> = None;
    for (offset, ch) in title.char_indices() {
        if ch.is_alphanumeric() {
            let word = word.get_or_insert_with(|| SearchWord {
                text: String::new(),
                characters: Vec::new(),
            });
            let start = word.text.len();
            word.text.extend(ch.to_lowercase());
            word.characters
                .push((start..word.text.len(), offset..offset + ch.len_utf8()));
        } else if let Some(word) = word.take() {
            words.push(word);
        }
    }
    if let Some(word) = word {
        words.push(word);
    }
    words
}

fn word_match_ranges(
    query: &[String],
    words: &[SearchWord],
    query_index: usize,
    word_index: usize,
) -> Option<Vec<std::ops::Range<usize>>> {
    if query_index == query.len() {
        return Some(Vec::new());
    }
    let word = words.get(word_index)?;
    let token = &query[query_index];
    if word.text.starts_with(token) {
        if let Some(rest) = word_match_ranges(query, words, query_index + 1, word_index + 1) {
            let mut ranges: Vec<_> = word
                .characters
                .iter()
                .filter(|(normalized, _)| normalized.start < token.len())
                .map(|(_, original)| original.clone())
                .collect();
            ranges.extend(rest);
            return Some(ranges);
        }
    }
    let mut initials = String::new();
    for end in word_index..words.len() {
        if let Some(first) = words[end].text.chars().next() {
            initials.push(first);
        }
        if initials == *token {
            if let Some(rest) = word_match_ranges(query, words, query_index + 1, end + 1) {
                let mut ranges: Vec<_> = words[word_index..=end]
                    .iter()
                    .filter_map(|word| {
                        word.characters
                            .first()
                            .map(|(_, original)| original.clone())
                    })
                    .collect();
                ranges.extend(rest);
                return Some(ranges);
            }
        }
        if initials.len() >= token.len() {
            break;
        }
    }
    None
}

/// Map the same visible-title matches used by search back to UTF-8 title ranges.
pub fn match_highlight_ranges(title: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let needle = normalize_search_text(query.trim());
    if needle.is_empty() {
        return Vec::new();
    }

    let mut direct = MappedSearchText::default();
    for (offset, ch) in title.char_indices() {
        direct.push(&ch.to_string(), offset..offset + ch.len_utf8());
    }
    if let Some(ranges) = direct.find(&needle) {
        return ranges;
    }

    let tokens = search_tokens(query);
    if !tokens.is_empty() {
        if let Some(ranges) = word_match_ranges(&tokens, &title_words(title), 0, 0) {
            return merge_highlight_ranges(ranges);
        }
    }

    let mut full = MappedSearchText::default();
    let mut initials = MappedSearchText::default();
    let mut offset = 0;
    while offset < title.len() {
        if let Some((phrase, readings)) = PHRASES
            .iter()
            .find(|(phrase, _)| title[offset..].starts_with(phrase))
        {
            for ((position, ch), reading) in phrase.char_indices().zip(readings.iter()) {
                let original = offset + position..offset + position + ch.len_utf8();
                full.push(reading, original.clone());
                if let Some(first) = reading.chars().next() {
                    initials.push(&first.to_string(), original);
                }
            }
            offset += phrase.len();
            continue;
        }
        let ch = title[offset..].chars().next().unwrap();
        let reading = syllable(ch)
            .map(str::to_string)
            .unwrap_or_else(|| ch.to_string());
        let original = offset..offset + ch.len_utf8();
        full.push(&reading, original.clone());
        if let Some(first) = reading.chars().next() {
            initials.push(&first.to_string(), original);
        }
        offset += ch.len_utf8();
    }
    full.find(&needle)
        .or_else(|| initials.find(&needle))
        .unwrap_or_default()
}

fn add_alias(aliases: &mut Vec<String>, value: &str) {
    if !value.trim().is_empty()
        && !aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(value))
    {
        aliases.push(value.to_string());
    }
}

fn merge_duplicate(existing: &mut AppEntry, incoming: AppEntry) {
    let preferred_is_incoming =
        source_priority(&incoming.source) > source_priority(&existing.source);
    let mut aliases = existing.aliases.clone();
    add_alias(&mut aliases, &existing.name);
    for alias in &incoming.aliases {
        add_alias(&mut aliases, alias);
    }
    add_alias(&mut aliases, &incoming.name);

    if preferred_is_incoming {
        let mut preferred = incoming;
        preferred.aliases = aliases;
        *existing = preferred;
    } else {
        existing.aliases = aliases;
    }
    existing.search = SearchFields::new(&existing.name, &existing.aliases, "");
}

fn matches_word_initials(
    query: &[String],
    name: &[String],
    query_index: usize,
    name_index: usize,
) -> bool {
    if query_index == query.len() {
        return true;
    }
    if name_index >= name.len() {
        return false;
    }
    let token = &query[query_index];
    if name[name_index].starts_with(token)
        && matches_word_initials(query, name, query_index + 1, name_index + 1)
    {
        return true;
    }

    let mut initials = String::new();
    for end in name_index..name.len() {
        if let Some(first) = name[end].chars().next() {
            initials.push(first);
        }
        if initials == *token && matches_word_initials(query, name, query_index + 1, end + 1) {
            return true;
        }
        if initials.len() >= token.len() {
            break;
        }
    }
    false
}

/// Scores website and feature entries with the same name, alias and pinyin
/// matching used for local applications.
pub fn score_entry(name: &str, aliases: &[String], extra: &str, needle: &str) -> Option<i32> {
    let normalized_needle = normalize_search_text(needle);
    if normalized_needle.is_empty() {
        return None;
    }
    let fields = SearchFields::new(name, aliases, extra);
    score_fields(&fields, &normalized_needle, &search_tokens(needle))
}

fn score_fields(fields: &SearchFields, needle: &str, tokens: &[String]) -> Option<i32> {
    let rank = if fields.normalized_name == needle {
        0
    } else if fields.normalized_name.starts_with(needle) {
        1
    } else if fields
        .normalized_aliases
        .iter()
        .any(|alias| alias == needle)
    {
        2
    } else if matches_word_initials(tokens, &fields.name_tokens, 0, 0) {
        3
    } else if fields.normalized_name.contains(needle) {
        4
    } else if fields
        .normalized_aliases
        .iter()
        .any(|alias| alias.starts_with(needle))
    {
        5
    } else if fields.pinyin.starts_with(needle) {
        6
    } else if fields.pinyin.contains(needle) {
        7
    } else if fields.initials.starts_with(needle) {
        8
    } else if fields.initials.contains(needle) {
        9
    } else if fields
        .alias_full_pinyin
        .iter()
        .any(|alias| alias.starts_with(needle))
    {
        10
    } else if fields
        .alias_full_pinyin
        .iter()
        .any(|alias| alias.contains(needle))
    {
        11
    } else if fields
        .alias_initials
        .iter()
        .any(|alias| alias.starts_with(needle))
    {
        12
    } else if fields
        .alias_initials
        .iter()
        .any(|alias| alias.contains(needle))
    {
        13
    } else if fields.normalized_text.contains(needle) {
        14
    } else {
        return None;
    };
    Some(1000 - rank * 50)
}

/// Uses the same zh-CN collation as the WPF launcher for equal-rank results.
pub fn compare_names_zh(left: &str, right: &str) -> Ordering {
    use windows_sys::Win32::Globalization::{
        CompareStringEx, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN,
    };
    let locale: Vec<u16> = "zh-CN\0".encode_utf16().collect();
    let left_wide: Vec<u16> = left.encode_utf16().collect();
    let right_wide: Vec<u16> = right.encode_utf16().collect();
    let result = unsafe {
        CompareStringEx(
            locale.as_ptr(),
            0,
            left_wide.as_ptr(),
            left_wide.len() as i32,
            right_wide.as_ptr(),
            right_wide.len() as i32,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    match result {
        CSTR_LESS_THAN => Ordering::Less,
        CSTR_GREATER_THAN => Ordering::Greater,
        CSTR_EQUAL => Ordering::Equal,
        _ => left.cmp(right),
    }
}

// -------------------------------------------------------------------- launch

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Index {
    /// What the entry would run, for dry-run diagnostics.
    pub fn command(&self, position: usize) -> Option<String> {
        match self.apps.get(position)?.launch.clone() {
            Launch::Shortcut { path, .. } | Launch::Executable(path) => Some(path),
            Launch::Packaged(id) => Some(format!("shell:AppsFolder\\{id}")),
            Launch::System(app) => system_launch(&app).map(|(path, _)| path.to_string()),
            Launch::None => None,
        }
    }

    pub fn launch(&self, position: usize) -> Result<String, String> {
        let app = self
            .apps
            .get(position)
            .ok_or_else(|| String::from("索引越界"))?;
        launch_entry(app)
    }
}

pub fn launch_entry(app: &AppEntry) -> Result<String, String> {
        let (target, arguments, working_directory) = match &app.launch {
            Launch::Shortcut {
                path,
                arguments,
                working_directory,
            } => (path.clone(), arguments.clone(), working_directory.clone()),
            Launch::Executable(path) => (path.clone(), String::new(), String::new()),
            Launch::Packaged(id) => (
                format!("shell:AppsFolder\\{id}"),
                String::new(),
                String::new(),
            ),
            Launch::System(system_app) => {
                let (path, arguments) =
                    system_launch(system_app).ok_or_else(|| String::from("未知系统应用"))?;
                (path.to_string(), arguments.to_string(), String::new())
            }
            Launch::None => return Err(String::from("该条目没有启动目标")),
        };
        unsafe {
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let verb = wide("open");
            let file = wide(&target);
            let parameters = wide(&arguments);
            let directory = wide(&working_directory);
            let result = ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                if arguments.is_empty() {
                    std::ptr::null()
                } else {
                    parameters.as_ptr()
                },
                if working_directory.is_empty() {
                    std::ptr::null()
                } else {
                    directory.as_ptr()
                },
                SW_SHOWNORMAL,
            );
            if (result as isize) <= 32 {
                return Err(format!("ShellExecute 返回 {}", result as isize));
            }
        }
        Ok(app.name.clone())
}

static GLOBAL: OnceLock<RwLock<Arc<Index>>> = OnceLock::new();

pub fn global() -> Arc<Index> {
    GLOBAL.get_or_init(|| RwLock::new(Arc::new(Index::load())))
        .read().unwrap().clone()
}

pub fn try_global() -> Option<Arc<Index>> {
    GLOBAL.get().map(|index| index.read().unwrap().clone())
}

pub fn load_and_refresh(profile: &std::path::Path, mut ready: impl FnMut()) {
    let path = profile.join("rust-app-index-v1.json");
    let cached = std::fs::read(&path).ok()
        .and_then(|data| serde_json::from_slice::<Index>(&data).ok());
    let initial = cached.unwrap_or_else(|| Index::from_snapshot(
        Snapshot { schema_version: 1, apps: system_apps() }, "Windows system apps"));
    GLOBAL.get_or_init(|| RwLock::new(Arc::new(initial)));
    ready();
    if let Some(snapshot) = discover_windows_apps() {
        let refreshed = Index::from_snapshot(snapshot, "Windows AppsFolder");
        let data = serde_json::to_vec(&refreshed).ok();
        if let Some(global) = GLOBAL.get() {
            *global.write().unwrap() = Arc::new(refreshed);
        }
        ready();
        if let Some(data) = data {
            let _ = std::fs::create_dir_all(profile);
            let temporary = path.with_extension("json.tmp");
            if std::fs::write(&temporary, data).is_ok() {
                let _ = std::fs::rename(temporary, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_catalog_preserves_search_and_launch_data() {
        let source = Index::from_json(r#"{"schemaVersion":1,"apps":[{"id":"demo","name":"微信","source":"desktop","aliases":[],"target":{"kind":"executable","path":"C:\\demo.exe"}}]}"#, "test").unwrap();
        let bytes = serde_json::to_vec(&source).unwrap();
        let restored: Index = serde_json::from_slice(&bytes).unwrap();
        let hit = restored.search("wx", 8).into_iter().next().unwrap();
        assert_eq!(restored.apps[hit.index].id, "demo");
        assert_eq!(restored.command(hit.index).as_deref(), Some(r"C:\demo.exe"));
    }

    fn highlighted<'a>(title: &'a str, query: &str) -> Vec<&'a str> {
        match_highlight_ranges(title, query)
            .into_iter()
            .map(|range| &title[range])
            .collect()
    }

    #[test]
    fn highlight_matches_visible_text_and_english_word_initials() {
        assert_eq!(highlighted("Visual Studio Code", "vs"), vec!["V", "S"]);
        assert_eq!(highlighted("Visual Studio Code", "studio"), vec!["Studio"]);
        assert_eq!(
            highlighted("Visual Studio Code", "vis st"),
            vec!["Vis", "St"]
        );
        assert_eq!(highlighted("Google Chrome", "gc"), vec!["G", "C"]);
        assert_eq!(
            highlighted("Developer PowerShell for VS 2022", "vs"),
            vec!["VS"]
        );
    }

    #[test]
    fn highlight_maps_pinyin_back_to_the_correct_chinese_characters() {
        assert_eq!(highlighted("回收站", "hui"), vec!["回"]);
        assert_eq!(highlighted("系统恢复设置", "hui"), vec!["恢"]);
        assert_eq!(highlighted("腾讯会议", "hui"), vec!["会"]);
        assert_eq!(highlighted("重庆银行", "chongqing"), vec!["重庆"]);
        assert_eq!(highlighted("重庆银行", "cqyh"), vec!["重庆银行"]);
    }

    #[test]
    fn highlight_keeps_unicode_boundaries_and_ignores_unmatched_titles() {
        assert_eq!(highlighted("📁 Café 设置", "cafe"), vec!["Café"]);
        assert_eq!(highlighted("翻译", "unrelated"), Vec::<&str>::new());
        assert_eq!(highlighted("Google Chrome", ""), Vec::<&str>::new());
    }

    #[test]
    #[ignore = "enumerates the Windows applications installed on the current machine"]
    fn windows_catalog_is_available_without_a_wpf_snapshot() {
        use windows::Win32::{System::Com::*, UI::Shell::*};
        let started = std::time::Instant::now();
        let snapshot = discover_windows_apps().expect("Windows AppsFolder should be enumerable");
        let index = Index::from_snapshot(snapshot, "Windows AppsFolder");
        assert!(!index.search("控制面板", 8).is_empty());
        let shell_apps: Vec<_> = index
            .apps
            .iter()
            .filter_map(|app| match &app.launch {
                Launch::Packaged(id) => Some(id),
                _ => None,
            })
            .collect();
        assert!(
            !shell_apps.is_empty(),
            "the machine's Start/Store applications should be indexed"
        );
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let mut resolvable = 0;
            for id in shell_apps.iter().take(20) {
                let name: Vec<u16> = format!("shell:AppsFolder\\{id}")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                if SHCreateItemFromParsingName::<_, _, IShellItem>(
                    windows::core::PCWSTR(name.as_ptr()),
                    None::<&IBindCtx>,
                )
                .is_ok()
                {
                    resolvable += 1;
                }
            }
            if initialized {
                CoUninitialize();
            }
            assert_eq!(
                resolvable,
                shell_apps.len().min(20),
                "indexed shell targets must resolve"
            );
        }
        eprintln!(
            "discovered {} apps ({} shell entries) in {:?}",
            index.apps.len(),
            shell_apps.len(),
            started.elapsed()
        );
    }

    #[test]
    fn unrelated_words_do_not_match_by_interleaved_letters() {
        assert_eq!(
            score_entry("Documentation for Desktop Apps", &[], "", "test"),
            None
        );
    }

    #[test]
    fn word_initials_match_without_general_subsequence_matching() {
        assert!(score_entry("Google Chrome", &[], "", "gc").is_some());
        assert_eq!(score_entry("Git Release Notes", &[], "", "test"), None);
    }

    #[test]
    fn broad_vs_search_prioritizes_apps_but_keeps_specific_tools_searchable() {
        let names = [
            "Visual Studio Code",
            "Visual Studio Installer",
            "Visual Studio 2022",
            "Developer Command Prompt for VS 2022",
            "Developer PowerShell for VS 2022",
            "x64 Native Tools Command Prompt for VS 2022",
            "x64_x86 Cross Tools Command Prompt for VS 2022",
            "x86 Native Tools Command Prompt for VS 2022",
            "x86_x64 Cross Tools Command Prompt for VS 2022",
        ];
        let apps = names
            .iter()
            .enumerate()
            .map(|(i, name)| CatalogApp {
                id: format!("test-{i}"),
                name: (*name).to_string(),
                aliases: Vec::new(),
                source: "start-apps".into(),
                target: Target {
                    kind: "packaged".into(),
                    app_id: format!("test-{i}"),
                    ..Default::default()
                },
                icon_reference: String::new(),
            })
            .collect();
        let index = Index::from_snapshot(
            Snapshot {
                schema_version: 1,
                apps,
            },
            "test",
        );
        let find = |query: &str| {
            index
                .search(query, 20)
                .iter()
                .map(|hit| index.apps[hit.index].name.clone())
                .collect::<Vec<_>>()
        };
        let broad = find("vs");
        assert_eq!(broad.len(), 5, "{broad:?}");
        assert!(
            broad[..3]
                .iter()
                .all(|name| name.starts_with("Visual Studio")),
            "{broad:?}"
        );
        assert!(broad
            .iter()
            .any(|name| name == "Developer Command Prompt for VS 2022"));
        assert!(broad
            .iter()
            .any(|name| name == "Developer PowerShell for VS 2022"));
        assert!(find("x64")
            .iter()
            .any(|name| name == "x64 Native Tools Command Prompt for VS 2022"));
        assert!(find("cross tools")
            .iter()
            .any(|name| name == "x64_x86 Cross Tools Command Prompt for VS 2022"));
    }

    #[test]
    fn duplicate_executable_and_shortcut_records_merge_but_keep_search_aliases() {
        let snapshot = r#"{
            "schemaVersion": 1,
            "apps": [
                {
                    "id": "executable1",
                    "name": "chrome",
                    "aliases": ["chrome.exe"],
                    "source": "desktop",
                    "target": {"kind": "executable", "path": "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"}
                },
                {
                    "id": "shortcut001",
                    "name": "Google Chrome",
                    "aliases": ["chrome"],
                    "source": "desktop",
                    "target": {
                        "kind": "shortcut",
                        "targetPath": "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
                        "arguments": "--source-shortcut-location=desktop",
                        "workingDirectory": "C:\\Program Files\\Google\\Chrome\\Application"
                    }
                },
                {
                    "id": "startapps01",
                    "name": "Google Chrome",
                    "aliases": ["Chrome"],
                    "source": "start-apps",
                    "target": {
                        "kind": "shortcut",
                        "targetPath": "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
                        "arguments": "--source-shortcut-location=start-menu",
                        "workingDirectory": "C:\\Program Files\\Google\\Chrome\\Application"
                    }
                }
            ]
        }"#;
        let index = Index::from_json(snapshot, "test").unwrap();

        assert_eq!(index.apps.len(), 1);
        assert_eq!(index.apps[0].name, "Google Chrome");
        assert_eq!(index.search("google chrome", 8).len(), 1);
        assert!(index.apps[0]
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case("chrome.exe")));
    }

    #[test]
    fn shortcuts_with_different_application_arguments_remain_distinct() {
        let snapshot = r#"{
            "schemaVersion": 1,
            "apps": [
                {"id":"normal001","name":"Google Chrome","aliases":[],"source":"desktop","target":{"kind":"shortcut","targetPath":"C:\\Chrome\\chrome.exe","arguments":"","workingDirectory":"C:\\Chrome"}},
                {"id":"private001","name":"Chrome Private","aliases":[],"source":"desktop","target":{"kind":"shortcut","targetPath":"C:\\Chrome\\chrome.exe","arguments":"--incognito","workingDirectory":"C:\\Chrome"}}
            ]
        }"#;
        let index = Index::from_json(snapshot, "test").unwrap();

        assert_eq!(index.apps.len(), 2);
    }

    #[test]
    fn system_control_panel_is_searchable_and_has_a_launch_target() {
        let snapshot = r#"{
            "schemaVersion": 1,
            "apps": [{
                "id": "control-panel",
                "name": "控制面板",
                "aliases": ["Control Panel", "control.exe"],
                "source": "system",
                "target": {"kind": "system", "app": "control-panel"},
                "iconReference": "C:\\Windows\\System32\\control.exe"
            }]
        }"#;
        let index = Index::from_json(snapshot, "test").unwrap();

        let hits = index.search("控制面板", 8);
        assert_eq!(hits.len(), 1);
        assert_eq!(index.command(hits[0].index).as_deref(), Some("control.exe"));
        assert_eq!(index.search("Control Panel", 8).len(), 1);
    }

    #[test]
    fn recycle_bin_is_searchable_and_opens_the_windows_shell_folder() {
        let index = Index::from_snapshot(
            Snapshot {
                schema_version: 1,
                apps: system_apps(),
            },
            "system",
        );
        assert_eq!(index.search("回收站", 8).len(), 1);
        assert_eq!(index.search("recycle bin", 8).len(), 1);
        assert_eq!(
            system_launch("recycle-bin"),
            Some(("explorer.exe", "shell:RecycleBinFolder"))
        );
    }
}
