//! Cloudflare challenge resolution.
//!
//! Opens a webview for the configured mirror and lets the user solve the
//! challenge; once `cf_clearance` appears in the WebView's cookie jar it is
//! copied into the persistent HTTP cookie jar.
//!
//! On Tauri 2 we open a dedicated webview window, run a small JS probe via
//! `webview.eval` that polls `document.cookie`, and forward the captured
//! cookie into our `reqwest_cookie_store`-backed jar through the regular
//! Tauri event bus.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Listener, Manager, WebviewWindowBuilder, WindowEvent};
use tokio::sync::oneshot;

use crate::config::Settings;
use crate::error::{AppError, AppResult};
use crate::http::HttpClient;
use crate::models::CfState;

/// Window label used for the CF challenge webview.
pub const CF_WINDOW_LABEL: &str = "cf-challenge";

/// Global CF state, updated from the webview and polled by the frontend.
static CF_STATE: Mutex<CfState> = Mutex::new(CfState::Unknown);

pub fn set_state(state: CfState) {
    *CF_STATE.lock() = state;
}

pub fn current_state() -> CfState {
    *CF_STATE.lock()
}

/// Mirrors `CookieInterceptor#endInterceptor()`: returns true once
/// `cf_clearance` is present.
pub fn is_solved() -> bool {
    matches!(current_state(), CfState::Solved)
}

/// Open (or focus) the CF challenge window. Mirrors
/// `CookieInterceptor#interceptInternal()` which calls `loadUrl(baseUrl)`.
pub fn open_challenge(
    app: &AppHandle,
    http: Arc<HttpClient>,
    base_url: String,
    settings: &Settings,
) -> AppResult<()> {
    set_state(CfState::Pending);
    // Reuse existing window if present.
    if let Some(existing) = app.get_webview_window(CF_WINDOW_LABEL) {
        let _ = existing.set_focus();
        return Ok(());
    }

    let (tx, rx) = oneshot::channel::<()>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let mut builder = WebviewWindowBuilder::new(
        app,
        CF_WINDOW_LABEL,
        tauri::WebviewUrl::External(
            base_url
                .parse()
                .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?,
        ),
    )
    .title("Cloudflare verification — solve then close")
    .inner_size(520.0, 700.0)
    .resizable(true)
    .initialization_script(&probe_script());
    if let Some(proxy) = crate::http::webview_proxy_url(settings) {
        builder = builder.proxy_url(proxy);
    }
    let window = builder
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;

    let tx_clone = tx.clone();
    let app_handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            // Window closed without resolution => fail this attempt.
            if let Some(sender) = tx_clone.lock().take() {
                let _ = sender.send(());
            }
            set_state(if is_solved() {
                CfState::Solved
            } else {
                CfState::Failed
            });
            let _ = app_handle.emit("cf-state", current_state());
        }
    });

    // Listen for cookies reported by the probe script.
    let app_clone = app.clone();
    let _id = app.listen_any("cf-cookie", move |event| {
        // `Event::payload()` returns `&str` in Tauri 2.
        let payload: &str = event.payload();
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
            if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                let value = json.get("value").and_then(|v| v.as_str()).unwrap_or("");
                let pair = format!("{}={}", name, value);
                let url = format!(
                    "https://{}/",
                    json.get("host")
                        .and_then(|v| v.as_str())
                        .unwrap_or("nhentai.net")
                );
                let _ = http.set_cookie_str(&url, &pair);
                if name == "cf_clearance" {
                    set_state(CfState::Solved);
                    let _ = app_clone.emit("cf-state", CfState::Solved);
                    if let Some(window) = app_clone.get_webview_window(CF_WINDOW_LABEL) {
                        let _ = window.close();
                    }
                }
            }
        }
    });

    // Wait up to 5 minutes for resolution, keeping the event listener alive
    // for the duration of the challenge window.
    let deadline = Instant::now() + Duration::from_secs(300);
    tokio::spawn(async move {
        let _guard = _id;
        let _ = tokio::time::timeout_at(deadline.into(), rx).await;
    });

    Ok(())
}

/// JS injected into the CF window. Polls `document.cookie` every second and
/// emits every cookie pair via the Tauri event bus. Mirrors the polling loop
/// in `CookieInterceptor#interceptInternal()`.
fn probe_script() -> String {
    r#"
    (function () {
      if (window.__nclientv4CfProbe) return;
      window.__nclientv4CfProbe = true;
      var seen = {};
      function emitCookies() {
        try {
          var host = location.host;
          var raw = document.cookie || "";
          raw.split(';').forEach(function (part) {
            var idx = part.indexOf('=');
            if (idx < 0) return;
            var name = part.slice(0, idx).trim();
            var value = part.slice(idx + 1).trim();
            var key = name + '@' + host;
            if (seen[key] !== value) {
              seen[key] = value;
              if (window.__TAURI_INTERNALS__) {
                window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
                  event: 'cf-cookie',
                  payload: { host: host, name: name, value: value }
                }).catch(function(){});
              }
            }
          });
        } catch (e) {}
      }
      setInterval(emitCookies, 1000);
      emitCookies();
    })();
    "#
    .to_string()
}
