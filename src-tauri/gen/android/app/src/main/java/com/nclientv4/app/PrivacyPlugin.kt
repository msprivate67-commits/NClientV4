package com.nclientv4.app

import android.app.Activity
import android.content.Context
import android.view.WindowManager
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class SetPrivacyScreenArgs {
    var enabled: Boolean = false
}

@TauriPlugin
class PrivacyPlugin(private val activity: Activity) : Plugin(activity) {
    private val preferences = activity.getSharedPreferences(
        "nclientv4_android_privacy",
        Context.MODE_PRIVATE,
    )

    init {
        applySecureFlag(preferences.getBoolean("enabled", false))
    }

    private fun applySecureFlag(enabled: Boolean) {
        activity.runOnUiThread {
            if (enabled) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
            }
        }
    }

    @Command
    fun setPrivacyScreen(invoke: Invoke) {
        val args = invoke.parseArgs(SetPrivacyScreenArgs::class.java)
        preferences.edit().putBoolean("enabled", args.enabled).apply()
        applySecureFlag(args.enabled)
        invoke.resolve()
    }
}
