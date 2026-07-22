# Axolotl Launcher

This is a fork of the [Modrinth monorepo](https://github.com/modrinth/code) (upstream remote: `https://github.com/modrinth/code.git`). The product is the **Axolotl Launcher** desktop app — not the Modrinth website or backend. When entering a project, read its `AGENTS.md` or `CLAUDE.md`.

## Architecture

- **Monorepo tooling:** Turborepo (`turbo.jsonc`) + pnpm workspaces (`pnpm-workspace.yaml`)
- **Frontend:** Vue 3 / Nuxt 3, Tailwind CSS v3
- **Backend:** Rust (present from upstream but not the product focus)
- **Desktop shell:** Tauri v2
- **Indentation:** TAB everywhere, never spaces

### Apps (`apps/`)

| App               | Description                              | Product? |
| ----------------- | ---------------------------------------- | -------- |
| `app`             | Desktop app shell (Tauri)                | Yes      |
| `app-frontend`    | Desktop app frontend (Vue 3/Vite)        | Yes      |
| `website`         | Axolotl marketing site (Nuxt 3)          | Yes      |
| `frontend`        | Modrinth website (Nuxt 3) — from upstream | No       |
| `labrinth`        | Backend API (Rust) — from upstream       | No       |
| `app-playground`  | Testing playground for app               | Yes      |
| `daedalus_client` | Daedalus client implementation           | No       |
| `docs`            | Documentation site (Astro)               | No       |

### Packages (`packages/`)

| Package            | Description                              |
| ------------------ | ---------------------------------------- |
| `app-lib`          | Shared Rust app library (crate name `theseus`) |
| `ui`               | Shared Vue component library (`@modrinth/ui`) |
| `assets`           | Styling and auto-generated icons         |
| `api-client`       | API client for Nuxt, Tauri, and Node/browser |
| `daedalus`         | Daedalus protocol                        |
| `tooling-config`   | ESLint, Prettier, TypeScript configs     |
| `utils`            | Shared utility functions (mostly deprecated) |
| `blog`             | Blog system and changelog data           |
| `moderation`       | Moderation utilities                     |
| `ariadne`          | Analytics library                        |
| `modrinth-log`     | Logging utilities                        |
| `modrinth-maxmind` | MaxMind GeoIP                            |
| `modrinth-util`    | General utilities                        |
| `muralpay`         | Payment processing                       |
| `path-util`        | Path utilities                           |
| `sqlx-tracing`     | SQLx query tracing                       |

## Product Scope

Axolotl changes are confined to:
- `apps/app` / `apps/app-frontend` / `apps/website`
- `packages/app-lib`
- Shared UI/assets packages as needed

The website (`apps/frontend`) and backend (`apps/labrinth`) exist from upstream but are **not** the product. Do not modify them unless needed for the desktop app or an upstream sync forces it.

### Axolotl Branding

- Bundle ID: `red.ghs.axolotl`, deep-link scheme: `axolotl`
- User-Agent: `garbage-human-studio/axolotl/${version} (${os})`
- Private Modrinth services (ads, telemetry, Archon, Intercom) are **disabled**
- Run `pnpm axolotl:brand-guard` to verify none of these invariants have been broken

## Dev Commands

- **Desktop app:** `pnpm app:dev` (copy `.env` template in `packages/app-lib/` first)
- **Axolotl website:** `pnpm website:dev`
- **Modrinth website:** `pnpm web:dev`
- **Storybook (packages/ui):** `pnpm storybook`

## Pre-PR Checks

Run from repo root **only when asked** (not after every prompt):

- **Desktop app:** `pnpm prepr:frontend:app`
- **Axolotl website:** `pnpm prepr:website`
- **Guardrails (always):** `pnpm axolotl:brand-guard` and `pnpm axolotl:i18n-check`

### Backend (Labrinth)

See `apps/labrinth/AGENTS.md`. Do not run `cargo test` for labrinth unless explicitly asked — it's slow.

### Rust checks

| Command | Scope |
| ------- | ----- |
| `cargo fmt --all --check` | All Rust workspace members |
| `cargo check --package theseus_gui --features updater` | Desktop app typecheck |
| `cargo nextest run --all-targets --no-fail-fast` (in `apps/app/` or `packages/app-lib/`) | Desktop app tests |
| `cargo clippy --all-targets --features updater` | Desktop app lint |

Turbo cache is disabled for `@modrinth/app#build` — Tauri builds are never cached.

## i18n

- **Source:** `en-US` (in `apps/app-frontend/src/locales/` and `packages/ui/src/locales/`)
- **Primary translation:** `zh-CN` — must be complete and have matching ICU arguments
- Run `pnpm axolotl:i18n-check` to verify zh-CN coverage
- Hardcoded strings in Vue templates/components must use the `@modrinth/ui` i18n system (vue-i18n), not raw text

## Release

Triggered by pushing a semver tag matching `v*`:

```bash
git tag -a v1.2.3 -m "Axolotl Launcher 1.2.3"
git push origin v1.2.3
```

The `axolotl-release.yml` workflow builds platform installers, signs update packages, and publishes the GitHub Release. Prerelease tags (e.g. `v1.2.3-beta.1`) create prereleases. The Tauri signing private key is stored in GitHub Secrets — never commit it.

## Upstream Sync

A scheduled workflow attempts to merge `upstream/main` weekly. If it fails due to conflicts, it opens an issue. When resolving:

1. Run `pnpm axolotl:brand-guard` and `pnpm axolotl:i18n-check` after the merge
2. Review `scripts/axolotl/upstream-impact-report.mjs` output for sensitive lines
3. Never force-push over Axolotl commits

## Remote Commits

Before pushing, inspect changed paths. If the commit does **not** touch the desktop app (`apps/app/`, `apps/app-frontend/`) or its dependencies, include `[skip ci]` in the commit message to skip the Axolotl desktop CI. Never use `[skip ci]` for commits that affect the desktop app build.

## Code Guidelines

- No "heading" comments like `=== Helper methods ===`
- Doc comments are fine; inline comments only when absolutely necessary
- Types from `@modrinth/utils` are mostly outdated — prefer types from `packages/api-client`
- When fixing problems, don't deflect ("I didn't introduce these") — just fix them

## Key Env Vars

| Variable | Purpose |
| -------- | ------- |
| `SQLX_OFFLINE=true` | Must be set for CI builds (uses cached sqlx data) |
| `CURSEFORGE_API_KEY` or `AXOLOTL_CURSEFORGE_API_KEY` | CurseForge API access |
| `AXOLOTL_CURSEFORGE_API_BASE_URL` | Custom CurseForge mirror URL |

## Project-Specific Instructions

- `apps/labrinth/AGENTS.md` — Backend API conventions
- `apps/frontend/CLAUDE.md` (also `apps/frontend/AGENTS.md`) — Modrinth website
- `.claude/skills/` — Specialized skill workflows (i18n-pass, tanstack-query, etc.)
- `standards/` — Frontend and maintenance standards

## Bash Guidelines

- Do not pipe output through `head`, `tail`, `less`, or `more`
- Use command-specific flags instead (e.g. `git log -n 10`, not `git log | head -10`)
- Do not create new Bash/shell/SQL script files unless explicitly asked
