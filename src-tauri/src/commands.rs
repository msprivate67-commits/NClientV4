//! Tauri command handlers — the bridge between the frontend and the backend.
//!
//! Every function here is exposed to JS via `invoke('name', { ... })`.

use std::path::PathBuf;

use chrono::Utc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::api::ApiClient;
use crate::cloudflare;
use crate::config::{AuthCredentials, Settings};
use crate::db::{DownloadRow, FavoriteRow, ReadProgressRow, TranslationCacheRow};
use crate::downloader::{DownloadRequest, DownloadStatus, COMPLETED_MARKER};
use crate::error::{AppError, AppResult};
use crate::http::HttpClient;
use crate::models::*;
use crate::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn api(state: &State<AppState>) -> ApiClient {
    ApiClient::new(state.http.clone(), state.config.clone())
}

fn settings(state: &State<AppState>) -> Settings {
    state.config.get()
}

// ===========================================================================
// Settings
// ===========================================================================

#[tauri::command]
pub fn settings_get(state: State<AppState>) -> AppResult<Settings> {
    Ok(settings(&state))
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, new_settings: Settings) -> AppResult<Settings> {
    let updated = state.config.replace(new_settings)?;
    // Mirror / UA change => rebuild HTTP client (cookies preserved).
    state.http.rebuild(&updated);
    state
        .downloads
        .set_download_dir(updated.download_dir.clone());
    Ok(updated)
}

/// Stream an OpenAI-compatible response through the native HTTP client. This
/// is used only when AI requests opt into the application's proxy; chunks stay
/// as bytes so UTF-8 characters split across network frames remain intact.
#[tauri::command]
pub async fn translation_stream_request(
    state: State<'_, AppState>,
    url: String,
    api_key: String,
    body: serde_json::Value,
    use_proxy: bool,
    on_chunk: Channel<Vec<u8>>,
) -> AppResult<()> {
    let parsed = url::Url::parse(&url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::Other(
            "translation URL must use HTTP or HTTPS".into(),
        ));
    }

    let current_settings = settings(&state);
    let mut request =
        state
            .http
            .external_request(reqwest::Method::POST, &url, use_proxy, &current_settings)?;
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let mut response = request.json(&body).send().await?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(AppError::Http {
            status: status.as_u16(),
            body: error_body,
        });
    }

    while let Some(chunk) = response.chunk().await? {
        if on_chunk.send(chunk.to_vec()).is_err() {
            break;
        }
    }
    Ok(())
}

// ===========================================================================
// AI translation cache
// ===========================================================================

/// One gallery's stored AI translations (title / tags / comments), keyed by
/// gallery ID. The frontend checks this before spending an AI request.
#[tauri::command]
pub fn translation_cache_get(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<Option<TranslationCacheRow>> {
    state.db.translation_cache_get(gallery_id)
}

/// Store (or refresh) one gallery's translations and enforce the configured
/// per-gallery cap (`tl_cache_limit`, trimmed newest-first).
#[tauri::command]
pub fn translation_cache_set(
    state: State<'_, AppState>,
    mut entry: TranslationCacheRow,
) -> AppResult<TranslationCacheRow> {
    entry.updated_at = Utc::now().to_rfc3339();
    let limit = settings(&state).tl_cache_limit;
    state.db.translation_cache_upsert(&entry, limit)?;
    Ok(entry)
}

/// Drop every cached translation. Returns the number of removed galleries.
#[tauri::command]
pub fn translation_cache_clear(state: State<'_, AppState>) -> AppResult<u32> {
    let removed = state.db.translation_cache_clear()?;
    Ok(removed as u32)
}

/// Number of galleries currently held in the translation cache.
#[tauri::command]
pub fn translation_cache_count(state: State<'_, AppState>) -> AppResult<u32> {
    let count = state.db.translation_cache_count()?;
    Ok(count.max(0) as u32)
}

#[tauri::command]
pub fn settings_get_paths(app: AppHandle) -> AppResult<serde_json::Value> {
    let data = app.path().app_data_dir()?;
    Ok(serde_json::json!({
        "app_data": data,
        "log_dir": app.path().app_log_dir().ok(),
    }))
}

#[tauri::command]
pub async fn settings_pick_directory(state: State<'_, AppState>) -> AppResult<Option<String>> {
    // No native directory dialog on Android — return the recommended default
    // (the app's external files directory) so the frontend can prefill the field
    // and the user can switch between app-scoped storage candidates via
    // `settings_list_download_candidates`.
    #[cfg(target_os = "android")]
    {
        let candidates = crate::config::download_candidates(&state.config.app_data);
        // Prefer the "app external storage (recommended)" entry; fall back to
        // the last candidate (internal storage), which always exists.
        let pick = candidates
            .iter()
            .find(|(label, _)| label.contains("recommended"))
            .or_else(|| candidates.last())
            .map(|(_, p)| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                state
                    .config
                    .app_data
                    .join("NClientV4")
                    .join("Download")
                    .to_string_lossy()
                    .to_string()
            });
        Ok(Some(pick))
    }
    #[cfg(not(target_os = "android"))]
    {
        // Desktop uses the native dialog in the frontend; nothing to return.
        let _ = state;
        Ok(None)
    }
}

