# Echo Frontend

React 19 + TypeScript + Vite frontend for the Echo Tauri 1 desktop app.

## Scripts

```bash
npm install
npm run lint
npm run build
```

`npm run build` writes static assets to `frontend/dist`, which the Tauri shell loads through `src-tauri/tauri.conf.json`.

## Notes

- Tauri IPC wrappers live in `src/api.ts`.
- App-level orchestration lives in `src/App.tsx`.
- Chat, sidebar, history search, forwarding, file UI, and group workflows live in `src/components/`.
- Theme tokens and runtime theme switching live in `src/theme.ts` and `src/index.css`.
