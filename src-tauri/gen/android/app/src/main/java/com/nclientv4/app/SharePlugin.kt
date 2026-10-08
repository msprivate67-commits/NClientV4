package com.nclientv4.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

object ShareTextStore {
    private const val PREFERENCES = "nclientv4_android_share"
    private const val PENDING_TEXT = "pending_text"

    fun store(context: Context, text: String) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString(PENDING_TEXT, text)
            .apply()
    }

    fun take(context: Context): String? {
        val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
        val text = preferences.getString(PENDING_TEXT, null)
        if (text != null) preferences.edit().remove(PENDING_TEXT).apply()
        return text
    }
}

@InvokeArg
class ShareTextArgs {
    lateinit var text: String
    lateinit var title: String
}

@TauriPlugin
class SharePlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun takeSharedText(invoke: Invoke) {
        val response = JSObject()
        response.put("text", ShareTextStore.take(activity))
        invoke.resolve(response)
    }

    @Command
    fun shareText(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(ShareTextArgs::class.java)
            val sendIntent = Intent(Intent.ACTION_SEND).apply {
                type = "text/plain"
                putExtra(Intent.EXTRA_TEXT, args.text)
            }
            activity.startActivity(Intent.createChooser(sendIntent, args.title))
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Unable to open Android share sheet")
        }
    }
}