/// Return candidate download directories `(label, path)` the user can pick from
/// when the native directory dialog is unavailable (Android). See
/// [`crate::config::download_candidates`].
#[tauri::command]
pub fn settings_list_download_candidates(
    state: State<'_, AppState>,
) -> AppResult<Vec<(String, String)>> {
    Ok(crate::config::download_candidates(&state.config.app_data)
        .into_iter()
        .map(|(label, path)| (label.to_string(), path.to_string_lossy().to_string()))
        .collect())
}

#[tauri::command]
pub fn settings_clear_cookies(state: State<'_, AppState>) -> AppResult<()> {
    state.http.clear_cookies()
}

// ===========================================================================
// Auth + Cloudflare
// ===========================================================================

#[tauri::command]
pub fn auth_get(state: State<'_, AppState>) -> AppResult<AuthCredentials> {
    Ok(state.config.get().auth)
}

#[tauri::command]
pub fn auth_set_api_key(state: State<'_, AppState>, api_key: String) -> AppResult<Settings> {
    let updated = state.config.update(|s| {
        s.auth.api_key = api_key.trim().to_string();
        s.auth.valid = true;
    })?;
    Ok(updated)
}

#[tauri::command]
pub fn auth_clear(state: State<'_, AppState>) -> AppResult<Settings> {
    let updated = state.config.update(|s| {
        s.auth = AuthCredentials::default();
    })?;
    Ok(updated)
}

#[tauri::command]
pub fn auth_status(state: State<'_, AppState>) -> AppResult<AuthStatus> {
    let s = settings(&state);
    Ok(AuthStatus {
        has_credentials: s.auth.has_credentials(),
        api_key_valid: s.auth.valid,
        cloudflare_solved: cloudflare::is_solved(),
    })
}

#[tauri::command]
pub async fn cloudflare_check(state: State<'_, AppState>) -> AppResult<bool> {
    // Probe the API base with a lightweight request; if CF blocks, we get
    // `AppError::Cloudflare`.
    let s = settings(&state);
    let url = format!("{}galleries?page=1", state.config.api_base_url());
    match state.http.get_text(&url, true, &s).await {
        Ok(_) => {
            cloudflare::set_state(crate::models::CfState::Solved);
            Ok(false)
        }
        Err(AppError::Cloudflare) => {
            cloudflare::set_state(crate::models::CfState::Needed);
            Ok(true)
        }
        Err(e) => {
            log::info!("cloudflare check: {e}");
            Ok(false)
        }
    }
}

#[tauri::command]
pub fn cloudflare_open_challenge(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    let base = state.config.base_url();
    let settings = state.config.get();
    cloudflare::open_challenge(&app, state.http.clone(), base, &settings)
}

#[tauri::command]
pub fn cloudflare_is_solved() -> bool {
    cloudflare::is_solved()
}

// ===========================================================================
// API: browse / search / random / detail
// ===========================================================================

#[tauri::command]
pub async fn api_browse(
    state: State<'_, AppState>,
    page: u32,
    sort: crate::config::SortType,
) -> AppResult<SearchPage> {
    api(&state).browse(page, sort).await
}

#[tauri::command]
pub async fn api_search(state: State<'_, AppState>, query: SearchQuery) -> AppResult<SearchPage> {
    api(&state).search(&query).await
}

#[tauri::command]
pub async fn api_random(state: State<'_, AppState>) -> AppResult<Gallery> {
    api(&state).random().await
}

