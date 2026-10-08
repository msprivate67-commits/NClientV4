//! Persistent user configuration.
//!
//! Persistent settings and API authentication, stored as JSON in the app data
//! directory so it is trivial to inspect or back up.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::AppResult;

/// App version, used for the User-Agent header.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default mirror (the original nhentai host).
pub const DEFAULT_MIRROR: &str = "nhentai.net";

/// Default User-Agent sent to nhentai API v2. The public API asks third-party
/// clients to identify themselves with a descriptive value.
pub const DEFAULT_UA: &str = concat!(
    "NClientV4/",
    env!("CARGO_PKG_VERSION"),
    " (unofficial nhentai desktop client)"
);

/// Title type used to display gallery titles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TitleType {
    Pretty,
    English,
    Japanese,
    #[default]
    Auto,
}

/// Sort orders, mirrors `SortType` (only the URL additions matter here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortType {
    #[default]
    RecentAllTime,
    PopularAllTime,
    PopularWeek,
    PopularDay,
    PopularMonth,
}

impl SortType {
    /// Mirrors `SortType#getUrlAddition`.
    pub fn url_addition(self) -> Option<&'static str> {
        match self {
            SortType::RecentAllTime => None,
            SortType::PopularAllTime => Some("popular"),
            SortType::PopularWeek => Some("popular-week"),
            SortType::PopularDay => Some("popular-today"),
            SortType::PopularMonth => Some("popular-month"),
        }
    }
}

/// Language filter, mirrors `Language` (the `All`/`Unknown` distinction in the
/// original collapses to `All` here for simplicity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    All,
    #[default]
    English,
    Japanese,
    Chinese,
}

/// Proxy type for HTTP requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    #[default]
    None,
    Http,
    Socks5,
}

/// Data usage policy, mirrors `Global#DataUsageType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DataUsageType {
    None,
    Thumbnail,
    #[default]
    Full,
}

/// Local gallery sort order, mirrors `LocalSortType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalSortType {
    #[default]
    TitleAsc,
    TitleDesc,
    DateAsc,
    DateDesc,
    PagesAsc,
    PagesDesc,
}

/// Authentication credentials, mirrors `AuthStore` + `AuthCredentials`.
/// `api_key` corresponds to the original `AuthCredentials.Type.API_KEY`
/// and is sent as `Authorization: Key <secret>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuthCredentials {
    pub api_key: String,
    /// Cached validity flag, refreshed whenever the API returns 401/403.
    pub valid: bool,
}

