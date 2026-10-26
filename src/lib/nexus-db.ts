/**
 * Typed wrappers around Tauri invoke() for all NEXUS DB commands.
 * Components import from this module — never from @tauri-apps/api directly.
 * Raw SQL never passes through this boundary.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AgentTaskCounts,
  AssignTaskAgentInput,
  CreateProjectInput,
  CreateRegistryEntryInput,
  CreateTaskInput,
  DbCounts,
  DbStatus,
  Project,
  ProjectTaskCounts,
  RegistryEntry,
  SearchResults,
  Settings,
  VoiceState,
  VoiceCommandSpec,
  VoiceIntent,
  VoiceOption,
  VoiceOutcome,
  Contact,
  ContactInput,
  VoiceSpeech,
  WakeOutcome,
  VoiceStatus,
  VoiceTranscript,
  Task,
  TaskWithProject,
  UpdateProjectInput,
  UpdateRegistryEntryInput,
  UpdateTaskInput,
  UpdateTaskStatusInput,
  WorkspaceSummary,
} from '../types/db';

export function getDbStatus(): Promise<DbStatus> {
  return invoke<DbStatus>('nexus_get_db_status');
}

export function getDbCounts(): Promise<DbCounts> {
  return invoke<DbCounts>('nexus_get_db_counts');
}

export function createProject(input: CreateProjectInput): Promise<Project> {
  return invoke<Project>('nexus_create_project', { input });
}

export function listProjects(): Promise<Project[]> {
  return invoke<Project[]>('nexus_list_projects');
}

export function updateProject(input: UpdateProjectInput): Promise<Project> {
  return invoke<Project>('nexus_update_project', { input });
}

export function createTask(input: CreateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_create_task', { input });
}

export function listTasks(projectId: number): Promise<Task[]> {
  return invoke<Task[]>('nexus_list_tasks', { projectId });
}

export function updateTask(input: UpdateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_update_task', { input });
}

export function updateTaskStatus(input: UpdateTaskStatusInput): Promise<Task> {
  return invoke<Task>('nexus_update_task_status', { input });
}

export function assignTaskAgent(input: AssignTaskAgentInput): Promise<Task> {
  return invoke<Task>('nexus_assign_task_agent', { input });
}

// ── Registry: IDEs ──────────────────────────────────────────────────────────

export function createIde(input: CreateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_create_ide', { input });
}

export function listIdes(enabledOnly: boolean): Promise<RegistryEntry[]> {
  return invoke<RegistryEntry[]>('nexus_list_ides', { enabledOnly });
}

export function updateIde(input: UpdateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_update_ide', { input });
}

export function deleteIde(id: number): Promise<void> {
  return invoke<void>('nexus_delete_ide', { id });
}

// ── Registry: AI agents ─────────────────────────────────────────────────────

export function createAgent(input: CreateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_create_agent', { input });
}

export function listAgents(enabledOnly: boolean): Promise<RegistryEntry[]> {
  return invoke<RegistryEntry[]>('nexus_list_agents', { enabledOnly });
}

export function updateAgent(input: UpdateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_update_agent', { input });
}

export function deleteAgent(id: number): Promise<void> {
  return invoke<void>('nexus_delete_agent', { id });
}

// ── Aggregates ──────────────────────────────────────────────────────────────

export function getWorkspaceSummary(): Promise<WorkspaceSummary> {
  return invoke<WorkspaceSummary>('nexus_get_workspace_summary');
}

export function countTasksByProject(): Promise<ProjectTaskCounts[]> {
  return invoke<ProjectTaskCounts[]>('nexus_count_tasks_by_project');
}

export function countTasksByAgent(): Promise<AgentTaskCounts[]> {
  return invoke<AgentTaskCounts[]>('nexus_count_tasks_by_agent');
}

export function listRecentTasks(limit: number): Promise<TaskWithProject[]> {
  return invoke<TaskWithProject[]>('nexus_list_recent_tasks', { limit });
}

// ── Settings ────────────────────────────────────────────────────────────────

export function getSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_get_settings');
}

export function updateSettings(input: Settings): Promise<Settings> {
  return invoke<Settings>('nexus_update_settings', { input });
}

export function resetSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_reset_settings');
}

// ── Search ──────────────────────────────────────────────────────────────────

export function searchWorkspace(query: string): Promise<SearchResults> {
  return invoke<SearchResults>('nexus_search_workspace', { query });
}

// Deletion is deliberately absent. Since NEXUS-012 it is an action, not a
// database call: it goes through the permission gate in src/lib/assistant.ts
// so it is confirmed and audited like anything else NEXUS does on your
// behalf. See nexus.delete_project and nexus.delete_task.

// ── Voice (NEXUS-010) ───────────────────────────────────────────────────────
//
// These only control the recognizer. Transcripts arrive as Tauri events, are
// matched by the NEXUS-009 registry, and require confirmation before anything
// runs. Nothing here executes a command.

export function voiceStatus(): Promise<VoiceStatus> {
  return invoke<VoiceStatus>('nexus_voice_status');
}

export function voiceRequestAuthorization(): Promise<void> {
  return invoke<void>('nexus_voice_request_authorization');
}

export function voiceStart(): Promise<void> {
  return invoke<void>('nexus_voice_start');
}

export function voiceStop(): Promise<void> {
  return invoke<void>('nexus_voice_stop');
}

// ── Contacts ────────────────────────────────────────────────────────────────
//
// The only people NEXUS can message. Every call returns the full list, so the
// caller never has to reconcile a local copy with what was stored.

export function listContacts(): Promise<Contact[]> {
  return invoke<Contact[]>('nexus_list_contacts');
}

/** Create when `id` is null, update otherwise. Returns the refreshed list. */
export function saveContact(contact: ContactInput): Promise<Contact[]> {
  return invoke<Contact[]>('nexus_save_contact', { contact });
}

