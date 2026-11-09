package com.nexus.mobile

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONArray
import java.io.File
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.spec.MGF1ParameterSpec
import javax.crypto.Cipher
import javax.crypto.spec.OAEPParameterSpec
import javax.crypto.spec.PSource
import kotlin.concurrent.thread

/**
 * Secrets sent from the Mac over USB by `scripts/mac-to-phone.sh`,
 * sealed so that only this phone can open them.
 *
 * The phone holds an RSA key pair in the Android Keystore, whose private half
 * never leaves it, and publishes the public half in the app's external folder.
 * The Mac encrypts each secret with that public key as it reads it from the
 * Keychain, so a secret is never in the clear on the cable, on the Mac's disk
 * or on the phone's. Opened secrets go straight into `SecretStore`.
 */
object MacTransfer {
  const val PUBLIC_KEY = "nexus-transfer-key.pem"
  const val SEALED = "nexus-secrets-from-mac.json"

  private const val KEYSTORE = "AndroidKeyStore"
  private const val ALIAS = "nexus_transfer"
  /** What the Mac's `openssl pkeyutl -pkeyopt rsa_padding_mode:oaep` makes. */
  private const val TRANSFORMATION = "RSA/ECB/OAEPWithSHA-1AndMGF1Padding"

  /** Writes the public key for the script, making the pair the first time.
   *  Off the main thread: a 4096-bit key takes a moment to make. */
  fun publishKey(dir: File) {
    thread(name = "nexus-transfer-key") {
      val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
      if (!keyStore.containsAlias(ALIAS)) {
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_RSA, KEYSTORE).apply {
          initialize(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_DECRYPT)
              .setKeySize(4096)
              .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_RSA_OAEP)
              .setDigests(KeyProperties.DIGEST_SHA1, KeyProperties.DIGEST_SHA256)
              .build()
          )
        }.generateKeyPair()
      }
      val encoded = keyStore.getCertificate(ALIAS).publicKey.encoded
      val body = Base64.encodeToString(encoded, Base64.DEFAULT)
      File(dir, PUBLIC_KEY).writeText("-----BEGIN PUBLIC KEY-----\n$body-----END PUBLIC KEY-----\n")
    }
  }

  /** Opens each sealed secret into the store, then deletes the file, opened
   *  or not, so a bad copy is not retried on every start. */
  fun takeSecrets(dir: File, store: SecretStore) {
    val sealed = File(dir, SEALED)
    if (!sealed.exists()) return
    try {
      val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
      val key = keyStore.getKey(ALIAS, null) as PrivateKey? ?: return
      val entries = JSONArray(sealed.readText())
      for (i in 0 until entries.length()) {
        val entry = entries.getJSONObject(i)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(
          Cipher.DECRYPT_MODE,
          key,
          OAEPParameterSpec("SHA-1", "MGF1", MGF1ParameterSpec.SHA1, PSource.PSpecified.DEFAULT),
        )
        val secret = String(cipher.doFinal(Base64.decode(entry.getString("sealed"), Base64.DEFAULT)), Charsets.UTF_8)
        // Named as `assistant/android_secrets.rs` names them.
        store.put(entry.getString("service") + "\u001f" + entry.getString("account"), secret)
      }
    } catch (e: Exception) {
      Logger.error("Secrets from the Mac could not be opened: ${e.javaClass.simpleName}")
    } finally {
      sealed.delete()
    }
  }
}
