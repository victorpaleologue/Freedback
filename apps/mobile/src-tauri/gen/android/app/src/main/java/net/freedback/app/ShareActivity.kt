package net.freedback.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle

/**
 * Dedicated share entry point ("Look up feedback…" in the share sheet and the
 * text-selection toolbar).
 *
 * CUSTOMIZED (keep when regenerating with `tauri android init`): receives
 * `ACTION_SEND text/plain` and `ACTION_PROCESS_TEXT`, rewrites the shared text
 * as a `freedback://share?text=<urlencoded>` VIEW intent aimed at
 * [MainActivity], and finishes immediately. The deep-link plugin
 * (tauri-plugin-deep-link) delivers that URI to Rust, which stores it as the
 * pending share (drained by the `take_pending_share` command) and emits a
 * `share` event to the webview.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = when (intent?.action) {
            Intent.ACTION_SEND -> intent.getStringExtra(Intent.EXTRA_TEXT)
            Intent.ACTION_PROCESS_TEXT ->
                intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
            else -> null
        }
        if (!text.isNullOrBlank()) {
            val uri = Uri.parse("freedback://share?text=" + Uri.encode(text))
            val forward = Intent(Intent.ACTION_VIEW, uri).apply {
                setClass(this@ShareActivity, MainActivity::class.java)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            }
            startActivity(forward)
        }
        finish()
    }
}
