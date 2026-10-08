//! Android share-target text bridge.

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, State, Wry,
};

#[cfg(target_os = "android")]
use serde::Deserialize;
#[cfg(target_os = "android")]
use serde::Serialize;
#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

pub struct AndroidShare {
    #[cfg(target_os = "android")]
    handle: PluginHandle<Wry>,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct SharedTextResponse {
    text: Option<String>,
}

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareTextArgs {
    text: String,
    title: String,
}

impl AndroidShare {
    fn take_text(&self) -> Result<Option<String>, String> {
        #[cfg(target_os = "android")]
        {
            let response = self
                .handle
                .run_mobile_plugin::<SharedTextResponse>("takeSharedText", ())
                .map_err(|error| error.to_string())?;
            return Ok(response.text);
        }

        #[cfg(not(target_os = "android"))]
        Ok(None)
    }

    fn share_text(&self, text: String, title: String) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<()>("shareText", ShareTextArgs { text, title })
                .map_err(|error| error.to_string())?;
        }

        #[cfg(not(target_os = "android"))]
        let _ = (text, title);

        Ok(())
    }
}

pub fn init() -> TauriPlugin<Wry> {
    Builder::new("android-share")
        .setup(|app: &AppHandle<Wry>, _api| {
            #[cfg(target_os = "android")]
            let share = AndroidShare {
                handle: _api.register_android_plugin("com.nclientv4.app", "SharePlugin")?,
            };
            #[cfg(not(target_os = "android"))]
            let share = AndroidShare {};

            app.manage(share);
            Ok(())
        })
        .build()
}

#[tauri::command]
pub fn android_share_take(share: State<'_, AndroidShare>) -> Result<Option<String>, String> {
    share.take_text()
}

#[tauri::command]
pub fn android_share_text(
    share: State<'_, AndroidShare>,
    text: String,
    title: String,
) -> Result<(), String> {
    share.share_text(text, title)
}