impl AuthCredentials {
    pub fn authorization_header(&self) -> Option<String> {
        let trimmed = self.api_key.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(format!("Key {}", trimmed))
        }
    }

    pub fn has_credentials(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// All persisted settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    // --- site / network -----------------------------------------------------
    /// nhentai mirror host, e.g. `nhentai.net`. Mirrors `Utility.getHost()`.
    pub mirror: String,
    /// Override User-Agent. Empty => use [`DEFAULT_UA`].
    pub user_agent: String,
    /// HTTP connect / read timeout in seconds.
    pub request_timeout_secs: u64,
    /// API key auth (sent as `Authorization: Key <key>`).
    pub auth: AuthCredentials,

    // --- proxy ---------------------------------------------------------------
    pub proxy_type: ProxyType,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub proxy_password: String,

    // --- browsing -----------------------------------------------------------
    pub sort_type: SortType,
    pub only_language: Language,
    pub title_type: TitleType,
    pub exact_tag_match: bool,
    pub remove_avoided_galleries: bool,
    pub show_titles: bool,

    // --- display ------------------------------------------------------------
    /// Number of page-thumbnail columns in gallery detail.
    /// 0 = auto (responsive CSS grid). 2-10 = fixed count.
    pub page_thumbnail_columns: u32,
    pub use_rtl: bool,
    pub default_zoom_pct: u32,
    pub reader_fit_mode: String,
    pub reader_direction: String,
    pub button_change_page: bool,

    // --- data usage ---------------------------------------------------------
    pub usage_wifi: DataUsageType,
    pub usage_mobile: DataUsageType,

    // --- history / favorites limits ----------------------------------------
    pub keep_history: bool,
    pub max_history: u32,
    pub favorite_limit: u32,

    // --- downloads ----------------------------------------------------------
    pub download_dir: PathBuf,
    /// Number of parallel gallery downloads. Must remain at 1 to avoid
    /// overloading the source server. Increasing this will cause excessive
    /// concurrent connections and may result in the server blocking the
    /// client IP. **DO NOT CHANGE without maintainer approval.**
    pub parallel_downloads: u32,
    /// Number of parallel page fetches *within* a single gallery download.
    /// Must remain at 8. Higher values risk server-side rate limiting.
    /// **DO NOT CHANGE without maintainer approval.**
    pub parallel_pages: u32,
    /// Whether download progress and completion notifications are shown.
    /// Defaults to enabled when loading settings created by older versions.
    #[serde(default = "notifications_enabled_default")]
    pub notifications_enabled: bool,
    /// Use Android FLAG_SECURE to hide content from Recents and captures.
    #[serde(default)]
    pub privacy_screen: bool,
    /// Inspect clipboard text for nhentai gallery links whenever the main
    /// window regains focus.
    #[serde(default = "clipboard_link_reader_default")]
    pub clipboard_link_reader: bool,

    // --- security -----------------------------------------------------------
    pub lock_screen: bool,
    pub pin: String,

    // --- ai translation ------------------------------------------------------
    pub tl_base_url: String,
    pub tl_model: String,
    pub tl_api_key: String,
    pub tl_target_lang: String,
    #[serde(default = "tl_comment_target_lang_default")]
    pub tl_comment_target_lang: String,
    pub tl_thinking: bool,
    #[serde(default = "tl_auto_translate_default")]
    pub tl_auto_translate: bool,
    #[serde(default)]
    pub tl_auto_translate_gallery_titles: bool,
    #[serde(default)]
    pub tl_use_proxy: bool,
    /// AI translation cache cap, in galleries. Records are keyed by gallery
    /// ID; once the table holds more entries than this the oldest ones are
    /// dropped. `0` disables the cache (every translation calls the AI again).
    #[serde(default = "tl_cache_limit_default")]
    pub tl_cache_limit: u32,

    #[serde(default)]
    pub app_language: String,
    /// Color theme preference: `dark`, `system`, or `light`.
    #[serde(default = "theme_default")]
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mirror: DEFAULT_MIRROR.to_string(),
            user_agent: String::new(),
            request_timeout_secs: 30,
            auth: AuthCredentials::default(),
            proxy_type: ProxyType::None,
            proxy_host: String::new(),
            proxy_port: 1080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            sort_type: SortType::RecentAllTime,
            only_language: Language::All,
            title_type: TitleType::Auto,
            exact_tag_match: false,
            remove_avoided_galleries: true,
            show_titles: true,
            page_thumbnail_columns: 0,
            use_rtl: false,
            default_zoom_pct: 100,
            reader_fit_mode: "height".into(),
            reader_direction: "vertical".into(),
            button_change_page: true,
            usage_wifi: DataUsageType::Full,
            usage_mobile: DataUsageType::Thumbnail,
            keep_history: true,
            max_history: 100,
            favorite_limit: 100,
            download_dir: PathBuf::new(), // filled in by ConfigStore::load_or_init
            // -----------------------------------------------------------------------
            // WARNING — SERVER-SIDE RATE LIMITING
            // -----------------------------------------------------------------------
            // parallel_downloads MUST remain at 1. Downloading multiple galleries
            // concurrently opens too many TCP connections to the source server and
            // will trigger IP bans / Cloudflare throttling. Do NOT increase this.
            // -----------------------------------------------------------------------
            // parallel_pages MUST remain at 8. This is the number of page images
            // fetched in parallel within a single gallery download. Higher values
            // (e.g. 16 or 32) create request spikes that the server may treat as
            // abuse. Do NOT increase this.
            // -----------------------------------------------------------------------
            parallel_downloads: 1,
            parallel_pages: 8,
            notifications_enabled: true,
            privacy_screen: false,
            clipboard_link_reader: true,
            lock_screen: false,
            pin: String::new(),
            tl_base_url: "https://api.deepseek.com".into(),
            tl_model: "deepseek-v4-flash".into(),
            tl_api_key: String::new(),
            tl_target_lang: "简体中文，尽量用古典章回体小说标题风格".into(),
            tl_comment_target_lang: tl_comment_target_lang_default(),
            tl_thinking: false,
            tl_auto_translate: true,
            tl_auto_translate_gallery_titles: false,
            tl_use_proxy: false,
            tl_cache_limit: tl_cache_limit_default(),
            app_language: String::new(),
            theme: theme_default(),
        }
    }
}

