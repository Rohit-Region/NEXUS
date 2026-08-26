# NEXUS-001: Desktop Foundation

## Overview

Establish the clean, working foundation for NEXUS — a macOS-first personal developer command center. This milestone delivers only the desktop application shell: no integrations, no AI, no backend.

---

## 1. Requirements

### 1.1 Functional Requirements

| ID | Requirement |
|----|-------------|
| F-01 | The application must launch as a native macOS desktop window using Tauri 2. |
| F-02 | The application name must be **NEXUS**. |
| F-03 | The native window title must be **NEXUS**. |
| F-04 | The UI must display NEXUS branding (logo / wordmark). |
| F-05 | The UI must display a system status indicator reading **"SYSTEM ONLINE"**. |
| F-06 | The UI must include a command input field with placeholder text **"Ask Nexus anything..."**. |
| F-07 | The command input must accept and echo typed text (controlled input). It must not execute commands. |
| F-08 | The UI must include a microphone button. It must be UI-only — no speech recognition. |
| F-09 | The application must include a basic NEXUS application icon suitable for the macOS bundle. The icon does not need to be final production branding. |

### 1.2 Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| N-01 | The application must be local-first. No remote backend, no cloud service, no authentication. |
| N-02 | The frontend must be built with React 18+, TypeScript 5+, and Vite 5+. |
| N-03 | The desktop shell must be provided by Tauri 2 (Rust-based native layer). |
| N-04 | The package manager must be pnpm. |
| N-05 | The visual theme must be a clean, premium black-and-red color palette. |
| N-06 | Code must be modular — `App.tsx` must not contain all application logic. |
| N-07 | Components must be reusable and individually maintained. |
| N-08 | TypeScript types must be defined where appropriate. |
| N-09 | Icons must use the `lucide-react` library. |
| N-10 | No SQLite, no authentication, no CI/CD, no auto-update, no cloud sync. |
| N-11 | The font stack must use native system monospace fonts only. No external fonts shall be downloaded or bundled. |
| N-12 | Tauri security configuration must use the Tauri-generated default unless a custom CSP is actually required. Tauri security must not be weakened. |
| N-13 | Kiro must not introduce additional frameworks, services, dependencies, databases, APIs, architectural layers, cloud services, or infrastructure unless explicitly required by the approved NEXUS-001 requirements. |

### 1.3 Explicitly Out of Scope

The following must NOT be implemented in NEXUS-001:

- Jira, Claude, PlayerZero, Cursor, Grok, ChatGPT integrations
- AI agent orchestration
- Voice recognition / text-to-speech
- Browser automation
- Terminal command execution
- IDE integrations (IntelliJ, VS Code, Cursor)
- News, weather, morning briefings
- Notifications, CI/CD pipelines
- Authentication or user accounts
- Cloud synchronization
- SQLite or any database
- Auto-update functionality
- Custom title bar or custom window chrome (deferred to a later milestone)

---

## 2. Design

### 2.1 Tech Stack

| Layer | Technology |
|-------|------------|
| Frontend framework | React 18 + TypeScript 5 |
| Build tool | Vite 5 |
| Desktop shell | Tauri 2 |
| Native layer | Rust (via Tauri) |
| Package manager | pnpm |
| Icons | lucide-react |

### 2.2 Project Structure

```
NEXUS/
├── src/                          # React/TypeScript frontend
│   ├── main.tsx                  # React entry point
│   ├── App.tsx                   # Root component (thin orchestrator only)
│   ├── types/
│   │   └── index.ts              # Shared TypeScript types
│   ├── components/
│   │   ├── AppShell/
│   │   │   ├── AppShell.tsx      # Top-level layout wrapper
│   │   │   └── AppShell.css
│   │   ├── Logo/
│   │   │   ├── Logo.tsx          # NEXUS wordmark / branding
│   │   │   └── Logo.css
│   │   ├── StatusBar/
│   │   │   ├── StatusBar.tsx     # "SYSTEM ONLINE" indicator
│   │   │   └── StatusBar.css
│   │   ├── Dashboard/
│   │   │   ├── Dashboard.tsx     # Main dashboard panel
│   │   │   └── Dashboard.css
│   │   └── CommandBar/
│   │       ├── CommandBar.tsx    # Command input + mic button
│   │       └── CommandBar.css
│   └── assets/
│       └── styles/
│           └── globals.css       # CSS custom properties, reset, theme tokens
├── src-tauri/                    # Tauri 2 / Rust layer
│   ├── icons/                    # Application icon set for macOS bundle
│   ├── src/
│   │   ├── main.rs               # Tauri application entry point
│   │   └── lib.rs                # Tauri app builder / commands
│   ├── Cargo.toml                # Rust manifest
│   ├── Cargo.lock
│   └── tauri.conf.json           # Tauri configuration (app name, window title, etc.)
├── index.html                    # Vite HTML entry
├── vite.config.ts                # Vite configuration
├── tsconfig.json                 # TypeScript configuration
├── tsconfig.node.json            # TypeScript config for Vite node scripts
├── package.json                  # pnpm manifest
└── pnpm-lock.yaml
```

