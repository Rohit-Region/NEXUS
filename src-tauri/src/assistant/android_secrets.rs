//! The phone's secret store, reached from Rust: the Android counterpart of the
//! Mac's Keychain behind `http::keychain_secret` and its siblings.
//!
//! The store itself is Kotlin (`SecretStore.kt`): an AES key held by the
//! Android Keystore encrypts each value. `MainActivity` hands an instance over
//! through `registerSecretStore` before the app starts, and the calls here go
//! straight to it over JNI on the calling thread. Tauri's plugin route would
//! hop to the UI thread and wait, which deadlocks when the caller is already
//! on it, as synchronous commands are.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};

static STORE: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

const NOT_READY: &str = "The phone's secret store is not ready.";

#[no_mangle]
pub extern "system" fn Java_com_nexus_mobile_MainActivity_registerSecretStore(
    env: JNIEnv,
    _activity: JObject,
    store: JObject,
) {
    if let (Ok(vm), Ok(store)) = (env.get_java_vm(), env.new_global_ref(store)) {
        let _ = STORE.set((vm, store));
    }
}

/// One name per secret, as the Keychain keys by service and account.
fn name(service: &str, account: &str) -> String {
    format!("{service}\u{1f}{account}")
}

fn with_store<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> Result<T, String> {
    with_object(&STORE, NOT_READY, f)
}

/// Run `f` on this thread with an object Kotlin registered in `slot`. A Java
/// exception is cleared and becomes an error, so it never stays pending on
/// the thread.
pub(super) fn with_object<T>(
    slot: &OnceLock<(JavaVM, GlobalRef)>,
    not_ready: &str,
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let (vm, object) = slot.get().ok_or(not_ready)?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let result = f(&mut env, object.as_obj());
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result.map_err(|e| e.to_string())
}

pub fn get(service: &str, account: &str) -> Option<String> {
    with_store(|env, store| {
        let key = env.new_string(name(service, account))?;
        let value = env
            .call_method(store, "get", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&key)])?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let value = JString::from(value);
        let text: String = env.get_string(&value)?.into();
        Ok(Some(text))
    })
    .ok()
    .flatten()
}

pub fn put(service: &str, account: &str, secret: &str) -> Result<(), String> {
    let saved = with_store(|env, store| {
        let key = env.new_string(name(service, account))?;
        let value = env.new_string(secret)?;
        env.call_method(
            store,
            "put",
            "(Ljava/lang/String;Ljava/lang/String;)Z",
            &[JValue::Object(&key), JValue::Object(&value)],
        )?
        .z()
    })
    .map_err(|e| format!("Could not reach the phone's secret store: {e}"))?;
    if saved {
        Ok(())
    } else {
        Err("The phone's secret store refused to save the credential.".to_string())
    }
}

pub fn remove(service: &str, account: &str) -> Result<(), String> {
    with_store(|env, store| {
        let key = env.new_string(name(service, account))?;
        env.call_method(store, "remove", "(Ljava/lang/String;)Z", &[JValue::Object(&key)])?.z()
    })
    .map(|_| ())
}
