import {
  createAgent,
  createIde,
  deleteAgent,
  deleteIde,
  listAgents,
  listIdes,
  updateAgent,
  updateIde,
} from '../../lib/nexus-db';
import { RegistryPanel } from '../RegistryPanel/RegistryPanel';
import type { RegistryKind } from '../RegistryPanel/RegistryPanel';
import './RegistryScreen.css';

const IDE_KIND: RegistryKind = {
  key: 'ide',
  title: 'IDEs',
  singular: 'IDE',
  typeLabel: 'IDE Type',
  typePlaceholder: 'editor, terminal, notebook...',
  pathPlaceholder: '/Applications/YourEditor.app',
  projectColumn: 'defaultIdeId',
  list: listIdes,
  create: createIde,
  update: updateIde,
  remove: deleteIde,
};

const AGENT_KIND: RegistryKind = {
  key: 'agent',
  title: 'AI Agents',
  singular: 'Agent',
  typeLabel: 'Agent Type',
  typePlaceholder: 'assistant, reviewer, planner...',
  pathPlaceholder: '/usr/local/bin/your-agent',
  projectColumn: 'defaultAgentId',
  list: listAgents,
  create: createAgent,
  update: updateAgent,
  remove: deleteAgent,
};

/** Layout shell. Owns no data: each panel fetches its own. */
export function RegistryScreen() {
  return (
    <section className="registry-screen" aria-label="Registry">
      <div className="registry-screen__header">
        <h2 className="registry-screen__title">Registry</h2>
        <p className="registry-screen__subtitle">
          IDEs and AI agents available to your projects. Entries are recorded
          only; nothing is launched or executed.
        </p>
      </div>

      <RegistryPanel kind={IDE_KIND} />
      <RegistryPanel kind={AGENT_KIND} />
    </section>
  );
}
