# React + TypeScript + Vite

This template provides a minimal setup to get React working in Vite with HMR and some ESLint rules.

## Theming (Claude Desktop look & feel)

The UI is reskinned to match Claude Desktop and is fully **theme-token driven**,
so the desktop app that wraps this same frontend inherits the look for free.

**Where the tokens live**

- `src/index.css` — the single source of truth. The `:root` block defines the
  **light** (warm cream) palette; `:root[data-theme="dark"]` overrides it with
  the **dark** (warm gray) palette. Both blocks declare the same variable names
  (`--color-bg`, `--color-surface`, `--color-primary`, `--color-text`, the
  semantic `--color-{success,warning,error,info}{,-soft,-text}` sets, radii,
  shadows, motion, fonts, and the `--hl-*` syntax-highlight tokens). The same
  file holds the global Markdown (`.md …`) and highlight.js token styles.
- Component styling stays in CSS Modules (`*.module.css`) and references those
  variables — no hard-coded colours — so re-palettes are one-file changes.

**To tweak the palette:** edit the coral accent (`--color-primary` and its
`-hover` / `-soft` / `-glow` companions) and the surface/text tokens in the two
blocks in `src/index.css`. Everything else follows automatically.

**Light/dark toggle**

- `src/hooks/useTheme.tsx` — `ThemeProvider` + `useTheme()`. Persists the
  preference (`light` | `dark` | `system`) to `localStorage["xenoclaw_theme"]`,
  follows the OS setting live when `system`, and writes `data-theme` on `<html>`.
- An inline script in `index.html` applies the saved/system theme before first
  paint to avoid a flash. The toggle lives in the sidebar footer.

**Markdown rendering** uses `react-markdown` + `remark-gfm` (GFM) +
`rehype-highlight` (syntax highlighting). Fenced code blocks get a per-block
copy button via `src/components/Markdown.tsx`. These are the only added
dependencies and back the "full Markdown / highlighted code" chat requirement.

---

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Expanding the ESLint configuration

If you are developing a production application, we recommend updating the configuration to enable type-aware lint rules:

```js
export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...

      // Remove tseslint.configs.recommended and replace with this
      tseslint.configs.recommendedTypeChecked,
      // Alternatively, use this for stricter rules
      tseslint.configs.strictTypeChecked,
      // Optionally, add this for stylistic rules
      tseslint.configs.stylisticTypeChecked,

      // Other configs...
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])
```

You can also install [eslint-plugin-react-x](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-x) and [eslint-plugin-react-dom](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-dom) for React-specific lint rules:

```js
// eslint.config.js
import reactX from 'eslint-plugin-react-x'
import reactDom from 'eslint-plugin-react-dom'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...
      // Enable lint rules for React
      reactX.configs['recommended-typescript'],
      // Enable lint rules for React DOM
      reactDom.configs.recommended,
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])
```
