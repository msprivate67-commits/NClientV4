package com.nclientv4.app

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        captureSharedText(intent)

        // Android 15+ enforces edge-to-edge layouts for apps targeting recent
        // SDKs. Inset Tauri's WebView by the system bars and any display cutout
        // so the app toolbar is not rendered underneath the notification bar.
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, windowInsets ->
            val safeInsets = windowInsets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout(),
            )
            view.setPadding(
                safeInsets.left,
                safeInsets.top,
                safeInsets.right,
                safeInsets.bottom,
            )
            windowInsets
        }
        ViewCompat.requestApplyInsets(content)

        onBackPressedDispatcher.addCallback(
            this,
            object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    dispatchHardwareBackToWebView()
                }
            },
        )
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (captureSharedText(intent)) dispatchShareToWebView()
    }

    private fun captureSharedText(intent: Intent?): Boolean {
        if (intent?.action != Intent.ACTION_SEND) return false
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim()
        if (text.isNullOrEmpty()) return false
        ShareTextStore.store(this, text)
        return true
    }

    private fun dispatchShareToWebView() {
        val webView = findWebView(findViewById(android.R.id.content))
        webView?.post {
            webView.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('nclientv4:android-share'));",
                null,
            )
        }
    }

    private fun dispatchHardwareBackToWebView() {
        val webView = findWebView(findViewById(android.R.id.content))
        webView?.post {
            webView.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('nclientv4:android-back'));",
                null,
            )
        }
    }

    private fun findWebView(view: View?): WebView? {
        if (view is WebView) return view
        if (view !is ViewGroup) return null

        for (i in 0 until view.childCount) {
            val webView = findWebView(view.getChildAt(i))
            if (webView != null) return webView
        }

        return null
    }
}
