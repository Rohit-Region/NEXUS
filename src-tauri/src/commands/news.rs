//! The News section: read the desk, change sources and interests.

use tauri::State;

use super::{put_setting, setting};
use crate::assistant::news_connector::fetch_one;
use crate::assistant::news_desk::{self, check_custom, Desk, NewsSettings};
use crate::db::DbState;

const SETTINGS_KEY: &str = "news_settings";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load(conn: &rusqlite::Connection) -> NewsSettings {
    setting(conn, SETTINGS_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Every source's headlines on one timeline. Cached for ten minutes unless
/// `refresh`. The lock is released before any feed is fetched.
#[tauri::command(async)]
pub fn nexus_news_desk(state: State<'_, DbState>, refresh: Option<bool>) -> Result<Desk, String> {
    let settings = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        load(&conn)
    };
    Ok(news_desk::desk(&settings, refresh.unwrap_or(false), now_unix()))
}

#[tauri::command(async)]
pub fn nexus_news_settings(state: State<'_, DbState>) -> Result<NewsSettings, String> {
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    Ok(load(&conn))
}

/// Save sources and interests. A newly added feed is fetched once first, so
/// a wrong address is caught here rather than as a silent gap later.
#[tauri::command(async)]
pub fn nexus_set_news_settings(state: State<'_, DbState>, settings: NewsSettings) -> Result<NewsSettings, String> {
    let settings = settings.sanitised();
    let previous = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        load(&conn)
    };
    for feed in &settings.custom {
        check_custom(feed)?;
        if !previous.custom.iter().any(|p| p.url == feed.url) {
            let items = fetch_one(&feed.label, feed.url.trim())
                .map_err(|e| format!("Could not read {}: {e}", feed.label))?;
            if items.is_empty() {
                return Err(format!("{} has no headlines NEXUS can read. Is it an RSS or Atom feed?", feed.label));
            }
        }
    }
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    put_setting(&conn, SETTINGS_KEY, &serde_json::to_string(&settings).map_err(|e| e.to_string())?)?;
    Ok(settings)
}