fn notifications_enabled_default() -> bool {
    true
}

fn clipboard_link_reader_default() -> bool {
    true
}

fn tl_auto_translate_default() -> bool {
    true
}

fn tl_cache_limit_default() -> u32 {
    1000
}

fn tl_comment_target_lang_default() -> String {
    "简体中文，古典文言文风格，或诗句对联风格".into()
}

fn theme_default() -> String {
    "system".into()
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn old_settings_enable_notifications_by_default() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("notifications_enabled");
        value.as_object_mut().unwrap().remove("privacy_screen");
        value
            .as_object_mut()
            .unwrap()
            .remove("clipboard_link_reader");
        value.as_object_mut().unwrap().remove("tl_auto_translate");
        value
            .as_object_mut()
            .unwrap()
            .remove("tl_auto_translate_gallery_titles");
        value
            .as_object_mut()
            .unwrap()
            .remove("tl_comment_target_lang");
        value.as_object_mut().unwrap().remove("tl_use_proxy");
        value.as_object_mut().unwrap().remove("tl_cache_limit");
        value.as_object_mut().unwrap().remove("app_language");
        value.as_object_mut().unwrap().remove("theme");
        let settings: Settings = serde_json::from_value(value).unwrap();

        assert!(settings.notifications_enabled);
        assert!(!settings.privacy_screen);
        assert!(settings.clipboard_link_reader);
        assert!(settings.tl_auto_translate);
        assert!(!settings.tl_auto_translate_gallery_titles);
        assert_eq!(
            settings.tl_comment_target_lang,
            "简体中文，古典文言文风格，或诗句对联风格"
        );
        assert!(!settings.tl_use_proxy);
        assert_eq!(settings.tl_cache_limit, 1000);
        assert!(settings.app_language.is_empty());
        assert_eq!(settings.theme, "system");
    }
}

/// Thread-safe settings store. Reads are cheap (inner RwLock), writes are
/// persisted to disk synchronously.
#[derive(Debug)]
pub struct ConfigStore {
    settings: RwLock<Settings>,
    path: PathBuf,
    pub app_data: PathBuf,
}

