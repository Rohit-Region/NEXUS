package com.nexus.mobile

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import android.provider.Telephony
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.lang.ref.WeakReference
import java.security.MessageDigest

/**
 * Bank texts read straight from this phone's inbox, for the Spending tab.
 *
 * On the Mac the texts arrive through a Google Sheet that a forwarder app on
 * the phone fills (`docs/bank-sms/Code.gs`); here NEXUS is on the phone that
 * receives them, so it reads them itself and needs no link or key. The same
 * rules as the script apply: only ICICI and HDFC senders, never a one-time
 * code. Each text gets the script's id, so one that also came in through the
 * Sheet is booked once. Rust calls `allowed`, `ask` and `after` over JNI (see
 * `assistant/android_sms.rs`).
 */
class BankInbox(private val context: Context) {
  private var activity = WeakReference<Activity>(null)

  /** The activity to ask from; it changes when the app is reopened. */
  fun attach(current: Activity) {
    activity = WeakReference(current)
  }

  fun allowed(): Boolean =
    context.checkSelfPermission(Manifest.permission.READ_SMS) == PackageManager.PERMISSION_GRANTED

  /**
   * Ask for permission to read texts. Android stops showing its prompt once
   * the user has refused twice, so after that this opens NEXUS's page in the
   * phone's settings instead. Returns at once; the answer is seen by
   * `allowed` later.
   */
  fun ask() {
    val current = activity.get() ?: return
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    current.runOnUiThread {
      val askedBefore = prefs.getBoolean(ASKED, false)
      if (askedBefore && !current.shouldShowRequestPermissionRationale(Manifest.permission.READ_SMS)) {
        current.startActivity(
          Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
        )
      } else {
        prefs.edit().putBoolean(ASKED, true).apply()
        current.requestPermissions(arrayOf(Manifest.permission.READ_SMS), REQUEST)
      }
    }
  }

  /**
   * Bank texts with an inbox row number above `after`, oldest first, at most
   * `limit` of them, as JSON: `{"last": n, "rows": [...]}`. `last` is the
   * highest row looked at, bank text or not, so other texts are not read
   * again. Shaped like the Sheet's rows (`bank_sms::Row`).
   */
  fun after(after: Long, limit: Int): String {
    val rows = JSONArray()
    var last = after
    context.contentResolver.query(
      Telephony.Sms.Inbox.CONTENT_URI,
      arrayOf(Telephony.Sms._ID, Telephony.Sms.ADDRESS, Telephony.Sms.BODY, Telephony.Sms.DATE, Telephony.Sms.DATE_SENT),
      "${Telephony.Sms._ID} > ?",
      arrayOf(after.toString()),
      "${Telephony.Sms._ID} ASC LIMIT $limit",
    )?.use { cursor ->
      while (cursor.moveToNext()) {
        val row = cursor.getLong(0)
        last = row
        val from = cursor.getString(1) ?: continue
        val text = cursor.getString(2) ?: continue
        if (!BANK_SENDER.containsMatchIn(from) || NEVER_STORE.containsMatchIn(text)) continue
        val sent = cursor.getLong(4)
        rows.put(
          JSONObject()
            .put("row", row)
            .put("received", cursor.getLong(3))
            .put("from", from)
            .put("text", text)
            .put("id", id(from, sent, text))
        )
      }
    }
    return JSONObject().put("last", last).put("rows", rows).toString()
  }

  private companion object {
    const val PREFS = "nexus_bank_inbox"
    const val ASKED = "asked"
    const val REQUEST = 7301

    // As in docs/bank-sms/Code.gs.
    val BANK_SENDER = Regex("ICICI|HDFC", RegexOption.IGNORE_CASE)
    val NEVER_STORE = Regex(
      "\\bOTP\\b|one[\\s-]?time\\s+pass|verification\\s+code|do not share",
      RegexOption.IGNORE_CASE,
    )

    /** The script's id: SHA-256 of `from|sent|text`, cut to the lengths it
     *  keeps, in web-safe base64, 22 characters. */
    fun id(from: String, sent: Long, text: String): String {
      val digest = MessageDigest.getInstance("SHA-256")
        .digest("${from.take(64)}|$sent|${text.take(1000)}".toByteArray(Charsets.UTF_8))
      return Base64.encodeToString(digest, Base64.URL_SAFE or Base64.NO_WRAP).take(22)
    }
  }
}
