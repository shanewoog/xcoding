import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Relative base is required for Tauri production/portable builds.
// Absolute "/assets/..." paths fail under the custom asset protocol and show a blank window.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  base: "./",
  server: {
    port: 1420,
    strictPort: true,
  },
});
