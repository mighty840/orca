# orca docs

The documentation site is an Astro project. **Content lives in
`src/content/docs/`** — this is the single source the site builds from
(`getCollection('docs')` in `src/pages/[...slug].astro`). Edit there.

- `src/content/docs/guide/` — user guides
- `src/content/docs/reference/` — CLI / API / self-healing reference
- `src/content/docs/legal/` — impressum, privacy

Build: `npm run build` (outputs `dist/`). Dev: `npm run dev`.

> A duplicate top-level `docs/guide/` + `docs/reference/` tree used to exist
> and silently diverged (the site froze at v0.2.11 while people edited the
> unpublished copy). It was consolidated into `src/content/docs/` on
> 2026-07-20 — do not recreate it.