export function deleteContact(id: number): Promise<Contact[]> {
  return invoke<Contact[]>('nexus_delete_contact', { id });
}

// ── Always-listening and the wake word ──────────────────────────────────────

/**
 * Bring always-listening into line with the saved preferences.
 *
 * The runtime flag is derived in Rust from voiceEnabled and alwaysListening
 * together, so the UI cannot leave the microphone held open by toggling the
 * two in the wrong order. Returns whether the microphone is now being kept
 * open. Safe to call repeatedly; a no-op when nothing changed.
 */
export function voiceSyncAlwaysListening(): Promise<boolean> {
  return invoke<boolean>('nexus_voice_sync_always_listening');
}

/**
 * Was this utterance addressed to NEXUS?
 *
 * With the microphone permanently open, most of what it hears is not meant
 * for NEXUS. `woke: false` means discard the transcript entirely: do not
 * resolve it, do not run it, do not show it.
 */
export function voiceWake(transcript: string): Promise<WakeOutcome> {
  return invoke<WakeOutcome>('nexus_voice_wake', { transcript });
}

// ── Spoken responses (NEXUS-011) ────────────────────────────────────────────
//
// The caller reports what happened; Rust picks the words from a deterministic
// template keyed on the executed command id. No transcript crosses this
// boundary, so nothing NEXUS heard can end up being spoken back.

/**
 * Speak the response for an outcome.
 *
 * Returns without speaking when voice is disabled in Settings or the
 * microphone is open. Both are ordinary results, reported as `spoken: false`
 * rather than as errors.
 */
export function voiceSpeak(outcome: VoiceOutcome): Promise<VoiceSpeech> {
  return invoke<VoiceSpeech>('nexus_voice_speak', { outcome });
}

/**
 * Speak a response NEXUS composed.
 *
 * Distinct from voiceSpeak, which speaks a fixed template keyed by an
 * executed action. This carries NEXUS's own words: an answer built from local
 * data, or a connector's description of what it found. Silent when voice is
 * off, and never speaks while the microphone is open.
 */
export function voiceSay(text: string): Promise<VoiceSpeech> {
  return invoke<VoiceSpeech>('nexus_voice_say', { text });
}

/** The result of a stock check. `available` is null when the page could not
 *  be read (bot wall, redirect), which is not the same as out of stock. */
export interface StockState {
  available: boolean | null;
  title: string | null;
}

/** Check whether a product URL is in stock. */
export function checkStock(url: string): Promise<StockState> {
  return invoke<StockState>('nexus_check_stock', { url });
}

/** Raise a macOS notification banner. */
export function desktopNotification(title: string, body: string): Promise<void> {
  return invoke<void>('nexus_desktop_notification', { title, body });
}

/** Ask macOS for calendar access (raises the system prompt). Returns the
 *  access state afterwards. */
export function requestCalendarAccess(): Promise<string> {
  return invoke<string>('nexus_request_calendar_access');
}

/** Whether NEXUS may use the calendar at all. Off: no reads, no macOS prompt. */
export function calendarEnabled(): Promise<boolean> {
  return invoke<boolean>('nexus_calendar_enabled');
}

export function setCalendarEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>('nexus_set_calendar_enabled', { enabled });
}

/** Open a web address in the default browser. */
export function openUrl(url: string): Promise<void> {
  return invoke<void>('nexus_open_url', { url });
}

/** Save the one number NEXUS may auto-send alerts to. Empty clears it. Returns
 *  the normalised number, or null when cleared. */
export function setWhatsAppAlertNumber(phone: string): Promise<string | null> {
  return invoke<string | null>('nexus_set_whatsapp_alert_number', { phone });
}

/** The saved alert number, if any. */
export function whatsAppAlertNumber(): Promise<string | null> {
  return invoke<string | null>('nexus_whatsapp_alert_number');
}

/**
 * Send an alert to the saved number, automatically, no confirmation.
 *
 * There is no recipient parameter on purpose: it sends only to the number
 * saved via setWhatsAppAlertNumber, so it cannot be aimed anywhere else.
 * Needs macOS Accessibility permission for the keystroke, and WhatsApp must be
 * able to come to the front.
 */
export function whatsAppSendAlert(message: string): Promise<void> {
  return invoke<void>('nexus_whatsapp_send_alert', { message });
}

/** Save the one ntfy topic NEXUS publishes alerts to. Empty clears it. Returns
 *  the normalised topic, or null when cleared. */
export function setNtfyTopic(topic: string): Promise<string | null> {
  return invoke<string | null>('nexus_set_ntfy_topic', { topic });
}

/** The saved ntfy topic, if any. */
export function ntfyTopic(): Promise<string | null> {
  return invoke<string | null>('nexus_ntfy_topic');
}

/**
 * Publish a loud alert to the saved ntfy topic.
 *
 * No topic parameter on purpose: it publishes only to the topic saved via
 * setNtfyTopic. Fires a high-priority notification on any device subscribed to
 * that topic in the ntfy app. Unlike WhatsApp, it needs no local permission and
 * no app focus; it is just an HTTPS POST.
 */
export function ntfySendAlert(title: string, message: string): Promise<void> {
  return invoke<void>('nexus_ntfy_send_alert', { title, message });
}

/**
 * Hold the Mac awake while watches are running, or let it sleep again.
 *
 * Starts or stops a `caffeinate` process. Idempotent, and it self-cleans if
 * NEXUS quits. It keeps checks running with the lid up; it cannot override a
 * closed lid.
 */
