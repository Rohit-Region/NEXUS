package com.nexus.mobile

import android.graphics.Color
import android.os.Bundle
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Before the app starts, so the first secret Rust asks for is there, and
    // anything copied over from the Mac is in place when it looks.
    val secrets = SecretStore(applicationContext)
    registerSecretStore(secrets)
    getExternalFilesDir(null)?.let { shared ->
      MacTransfer.takeSecrets(shared, secrets)
      MacTransfer.publishKey(shared)
    }
    takeDataFromMac()
    // One inbox for the process, as Rust keeps the first it is given; each
    // new activity becomes the one it asks for permission from.
    (bankInbox ?: BankInbox(applicationContext).also {
      bankInbox = it
      registerBankInbox(it)
    }).attach(this)
    // NEXUS is always dark, so the bar icons are always light.
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
    )
    super.onCreate(savedInstanceState)
    // The WebView does not report the system bars to the page, so the page
    // is kept clear of them here: the status bar, the gesture bar, a camera
    // cutout and the open keyboard.
    ViewCompat.setOnApplyWindowInsetsListener(findViewById(android.R.id.content)) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or
          WindowInsetsCompat.Type.displayCutout() or
          WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  /**
   * The Mac's data, copied over by `scripts/mac-to-phone.sh`. The script can
   * only write to this app's external folder, which other apps with storage
   * access can read, so the file is moved into private storage at once.
   * Rust puts it in place of the database on start (`db/from_mac.rs`).
   */
  private fun takeDataFromMac() {
    val pushed = File(getExternalFilesDir(null) ?: return, DATA_FROM_MAC)
    if (!pushed.exists()) return
    try {
      pushed.copyTo(File(dataDir, DATA_FROM_MAC), overwrite = true)
    } finally {
      pushed.delete()
    }
  }

  /** Hands the store to Rust (`assistant/android_secrets.rs`). */
  private external fun registerSecretStore(store: SecretStore)

  /** Hands the inbox to Rust (`assistant/android_sms.rs`). */
  private external fun registerBankInbox(inbox: BankInbox)

  private companion object {
    /** The name `db/from_mac.rs` looks for. */
    const val DATA_FROM_MAC = "nexus-db-from-mac.sqlite"

    var bankInbox: BankInbox? = null

    init {
      // Loaded again by the generated `Rust` object later; a second load is
      // a no-op. Needed here because `registerSecretStore` runs first.
      System.loadLibrary("app_lib")
    }
  }
}