#[tauri::command]
pub async fn api_get_gallery(state: State<'_, AppState>, id: i64) -> AppResult<Gallery> {
    let g = api(&state).gallery(id).await?;
    for tag in &g.tags {
        let _ = state.db.tag_insert_or_update(tag);
    }
    let s = settings(&state);
    // Record visit in local history.
    if s.keep_history {
        let _ = state.db.history_add(
            g.id,
            &g.best_title(s.title_type),
            g.media_id,
            g.thumbnail.thumbnail_or_path().unwrap_or(""),
        );
    }
    Ok(g)
}

#[tauri::command]
pub async fn api_get_user(state: State<'_, AppState>) -> AppResult<User> {
    api(&state).user().await
}

#[tauri::command]
pub async fn api_get_comments(
    state: State<'_, AppState>,
    gallery_id: i64,
    page: u32,
) -> AppResult<CommentsPage> {
    api(&state).comments(gallery_id, page, 50).await
}

#[tauri::command]
pub async fn api_get_favorites_page(
    state: State<'_, AppState>,
    page: u32,
    query: Option<String>,
) -> AppResult<FavoritesPage> {
    api(&state).favorites_page(page, query.as_deref()).await
}

#[tauri::command]
pub async fn api_check_favorite(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<FavoriteStatus> {
    api(&state).check_favorite(gallery_id).await
}

#[tauri::command]
pub async fn api_add_favorite(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<FavoriteStatus> {
    api(&state).add_favorite(gallery_id).await
}

#[tauri::command]
pub async fn api_remove_favorite(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<FavoriteStatus> {
    api(&state).remove_favorite(gallery_id).await
}

#[tauri::command]
pub async fn api_sync_local_favorites(state: State<'_, AppState>) -> AppResult<u32> {
    let local = state.db.fav_list(u32::MAX, 0)?;
    let client = api(&state);
    let mut synced = 0;
    for fav in local {
        match client.add_favorite(fav.id).await {
            Ok(_) => synced += 1,
            Err(e) => {
                log::warn!("failed to sync favorite {}: {e}", fav.id);
            }
        }
    }
    Ok(synced)
}

#[tauri::command]
pub async fn api_get_tags(
    state: State<'_, AppState>,
    type_filter: Option<TagType>,
) -> AppResult<Vec<Tag>> {
    let cached = state.db.tags_by_type(type_filter).unwrap_or_default();
    if !cached.is_empty() {
        return Ok(cached);
    }
    let remote = api(&state).popular_tags().await?;
    for t in &remote {
        let _ = state.db.tag_insert_or_update(t);
    }
    Ok(remote)
}

#[tauri::command]
pub async fn api_get_popular_tags(state: State<'_, AppState>) -> AppResult<Vec<Tag>> {
    api(&state).popular_tags().await
}

// ===========================================================================
// Favorites (local DB)
// ===========================================================================

#[tauri::command]
pub fn fav_add(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    media_id: i64,
    thumbnail: String,
) -> AppResult<()> {
    state.db.fav_add(id, &title, media_id, &thumbnail)
}

#[tauri::command]
pub fn fav_remove(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    state.db.fav_remove(id)
}

#[tauri::command]
pub fn fav_is_favorite(state: State<'_, AppState>, id: i64) -> AppResult<bool> {
    state.db.fav_is(id)
}

#[tauri::command]
pub fn fav_list(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<FavoriteRow>> {
    state.db.fav_list(limit.unwrap_or(100), offset.unwrap_or(0))
}

// ===========================================================================
// Tags (local DB)
// ===========================================================================

#[tauri::command]
pub fn tags_get_all(state: State<'_, AppState>) -> AppResult<Vec<Tag>> {
    state.db.tags_all()
}

#[tauri::command]
pub fn tags_get_by_type(
    state: State<'_, AppState>,
    type_filter: Option<TagType>,
) -> AppResult<Vec<Tag>> {
    state.db.tags_by_type(type_filter)
}

#[tauri::command]
pub fn tags_set_status(state: State<'_, AppState>, id: i64, status: TagStatus) -> AppResult<()> {
    state.db.tag_set_status(id, status)
}

#[tauri::command]
pub async fn tags_get_blacklist(state: State<'_, AppState>) -> AppResult<Vec<Tag>> {
    let remote = api(&state).blacklist().await?;
    state.db.replace_blacklist(&remote)?;
    Ok(remote)
}

#[tauri::command]
pub async fn tags_add_blacklist(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    api(&state).update_blacklist(&[id], &[]).await?;
    state.db.tag_set_blacklist(id, true)
}

#[tauri::command]
pub async fn tags_remove_blacklist(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    api(&state).update_blacklist(&[], &[id]).await?;
    state.db.tag_set_blacklist(id, false)
}

#[tauri::command]
pub async fn tags_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> AppResult<Vec<Tag>> {
    api(&state).search_tags(&query, limit.unwrap_or(50)).await
}

#[tauri::command]
pub async fn tags_get_popular(state: State<'_, AppState>) -> AppResult<Vec<Tag>> {
    api(&state).popular_tags().await
}

// ===========================================================================
// History
// ===========================================================================

#[tauri::command]
pub fn history_add(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    media_id: i64,
    thumbnail: String,
) -> AppResult<()> {
    state.db.history_add(id, &title, media_id, &thumbnail)
}

#[tauri::command]
pub fn history_list(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> AppResult<Vec<HistoryEntry>> {
    state.db.history_list(limit.unwrap_or(200))
}

#[tauri::command]
pub fn history_clear(state: State<'_, AppState>) -> AppResult<()> {
    state.db.history_clear()
}

// ===========================================================================
// Read progress
// ===========================================================================

#[tauri::command]
pub fn read_progress_set(
    state: State<'_, AppState>,
    gallery_id: i64,
    last_page: usize,
    total_pages: usize,
) -> AppResult<()> {
    state
        .db
        .read_progress_upsert(gallery_id, last_page, total_pages)
}

#[tauri::command]
pub fn read_progress_reset(state: State<'_, AppState>, gallery_id: i64) -> AppResult<()> {
    state.db.read_progress_reset(gallery_id)
}

#[tauri::command]
pub fn read_progress_get(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<Option<ReadProgressRow>> {
    state.db.read_progress_get(gallery_id)
}

/// IDs of galleries the user has read >= 50% of. The frontend uses this to
/// badge covers in the gallery grid and local library.
#[tauri::command]
pub fn read_progress_ids(state: State<'_, AppState>) -> AppResult<Vec<i64>> {
    state.db.read_progress_ids()
}

/// Save the exact page the user stopped at in the local reader (resume point).
#[tauri::command]
pub fn local_reader_progress_set(
    state: State<'_, AppState>,
    gallery_id: i64,
    page: usize,
    total_pages: usize,
) -> AppResult<()> {
    state
        .db
        .local_reader_progress_set(gallery_id, page, total_pages)
}

/// Fetch the saved resume page (1-based) for a gallery in the local reader.
#[tauri::command]
pub fn local_reader_progress_get(
    state: State<'_, AppState>,
    gallery_id: i64,
) -> AppResult<Option<usize>> {
    state.db.local_reader_progress_get(gallery_id)
}

// ===========================================================================
// Local library
// ===========================================================================

#[tauri::command]
pub async fn local_scan(state: State<'_, AppState>) -> AppResult<Vec<LocalGallery>> {
    // `mut` is only needed on Android, where we may push an extra scan dir.
    #[allow(unused_mut)]
    let mut dirs = vec![state.config.download_dir()];
    // On Android also scan the internal fallback if different.
    #[cfg(target_os = "android")]
    {
        let internal = &state.config.app_data.join("NClientV4").join("Download");
        if *internal != *dirs[0] && internal.exists() {
            dirs.push(internal.clone());
        }
    }
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut found = Vec::new();
        for dir in &dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let path = e.path();
                    if let Some(lg) = read_local_gallery(&path) {
                        let _ = db.local_upsert(&lg);
                        found.push(lg);
                    } else {
                        let _ = db.local_remove(&path.to_string_lossy());
                    }
                }
            }
        }
        // Disk is the source of truth. Remove cached rows for folders deleted
        // from the configured scan roots instead of resurrecting DB-only data.
        if let Ok(all) = db.local_all() {
            for lg in all {
                let path = PathBuf::from(&lg.folder);
                if found.iter().any(|item| item.folder == lg.folder) {
                    continue;
                }
                if !dirs.iter().any(|dir| path.starts_with(dir)) {
                    if let Some(revalidated) = read_local_gallery(&path) {
                        let _ = db.local_upsert(&revalidated);
                        found.push(revalidated);
                        continue;
                    }
                }
                if dirs.iter().any(|dir| path.starts_with(dir)) || !path.exists() {
                    let _ = db.local_remove(&lg.folder);
                }
            }
        }
        Ok(found)
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
}

/// IDs of galleries the user has downloaded (present on disk in the local
/// library). The frontend uses this to badge online gallery covers with a
/// "downloaded" mark and to disable re-downloading.
#[tauri::command]
pub fn local_ids(state: State<'_, AppState>) -> AppResult<Vec<i64>> {
    state.db.local_ids()
}

#[tauri::command]
pub fn local_get(state: State<'_, AppState>, gallery_id: i64) -> AppResult<Option<LocalGallery>> {
    state.db.local_get(gallery_id)
}

#[tauri::command]
pub fn local_set_translated_title(
    state: State<'_, AppState>,
    gallery_id: i64,
    title: String,
) -> AppResult<()> {
    state.db.local_set_translated_title(gallery_id, &title)
}

/// Read the full `Gallery` JSON cached on disk in a downloaded folder's
/// `.nomedia` file. This is the offline source for tags + related galleries on
/// the local detail page. Returns `None` when the folder or metadata file is
/// missing/unreadable (e.g. imported folders) — callers degrade gracefully.
#[tauri::command]
pub fn local_get_meta(state: State<'_, AppState>, gallery_id: i64) -> AppResult<Option<Gallery>> {
    let Some(lg) = state.db.local_get(gallery_id)? else {
        return Ok(None);
    };
    let nomedia = PathBuf::from(&lg.folder).join(".nomedia");
    let Ok(content) = std::fs::read_to_string(&nomedia) else {
        return Ok(None);
    };
    match serde_json::from_str::<Gallery>(&content) {
        Ok(g) => Ok(Some(g)),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub fn local_list(state: State<'_, AppState>) -> AppResult<Vec<LocalGallery>> {
    // Revalidate every folder so an interrupted download cannot survive in the
    // cache as a completed local gallery.
    let mut found_folders = Vec::new();
    for item in state.db.local_all().unwrap_or_default() {
        let path = PathBuf::from(&item.folder);
        if let Some(revalidated) = read_local_gallery(&path) {
            found_folders.push(revalidated.folder.clone());
            let _ = state.db.local_upsert(&revalidated);
        } else {
            let _ = state.db.local_remove(&item.folder);
        }
    }
    let dir = state.config.download_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let p = e.path();
            if let Some(lg) = read_local_gallery(&p) {
                found_folders.push(lg.folder.clone());
                let _ = state.db.local_upsert(&lg);
            } else {
                let _ = state.db.local_remove(&p.to_string_lossy());
            }
        }
    }
    Ok(state
        .db
        .local_all()?
        .into_iter()
        .filter(|item| found_folders.iter().any(|folder| folder == &item.folder))
        .collect())
}

#[tauri::command]
pub fn local_delete(state: State<'_, AppState>, folder: String) -> AppResult<()> {
    let path = PathBuf::from(&folder);
    if path.exists() {
        std::fs::remove_dir_all(&path)?;
    }
    state.db.local_remove(&folder)
}

#[tauri::command]
pub fn local_import_folder(_folder: String) -> AppResult<bool> {
    // Reserved: future "import existing gallery folder" flow.
    Ok(false)
}

fn read_local_gallery(folder: &std::path::Path) -> Option<LocalGallery> {
    let mut id = 0i64;
    let mut title = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut media_id = 0i64;
    let mut expected_pages = None;
    let mut has_gallery_metadata = false;

    // Read the `.<id>` marker file.
    if let Ok(entries) = std::fs::read_dir(folder) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.len() > 1
                && name.starts_with('.')
                && name[1..].chars().all(|c| c.is_ascii_digit())
            {
                id = name[1..].parse().unwrap_or(0);
            }
        }
    }
    // Read metadata from `.nomedia` if present.
    let nomedia = folder.join(".nomedia");
    if let Ok(content) = std::fs::read_to_string(&nomedia) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            has_gallery_metadata = true;
            if id == 0 {
                id = v.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
            }
            media_id = v
                .get("media_id")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse().ok())
                .or_else(|| v.get("media_id").and_then(|x| x.as_i64()))
                .unwrap_or(0);
            expected_pages = v
                .get("pages")
                .and_then(|pages| pages.as_array())
                .map(Vec::len)
                .or_else(|| {
                    v.get("num_pages")
                        .and_then(|pages| pages.as_u64())
                        .map(|pages| pages as usize)
                });
            // The .nomedia file stores a full `Gallery` whose title object is
            // serialized as `titles` (plural). Restore the gallery title from
            // it so the library shows the real title instead of the on-disk
            // folder name (`[id] title`).
            let titles = v.get("titles");
            let pick = |key: &str| -> String {
                titles
                    .and_then(|t| t.get(key))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let pretty = pick("pretty");
            let english = pick("english");
            let new_title = if !pretty.is_empty() {
                pretty
            } else if !english.is_empty() {
                english
            } else {
                String::new()
            };
            if !new_title.is_empty() {
                title = new_title;
            }
        }
    }

    let mut page_files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".png")
                || lower.ends_with(".gif")
                || lower.ends_with(".webp")
            {
                page_files.push(e.path().to_string_lossy().to_string());
            }
        }
    }
    page_files.sort();
    if page_files.is_empty() && id == 0 {
        return None;
    }

    // New downloads carry an explicit completion manifest. For galleries made
    // by older releases, a full set of files from `.nomedia` is accepted and
    // its newest page timestamp becomes the stable completion time.
    let completion_path = folder.join(COMPLETED_MARKER);
    let completion = std::fs::read_to_string(&completion_path).ok();
    let mut completed_at = None;
    let mut manifest_valid = false;
    if let Some(raw) = completion.as_deref() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            completed_at = value
                .get("completed_at")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if let Some(files) = value.get("page_files").and_then(|value| value.as_array()) {
                manifest_valid = !files.is_empty()
                    && files.iter().all(|value| {
                        value
                            .as_str()
                            .map(|name| folder.join(name).is_file())
                            .unwrap_or(false)
                    });
            }
        } else if !raw.trim().is_empty() {
            // Compatibility with the short-lived plain timestamp format.
            completed_at = Some(raw.trim().to_string());
        }
    }
    if !manifest_valid {
        let looks_complete = expected_pages
            .map(|expected| expected > 0 && page_files.len() >= expected)
            .unwrap_or(!has_gallery_metadata && !page_files.is_empty() || completion.is_some());
        if !looks_complete {
            return None;
        }
    }
    let completed_at = completed_at.unwrap_or_else(|| {
        page_files
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
            .max()
            .map(chrono::DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now)
            .to_rfc3339()
    });

    Some(LocalGallery {
        id,
        title,
        thumbnail_path: page_files.first().cloned(),
        folder: folder.to_string_lossy().to_string(),
        num_pages: page_files.len(),
        page_files,
        media_id,
        scanned_at: completed_at,
        translated_title: String::new(),
    })
}

