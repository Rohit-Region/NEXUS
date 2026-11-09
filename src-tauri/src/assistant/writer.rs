//! Who writes text for NEXUS to show: study plans, resume summaries, cover
//! letters, interview practice and the news radar.
//!
//! On the Mac, its local model (Ollama), so none of it leaves the machine.
//! The phone cannot run one, so there it is the cloud models set up under
//! Settings > Reasoning, in their order (Groq, the free one, last), and only
//! with both switches on: every one of these prompts carries the user's own
//! profile, resume or holdings. One that fails hands over to the next. Each
//! call from the phone is audited like any other consultation.
//!
//! A `Writer` is chosen under the database lock and used after it is
//! released, because writing takes seconds and must not hold the database.

use rusqlite::Connection;

use crate::db::DbState;

pub struct Writer {
    #[cfg(not(target_os = "android"))]
    model: String,
    #[cfg(target_os = "android")]
    cloud: Result<Vec<super::cloud_provider::CloudProvider>, &'static str>,
}

impl Writer {
    #[cfg(not(target_os = "android"))]
    pub fn for_conn(conn: &Connection) -> Writer {
        Writer { model: super::ollama_provider::local_model(conn) }
    }

    #[cfg(target_os = "android")]
    pub fn for_conn(conn: &Connection) -> Writer {
        let policy = super::reasoning::read_policy(conn);
        let cloud = if !policy.external_reasoning_allowed {
            Err("Writing this on the phone uses a cloud model. Turn on \"Let NEXUS ask a model that is not on this phone\" in Settings > Reasoning.")
        } else if !policy.content_sharing_allowed {
            Err("This sends your profile or resume to a cloud model. Turn on \"Include your own content in the question\" in Settings > Reasoning.")
        } else {
            Some(super::cloud_provider::with_keys(conn))
                .filter(|keyed| !keyed.is_empty())
                .ok_or("Add a Claude or Groq API key in Settings > Reasoning to write this on the phone.")
        };
        Writer { cloud }
    }

    /// Which provider writes first, as recorded beside what it wrote.
    pub fn provider(&self) -> &'static str {
        #[cfg(not(target_os = "android"))]
        return "ollama";
        #[cfg(target_os = "android")]
        return self.first().map(|c| c.vendor.id()).unwrap_or("none");
    }

    pub fn model(&self) -> String {
        #[cfg(not(target_os = "android"))]
        return self.model.clone();
        #[cfg(target_os = "android")]
        return self.first().map(|c| c.model.clone()).unwrap_or_default();
    }

    #[cfg(target_os = "android")]
    fn first(&self) -> Option<&super::cloud_provider::CloudProvider> {
        self.cloud.as_ref().ok().and_then(|keyed| keyed.first())
    }

    #[cfg(not(target_os = "android"))]
    pub fn write(&self, _db: &DbState, prompt: &str) -> Result<String, String> {
        super::ollama_provider::generate_text(&self.model, prompt)
    }

    #[cfg(target_os = "android")]
    pub fn write(&self, db: &DbState, prompt: &str) -> Result<String, String> {
        use super::permission::Reach;
        use super::reasoning::{record_use, ContextCategory, Purpose};

        let keyed = self.cloud.as_ref().map_err(|reason| reason.to_string())?;
        let mut failed = Vec::new();
        for cloud in keyed {
            let started = std::time::Instant::now();
            let result = cloud.write(super::ollama_provider::WRITER_PROMPT, prompt);
            if let Ok(conn) = db.0.lock() {
                let _ = record_use(
                    &conn,
                    cloud.vendor.id(),
                    &cloud.model,
                    Reach::LeavesMachine,
                    Purpose::Draft,
                    &[ContextCategory::Request, ContextCategory::ExternalContent],
                    if result.is_ok() { "written" } else { "failed" },
                    started.elapsed().as_millis(),
                );
            }
            match result {
                Ok(text) => return Ok(text),
                Err(why) => failed.push(why),
            }
        }
        Err(failed.join(" "))
    }
}
