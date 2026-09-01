//! NEXUS-015 defect G: the weather.
//!
//! A small connector, but it is the first thing NEXUS does purely for the
//! user rather than for the work, and it is the first outbound request made
//! on their behalf rather than to a service they already use. So it follows
//! the same rules as everything else rather than being waved through:
//!
//! - It is an action, so it passes the permission gate.
//! - It is marked `LeavesMachine`, because it does.
//! - It arrives with **no grant**, so it does nothing until allowed.
//! - It is **not** folded into the greeting. A greeting is answered from
//!   local data and must keep working with the network off; quietly making
//!   it reach the internet would break that guarantee for a nicety.
//!
//! `wttr.in` needs no API key and no account. With no location configured it
//! infers one from the request's IP address, which means the request reveals
//! roughly where the user is. Configuring a city avoids that, and the status
//! action says so plainly rather than leaving it to be discovered.

use rusqlite::Connection;

use super::action::{ActionError, ActionSpec};
use super::connector::{Capabilities, Connector, ConnectorStatus, ExecCtx};
use super::http::{send, HttpError, Request};
use super::permission::{ConfirmPolicy, Permission, Reach};

pub const CONNECTOR_ID: &str = "weather";

/// One line, no key, no account. `%l` location, `%C` condition, `%t`
/// temperature, `%f` what it feels like.
const FORMAT: &str = "%l:+%C+%t+feels+%f";
const BASE: &str = "https://wttr.in";

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "weather.current",
        connector_id: CONNECTOR_ID,
        summary: "Check the weather",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LeavesMachine,
        reversible: true,
    },
    ActionSpec {
        id: "weather.status",
        connector_id: CONNECTOR_ID,
        summary: "Check how the weather is configured",
        permission: Permission::Read,
        confirm: ConfirmPolicy::Never,
        reach: Reach::LocalOnly,
        reversible: true,
    },
];

/// The city the user configured, if any.
fn configured_location(conn: &Connection) -> Option<String> {
    let raw: String = conn
        .query_row(
            "SELECT config_json FROM connectors WHERE connector_id = ?1",
            [CONNECTOR_ID],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()?;
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()?
        .get("location")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 80)
}

/// Percent-encode a place name for a URL path.
///
/// Hand-rolled against the unreserved set rather than adding a crate for a
/// dozen lines, matching how the browser connector encodes a query.
fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub struct WeatherConnector;