export function setKeepAwake(enabled: boolean): Promise<boolean> {
  return invoke<boolean>('nexus_set_keep_awake', { enabled });
}

/** The personal profile NEXUS reads to personalise every section. Keys are
 *  the known field names ('goals', 'role_target', ...); values are free text. */
export function getProfile(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>('nexus_get_profile');
}

/** Set one profile field, or clear it with a blank value. Returns the stored
 *  value (empty when cleared). */
export function setProfileField(key: string, value: string): Promise<string> {
  return invoke<string>('nexus_set_profile_field', { key, value });
}

export type HoldingKind = 'stock' | 'mf' | 'manual' | 'ulip';

export interface HoldingView {
  id: number;
  symbol: string;
  label: string;
  quantity: number;
  kind: HoldingKind;
  price: number | null;
  prevClose: number | null;
  currency: string;
  value: number | null;
  dayChange: number | null;
  error: string | null;
  /** Where it is held: "ICICI Direct", "Zerodha", a ULIP policy; "" if unset. */
  broker: string;
  /** What was paid for it in total, when known. */
  invested: number | null;
  /** For a NAV you entered (a ULIP fund), the date it is from. */
  priceAsOf: string | null;
}

export interface PortfolioGroup {
  name: string;
  value: number;
  invested: number | null;
  gain: number | null;
}

export interface Portfolio {
  holdings: HoldingView[];
  totalValue: number;
  totalDayChange: number;
  currency: string;
  groups: PortfolioGroup[];
}

export interface FundMatch {
  code: string;
  name: string;
}

/**
 * Add a holding.
 * - `stock`: a Yahoo symbol (NSE stock `RELIANCE.NS`, gold ETF `GOLDBEES.NS`),
 *   quantity = units.
 * - `mf`: an AMFI scheme code, quantity = units.
 * - `manual`: symbol ignored, quantity = the rupee value (ULIP, cash). Needs a label.
 */
export function addHolding(
  symbol: string,
  label: string,
  quantity: number,
  kind: HoldingKind = 'stock',
): Promise<number> {
  return invoke<number>('nexus_add_holding', { symbol, label, quantity, kind });
}

/** Search Indian mutual funds by name (needs 3+ letters). */
export function searchFunds(query: string): Promise<FundMatch[]> {
  return invoke<FundMatch[]>('nexus_search_funds', { query });
}

// -- Money: goals -------------------------------------------------------------

export interface GoalProgress {
  id: number;
  name: string;
  targetAmount: number;
  currentAmount: number;
  targetDate: string;
  expectedReturn: number;
  /** 0..100, how much is saved. */
  percent: number;
  yearsLeft: number;
  /** Monthly saving needed to hit the target by the date, or null if the date
   *  is unreadable. */
  requiredMonthly: number | null;
}

export function listGoals(): Promise<GoalProgress[]> {
  return invoke<GoalProgress[]>('nexus_list_goals');
}

export function addGoal(
  name: string,
  targetAmount: number,
  targetDate: string,
  expectedReturn: number,
): Promise<number> {
  return invoke<number>('nexus_add_goal', { name, targetAmount, targetDate, expectedReturn });
}

export function updateGoalAmount(id: number, currentAmount: number): Promise<void> {
  return invoke<void>('nexus_update_goal_amount', { id, currentAmount });
}

export function removeGoal(id: number): Promise<void> {
  return invoke<void>('nexus_remove_goal', { id });
}

// -- Money: retirement --------------------------------------------------------

export interface RetirementInput {
  currentAge: number;
  retirementAge: number;
  currentCorpus: number;
  monthlySip: number;
  expectedReturn: number;
  inflation: number;
  monthlyExpenseToday: number;
}

export interface RetirementProjection {
  years: number;
  projectedCorpus: number;
  targetCorpus: number;
  onTrack: boolean;
  shortfall: number;
  requiredMonthly: number;
  extraMonthly: number;
}

/** Project a retirement outcome. Pure math over the assumptions you give;
 *  nothing is stored. A projection, not a prediction. */
export function retirementProjection(input: RetirementInput): Promise<RetirementProjection> {
  return invoke<RetirementProjection>('nexus_retirement_projection', { input });
}

// -- Money: bills & tax dates -------------------------------------------------

export interface BillView {
  id: number;
  name: string;
  kind: string;
  amount: number | null;
  recurrence: string;
  nextDue: string;
  daysUntil: number;
}

export interface TaxDate {
  name: string;
  date: string;
  daysUntil: number;
}

export function listBills(): Promise<BillView[]> {
  return invoke<BillView[]>('nexus_list_bills');
}

export function addBill(
  name: string,
  kind: string,
  amount: number | null,
  recurrence: string,
  anchorDate: string,
): Promise<number> {
  return invoke<number>('nexus_add_bill', { name, kind, amount, recurrence, anchorDate });
}

export function removeBill(id: number): Promise<void> {
  return invoke<void>('nexus_remove_bill', { id });
}

/** Upcoming India statutory tax dates (advance tax, ITR). Computed, not stored. */
export function taxDates(): Promise<TaxDate[]> {
  return invoke<TaxDate[]>('nexus_tax_dates');
}

// -- Learn --------------------------------------------------------------------

export interface SkillSummary {
  id: number;
  name: string;
  topicsTotal: number;
  topicsDone: number;
  cardsTotal: number;
  cardsDue: number;
}

export interface TopicView {
  id: number;
  title: string;
  done: boolean;
  cards: number;
}

export interface CardView {
  id: number;
  skillId: number;
  skillName: string;
  front: string;
  back: string;
}

