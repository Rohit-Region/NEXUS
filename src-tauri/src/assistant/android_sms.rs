//! Bank texts from the phone's own inbox: the phone's source for Spending, in
//! place of the Mac's Google Sheet (`bank_sms.rs`), which needs a link and a
//! key. The reading is Kotlin (`BankInbox.kt`), handed over by `MainActivity`
//! through `registerBankInbox` and called here over JNI, as the secret store is.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use serde::Deserialize;

use super::android_secrets::with_object;
use super::bank_sms::Row;

static INBOX: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

const NOT_READY: &str = "The phone's inbox is not ready.";
/// Texts looked at per call, bank or not; a first read pages through the
/// whole inbox this many at a time.
const PAGE: i32 = 500;

#[no_mangle]
pub extern "system" fn Java_com_nexus_mobile_MainActivity_registerBankInbox(
    env: JNIEnv,
    _activity: JObject,
    inbox: JObject,
) {
    if let (Ok(vm), Ok(inbox)) = (env.get_java_vm(), env.new_global_ref(inbox)) {
        let _ = INBOX.set((vm, inbox));
    }
}

/// Whether NEXUS may read the phone's texts.
pub fn allowed() -> bool {
    with_object(&INBOX, NOT_READY, |env, inbox| env.call_method(inbox, "allowed", "()Z", &[])?.z()).unwrap_or(false)
}

/// Show Android's prompt (or NEXUS's settings page, once the prompt is
/// refused for good). Returns before the user answers.
pub fn ask() -> Result<(), String> {
    with_object(&INBOX, NOT_READY, |env, inbox| env.call_method(inbox, "ask", "()V", &[]).map(|_| ()))
}

#[derive(Deserialize)]
struct Page {
    last: i64,
    rows: Vec<Row>,
}

/// Bank texts after inbox row `after`, and the new cursor.
pub fn after(after: i64) -> Result<(i64, Vec<Row>), String> {
    let raw = with_object(&INBOX, NOT_READY, |env, inbox| {
        let json = env
            .call_method(inbox, "after", "(JI)Ljava/lang/String;", &[JValue::Long(after), JValue::Int(PAGE)])?
            .l()?;
        let json = JString::from(json);
        let text: String = env.get_string(&json)?.into();
        Ok(text)
    })
    .map_err(|e| format!("Could not read the phone's texts: {e}"))?;
    let page: Page = serde_json::from_str(&raw).map_err(|e| format!("Could not read the phone's texts: {e}"))?;
    Ok((page.last, page.rows))
}
