import { useCallback, useEffect, useState } from 'react';
import { Plus, ServerCog } from 'lucide-react';
import { listProjects } from '../../lib/nexus-db';
import type { RegistryFormValues } from '../../types';
import type {
  CreateRegistryEntryInput,
  RegistryEntry,
  UpdateRegistryEntryInput,
} from '../../types/db';
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
  list: (enabledOnly: boolean) => Promise<RegistryEntry[]>;
  create: (input: CreateRegistryEntryInput) => Promise<RegistryEntry>;
  update: (input: UpdateRegistryEntryInput) => Promise<RegistryEntry>;
  remove: (id: number) => Promise<void>;
}

interface RegistryPanelProps {
  kind: RegistryKind;
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

export function RegistryPanel({ kind }: RegistryPanelProps) {
  const [entries, setEntries] = useState<RegistryEntry[]>([]);
  const [usage, setUsage] = useState<Map<number, number>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [busyId, setBusyId] = useState<number | null>(null);

  // enabledOnly = false: the registry shows every entry, enabled or not.
  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [rows, projects] = await Promise.all([kind.list(false), listProjects()]);
      setEntries(rows);

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

  // ── Actions ──────────────────────────────────────────────────────────────

  async function handleCreate(values: RegistryFormValues) {
    setSubmitting(true);
    setError(null);
    try {
      await kind.create({
        name: values.name.trim(),
        entryType: values.entryType.trim(),
        executablePath: optional(values.executablePath),
        enabled: values.enabled,
      });
      setShowCreateForm(false);
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
      await kind.update({
        id,
        name: values.name.trim(),
        entryType: values.entryType.trim(),
        executablePath: optional(values.executablePath),
        enabled: values.enabled,
      });
      setEditingId(null);
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
      await kind.update({
        id: entry.id,
        name: entry.name,
        entryType: entry.entryType,
        executablePath: entry.executablePath ?? undefined,
        enabled: !entry.enabled,
      });
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

  const disabledCount = entries.filter((e) => !e.enabled).length;
  const isEmpty = !loading && entries.length === 0;

  return (
    <section className="registry-panel" aria-label={kind.title}>
      <div className="registry-panel__header">
        <div className="registry-panel__title-group">
          <h3 className="registry-panel__title">{kind.title}</h3>
          {!loading && (
            <span className="registry-panel__count">{entries.length}</span>
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

      {entries.length > 0 && (
        <div className="registry-panel__items">
          {entries.map((entry) => (
            <RegistryCard
              key={entry.id}
              entry={entry}
              singular={kind.singular}
              projectUsage={usage.get(entry.id) ?? 0}
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