export interface LearnStats {
  streak: number;
  dueToday: number;
  reviewedToday: number;
}

export type Grade = 'again' | 'good' | 'easy';

export function listSkills(): Promise<SkillSummary[]> {
  return invoke<SkillSummary[]>('nexus_list_skills');
}

export function addSkill(name: string): Promise<number> {
  return invoke<number>('nexus_add_skill', { name });
}

export function removeSkill(id: number): Promise<void> {
  return invoke<void>('nexus_remove_skill', { id });
}

export function skillTopics(skillId: number): Promise<TopicView[]> {
  return invoke<TopicView[]>('nexus_skill_topics', { skillId });
}

/** Draft (or redraft) a skill's plan with the local model. Takes a few seconds. */
export function generatePlan(skillId: number): Promise<TopicView[]> {
  return invoke<TopicView[]>('nexus_generate_plan', { skillId });
}

export function setTopicDone(id: number, done: boolean): Promise<void> {
  return invoke<void>('nexus_set_topic_done', { id, done });
}

/** A short explanation of a topic from the local model, written for you. */
export function explainTopic(topicId: number): Promise<string> {
  return invoke<string>('nexus_explain_topic', { topicId });
}

/** Generate flashcards for a topic; returns how many were added. */
export function generateCards(topicId: number): Promise<number> {
  return invoke<number>('nexus_generate_cards', { topicId });
}

export function dueCards(skillId?: number, limit?: number): Promise<CardView[]> {
  return invoke<CardView[]>('nexus_due_cards', { skillId: skillId ?? null, limit: limit ?? null });
}

export function reviewCard(id: number, grade: Grade): Promise<void> {
  return invoke<void>('nexus_review_card', { id, grade });
}

export function learnStats(): Promise<LearnStats> {
  return invoke<LearnStats>('nexus_learn_stats');
}

// -- Career -------------------------------------------------------------------

export interface ScoredJob {
  title: string;
  company: string;
  url: string;
  location: string;
  posted: string | null;
  tags: string[];
  source: string;
  /** 0..100. Ranks the list; not a verdict on fit. */
  score: number;
  matched: string[];
  gaps: string[];
  /** For someone based in India, read from the listing's location text. */
  eligibility: 'open' | 'restricted' | 'unclear';
  /** Plain-text description, snapshotted into the tracker for tailoring. */
  description: string;
}

export type ApplicationStatus = 'saved' | 'applied' | 'interviewing' | 'offer' | 'rejected';

export interface Application {
  id: number;
  company: string;
  role: string;
  url: string;
  source: string;
  status: ApplicationStatus;
  nextAction: string;
  nextActionDate: string | null;
  notes: string;
  updatedAt: string;
  /** The job description tailoring works against. */
  description: string;
  tailoredResume: string;
  coverLetter: string;
}

/** Search Remotive + We Work Remotely and rank against your Me profile. */
export function discoverJobs(query: string): Promise<ScoredJob[]> {
  return invoke<ScoredJob[]>('nexus_discover_jobs', { query });
}

export function listApplications(): Promise<Application[]> {
  return invoke<Application[]>('nexus_list_applications');
}

/** Track a role. Re-tracking the same link returns the existing id. */
export function addApplication(
  company: string,
  role: string,
  url: string,
  source: string,
  status: ApplicationStatus = 'saved',
  description = '',
): Promise<number> {
  return invoke<number>('nexus_add_application', { company, role, url, source, status, description });
}

export function updateApplication(
  id: number,
  status: ApplicationStatus,
  nextAction: string,
  nextActionDate: string | null,
  notes: string,
): Promise<void> {
  return invoke<void>('nexus_update_application', { id, status, nextAction, nextActionDate, notes });
}

export function removeApplication(id: number): Promise<void> {
  return invoke<void>('nexus_remove_application', { id });
}

/** A stored long-form document ('resume'), or '' if not set. */
export function getDocument(key: string): Promise<string> {
  return invoke<string>('nexus_get_document', { key });
}

export function setDocument(key: string, content: string): Promise<void> {
  return invoke<void>('nexus_set_document', { key, content });
}

export function setApplicationDescription(id: number, description: string): Promise<void> {
  return invoke<void>('nexus_set_application_description', { id, description });
}

export interface TailoredResume {
  /** Your resume reordered for the role. Every line is yours. */
  reordered: string;
  /** Short model-written summary, if the model produced one. */
  summary: string | null;
  /** Summary claims your resume doesn't back up. Check before sending. */
  warnings: string[];
  moved: string[];
  matched: string[];
  note: string | null;
}

export interface CoverLetter {
  text: string;
  /** Claims your resume doesn't back up. Check before sending. */
  warnings: string[];
}

/** Reorder your base resume for an application, plus a fact-checked summary. */
export function tailorResume(id: number): Promise<TailoredResume> {
  return invoke<TailoredResume>('nexus_tailor_resume', { id });
}

/** Draft a cover letter for an application, fact-checked against your resume. */
export function coverLetter(id: number): Promise<CoverLetter> {
  return invoke<CoverLetter>('nexus_cover_letter', { id });
}

// -- Career: interview prep -----------------------------------------------------

export interface RoleBrief {
  /** The job's requirements your resume covers. */
  have: string[];
  /** The job's requirements your resume doesn't mention. */
  gaps: string[];
  /** Whether a requirements list was found in the description at all. */
  found: boolean;
  askThem: string[];
}

export interface InterviewQuestion {
  id: number;
  question: string;
  answer: string;
  feedback: string;
  /** Claims in the coach's suggested opening that your resume and answer don't support. */
  warnings: string[];
}

