//! NClientV4 - cross-platform nhentai API v2 desktop client.
//!
//! Module layout:
//! - [`config`]    : persistent user settings (mirror, UA, auth, paths, ...)
//! - [`error`]     : shared error / result types
//! - [`http`]      : HTTP client with UA + persistent cookie jar + Cloudflare detection
//! - [`api`]       : nhentai API v2 client (browse / search / detail / favorites / comments)
//! - [`models`]    : serializable data structures for nhentai API v2 (Gallery, Tag, Page, ...)
//! - [`cloudflare`] : Cloudflare challenge detection + cf_clearance capture via webview
//! - [`db`]        : SQLite store for favorites, history, tags, local library, blacklists
//! - [`downloader`]: gallery download manager (queue, progress, pause / cancel)
//! - [`export`]    : PDF / ZIP export
//! - [`commands`]  : Tauri command handlers exposed to the frontend

pub mod android_share;
pub mod api;
pub mod cloudflare;
pub mod commands;
pub mod config;
pub mod db;
pub mod downloader;
pub mod error;
pub mod export;
pub mod http;
pub mod image_protocol;
pub mod models;
pub mod notifications;
pub mod privacy;

use commands::*;
use std::sync::Arc;
use tauri::Manager;

use crate::android_share::*;
use crate::api::ApiClient;
use crate::config::ConfigStore;
use crate::db::Database;
use crate::downloader::DownloadManager;
use crate::http::HttpClient;
use crate::notifications::*;
use crate::privacy::*;

/// Shared application state, reachable from every Tauri command.
pub struct AppState {
    pub config: Arc<ConfigStore>,
    pub http: Arc<HttpClient>,
    pub db: Database,
    pub downloads: Arc<DownloadManager>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .try_init()
        .ok();

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(image_protocol::SCHEME, image_protocol::handle)
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        .plugin(android_share::init())
        .plugin(privacy::init())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app_data dir");
            std::fs::create_dir_all(&app_data).ok();

            let config = Arc::new(ConfigStore::load_or_init(&app_data));
            let db = Database::open(&app_data).expect("failed to open database");
            crate::db::register_global(&db);
            let http = Arc::new(HttpClient::new(&config));
            let downloads = Arc::new(DownloadManager::new(
                config.download_dir().to_path_buf(),
                http.clone(),
                db.clone(),
            ));

            // Wire the app handle so the downloader can emit progress events.
            let downloads_for_handle = downloads.clone();
            let app_handle = app.handle().clone();
            downloads_for_handle.set_app_handle(app_handle);

            // Rebuild interrupted queue entries and continue from valid page
            // files already present in their original folders.
            downloads
                .clone()
                .restore_unfinished(ApiClient::new(http.clone(), config.clone()));

            app.manage(AppState {
                config,
                http,
                db,
                downloads,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // config / settings
            settings_get,
            settings_set,
            translation_stream_request,
            translation_cache_get,
            translation_cache_set,
            translation_cache_clear,
            translation_cache_count,
            settings_get_paths,
            settings_pick_directory,
            settings_list_download_candidates,
            settings_clear_cookies,
            // auth / cloudflare
            auth_get,
            auth_set_api_key,
            auth_clear,
            auth_status,
            cloudflare_check,
            cloudflare_open_challenge,
            cloudflare_is_solved,
            // api
            api_browse,
            api_search,
            api_random,
            api_get_gallery,
            api_get_user,
            api_get_comments,
            api_get_favorites_page,
            api_check_favorite,
            api_add_favorite,
            api_remove_favorite,
            api_sync_local_favorites,
            api_get_tags,
            api_get_popular_tags,
            // favorites / tags (local DB)
            fav_add,
            fav_remove,
            fav_list,
            fav_is_favorite,
            tags_get_all,
            tags_get_by_type,
            tags_set_status,
            tags_get_blacklist,
            tags_add_blacklist,
            tags_remove_blacklist,
            tags_search,
            tags_get_popular,
            // history
            history_add,
            history_list,
            history_clear,
            // read progress
            read_progress_set,
            read_progress_reset,
            read_progress_get,
            read_progress_ids,
            local_reader_progress_set,
            local_reader_progress_get,
            // local library
            local_scan,
            local_list,
            local_ids,
            local_get,
            local_get_meta,
            local_set_translated_title,
            local_delete,
            local_import_folder,
            // downloader
            download_gallery,
            download_range,
            download_list,
            download_cancel,
            download_delete,
            download_pause,
            download_resume,
            download_clear,
            download_rows,
            download_pause_ids,
            download_resume_ids,
            download_cancel_ids,
            download_delete_ids,
            windows_download_progress,
            windows_download_complete,
            android_privacy_set,
            android_share_take,
            android_share_text,
            // export
            export_pdf,
            export_zip,
            // version
            get_app_version,
            get_latest_release,
            // misc
            open_in_browser,
            open_api_key_docs,
            open_path,
            resolve_asset,
            image_proxy_url,
            image_fetch,
            read_local_image,
            register_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