impl Connector for WeatherConnector {
    fn id(&self) -> &'static str {
        CONNECTOR_ID
    }

    fn display_name(&self) -> &'static str {
        "Weather"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        ACTIONS
    }

    fn capabilities(&self, _conn: &Connection) -> Capabilities {
        Capabilities {
            available: ACTIONS.iter().map(|s| s.id.to_string()).collect(),
            unavailable: Vec::new(),
        }
    }

    fn status(&self, conn: &Connection) -> ConnectorStatus {
        // Ready either way: without a location it still works, it just infers
        // one from the request. Degraded would overstate the problem.
        if configured_location(conn).is_some() {
            ConnectorStatus::Ready
        } else {
            ConnectorStatus::Degraded
        }
    }

    fn zero_input_actions(&self) -> &'static [&'static str] {
        &["weather.current", "weather.status"]
    }

    fn summarize(&self, action_id: &str, _input: &serde_json::Value, conn: &Connection) -> String {
        match action_id {
            "weather.current" => match configured_location(conn) {
                Some(place) => format!("Check the weather in {place}"),
                None => "Check the weather where you are".to_string(),
            },
            other => ACTIONS
                .iter()
                .find(|s| s.id == other)
                .map(|s| s.summary.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn describe_result(&self, action_id: &str, output: &serde_json::Value) -> Option<String> {
        match action_id {
            "weather.current" => Some(output.get("report")?.as_str()?.to_string()),
            "weather.status" => Some(match output.get("location")?.as_str() {
                Some(place) => format!("Weather is set to {place}."),
                None => "No location set, so NEXUS asks by IP address, which tells the \
                         weather service roughly where you are. Set a city to avoid that."
                    .to_string(),
            }),
            _ => None,
        }
    }

    fn dispatch(
        &self,
        action_id: &str,
        _input: serde_json::Value,
        ctx: &ExecCtx<'_>,
    ) -> Result<serde_json::Value, ActionError> {
        match action_id {
            "weather.status" => Ok(serde_json::json!({
                "location": configured_location(ctx.conn),
                "usesIpWhenUnset": true,
                "service": "wttr.in",
            })),

            "weather.current" => {
                let place = configured_location(ctx.conn);
                let url = match &place {
                    Some(place) => format!("{BASE}/{}?format={FORMAT}", encode(place)),
                    None => format!("{BASE}/?format={FORMAT}"),
                };

                let response = send(Request::get(&url)).map_err(|e| match e {
                    HttpError::Unreachable { .. } => ActionError::Failed {
                        detail: "The weather service could not be reached.".to_string(),
                    },
                    other => ActionError::Failed {
                        detail: other.to_string(),
                    },
                })?;

                if !response.ok() {
                    return Err(ActionError::Failed {
                        detail: match place {
                            Some(place) => {
                                format!("The weather service does not know \"{place}\".")
                            }
                            None => "The weather service did not answer.".to_string(),
                        },
                    });
                }

                let report = response.body.trim().to_string();
                // A one-line format returning a paragraph means the service
                // answered with an error page, not a forecast.
                if report.is_empty() || report.lines().count() > 2 || report.len() > 200 {
                    return Err(ActionError::Failed {
                        detail: "The weather service returned something unexpected.".to_string(),
                    });
                }

                Ok(serde_json::json!({ "report": report, "location": place }))
            }

            other => Err(ActionError::UnknownAction {
                action_id: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::MIGRATIONS;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).expect("migrate");
        }
        crate::assistant::register_connectors(&conn).expect("register");
        conn
    }

    #[test]
    fn a_place_name_is_encoded_for_the_url() {
        assert_eq!(encode("New York"), "New%20York");
        assert_eq!(encode("Coimbatore"), "Coimbatore");
        // The property that matters: nothing survives that could change the
        // request's shape.
        let hostile = encode("x?format=%l&evil=1/../../etc");
        assert!(!hostile.contains('?') && !hostile.contains('&') && !hostile.contains('/'));
    }

    #[test]
    fn no_location_is_configured_by_default() {
        let conn = test_conn();
        assert_eq!(configured_location(&conn), None);
    }

    #[test]
    fn a_configured_location_is_read_back() {
        let conn = test_conn();
        conn.execute(
            "UPDATE connectors SET config_json = '{\"location\":\"Coimbatore\"}'
              WHERE connector_id = 'weather'",
            [],
        )
        .expect("configure");
        assert_eq!(configured_location(&conn).as_deref(), Some("Coimbatore"));
    }

    #[test]
    fn a_nonsense_configuration_is_ignored_rather_than_sent() {
        let conn = test_conn();
        for bad in [
            "{\"location\":\"\"}",
            "{\"location\":\"   \"}",
            "not json",
            "{}",
        ] {
            conn.execute(
                "UPDATE connectors SET config_json = ?1 WHERE connector_id = 'weather'",
                [bad],
            )
            .expect("configure");
            assert_eq!(configured_location(&conn), None, "{bad}");
        }
    }

    #[test]
    fn checking_the_weather_leaves_the_machine_and_says_so() {
        // The honest marking that lets the offline contract and any future
        // privacy rule see this for what it is.
        let current = ACTIONS
            .iter()
            .find(|s| s.id == "weather.current")
            .expect("present");
        assert_eq!(current.reach, Reach::LeavesMachine);
        assert_eq!(current.permission, Permission::Read);

        // Asking how it is configured is purely local.
        let status = ACTIONS
            .iter()
            .find(|s| s.id == "weather.status")
            .expect("present");
        assert_eq!(status.reach, Reach::LocalOnly);
    }

    #[test]
    fn the_status_warns_that_an_unset_location_reveals_one() {
        let described = WeatherConnector
            .describe_result("weather.status", &serde_json::json!({ "location": null }))
            .expect("described");
        assert!(described.contains("IP address"), "{described}");
        assert!(described.contains("Set a city"), "{described}");
    }

    #[test]
    fn a_new_connector_arrives_with_no_grant() {
        // Weather is the first request NEXUS makes purely for the user. It
        // still does nothing until allowed.
        let conn = test_conn();
        let granted =
            crate::assistant::permission::granted_levels(&conn, CONNECTOR_ID).expect("read");
        assert!(granted.is_empty(), "got {granted:?}");
    }

    #[test]
    fn no_api_key_is_involved() {
        let production = include_str!("weather_connector.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("marker");
        for forbidden in ["api_key", "apiKey", "keychain_secret", "Authorization"] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.id.starts_with("weather."), "{}", spec.id);
            assert_eq!(spec.connector_id, CONNECTOR_ID);
        }
    }
}