/** What the role asks for against your resume. Built from text, never generated. */
export function roleBrief(id: number): Promise<RoleBrief> {
  return invoke<RoleBrief>('nexus_role_brief', { id });
}

export function interviewQuestions(id: number): Promise<InterviewQuestion[]> {
  return invoke<InterviewQuestion[]>('nexus_interview_questions', { id });
}

/** New practice questions. Unanswered ones are replaced; answered ones kept. */
export function generateInterviewQuestions(id: number): Promise<InterviewQuestion[]> {
  return invoke<InterviewQuestion[]>('nexus_generate_interview_questions', { id });
}

export function interviewFeedback(questionId: number, answer: string): Promise<InterviewQuestion> {
  return invoke<InterviewQuestion>('nexus_interview_feedback', { questionId, answer });
}

export function deleteInterviewQuestion(id: number): Promise<void> {
  return invoke<void>('nexus_delete_interview_question', { id });
}

export interface TodaySummary {
  name: string | null;
  partOfDay: 'morning' | 'afternoon' | 'evening';
  /** Null when the calendar couldn't be read: unknown, not empty. */
  calendar: { count: number; nextTitle: string | null; nextAt: string | null } | null;
  remindersDue: number;
  billsDue: { name: string; daysUntil: number; amount: number | null }[];
  taxSoon: { name: string; date: string; daysUntil: number } | null;
  followUpsDue: number;
  interviewing: number;
  cardsDue: number;
  streak: number;
  /** The one-line summary, assembled from the fields above (not generated). */
  line: string;
}

export interface WellbeingSettings {
  breaks: boolean;
  /** Continuous minutes at the keyboard before a break nudge (15 to 180). */
  breakMinutes: number;
  hydration: boolean;
  /** Minutes of activity between water reminders (30 to 240). */
  hydrationMinutes: number;
  winddown: boolean;
  /** Local "HH:MM". */
  winddownAt: string;
  /** Minutes without input that count as having stepped away (2 to 30). */
  awayMinutes: number;
}

export interface WellbeingStatus {
  activeMinutes: number;
  nextBreakIn: number | null;
  enabled: boolean;
}

export function wellbeingSettings(): Promise<WellbeingSettings> {
  return invoke<WellbeingSettings>('nexus_wellbeing_settings');
}

/** Save; values are clamped to sane ranges and the stored version returned. */
export function setWellbeingSettings(settings: WellbeingSettings): Promise<WellbeingSettings> {
  return invoke<WellbeingSettings>('nexus_set_wellbeing_settings', { settings });
}

export function wellbeingStatus(): Promise<WellbeingStatus> {
  return invoke<WellbeingStatus>('nexus_wellbeing_status');
}

export interface StandupInput {
  /** Start of the "yesterday" window, YYYY-MM-DD. */
  since: string;
  recent: { key: string; summary: string; status: string; category: string | null }[];
  open: { key: string; summary: string; status: string; category: string | null }[];
  prs: {
    repo: string;
    number: number;
    title: string;
    state: string;
    isDraft: boolean;
    mergedAt: string | null;
    reviewDecision: string | null;
    checks: { failing: number; pending: number } | null;
  }[];
  meetings: { title: string; at: string; upcoming: boolean }[];
}

export interface Standup {
  yesterday: string[];
  today: string[];
  blockers: string[];
  meetings: string[];
  nextMeeting: { title: string; at: string; related: string[] } | null;
  /** Plain text, ready to paste into chat. */
  text: string;
}

// -- Phone approvals ------------------------------------------------------------

export interface PhoneApprovalStatus {
  enabled: boolean;
  /** The private topic to subscribe to in the ntfy app, once generated. */
  approveTopic: string | null;
}

export function phoneApprovals(): Promise<PhoneApprovalStatus> {
  return invoke<PhoneApprovalStatus>('nexus_phone_approvals');
}

export function setPhoneApprovals(enabled: boolean): Promise<PhoneApprovalStatus> {
  return invoke<PhoneApprovalStatus>('nexus_set_phone_approvals', { enabled });
}

/** Send a harmless Approve/Deny request to check the round trip. */
export function phoneApprovalTest(): Promise<void> {
  return invoke<void>('nexus_phone_approval_test');
}

// -- Supervised task clone ------------------------------------------------------

export interface CloneProject {
  id: number;
  name: string;
  path: string;
  /** Your consent for this project's code to be read by Claude Code. */
  allowed: boolean;
  isGit: boolean;
  testLabel: string | null;
}

export type CloneStatus =
  | 'queued'
  | 'preparing'
  | 'editing'
  | 'testing'
  | 'ready'
  | 'failed'
  | 'applied'
  | 'discarded';

export interface CloneRun {
  id: number;
  projectId: number;
  projectName: string;
  task: string;
  status: CloneStatus;
  /** Claude Code's own summary of what it changed. */
  summary: string;
  diff: string;
  diffStat: string;
  testLabel: string;
  /** What NEXUS observed; null when no tests ran. Never assumed. */
  testPassed: boolean | null;
  testOutput: string;
  error: string;
  createdAt: string;
}

export function cloneProjects(): Promise<CloneProject[]> {
  return invoke<CloneProject[]>('nexus_clone_projects');
}

export function setCloneAllowed(projectId: number, allowed: boolean): Promise<void> {
  return invoke<void>('nexus_set_clone_allowed', { projectId, allowed });
}

export function cloneRuns(): Promise<CloneRun[]> {
  return invoke<CloneRun[]>('nexus_clone_runs');
}

/** Start a task in an allowed project. Runs in the background; poll cloneRuns. */
export function cloneStart(projectId: number, task: string): Promise<number> {
  return invoke<number>('nexus_clone_start', { projectId, task });
}