### 2.3 Visual Theme

The premium black-and-red theme is defined as CSS custom properties in `globals.css` and consumed by all components.

| Token | Value | Usage |
|-------|-------|-------|
| `--color-bg-primary` | `#0a0a0a` | Main window background |
| `--color-bg-secondary` | `#111111` | Panel / card backgrounds |
| `--color-bg-tertiary` | `#1a1a1a` | Input / elevated surfaces |
| `--color-accent` | `#e63946` | NEXUS red — primary accent |
| `--color-accent-dim` | `#991b22` | Subdued red (hover states, glows) |
| `--color-text-primary` | `#f5f5f5` | Primary text |
| `--color-text-secondary` | `#888888` | Secondary / muted text |
| `--color-text-muted` | `#444444` | Placeholder / disabled text |
| `--color-border` | `#222222` | Subtle borders |
| `--color-border-accent` | `#e63946` | Focused / active borders |
| `--font-mono` | `ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace` | All UI text — native system stack, no download |
| `--radius-sm` | `4px` | Small elements |
| `--radius-md` | `8px` | Cards, panels |
| `--radius-lg` | `12px` | Large panels, windows |

### 2.4 Component Specifications

#### `AppShell`
- Full-viewport layout container.
- Arranges: `Logo` (top-left), `StatusBar` (top-right), `Dashboard` (center, flex-grow), `CommandBar` (bottom, pinned).
- Uses CSS Grid or Flexbox column layout.
- Applies the `--color-bg-primary` background.
- Renders a subtle red accent line / glow at the top edge (branding detail).

#### `Logo`
- Renders the NEXUS wordmark in all-caps, monospace font.
- The `X` character is highlighted with `--color-accent` red.
- Optionally renders a small geometric icon to the left.
- Props: none (static branding).

#### `StatusBar`
- Displays a pulsing green dot + the text **"SYSTEM ONLINE"** in muted green.
- Displays the current date/time (rendered client-side, refreshed every second).
- Props: none (self-contained).

#### `Dashboard`
- Main content area. Currently a placeholder panel.
- Shows a large, dimmed NEXUS wordmark watermark in the background.
- Has a centered welcome message: **"NEXUS Command Center"** + a subtitle.
- Designed to accept future widget panels via composition.
- Props: none (static shell).

#### `CommandBar`
- A single-row input area pinned to the bottom of the window.
- Contains:
  - A text input (`<input type="text">`) with placeholder **"Ask Nexus anything..."**.
  - A `Mic` icon button (lucide-react `Mic` icon), styled with the accent red.
- State: `commandValue: string` (controlled input).
- The mic button has a hover/active state but does nothing on click.
- Props: none (self-contained, manages its own state).

### 2.5 Tauri 2 Configuration

**Window**
- Title: `NEXUS`
- Width: `1200`, Height: `800`
- Min Width: `900`, Min Height: `600`
- Decorations: `true` (native macOS window decorations; custom title bar deferred to a later milestone)
- Resizable: `true`
- Center: `true`
- Transparent: `false`

**Bundle**
- Identifier: `com.nexus.app`
- Product name: `NEXUS`
- Version: `0.1.0`
- Icons: Tauri-generated icon set in `src-tauri/icons/`

**Security**
- Use the Tauri-generated default security configuration. Do not introduce a custom CSP unless one is actually required for NEXUS-001 functionality. Do not weaken Tauri security.

### 2.6 TypeScript Types (`src/types/index.ts`)

```typescript
// System status values — will expand in future milestones
export type SystemStatus = 'ONLINE' | 'OFFLINE' | 'DEGRADED';

// Base props shared by panel components
export interface PanelProps {
  className?: string;
}

// CommandBar state
export interface CommandState {
  value: string;
  isListening: boolean; // always false in NEXUS-001
}
```

### 2.7 Dependency List

Only the dependencies listed below are permitted in NEXUS-001. No additional dependencies may be introduced.

