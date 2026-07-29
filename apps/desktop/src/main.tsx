import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// The Rust side creates the WebView hidden. Wait for the first React frame so
// Windows never shows the empty WebView/desktop background during startup.
if ("__TAURI_INTERNALS__" in window) {
  requestAnimationFrame(() => {
    void invoke("show_main_window");
  });
}