/** Apply a finished run to your working copy as uncommitted changes. */
export function cloneApply(id: number): Promise<string> {
  return invoke<string>('nexus_clone_apply', { id });
}

export function cloneDiscard(id: number): Promise<void> {
  return invoke<void>('nexus_clone_discard', { id });
}

/** Compose the standup draft. Pure formatting over data you already fetched. */
export function composeStandup(input: StandupInput): Promise<Standup> {
  return invoke<Standup>('nexus_compose_standup', { input });
}

/** Your day, from your own data: meetings, reminders, bills, follow-ups, study. */
export function today(): Promise<TodaySummary> {
  return invoke<TodaySummary>('nexus_today');
}

/** Save the tailored resume or cover letter to Downloads and reveal it in
 *  Finder. Returns the file path. Never fills in or submits a form. */
export function exportApplicationDoc(id: number, kind: 'resume' | 'cover'): Promise<string> {
  return invoke<string>('nexus_export_application_doc', { id, kind });
}

export function removeHolding(id: number): Promise<void> {
  return invoke<void>('nexus_remove_holding', { id });
}

/** The portfolio, priced live in INR. Each holding carries its own error if a
 *  symbol could not be priced; the rest still total. */
export function portfolio(): Promise<Portfolio> {
  return invoke<Portfolio>('nexus_portfolio');
}

export interface NewsItem {
  title: string;
  link: string;
  source: string;
  published: string | null;
}

export interface NewsBriefing {
  /** The lead line. Model-written when `generated`, else the top headline. */
  headline: string;
  generated: boolean;
  items: NewsItem[];
}

/**
 * The daily news briefing: headlines from public feeds, plus a one-line
 * heads-up the local model writes from them.
 *
 * The model call can take a few seconds on a local model; the headlines
 * themselves are fast. A missing or busy model still returns the news with the
 * top headline as its lead, so this never fails for want of a model.
 */
export function newsBriefing(limit?: number): Promise<NewsBriefing> {
  return invoke<NewsBriefing>('nexus_news_briefing', { limit });
}

/** Just the headlines, no model call. Fast; used to fill the feed immediately
 *  while the briefing's model-written lead is still composing. */
export function fetchNews(limit?: number): Promise<NewsItem[]> {
  return invoke<NewsItem[]>('nexus_fetch_news', { limit });
}

/** Silence the synthesizer. Safe when nothing is being spoken. */
export function voiceStopSpeaking(): Promise<void> {
  return invoke<void>('nexus_voice_stop_speaking');
}

/** English voices installed on this machine, for the Settings picker. */
export function voiceListVoices(): Promise<VoiceOption[]> {
  return invoke<VoiceOption[]>('nexus_voice_list_voices');
}

/** Transcripts arrive as events. They are never persisted anywhere. */
export function onVoiceTranscript(
  cb: (t: VoiceTranscript) => void,
): Promise<UnlistenFn> {
  return listen<VoiceTranscript>('nexus://voice/transcript', (e) => cb(e.payload));
}

export function onVoiceState(cb: (s: VoiceState) => void): Promise<UnlistenFn> {
  return listen<VoiceState>('nexus://voice/state', (e) => cb(e.payload));
}

export function onVoiceError(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>('nexus://voice/error', (e) => cb(e.payload));
}

/**
 * Resolve a spoken transcript to candidate commands.
 *
 * The registry is sent from the frontend so `src/lib/commands.ts` stays the
 * single source of truth. Only used on the voice path; typing still goes
 * through the unchanged NEXUS-009 matcher.
 */
export function resolveVoiceIntent(
  transcript: string,
  commands: VoiceCommandSpec[],
  projectNames: string[],
): Promise<VoiceIntent> {
  return invoke<VoiceIntent>('nexus_resolve_voice_intent', {
    transcript,
    commands,
    projectNames,
  });
}

// -- Money: spending -------------------------------------------------------------

export type SpendKind = 'spend' | 'income' | 'refund' | 'transfer';

export interface SpendStatus {
  connected: boolean;
  /** Unix seconds of the last successful sync. */
  lastSync: number | null;
  lastError: string | null;
  /** Bank messages waiting for the user to confirm. */
  review: number;
}

export interface SyncReport {
  added: number;
  review: number;
  ignored: number;
}

export interface Expense {
  id: number;
  source: 'sms' | 'manual';
  occurredOn: string;
  occurredAt: number | null;
  amountPaise: number;
  kind: SpendKind;
  payee: string;
  category: string;
  account: string;
  channel: string;
  note: string;
}

export interface SpendMonth {
  month: string;
  today: string;
  daysInMonth: number;
  /** Weekday of the 1st, Monday = 0. */
  firstWeekday: number;
  days: { date: string; spendPaise: number; count: number }[];
  categories: { category: string; spendPaise: number; count: number; budgetPaise: number | null }[];
  payees: { payee: string; category: string; spendPaise: number; count: number }[];
  accounts: { account: string; spendPaise: number }[];
  /** Spending per weekday, Monday first. */
  weekdays: number[];
  spendPaise: number;
  incomePaise: number;
  refundPaise: number;
  transferPaise: number;
  /** Last month's spending over the same days, for comparison. */
  previousPaise: number;
  transactions: Expense[];
  allCategories: string[];
  savings: Savings;
  /** Six months, oldest first, ending with this one. */
  history: { month: string; spendPaise: number; savings: Savings }[];
  usualIncomePaise: number | null;
  balances: AccountBalance[];
}

export interface AccountBalance {
  account: string;
  /** The last known balance carried forward through later payments. */
  balancePaise: number;
  /** Unix seconds when that balance was stated. */
  asOf: number;
  source: 'sms' | 'manual';
  /** Payments since then, already applied. */
  adjusted: number;
}