// ===========================================================================
// Downloader
// ===========================================================================

#[tauri::command]
pub async fn download_gallery(
    state: State<'_, AppState>,
    req: DownloadRequest,
) -> AppResult<serde_json::Value> {
    let api = api(&state);
    let mgr = state.downloads.clone();
    let entry = mgr.enqueue(api, req).await?;
    Ok(serde_json::to_value(entry)?)
}

#[tauri::command]
pub async fn download_range(
    state: State<'_, AppState>,
    gallery_id: i64,
    from_page: Option<usize>,
    to_page: Option<usize>,
) -> AppResult<serde_json::Value> {
    let api = api(&state);
    let mgr = state.downloads.clone();
    let entry = mgr
        .enqueue(
            api,
            DownloadRequest {
                gallery_id,
                from_page,
                to_page,
            },
        )
        .await?;
    Ok(serde_json::to_value(entry)?)
}

#[tauri::command]
pub fn download_list(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    state
        .downloads
        .list()
        .into_iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect()
}

#[tauri::command]
pub async fn download_cancel(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    state.downloads.cancel(id)
}

#[tauri::command]
pub fn download_delete(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    state.downloads.delete_download(id)
}

#[tauri::command]
pub fn download_pause(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    state.downloads.pause(id)
}

#[tauri::command]
pub async fn download_resume(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let api = api(&state);
    state.downloads.clone().resume(api, id)
}

