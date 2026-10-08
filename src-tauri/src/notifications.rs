//! Platform-specific download notifications.
//!
//! The cross-platform Tauri notification plugin cannot update Windows toast
//! progress in place. Windows therefore uses the WinRT progress API so page
//! events update one existing toast without repeatedly showing popups.

use tauri::AppHandle;

#[cfg(windows)]
use tauri::Manager;
#[cfg(windows)]
use tauri_winrt_notification::{Duration, IconCrop, NotificationUpdateResult, Progress, Toast};

#[cfg(windows)]
const DOWNLOAD_TAG: &str = "nclientv4-download-progress";

#[cfg(windows)]
static USES_POWERSHELL_FALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn notification_icon(app: &AppHandle) -> std::path::PathBuf {
    if cfg!(debug_assertions) {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons/icon.png")
    } else {
        app.path()
            .resource_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("icons/icon.png")
    }
}

#[cfg(windows)]
fn toast(app: &AppHandle, app_id: &str, heading: &str, progress: &Progress) -> Toast {
    let mut toast = Toast::new(app_id)
        .title("NClientV4")
        .text1(heading)
        .progress(progress)
        .duration(Duration::Long)
        .sound(None);
    let icon = notification_icon(app);
    if icon.exists() {
        toast = toast.icon(&icon, IconCrop::Square, "NClientV4");
    }
    toast
}

#[cfg(windows)]
fn app_ids(app: &AppHandle) -> [&str; 2] {
    [app.config().identifier.as_str(), Toast::POWERSHELL_APP_ID]
}

/// Show or silently update the single Windows download progress toast.
#[tauri::command]
pub fn windows_download_progress(
    app: AppHandle,
    title: String,
    status: String,
    value_string: String,
    value: f32,
    initial: bool,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let progress = Progress {
            tag: DOWNLOAD_TAG.to_string(),
            title: title.clone(),
            status,
            value: value.clamp(0.0, 1.0),
            value_string,
        };

        if !initial {
            let fallback = USES_POWERSHELL_FALLBACK.load(std::sync::atomic::Ordering::Relaxed);
            let app_id = if fallback {
                Toast::POWERSHELL_APP_ID
            } else {
                app.config().identifier.as_str()
            };
            return match toast(&app, app_id, &title, &progress).set_progress(&progress) {
                Ok(NotificationUpdateResult::Succeeded)
                | Ok(NotificationUpdateResult::NotificationNotFound) => Ok(()),
                Ok(_) => Err("Windows rejected the progress update".to_string()),
                Err(error) => Err(error.to_string()),
            };
        }

        for (index, app_id) in app_ids(&app).into_iter().enumerate() {
            let candidate = toast(&app, app_id, &title, &progress);
            if candidate.show().is_ok() {
                USES_POWERSHELL_FALLBACK.store(index == 1, std::sync::atomic::Ordering::Relaxed);
                return Ok(());
            }
        }
        return Err("Windows rejected the download notification".to_string());
    }

    #[cfg(not(windows))]
    {
        let _ = (app, title, status, value_string, value, initial);
        Ok(())
    }
}

/// Replace the Windows progress toast with a final, visible completion state.
#[tauri::command]
pub fn windows_download_complete(
    app: AppHandle,
    title: String,
    status: String,
    value_string: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let progress = Progress {
            tag: DOWNLOAD_TAG.to_string(),
            title: title.clone(),
            status,
            value: 1.0,
            value_string,
        };
        let fallback = USES_POWERSHELL_FALLBACK.load(std::sync::atomic::Ordering::Relaxed);
        let preferred_app_id = if fallback {
            Toast::POWERSHELL_APP_ID
        } else {
            app.config().identifier.as_str()
        };
        if toast(&app, preferred_app_id, &title, &progress)
            .show()
            .is_ok()
        {
            return Ok(());
        }
        for app_id in app_ids(&app) {
            if toast(&app, app_id, &title, &progress).show().is_ok() {
                return Ok(());
            }
        }
        return Err("Windows rejected the completion notification".to_string());
    }

    #[cfg(not(windows))]
    {
        let _ = (app, title, status, value_string);
        Ok(())
    }
}