export interface Savings {
  /** Income used: recorded, else the usual income, else null. */
  incomePaise: number | null;
  /** True when the usual income stands in for no recorded income. */
  estimated: boolean;
  /** Money in plus refunds less spending; negative means overspent. */
  savedPaise: number | null;
  /** Saved as a percent of income. */
  rate: number | null;
}

export interface ExpenseInput {
  date: string;
  /** Rupees. */
  amount: number;
  payee: string;
  category: string;
  account: string;
  kind: SpendKind;
  note: string;
}

export interface SmsReading {
  amountPaise: number | null;
  debit: boolean | null;
  date: string | null;
  payee: string | null;
  account: string;
  channel: string;
  card: boolean;
}

export interface ReviewItem {
  smsId: string;
  sender: string;
  text: string;
  receivedAt: number | null;
  reason: string;
  suggestion: SmsReading | null;
  category: string;
  kind: SpendKind;
}

export function spendStatus(): Promise<SpendStatus> {
  return invoke<SpendStatus>('nexus_spend_status');
}

export function spendConnect(link: string): Promise<SpendStatus> {
  return invoke<SpendStatus>('nexus_spend_connect', { link });
}

export function spendDisconnect(): Promise<SpendStatus> {
  return invoke<SpendStatus>('nexus_spend_disconnect');
}

export function spendSync(): Promise<SyncReport> {
  return invoke<SyncReport>('nexus_spend_sync');
}

/** Book bank messages pasted by hand; several can be separated by a blank line. */
export function spendPaste(text: string): Promise<SyncReport> {
  return invoke<SyncReport>('nexus_spend_paste', { text });
}

export function spendMonth(month?: string): Promise<SpendMonth> {
  return invoke<SpendMonth>('nexus_spend_month', { month: month ?? null });
}

export function spendAdd(input: ExpenseInput): Promise<number> {
  return invoke<number>('nexus_spend_add', { input });
}

export function spendUpdate(id: number, input: ExpenseInput, rememberPayee: boolean): Promise<void> {
  return invoke<void>('nexus_spend_update', { id, input, rememberPayee });
}

export function spendDelete(id: number): Promise<void> {
  return invoke<void>('nexus_spend_delete', { id });
}

/** Usual monthly income in rupees, used for months with none recorded; null clears it. */
export function spendSetIncome(amount: number | null): Promise<void> {
  return invoke<void>('nexus_spend_set_income', { amount });
}

/** Set an account's balance as it stands now, in rupees. */
export function spendSetBalance(account: string, amount: number): Promise<void> {
  return invoke<void>('nexus_spend_set_balance', { account, amount });
}

export function spendSetBudget(category: string, amount: number | null): Promise<void> {
  return invoke<void>('nexus_spend_set_budget', { category, amount });
}

export function spendReview(): Promise<ReviewItem[]> {
  return invoke<ReviewItem[]>('nexus_spend_review');
}

export function spendAccept(smsId: string, input: ExpenseInput, rememberPayee: boolean): Promise<number> {
  return invoke<number>('nexus_spend_accept', { smsId, input, rememberPayee });
}

export function spendIgnore(smsId: string): Promise<void> {
  return invoke<void>('nexus_spend_ignore', { smsId });
}

export interface SpendLockState {
  open: boolean;
  method: 'touchId' | 'password';
}

export function spendLockState(): Promise<SpendLockState> {
  return invoke<SpendLockState>('nexus_spend_lock_state');
}

/** Shows the macOS Touch ID prompt and resolves once it is answered. */
export function spendUnlock(): Promise<void> {
  return invoke<void>('nexus_spend_unlock');
}

export function spendLock(): Promise<void> {
  return invoke<void>('nexus_spend_lock');
}

/** True for the error a Spending command returns while locked. */
export function isSpendLocked(err: unknown): boolean {
  return String(err).includes('Spending is locked');
}

// -- Phone call alerts ------------------------------------------------------------

export interface CallSettings {
  enabled: boolean;
  meetings: boolean;
  /** Minutes before a meeting starts. */
  meetingLead: number;
  reminders: boolean;
  /** Names to ring for, as they appear in WhatsApp, Teams or Mail. */
  people: string[];
  includeText: boolean;
  onlyWhenAway: boolean;
}

export interface CallView {
  settings: CallSettings;
  /** The private topic to subscribe to in the ntfy app, once generated. */
  topic: string | null;
}

export function callSettings(): Promise<CallView> {
  return invoke<CallView>('nexus_call_settings');
}

export function setCallSettings(settings: CallSettings): Promise<CallView> {
  return invoke<CallView>('nexus_set_call_settings', { settings });
}

export function callTest(): Promise<void> {
  return invoke<void>('nexus_call_test');
}

// -- Portfolio chat ----------------------------------------------------------------

/** One change read from the chat box; sent back unchanged to apply it. */
export type PortfolioAction = { type: string } & Record<string, unknown>;

export interface PortfolioPreview {
  actions: PortfolioAction[];
  /** Each action in words, same order, for confirming. */
  lines: string[];
  notUnderstood: string[];
}

export function portfolioChat(text: string): Promise<PortfolioPreview> {
  return invoke<PortfolioPreview>('nexus_portfolio_chat', { text });
}

export function portfolioChatApply(actions: PortfolioAction[]): Promise<string[]> {
  return invoke<string[]>('nexus_portfolio_chat_apply', { actions });
}

// -- News ---------------------------------------------------------------------------

