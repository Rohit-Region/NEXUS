mod assistant;
mod commands;
mod db;
mod voice;

use assistant::approval::ApprovalStore;
use assistant::session::AssistantSession;
use db::DbState;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager};

/// How often the watcher looks for something worth saying.
///
/// Short enough that "you got a message" is still news, long enough that a
/// read of a SQLite file another process owns costs nothing. The freshness
/// window in the notification connector is what actually stops stale
/// messages being announced, so this does not need to be tight.
const NOTIFICATION_POLL: Duration = Duration::from_secs(8);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Initialize logging in debug builds.
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Initialize the SQLite database and register it as managed state.
            let conn = db::init(app.handle()).expect("Failed to initialize NEXUS database");
            commands::load_calendar_switch(&conn);
            app.manage(DbState(Mutex::new(conn)));

            // NEXUS-012: pending approvals live here and nowhere else.
            // Deliberately not persisted, so a restart cannot leave a queue
            // of pre-approved actions waiting to fire.
            app.manage(ApprovalStore::default());

            // NEXUS-013: assistant state and the conversation. In memory and
            // bounded; a conversation that survived a restart would be a
            // transcript on disk, which the voice milestones deliberately
            // avoided creating.
            app.manage(AssistantSession::default());

            // NEXUS-024 F-05 and NEXUS-025: the watcher.
            //
            // **In Rust rather than a timer in the frontend, and that is the
            // point of the milestone.** NEXUS speaking first cannot depend on
            // a component being mounted, a panel being open, or a view being
            // the current one. The window is a surface NEXUS talks *through*,
            // not the thing that decides whether it talks.
            //
            // A plain thread rather than an async task: it sleeps almost all
            // of the time, takes the database lock briefly, and has nothing
            // to await. The handle is dropped deliberately; the thread ends
            // with the process.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(NOTIFICATION_POLL);

                    let Some(db) = handle.try_state::<DbState>() else {
                        continue;
                    };
                    let Some(session) = handle.try_state::<AssistantSession>() else {
                        continue;
                    };
                    let Some(approvals) = handle.try_state::<ApprovalStore>() else {
                        continue;
                    };

                    // The lock is taken and released inside the tick. Holding
                    // it across the sleep would block every command in the
                    // application for the whole interval.
                    let found = {
                        let Ok(conn) = db.0.lock() else { continue };
                        commands::poll_notifications(&conn, &session, &approvals)
                    };

                    if let Ok(poll) = found {
                        if poll.announcement.is_some() {
                            let _ = handle.emit(commands::EVENT_NOTIFICATION, poll);
                        }
                    }
                });
            }

            // Phone approvals: reads the private reply topic for Approve /
            // Deny taps, but only while a request is actually waiting, so an
            // idle NEXUS makes no requests at all. The cursor starts at the
            // moment NEXUS started, so taps from before a restart are never
            // acted on.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut cursor = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_else(|_| "1m".to_string());
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        if !assistant::phone_approval::any_waiting() {
                            continue;
                        }
                        let Some(db) = handle.try_state::<DbState>() else {
                            continue;
                        };
                        commands::phone_poll_once(&handle, &db, &mut cursor);
                    }
                });
            }

            // Wellbeing nudges. A thread of its own, like the watcher above,
            // so breaks are counted whichever screen is open. It wakes once a
            // minute, takes the database lock only to read settings and the
            // local clock, and posts a macOS notification when one is due.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    let Some(db) = handle.try_state::<DbState>() else {
                        continue;
                    };
                    for nudge in commands::wellbeing_tick(&db) {
                        let _ = commands::nexus_desktop_notification(
                            nudge.title().to_string(),
                            nudge.body(),
                        );
                    }
                });
            }

            // Phone call alerts: ring the phone for a meeting about to
            // start, a message from a chosen person, or a reminder due now.
            // Does nothing until switched on in Me.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(commands::phone_calls::TICK);
                    if let Some(db) = handle.try_state::<DbState>() {
                        commands::phone_calls::tick(&db);
                    }
                });
            }

            // Spending: pull new bank SMS from the user's Sheet every five
            // minutes, so the Spending tab is current without a press of
            // Sync. Does nothing until the Sheet is connected, and a failure
            // is recorded for the tab to show rather than retried in a loop.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(20));
                    loop {
                        if let Some(db) = handle.try_state::<DbState>() {
                            let _ = commands::spending::sync_once(&db);
                        }
                        std::thread::sleep(std::time::Duration::from_secs(300));
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::nexus_get_db_status,
            commands::nexus_get_db_counts,
            commands::nexus_create_project,
            commands::nexus_list_projects,
            commands::nexus_update_project,
            commands::nexus_create_task,
            commands::nexus_list_tasks,
            commands::nexus_update_task,
            commands::nexus_update_task_status,
            commands::nexus_create_ide,
            commands::nexus_list_ides,
            commands::nexus_update_ide,
            commands::nexus_delete_ide,
            commands::nexus_create_agent,
            commands::nexus_list_agents,
            commands::nexus_update_agent,
            commands::nexus_delete_agent,
            commands::nexus_assign_task_agent,
            commands::nexus_get_workspace_summary,
            commands::nexus_count_tasks_by_project,
            commands::nexus_count_tasks_by_agent,
            commands::nexus_list_recent_tasks,
            commands::nexus_get_settings,
            commands::nexus_update_settings,
            commands::nexus_reset_settings,
            commands::nexus_search_workspace,
            commands::nexus_voice_status,
            commands::nexus_voice_request_authorization,
            commands::nexus_voice_start,
            commands::nexus_voice_stop,
            commands::nexus_voice_sync_always_listening,
            commands::nexus_voice_wake,
            commands::nexus_resolve_voice_intent,
            commands::nexus_voice_speak,
            commands::nexus_voice_stop_speaking,
            commands::nexus_voice_say,
            commands::nexus_voice_list_voices,
            commands::nexus_execute_action,
            commands::nexus_cancel_approval,
            commands::nexus_list_connectors,
            commands::nexus_set_permission_grant,
            commands::nexus_set_connector_enabled,
            commands::nexus_set_connector_config,
            commands::nexus_store_connector_secret,
            commands::nexus_connector_secret_present,
            commands::nexus_vault_set,
            commands::nexus_vault_status,
            commands::nexus_list_contacts,
            commands::nexus_save_contact,
            commands::nexus_delete_contact,
            commands::nexus_list_audit,
            commands::nexus_reasoning_status,
            commands::nexus_set_reasoning_policy,
            commands::nexus_set_reasoning_model,
            commands::nexus_local_models,
            commands::nexus_list_ai_audit,
            commands::nexus_list_suggestions,
            commands::nexus_dismiss_suggestion,
            commands::nexus_restore_suggestion,
            commands::nexus_surface_suggestions,
            commands::nexus_preview_suggestions,
            commands::nexus_briefing,
            commands::nexus_notifications_poll,
            commands::nexus_notification_policy,
            commands::nexus_set_notification_policy,
            commands::nexus_list_commitments,
            commands::nexus_delete_commitment,
            commands::nexus_create_commitment,
            commands::nexus_defer_commitment,
            commands::nexus_accept_suggestion,
            commands::nexus_proactive_policy,
            commands::nexus_set_proactive_policy,
            commands::nexus_assistant_snapshot,
            commands::nexus_assistant_context,
            commands::nexus_assistant_resolve,
            commands::nexus_assistant_remember_list,
            commands::nexus_assistant_settle,
            commands::nexus_assistant_clear,
            commands::nexus_assistant_ask,
            commands::nexus_set_user_name,
            commands::nexus_user_name,
            commands::nexus_check_stock,
            commands::nexus_request_calendar_access,
            commands::nexus_calendar_enabled,
            commands::news::nexus_news_desk,
            commands::news::nexus_news_settings,
            commands::news::nexus_set_news_settings,
            commands::stocks::nexus_stock_board,
            commands::stocks::nexus_stock_picks,
            commands::stocks::nexus_fund_picks,
            commands::stocks::nexus_pick_track_record,
            commands::portfolio_chat::nexus_portfolio_chat,
            commands::portfolio_chat::nexus_portfolio_chat_apply,
            commands::phone_calls::nexus_call_settings,
            commands::phone_calls::nexus_set_call_settings,
            commands::phone_calls::nexus_call_test,
            commands::spending::nexus_spend_status,
            commands::spending::nexus_spend_lock_state,
            commands::spending::nexus_spend_unlock,
            commands::spending::nexus_spend_lock,
            commands::spending::nexus_spend_connect,
            commands::spending::nexus_spend_disconnect,
            commands::spending::nexus_spend_sync,
            commands::spending::nexus_spend_paste,
            commands::spending::nexus_spend_month,
            commands::spending::nexus_spend_add,
            commands::spending::nexus_spend_update,
            commands::spending::nexus_spend_delete,
            commands::spending::nexus_spend_set_budget,
            commands::spending::nexus_spend_set_income,
            commands::spending::nexus_spend_set_balance,
            commands::spending::nexus_spend_review,
            commands::spending::nexus_spend_accept,
            commands::spending::nexus_spend_ignore,
            commands::nexus_set_calendar_enabled,
            commands::nexus_open_url,
            commands::nexus_desktop_notification,
            commands::nexus_set_whatsapp_alert_number,
            commands::nexus_whatsapp_alert_number,
            commands::nexus_whatsapp_send_alert,
            commands::nexus_set_ntfy_topic,
            commands::nexus_ntfy_topic,
            commands::nexus_ntfy_send_alert,
            commands::nexus_set_keep_awake,
            commands::nexus_fetch_news,
            commands::nexus_news_briefing,
            commands::nexus_get_profile,
            commands::nexus_set_profile_field,
            commands::nexus_add_holding,
            commands::nexus_remove_holding,
            commands::nexus_portfolio,
            commands::nexus_search_funds,
            commands::nexus_list_goals,
            commands::nexus_add_goal,
            commands::nexus_update_goal_amount,
            commands::nexus_remove_goal,
            commands::nexus_retirement_projection,
            commands::nexus_list_bills,
            commands::nexus_add_bill,
            commands::nexus_remove_bill,
            commands::nexus_tax_dates,
            commands::nexus_list_skills,
            commands::nexus_add_skill,
            commands::nexus_remove_skill,
            commands::nexus_skill_topics,
            commands::nexus_generate_plan,
            commands::nexus_set_topic_done,
            commands::nexus_explain_topic,
            commands::nexus_generate_cards,
            commands::nexus_due_cards,
            commands::nexus_review_card,
            commands::nexus_learn_stats,
            commands::nexus_discover_jobs,
            commands::nexus_list_applications,
            commands::nexus_add_application,
            commands::nexus_update_application,
            commands::nexus_remove_application,
            commands::nexus_get_document,
            commands::nexus_set_document,
            commands::nexus_set_application_description,
            commands::nexus_tailor_resume,
            commands::nexus_cover_letter,
            commands::nexus_export_application_doc,
            commands::nexus_today,
            commands::nexus_compose_standup,
            commands::nexus_clone_projects,
            commands::nexus_set_clone_allowed,
            commands::nexus_clone_runs,
            commands::nexus_clone_start,
            commands::nexus_clone_apply,
            commands::nexus_clone_discard,
            commands::nexus_phone_approvals,
            commands::nexus_set_phone_approvals,
            commands::nexus_phone_approval_test,
            commands::nexus_wellbeing_settings,
            commands::nexus_set_wellbeing_settings,
            commands::nexus_wellbeing_status,
            commands::nexus_role_brief,
            commands::nexus_interview_questions,
            commands::nexus_generate_interview_questions,
            commands::nexus_interview_feedback,
            commands::nexus_delete_interview_question,
            commands::nexus_assistant_cancel_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
