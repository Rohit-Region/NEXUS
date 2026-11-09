package com.nexus.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The phone's stand-in for the Mac's Keychain: tokens and links that must not
 * sit in the database in the clear.
 *
 * Each value is encrypted with an AES key that lives in the Android Keystore,
 * which never hands the key out, and the ciphertext is kept in the app's
 * private storage. Rust calls `get`, `put` and `remove` over JNI from whichever
 * thread needs a secret (see `assistant/android_secrets.rs`).
 */
class SecretStore(context: Context) {
  private val prefs = context.getSharedPreferences("nexus_secrets", Context.MODE_PRIVATE)

  @Synchronized
  fun get(name: String): String? {
    val stored = prefs.getString(name, null) ?: return null
    return try {
      val bytes = Base64.decode(stored, Base64.NO_WRAP)
      val cipher = Cipher.getInstance(TRANSFORMATION)
      cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, bytes, 0, IV_BYTES))
      String(cipher.doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES), Charsets.UTF_8)
    } catch (e: Exception) {
      // Unreadable, e.g. restored from a backup onto a phone whose Keystore
      // never held this key. Forget it so it can simply be entered again.
      prefs.edit().remove(name).commit()
      null
    }
  }

  @Synchronized
  fun put(name: String, value: String): Boolean {
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, key())
    val sealed = cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
    return prefs.edit().putString(name, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()
  }

  @Synchronized
  fun remove(name: String): Boolean = prefs.edit().remove(name).commit()

  private fun key(): SecretKey {
    val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    (keyStore.getKey(ALIAS, null) as SecretKey?)?.let { return it }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
    generator.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build()
    )
    return generator.generateKey()
  }

  private companion object {
    const val KEYSTORE = "AndroidKeyStore"
    const val ALIAS = "nexus_secrets"
    const val TRANSFORMATION = "AES/GCM/NoPadding"
    const val IV_BYTES = 12
    const val TAG_BITS = 128
  }
}