#[tauri::command]
pub fn download_clear(state: State<'_, AppState>) -> AppResult<()> {
    state.downloads.clear_finished()
}

/// Surface persisted (resumable) downloads to the frontend on startup.
#[tauri::command]
pub fn download_rows(state: State<'_, AppState>) -> AppResult<Vec<DownloadRow>> {
    state.db.downloads_all()
}

#[tauri::command]
pub fn download_pause_ids(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    state.downloads.pause_ids(&ids)
}

#[tauri::command]
pub async fn download_resume_ids(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    let api = api(&state);
    state.downloads.clone().resume_ids(api, &ids)
}

#[tauri::command]
pub fn download_cancel_ids(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    state.downloads.cancel_ids(&ids)
}

#[tauri::command]
pub fn download_delete_ids(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    state.downloads.delete_ids(&ids)
}

// ===========================================================================
// Export
// ===========================================================================

#[tauri::command]
pub fn export_pdf(folder: String, out: Option<String>) -> AppResult<String> {
    let path = crate::export::export_pdf(
        std::path::Path::new(&folder),
        out.as_deref().map(std::path::Path::new),
    )?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn export_zip(folder: String, out: Option<String>) -> AppResult<String> {
    let path = crate::export::export_zip(
        std::path::Path::new(&folder),
        out.as_deref().map(std::path::Path::new),
    )?;
    Ok(path.to_string_lossy().to_string())
}

// ===========================================================================
// Version
// ===========================================================================

#[tauri::command]
pub fn get_app_version() -> String {
    crate::config::APP_VERSION.to_string()
}

// ===========================================================================
// Latest release (GitHub)
// ===========================================================================

/// Repo whose releases we check. Hardcoded — it is also the target of the
/// sidebar's "Get Latest Version" link, so they must stay in sync.
const RELEASE_REPO: &str = "msprivate67-commits/NClientV4";

/// Latest release info surfaced to the frontend.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LatestRelease {
    /// Release tag, e.g. "v0.1.3".
    pub tag: String,
    /// Release name / title (may be empty).
    pub name: String,
    /// HTML URL of the release page.
    pub html_url: String,
    /// True when the remote tag is strictly newer than the running app.
    pub is_newer: bool,
    /// Whether this release is marked as a pre-release.
    pub prerelease: bool,
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    prerelease: bool,
}