**Runtime dependencies (frontend)**
```
react
react-dom
lucide-react
```

**Dev dependencies (frontend)**
```
@types/react
@types/react-dom
@vitejs/plugin-react
typescript
vite
@tauri-apps/cli       (v2)
```

**Rust dependencies (src-tauri/Cargo.toml)**
```
tauri = { version = "2", features = [] }
tauri-build = { version = "2" }
```
`serde` and `serde_json` are not included in the approved list. They may only be present if the minimal generated Tauri project requires them for compilation. Do not add them for future functionality.

---

## 3. Implementation Tasks

The following tasks are to be executed in order after this spec is approved.

| # | Task | Description |
|---|------|-------------|
| T-01 | **Environment setup** | Install Rust toolchain (via rustup) and pnpm. Verify all prerequisites. |
| T-02 | **Initialize project in existing repo** | Inspect the existing NEXUS repository first. If the React/Vite frontend does not already exist, initialize React + TypeScript + Vite in the existing repository. Then initialize Tauri 2 using `pnpm tauri init`. Do not create a second project, nested project, or overwrite unrelated existing repository content. |
| T-03 | **Configure Tauri** | Set app name, window title, dimensions, native decorations (`true`), bundle identifier in `tauri.conf.json`. Do not add a custom CSP unless required. |
| T-04 | **Generate placeholder application icon** | Generate a simple functional placeholder icon set suitable for the macOS bundle and place it in `src-tauri/icons/`. Register icons in `tauri.conf.json`. Do not invest significant effort in final branding. |
| T-05 | **Install frontend dependencies** | Add `lucide-react`; verify all packages resolve. |
| T-06 | **Create CSS theme** | Implement `globals.css` with all design tokens, using the native system monospace font stack. |
| T-07 | **Build `Logo` component** | Implement NEXUS wordmark with red X accent. |
| T-08 | **Build `StatusBar` component** | Implement SYSTEM ONLINE indicator with live clock. |
| T-09 | **Build `Dashboard` component** | Implement dashboard shell with watermark and welcome text. |
| T-10 | **Build `CommandBar` component** | Implement controlled text input + mic button. |
| T-11 | **Build `AppShell` component** | Compose all components into the full-viewport layout. |
| T-12 | **Wire `App.tsx`** | Thin root component rendering only `AppShell`. |
| T-13 | **Define TypeScript types** | Create `src/types/index.ts` with shared types. |
| T-14 | **Verify build** | Run all three of the following and confirm each succeeds: `pnpm tauri dev` (confirms the app launches as a native macOS desktop window with correct title, theme, and all components rendered), `pnpm build` (confirms the frontend builds cleanly), and `pnpm tauri build` (confirms the production macOS application bundle is generated successfully and can be launched). |

---

## 4. Acceptance Criteria

The milestone is complete when all of the following are true:

- [ ] `pnpm tauri dev` launches a native macOS desktop window titled **NEXUS**.
- [ ] The application runs as a native macOS desktop application independently of a browser tab.
- [ ] The window displays the NEXUS logo/wordmark with the red X accent.
- [ ] The window displays **"SYSTEM ONLINE"** with a pulsing indicator.
- [ ] The window displays a live clock.
- [ ] The window displays the dashboard shell with a welcome message.
- [ ] The command input accepts text with placeholder "Ask Nexus anything...".
- [ ] The microphone button is visible and styled; clicking it does nothing.
- [ ] No runtime errors appear in the Tauri webview console.
- [ ] The codebase has no component logic in `App.tsx` beyond rendering `AppShell`.
- [ ] All five components (`AppShell`, `Logo`, `StatusBar`, `Dashboard`, `CommandBar`) exist as separate files.
- [ ] `src/types/index.ts` exists with `SystemStatus`, `PanelProps`, and `CommandState`.
- [ ] The macOS window uses native decorations. No custom title bar is implemented.
- [ ] No external fonts are downloaded or bundled; all typography uses the native system monospace stack.
- [ ] A NEXUS application icon is present in `src-tauri/icons/` and registered in `tauri.conf.json`.
- [ ] `pnpm build` completes without errors.
- [ ] `pnpm tauri build` generates the production macOS application bundle without errors.
- [ ] The generated application bundle launches successfully as a standalone macOS application.
- [ ] No out-of-scope features (AI, voice, integrations, DB, cloud, auth, CI/CD, auto-update) are present.
- [ ] No dependencies beyond the approved list in section 2.7 are introduced.
