import { useCallback, useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { deleteContact, listContacts, saveContact } from '../../lib/nexus-db';
import type { Contact } from '../../types/db';
import './ContactsPanel.css';

/**
 * The people NEXUS can message by name.
 *
 * WhatsApp identifies people by number, so without this "message Divi"
 * resolves to nobody. Entered by hand on purpose: reading the macOS address
 * book would mean a Contacts permission and access to everyone the user
 * knows, to solve a problem that is a few rows wide.
 *
 * Validation lives in Rust, against the same check the connector applies
 * before dialling, so a contact that saves is a contact that can be
 * messaged. Errors here are that check's own words.
 */
export function ContactsPanel() {
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [name, setName] = useState('');
  const [phone, setPhone] = useState('');
  const [editing, setEditing] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setContacts(await listContacts());
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  function reset() {
    setEditing(null);
    setName('');
    setPhone('');
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      setContacts(await saveContact({ id: editing, name, phone }));
      reset();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: number) {
    setBusy(true);
    setError(null);
    try {
      setContacts(await deleteContact(id));
      if (editing === id) reset();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="contacts-panel" aria-label="Contacts">
      <h3 className="contacts-panel__title">Contacts</h3>
      <p className="contacts-panel__intro">
        Who NEXUS can message by name. Say &ldquo;send a message to Divi saying
        I&rsquo;m running late&rdquo; and it opens WhatsApp with the message
        ready; you still press send. Numbers need a country code. Nothing is
        synced, and your macOS address book is not read.
      </p>

      {error && (
        <p className="contacts-panel__error" role="alert">
          {error}
        </p>
      )}

      <div className="contacts-panel__form">
        <input
          className="nexus-input"
          type="text"
          placeholder="Name, as you would say it"
          value={name}
          onChange={(e) => setName(e.target.value)}
          disabled={busy}
        />
        <input
          className="nexus-input"
          type="tel"
          placeholder="+91 98765 43210"
          value={phone}
          onChange={(e) => setPhone(e.target.value)}
          disabled={busy}
        />
        <button
          className="nexus-btn nexus-btn--primary"
          type="button"
          onClick={() => void save()}
          disabled={busy || name.trim() === '' || phone.trim() === ''}
        >
          {editing === null ? 'Add' : 'Save'}
        </button>
        {editing !== null && (
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={reset}
            disabled={busy}
          >
            Cancel
          </button>
        )}
      </div>

      {contacts.length === 0 ? (
        <p className="contacts-panel__status">No contacts yet.</p>
      ) : (
        <ul className="contacts-panel__list">
          {contacts.map((contact) => (
            <li className="contacts-panel__row" key={contact.id}>
              <button
                className="contacts-panel__pick"
                type="button"
                onClick={() => {
                  setEditing(contact.id);
                  setName(contact.name);
                  setPhone(`+${contact.phone}`);
                }}
                disabled={busy}
              >
                <span className="contacts-panel__name">{contact.name}</span>
                <span className="contacts-panel__phone">+{contact.phone}</span>
              </button>
              <button
                className="nexus-btn nexus-btn--secondary"
                type="button"
                aria-label={`Delete ${contact.name}`}
                onClick={() => void remove(contact.id)}
                disabled={busy}
              >
                <Trash2 size={14} strokeWidth={2} aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
