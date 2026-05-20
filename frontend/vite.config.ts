import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    // Tauri v1's custom protocol doesn't return CORS headers, so we must
    // strip the `crossorigin` attribute that Vite adds to module scripts.
    {
      name: "strip-crossorigin",
      transformIndexHtml(html) {
        return html.replace(/\s*crossorigin\b\s*/g, " ");
      },
    },
  ],
  base: "./",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: ["es2022", "chrome105", "safari16"],
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});