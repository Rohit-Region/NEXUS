import { useCallback, useEffect, useState } from 'react';
import { Plus, ServerCog } from 'lucide-react';
import { countTasksByAgent, listProjects } from '../../lib/nexus-db';
import {
  matchesQuery,
  normalizeQuery,
  registryComparator,
  sortWithIdTiebreak,
  REGISTRY_SORT_OPTIONS,
} from '../../lib/list-filters';
import type {
  EnabledFilter,
  RegistryFormValues,
  RegistrySortMode,
} from '../../types';
import type {
  CreateRegistryEntryInput,
  RegistryEntry,
  Settings,
  UpdateRegistryEntryInput,
} from '../../types/db';
import { ListControls } from '../ListControls/ListControls';
import { RegistryCard } from '../RegistryCard/RegistryCard';
import { RegistryForm } from '../RegistryForm/RegistryForm';
import './RegistryPanel.css';

/**
 * Descriptor for one registry kind. The command functions are injected, so the
 * panel never chooses which command to call: it calls what it was handed.
 */
export interface RegistryKind {
  key: 'ide' | 'agent';
  title: string;
  singular: string;
  typeLabel: string;
  typePlaceholder: string;
  pathPlaceholder: string;
  /** Which project column references this kind, for the usage count. */
  projectColumn: 'defaultIdeId' | 'defaultAgentId';
  /**
   * Whether this kind can hold task assignments. Agents can; IDEs cannot.
   * Keeps the IDE panel from issuing a query that has no meaning for it.
   */
  countsTasks: boolean;
  list: (enabledOnly: boolean) => Promise<RegistryEntry[]>;
  create: (input: CreateRegistryEntryInput) => Promise<RegistryEntry>;
  update: (input: UpdateRegistryEntryInput) => Promise<RegistryEntry>;
  remove: (id: number) => Promise<void>;
}

interface RegistryPanelProps {
  kind: RegistryKind;
  settings: Settings;
}

/** The single predicate used by both the render and the R-02 hidden check. */
function entryMatches(
  entry: RegistryEntry,
  normalized: string,
  enabledFilter: EnabledFilter,
): boolean {
  const enabledOk =
    enabledFilter === 'all' ||
    (enabledFilter === 'enabled' ? entry.enabled : !entry.enabled);
  return (
    enabledOk &&
    matchesQuery(normalized, [entry.name, entry.entryType, entry.executablePath])
  );
}

/** Empty strings become undefined so optional columns stay NULL in SQLite. */
function optional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function toFormValues(entry: RegistryEntry): RegistryFormValues {
  return {
    name: entry.name,
    entryType: entry.entryType,
    executablePath: entry.executablePath ?? '',
    enabled: entry.enabled,
  };
}

