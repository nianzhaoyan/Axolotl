# Axolotl Launcher

Axolotl Launcher is a free, cross-platform Minecraft launcher maintained by Garbage Human Studio and developed by Mystic Stars.

This repository is a downstream fork of the Modrinth monorepo. Axolotl's product changes are limited to the desktop app in `apps/app-frontend`, `apps/app`, `packages/app-lib`, and the shared UI required by those packages. The Modrinth website and backend are not Axolotl products.

Axolotl Launcher is an independent, unofficial client using the public Modrinth API. It is not affiliated with or endorsed by Rinth, Inc.

## Development

Install pnpm, Rust, and the Tauri v2 prerequisites, then run:

```text
pnpm install
pnpm app:dev
```

## Upstream synchronization

The Modrinth repository is configured as the `upstream` remote. Fetch and merge it through a reviewed pull request; do not force-push upstream history over Axolotl changes.

## License

The desktop packages remain licensed under GPL-3.0-only. See each package's `LICENSE` and `COPYING.md` files for details.

Official website: https://www.ghs.red