/// Parse a version string like "v0.1.3" / "0.1.3" / "0.1.3-rc1" into a
/// (major, minor, patch) triple. Non-numeric / trailing parts are ignored so
/// pre-release suffixes don't break the comparison.
fn parse_version(s: &str) -> (u64, u64, u64) {
    let s = s.trim().trim_start_matches(|c| c == 'v' || c == 'V');
    let mut it = s.split('.');
    let major = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = it
        .next()
        .and_then(|p| p.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

/// Fetch the latest published (non-prerelease) release from GitHub and compare
/// it against the running app version. Runs on startup in the background; any
/// network failure is surfaced as an error and the frontend simply shows the
/// current version. Returns `None` when no release has been published yet
/// (GitHub answers 404 for `/releases/latest`).
#[tauri::command]
pub async fn get_latest_release() -> AppResult<Option<LatestRelease>> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        RELEASE_REPO
    );

    // A standalone client: we deliberately do NOT reuse the app's nhentai
    // HttpClient, which carries a nhentai referer / cookies / proxy tuned for
    // the mirror. GitHub requires a User-Agent header or it answers 403.
    let client = reqwest::Client::builder()
        .user_agent(concat!("NClientV4/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(12))
        .build()?;

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    let status = resp.status();
    // 404 means no (stable) release has been published yet — not an error.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }

    let rel: GithubRelease = resp.json().await?;
    let current = parse_version(env!("CARGO_PKG_VERSION"));
    let latest = parse_version(&rel.tag_name);
    Ok(Some(LatestRelease {
        tag: rel.tag_name,
        name: rel.name.unwrap_or_default(),
        html_url: rel.html_url,
        is_newer: latest > current,
        prerelease: rel.prerelease,
    }))
}

// ===========================================================================
// Misc: open URLs / paths, asset resolution, image proxy
// ===========================================================================

#[tauri::command]
pub fn open_in_browser(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<()> {
    let base = state.config.base_url();
    let url = if path.starts_with("http") {
        path
    } else if let Ok(id) = path.parse::<i64>() {
        format!("{}g/{}", base, id)
    } else {
        format!("{}{}", base, path.trim_start_matches('/'))
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub fn open_api_key_docs(app: AppHandle) -> AppResult<()> {
    app.opener()
        .open_url(
            "https://nhentai.net/api/v2/docs#/galleries/get_all_galleries_api_v2_galleries_get",
            None::<&str>,
        )
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

/// Convert a `file://` or absolute path to an asset URL the frontend can
/// load in `<img src>`. The scheme is platform-specific: WebView2
/// (Windows/Android) requires `http://asset.localhost/`, while macOS/Linux
/// use `asset://localhost/`.
#[tauri::command]
pub fn resolve_asset(path: String) -> AppResult<String> {
    let p = if let Some(rest) = path.strip_prefix("file://") {
        rest.to_string()
    } else {
        path
    };
    Ok(asset_url(&p))
}

/// For remote images we cannot load directly from the renderer (CSP), return
/// a hint the frontend uses to set `src` — the `asset` protocol handler
/// serves local files; remote ones go through the normal `<img>` with a
/// relaxed CSP. This command exists so the frontend can ask for the right
/// scheme given a path/URL.
#[tauri::command]
pub fn image_proxy_url(url: String) -> String {
    if url.starts_with("http") {
        url
    } else {
        asset_url(url.trim_start_matches('/'))
    }
}

/// Fetch image bytes through the native HTTP client and return a raw IPC body.
///
/// Do not make Android WebView wait on an asynchronous custom-scheme request:
/// pending intercepted requests can suppress otherwise-ready UI frames. Raw
/// IPC lets the page shell paint immediately while Rust downloads in the
/// background; the frontend converts these bytes to a shared blob URL.
#[tauri::command]
pub async fn image_fetch(
    state: State<'_, AppState>,
    source: String,
) -> Result<tauri::ipc::Response, String> {
    let current_settings = settings(&state);
    let image = crate::image_protocol::load_image(&state.http, &current_settings, &source).await?;
    Ok(tauri::ipc::Response::new(image.body.as_ref().to_vec()))
}

/// Build a per-platform asset URL for a local path. Mirrors the scheme
/// selection done by Tauri's frontend `convertFileSrc` helper:
/// `http://asset.localhost/<path>` on Windows & Android, `asset://localhost/`
/// elsewhere. The path is percent-encoded like the JS `encodeURIComponent`
/// that `convertFileSrc` uses.
fn asset_url(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let encoded = percent_encode_path(&normalized);
    // WebView2 (Windows) and Android's WebView only recognise the asset
    // protocol under the `http://asset.localhost` host; macOS/Linux use the
    // bare `asset://` scheme.
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        format!("http://asset.localhost/{}", encoded)
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        format!("asset://localhost/{}", encoded)
    }
}

/// Percent-encode a path for use in a URL path component, matching JS
/// `encodeURIComponent` (encodes everything except the unreserved set
/// `A-Za-z0-9-_.!~*'()` and the path separators `/`).
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
            | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Read a local image file as base64 data URL. Useful when the asset protocol
/// is unavailable or for tiny thumbnails.
#[tauri::command]
pub fn read_local_image(path: String) -> AppResult<Option<String>> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&p)?;
    let mime = mime_guess::from_path(&p)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    // Default to image/jpeg for known image extensions that mime_guess reports
    // as application/octet-stream (e.g. some .webp / .gif variants).
    let mime = if mime == "application/octet-stream" {
        "image/jpeg".to_string()
    } else {
        mime
    };
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    Ok(Some(format!("data:{};base64,{}", mime, b64)))
}

/// Register the download manager's app handle for event emission. Called once
/// from the frontend shortly after startup (and also at `setup` time).
#[tauri::command]
pub fn register_app(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    state.downloads.set_app_handle(app);
    Ok(())
}

// Suppress unused-import warning while keeping `HttpClient` /
// `DownloadStatus` reachable for type inference in tooling.
#[allow(dead_code)]
fn _type_anchors(_h: &HttpClient, _s: DownloadStatus, _t: &Tag) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!(
            "nclientv4-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn interrupted_gallery_is_not_added_to_local_library() {
        let folder = test_folder("partial-gallery");
        std::fs::write(folder.join(".123"), []).unwrap();
        std::fs::write(folder.join("001.jpg"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        std::fs::write(
            folder.join(".nomedia"),
            serde_json::json!({
                "id": 123,
                "pages": [{"path": "one.jpg"}, {"path": "two.jpg"}]
            })
            .to_string(),
        )
        .unwrap();

        assert!(read_local_gallery(&folder).is_none());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn completion_manifest_supports_finished_page_ranges_and_stable_time() {
        let folder = test_folder("range-gallery");
        let completed_at = "2026-07-28T12:34:56+00:00";
        std::fs::write(folder.join(".456"), []).unwrap();
        std::fs::write(folder.join("002.jpg"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        std::fs::write(
            folder.join(".nomedia"),
            serde_json::json!({
                "id": 456,
                "pages": [
                    {"path": "one.jpg"},
                    {"path": "two.jpg"},
                    {"path": "three.jpg"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            folder.join(COMPLETED_MARKER),
            serde_json::json!({
                "completed_at": completed_at,
                "page_files": ["002.jpg"]
            })
            .to_string(),
        )
        .unwrap();

        let gallery = read_local_gallery(&folder).unwrap();
        assert_eq!(gallery.num_pages, 1);
        assert_eq!(gallery.scanned_at, completed_at);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