impl ConfigStore {
    /// Load settings from `<app_data>/settings.json`, creating it if missing.
    /// `download_dir` defaults to `<app_data>/NClientV4/Download`.
    pub fn load_or_init(app_data: &Path) -> Self {
        let path = app_data.join("settings.json");
        let default_download = default_download_dir(app_data);

        let mut settings = match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                log::warn!("settings.json invalid ({e}), using defaults");
                Settings {
                    download_dir: default_download.clone(),
                    ..Settings::default()
                }
            }),
            Err(_) => Settings {
                download_dir: default_download.clone(),
                ..Settings::default()
            },
        };

        if settings.download_dir.as_os_str().is_empty() {
            settings.download_dir = default_download;
        }
        fs::create_dir_all(&settings.download_dir).ok();

        let store = Self {
            settings: RwLock::new(settings.clone()),
            path,
            app_data: app_data.to_path_buf(),
        };
        store.persist(&settings);
        store
    }

    pub fn get(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    /// Path of the `settings.json` file (its parent dir holds cookies.json
    /// and the database too).
    pub fn path_of_setting(&self) -> Option<PathBuf> {
        if self.path.as_os_str().is_empty() {
            None
        } else {
            Some(self.path.clone())
        }
    }

    pub fn update<F: FnOnce(&mut Settings)>(&self, f: F) -> AppResult<Settings> {
        let mut settings = self.settings.write().unwrap();
        f(&mut settings);
        let snapshot = settings.clone();
        drop(settings);
        self.persist(&snapshot);
        Ok(snapshot)
    }

    pub fn replace(&self, new_settings: Settings) -> AppResult<Settings> {
        let mut sanitized = new_settings;
        if sanitized.download_dir.as_os_str().is_empty() {
            sanitized.download_dir = self.get().download_dir;
        }
        fs::create_dir_all(&sanitized.download_dir).ok();
        self.persist(&sanitized);
        *self.settings.write().unwrap() = sanitized.clone();
        Ok(sanitized)
    }

    fn persist(&self, settings: &Settings) {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).ok();
        }
        match serde_json::to_string_pretty(settings) {
            Ok(json) => {
                if let Err(e) = fs::write(&self.path, json) {
                    log::error!("failed to write settings.json: {e}");
                }
            }
            Err(e) => log::error!("failed to serialize settings: {e}"),
        }
    }

    // --- convenience accessors used by other modules -----------------------

    /// Mirrors `Utility.getBaseUrl()` -> `https://<mirror>/`.
    pub fn base_url(&self) -> String {
        format!("https://{}/", self.get().mirror)
    }

    /// Mirrors `Utility.getApiBaseUrl()` -> `https://<mirror>/api/v2/`.
    pub fn api_base_url(&self) -> String {
        format!("https://{}/api/v2/", self.get().mirror)
    }

    pub fn download_dir(&self) -> PathBuf {
        self.get().download_dir
    }

    /// Effective User-Agent (override or default).
    pub fn user_agent(&self) -> String {
        let s = self.get();
        if s.user_agent.trim().is_empty() {
            DEFAULT_UA.to_string()
        } else {
            s.user_agent.trim().to_string()
        }
    }
}

/// Quick helper to read the cookie store file path used by [`crate::http`].
pub fn cookie_db_path(app_data: &Path) -> PathBuf {
    app_data.join("cookies.json")
}

/// Default download directory, platform-aware.
///
/// On Android the download dir lives under the app's own **external files
/// directory** (`<external>/Android/data/com.nclientv4.app/files/NClientV4/Download`),
/// which is part of scoped storage: it is world-visible in file managers yet
/// needs **no runtime permission** to read/write on Android 10+ (API 29+). This
/// is the standard location for app-managed downloadable content.
///
/// The Tauri path API (`app_data_dir`) resolves to *internal* private storage
/// (`/data/user/0/<pkg>`) on Android, which is the wrong place for large
/// downloads — so we resolve the external files directory ourselves via
/// [`android_external_files_dir`] and fall back to internal storage only if
/// every external candidate is unwritable.
fn default_download_dir(app_data: &Path) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        if let Some(ext) = android_external_files_dir() {
            let dir = ext.join("NClientV4").join("Download");
            log::info!("Android download dir (app external): {}", dir.display());
            return dir;
        }
        let fallback = app_data.join("NClientV4").join("Download");
        log::warn!(
            "No writable app-external storage found; using internal: {}",
            fallback.display()
        );
        fallback
    }
    #[cfg(not(target_os = "android"))]
    {
        app_data.join("NClientV4").join("Download")
    }
}

