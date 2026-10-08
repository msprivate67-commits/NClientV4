//! Android-native privacy controls.

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, State, Wry,
};

#[cfg(target_os = "android")]
use serde::Serialize;
#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

pub struct AndroidPrivacy {
    #[cfg(target_os = "android")]
    handle: PluginHandle<Wry>,
}

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetPrivacyScreenArgs {
    enabled: bool,
}

impl AndroidPrivacy {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin::<()>("setPrivacyScreen", SetPrivacyScreenArgs { enabled })
                .map_err(|error| error.to_string())?;
        }

        #[cfg(not(target_os = "android"))]
        let _ = enabled;

        Ok(())
    }
}

pub fn init() -> TauriPlugin<Wry> {
    Builder::new("android-privacy")
        .setup(|app: &AppHandle<Wry>, _api| {
            #[cfg(target_os = "android")]
            let privacy = AndroidPrivacy {
                handle: _api.register_android_plugin("com.nclientv4.app", "PrivacyPlugin")?,
            };
            #[cfg(not(target_os = "android"))]
            let privacy = AndroidPrivacy {};

            app.manage(privacy);
            Ok(())
        })
        .build()
}

#[tauri::command]
pub fn android_privacy_set(
    privacy: State<'_, AndroidPrivacy>,
    enabled: bool,
) -> Result<(), String> {
    privacy.set_enabled(enabled)
}