export interface DeskItem {
  title: string;
  link: string;
  source: string;
  topic: string;
  /** Unix seconds, when the feed's date could be read. */
  published: number | null;
  summary: string | null;
  /** Your interests this item mentions. */
  matches: string[];
}

export interface NewsSource {
  id: string;
  label: string;
  topic: string;
  on: boolean;
  /** On unless switched off (built-in sources). */
  defaultOn: boolean;
  custom: boolean;
  /** Why it returned nothing on the last read. */
  error: string | null;
}

export interface NewsDesk {
  items: DeskItem[];
  sources: NewsSource[];
  interests: string[];
  topics: string[];
  fetchedAt: number;
}

export interface CustomFeed {
  label: string;
  url: string;
  topic: string;
}

export interface NewsSettings {
  enabled: string[];
  disabled: string[];
  custom: CustomFeed[];
  interests: string[];
}

export function newsDesk(refresh = false): Promise<NewsDesk> {
  return invoke<NewsDesk>('nexus_news_desk', { refresh });
}

export function newsSettings(): Promise<NewsSettings> {
  return invoke<NewsSettings>('nexus_news_settings');
}

export function setNewsSettings(settings: NewsSettings): Promise<NewsSettings> {
  return invoke<NewsSettings>('nexus_set_news_settings', { settings });
}

/** One instrument on the Stocks board. Returns are percentages. */
export interface StockRow {
  symbol: string;
  name: string;
  price: number;
  changePct: number | null;
  ret1w: number | null;
  ret1m: number | null;
  ret3m: number | null;
  vsSma20: number | null;
  vsSma50: number | null;
  fromHigh: number | null;
  volumeRatio: number | null;
  spark: number[];
}

export interface StockBoard {
  indices: StockRow[];
  stocks: StockRow[];
  skipped: string[];
  asOf: number;
  fetchedAt: number;
}

export function stockBoard(refresh = false): Promise<StockBoard> {
  return invoke<StockBoard>('nexus_stock_board', { refresh });
}

/** One input to a suggestion score, 0 to 100; `null` when there was no data. */
export interface ScoreFactor {
  name: string;
  score: number | null;
  weight: number;
}

export interface StockHeadline {
  title: string;
  link: string;
  source: string;
  published: number | null;
  tone: -1 | 0 | 1;
}

export interface StockFundamentals {
  displayName: string | null;
  pe: number | null;
  industryPe: number | null;
  roe: number | null;
  debtEquity: number | null;
  revenueGrowth: number | null;
  profitGrowth: number | null;
  profitQYoy: number | null;
  instChange: number | null;
  promoterChange: number | null;
}

export type StockVerdict = 'Buy' | 'Accumulate' | 'Hold' | 'Avoid';

export interface StockPick {
  symbol: string;
  name: string;
  price: number;
  changePct: number | null;
  ret1m: number | null;
  ret3m: number | null;
  score: number;
  verdict: StockVerdict;
  factors: ScoreFactor[];
  reasons: string[];
  risks: string[];
  headlines: StockHeadline[];
  buyNear: number | null;
  support: number | null;
  fundamentals: StockFundamentals;
}

export interface StockPicks {
  picks: StockPick[];
  nifty3m: number | null;
  gaps: string[];
  fetchedAt: number;
}

export function stockPicks(refresh = false): Promise<StockPicks> {
  return invoke<StockPicks>('nexus_stock_picks', { refresh });
}

export interface Weighted {
  name: string;
  pct: number;
}

export type FundVerdict = 'Top pick' | 'Good' | 'Average' | 'Weak';

export interface FundPick {
  code: string;
  id: string;
  name: string;
  fundHouse: string | null;
  score: number;
  verdict: FundVerdict;
  factors: ScoreFactor[];
  reasons: string[];
  risks: string[];
  aum: number | null;
  expenseRatio: number | null;
  minSip: number | null;
  minLumpsum: number | null;
  ret1y: number | null;
  ret3y: number | null;
  ret5y: number | null;
  catRet3y: number | null;
  sharpe: number | null;
  stdDev: number | null;
  alpha: number | null;
  manager: string | null;
  managerYears: number | null;
  flowPct: number | null;
  exitLoad: string | null;
  topHoldings: Weighted[];
  topSectors: Weighted[];
}

export interface FundPicks {
  funds: FundPick[];
  tooNew: string[];
  flowDays: number;
  flowMinDays: number;
  gaps: string[];
  fetchedAt: number;
}

export function fundPicks(refresh = false): Promise<FundPicks> {
  return invoke<FundPicks>('nexus_fund_picks', { refresh });
}

export interface PickOutcome {
  count: number;
  /** Mean return beyond the benchmark, in percentage points. */
  avgExcess: number | null;
  /** Share that beat the benchmark, 0 to 1. */
  hitRate: number | null;
}

export interface PickGroupRecord {
  label: string;
  /** One per entry of `TrackRecord.horizons`. */
  outcomes: PickOutcome[];
}

export interface FactorSkill {
  name: string;
  skill: number | null;
  baseWeight: number;
  weight: number;
}

export interface PickLearning {
  days: number;
  needed: number;
  active: boolean;
  confidence: number;
  factors: FactorSkill[];
}

export interface PastPick {
  day: number;
  symbol: string;
  verdict: string;
  score: number;
  excess: (number | null)[];
}

export interface TrackRecord {
  horizons: number[];
  stockDays: number;
  fundDays: number;
  firstDay: number | null;
  stocks: PickGroupRecord[];
  funds: PickGroupRecord[];
  learning: PickLearning;
  recent: PastPick[];
}

export function pickTrackRecord(): Promise<TrackRecord> {
  return invoke<TrackRecord>('nexus_pick_track_record');
}