/// Candidate download directories the user can pick from in Settings, in
/// preference order. Each entry is `(label, path)`. Used by the
/// `settings_list_download_candidates` command so the frontend can offer a
/// chooser when the native directory dialog is unavailable (e.g. Android).
///
/// On Android this surfaces the app's own external files directory under
/// `Android/data/<pkg>/files` (no storage permission needed), plus internal app
/// storage as a last resort. On other platforms only the default app-data
/// directory is returned.
pub fn download_candidates(app_data: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    #[cfg(target_os = "android")]
    {
        if let Some(ext) = android_external_files_dir() {
            out.push((
                "App external storage (recommended)",
                ext.join("NClientV4").join("Download"),
            ));
        }
        out.push((
            "Internal app storage",
            app_data.join("NClientV4").join("Download"),
        ));
    }
    #[cfg(not(target_os = "android"))]
    {
        out.push(("App data", default_download_dir(app_data)));
    }
    out
}

/// Resolve the app's external files directory on Android, i.e.
/// `<external_root>/Android/data/com.nclientv4.app/files`.
///
/// Tauri 2 has no Rust API exposing `Context.getExternalFilesDir()`, and the
/// legacy `EXTERNAL_STORAGE` env var is no longer populated for app processes
/// on Android 11+. Instead we probe the well-known external-storage mount
/// points, build the app-specific `Android/data/<pkg>/files` path under each,
/// and pick the first one we can actually create **and write** to (creating a
/// dir alone can succeed on read-only mounts, so a write probe is required).
///
/// Returns `None` if no candidate is writable (caller should then use internal
/// app storage as a last resort).
#[cfg(target_os = "android")]
fn android_external_files_dir() -> Option<PathBuf> {
    /// Package-specific suffix under external storage (scoped-storage app dir).
    const APP_EXT_SUFFIX: &str = "Android/data/com.nclientv4.app/files";

    // Candidate external-storage roots, most common first. Order matters: the
    // canonical primary external storage is `/storage/emulated/0`; the others
    // cover vendor/legacy layouts and secondary SD cards.
    const CANDIDATE_ROOTS: &[&str] = &[
        "/storage/emulated/0",
        "/sdcard",
        "/storage/sdcard0",
        "/mnt/sdcard",
        "/storage/self/primary",
    ];

    // 1. Physical mount points (preferred — always present on real devices).
    for root in CANDIDATE_ROOTS {
        let path = PathBuf::from(root).join(APP_EXT_SUFFIX);
        if dir_is_writable(&path) {
            log::info!("Android external files dir: {}", path.display());
            return Some(path);
        }
    }

    // 2. Legacy `EXTERNAL_STORAGE` env var, if the vendor still sets it.
    if let Ok(ext) = std::env::var("EXTERNAL_STORAGE") {
        let trimmed = ext.trim_end_matches('/');
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed).join(APP_EXT_SUFFIX);
            if dir_is_writable(&path) {
                log::info!(
                    "Android external files dir (via EXTERNAL_STORAGE): {}",
                    path.display()
                );
                return Some(path);
            }
        }
    }

    log::warn!("No writable Android external files dir found among candidates");
    None
}

/// Ensure `dir` exists and is writable by creating it (recursively) and writing
/// a throwaway probe file. The probe file is removed afterwards. Returns `false`
/// if any step fails — callers treat a `false` as "this location is unusable".
#[cfg(target_os = "android")]
fn dir_is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write_probe");
    let writable = std::fs::write(&probe, b"x").is_ok();
    if writable {
        let _ = std::fs::remove_file(&probe);
    }
    writable
}

/// Check whether the trailing two bytes of a JPEG file are `FF D9`.
/// Mirrors `Global#isJPEGCorrupted`.
pub fn is_jpeg_corrupted(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    if !path.exists() {
        return true;
    }
    let Ok(mut f) = fs::OpenOptions::new().read(true).open(path) else {
        return true;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < 10 {
        return true;
    }
    if f.seek(SeekFrom::End(-2)).is_err() {
        return true;
    }
    let mut eoi = [0u8; 2];
    if f.read_exact(&mut eoi).is_err() {
        return true;
    }
    eoi[0] != 0xFF || eoi[1] != 0xD9
}