export function RegistryPanel({ kind, settings }: RegistryPanelProps) {
  const [entries, setEntries] = useState<RegistryEntry[]>([]);
  const [usage, setUsage] = useState<Map<number, number>>(new Map());
  const [taskUsage, setTaskUsage] = useState<Map<number, number>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [busyId, setBusyId] = useState<number | null>(null);

  // NEXUS-007 controls. Session-local, and independent per panel instance:
  // filtering IDEs does not filter agents.
  const [query, setQuery] = useState('');
  // Seeded from settings on mount; session changes never write back.
  const [sort, setSort] = useState<RegistrySortMode>(settings.registrySort);
  // The mount-time seed is what Reset returns to (see ProjectList).
  const [seededSort] = useState<RegistrySortMode>(settings.registrySort);
  const [enabledFilter, setEnabledFilter] = useState<EnabledFilter>('all');
  const [hiddenNotice, setHiddenNotice] = useState<string | null>(null);

  // enabledOnly = false: the registry shows every entry, enabled or not.
  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [rows, projects, taskCounts] = await Promise.all([
        kind.list(false),
        listProjects(),
        kind.countsTasks ? countTasksByAgent() : Promise.resolve([]),
      ]);
      setEntries(rows);
      setTaskUsage(new Map(taskCounts.map((c) => [c.agentId, c.taskCount])));

      // Usage counts for the delete warning; no new command required.
      const counts = new Map<number, number>();
      for (const project of projects) {
        const ref = project[kind.projectColumn];
        if (ref != null) counts.set(ref, (counts.get(ref) ?? 0) + 1);
      }
      setUsage(counts);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [kind]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // R-02: a save that lands outside the active controls must never look like
  // it failed. Returns the name to announce, or null when it stays visible.
  function noticeFor(entry: RegistryEntry): string | null {
    return entryMatches(entry, normalizeQuery(query), enabledFilter)
      ? null
      : entry.name;
  }

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate(values: RegistryFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      const created = await kind.create({
        name: values.name.trim(),
        entryType: values.entryType.trim(),
        executablePath: optional(values.executablePath),
        enabled: values.enabled,
      });
      setShowCreateForm(false);
      setHiddenNotice(noticeFor(created));
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleSave(id: number, values: RegistryFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      const saved = await kind.update({
        id,
        name: values.name.trim(),
        entryType: values.entryType.trim(),
        executablePath: optional(values.executablePath),
        enabled: values.enabled,
      });
      setEditingId(null);
      setHiddenNotice(noticeFor(saved));
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleToggleEnabled(entry: RegistryEntry) {
    setBusyId(entry.id);
    setError(null);
    try {
      const toggled = await kind.update({
        id: entry.id,
        name: entry.name,
        entryType: entry.entryType,
        executablePath: entry.executablePath ?? undefined,
        enabled: !entry.enabled,
      });
      setHiddenNotice(noticeFor(toggled));
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyId(null);
    }
  }

  async function handleDelete(id: number) {
    setBusyId(id);
    setError(null);
    try {
      await kind.remove(id);
      setConfirmDeleteId(null);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusyId(null);
    }
  }

  // Only one entry may be editing or awaiting confirmation at a time.
  function openEdit(id: number) {
    setConfirmDeleteId(null);
    setEditingId((prev) => (prev === id ? null : id));
  }

  function requestDelete(id: number) {
    setEditingId(null);
    setConfirmDeleteId(id);
  }

  // ── Render ───────────────────────────────────────────────────────────────

  const normalized = normalizeQuery(query);
  const isActive =
    normalized.length > 0 || sort !== seededSort || enabledFilter !== 'all';

  const visible = sortWithIdTiebreak(
    entries.filter((e) => entryMatches(e, normalized, enabledFilter)),
    registryComparator(sort),
  );

  function resetControls() {
    setQuery('');
    setSort(seededSort);
    setEnabledFilter('all');
    setHiddenNotice(null);
  }

  // Reads the UNFILTERED array: it reports what the registry holds, not what
  // the filter shows (spec 007 7.5).
  const disabledCount = entries.filter((e) => !e.enabled).length;
  const isEmpty = !loading && entries.length === 0;

  return (
    <section className="registry-panel" aria-label={kind.title}>
      <div className="registry-panel__header">
        <div className="registry-panel__title-group">
          <h3 className="registry-panel__title">{kind.title}</h3>
          {!loading && (
            <span className="registry-panel__count">
              {isActive
                ? `${visible.length} of ${entries.length}`
                : entries.length}
            </span>
          )}
          {disabledCount > 0 && (
            <span className="nexus-chip nexus-chip--muted">
              {disabledCount} disabled
            </span>
          )}
        </div>

        <button
          className="nexus-btn nexus-btn--primary"
          type="button"
          onClick={() => {
            setEditingId(null);
            setConfirmDeleteId(null);
            setShowCreateForm((prev) => !prev);
          }}
          aria-expanded={showCreateForm}
        >
          <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
          New {kind.singular}
        </button>
      </div>

      {entries.length > 0 && (
        <ListControls
          searchValue={query}
          onSearchChange={(v) => {
            setQuery(v);
            setHiddenNotice(null);
          }}
          searchPlaceholder="Search name, type or path"
          sortValue={sort}
          sortOptions={REGISTRY_SORT_OPTIONS}
          onSortChange={(v) => {
            setSort(v);
            setHiddenNotice(null);
          }}
          filterSlot={
            <div
              className="nexus-filter-bar"
              role="group"
              aria-label="Filter by enabled state"
            >
              {(['all', 'enabled', 'disabled'] as EnabledFilter[]).map((f) => (
                <button
                  key={f}
                  className={`nexus-btn ${
                    enabledFilter === f
                      ? 'nexus-btn--primary'
                      : 'nexus-btn--secondary'
                  }`}
                  type="button"
                  onClick={() => {
                    setEnabledFilter(f);
                    setHiddenNotice(null);
                  }}
                  aria-pressed={enabledFilter === f}
                >
                  {f}
                </button>
              ))}
            </div>
          }
          isActive={isActive}
          onReset={resetControls}
          disabled={loading}
        />
      )}

      {hiddenNotice && (
        <div className="nexus-notice" role="status">
          <span>
            Saved &quot;{hiddenNotice}&quot;, but it is hidden by the current
            filters.
          </span>
          <div className="nexus-notice__actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={resetControls}
            >
              Reset Filters
            </button>
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={() => setHiddenNotice(null)}
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

      {showCreateForm && (
        <RegistryForm
          mode="create"
          singular={kind.singular}
          typeLabel={kind.typeLabel}
          typePlaceholder={kind.typePlaceholder}
          pathPlaceholder={kind.pathPlaceholder}
          onSubmit={handleCreate}
          onCancel={() => setShowCreateForm(false)}
          submitting={submitting}
        />
      )}

      {error && (
        <p className="registry-panel__error" role="alert">
          {error}
        </p>
      )}

      {loading && (
        <p className="registry-panel__loading">Loading {kind.title.toLowerCase()}...</p>
      )}

      {isEmpty && !showCreateForm && (
        <div className="registry-panel__empty">
          <ServerCog size={22} strokeWidth={1.5} aria-hidden="true" />
          <span className="registry-panel__empty-title">
            No {kind.title.toLowerCase()} registered
          </span>
          <span className="registry-panel__empty-text">
            Register one to make it available when assigning defaults. NEXUS
            records the details only; it does not launch anything.
          </span>
          <button
            className="nexus-btn nexus-btn--primary"
            type="button"
            onClick={() => setShowCreateForm(true)}
          >
            <Plus size={12} strokeWidth={2.5} aria-hidden="true" />
            Register {kind.singular}
          </button>
        </div>
      )}

      {!loading && entries.length > 0 && visible.length === 0 && (
        <div className="nexus-no-results">
          <span className="nexus-no-results__title">
            No matching {kind.title.toLowerCase()}
          </span>
          <span className="nexus-no-results__text">
            {entries.length} in total. Relax the filters to see them.
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={resetControls}
          >
            Reset Filters
          </button>
        </div>
      )}

      {visible.length > 0 && (
        <div className="registry-panel__items">
          {visible.map((entry) => (
            <RegistryCard
              key={entry.id}
              entry={entry}
              singular={kind.singular}
              projectUsage={usage.get(entry.id) ?? 0}
              taskUsage={kind.countsTasks ? taskUsage.get(entry.id) ?? 0 : null}
              isEditing={editingId === entry.id}
              isConfirmingDelete={confirmDeleteId === entry.id}
              busy={busyId === entry.id}
              onToggleEnabled={() => void handleToggleEnabled(entry)}
              onEditToggle={() => openEdit(entry.id)}
              onDeleteRequest={() => requestDelete(entry.id)}
              onDeleteCancel={() => setConfirmDeleteId(null)}
              onDeleteConfirm={() => void handleDelete(entry.id)}
            >
              <RegistryForm
                mode="edit"
                singular={kind.singular}
                typeLabel={kind.typeLabel}
                typePlaceholder={kind.typePlaceholder}
                pathPlaceholder={kind.pathPlaceholder}
                initialValues={toFormValues(entry)}
                onSubmit={(values) => handleSave(entry.id, values)}
                onCancel={() => setEditingId(null)}
                submitting={submitting}
              />
            </RegistryCard>
          ))}
        </div>
      )}
    </section>
  );
}
