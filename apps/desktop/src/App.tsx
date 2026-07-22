import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type { PingResult } from "@xcoding/protocol";

const unavailable = "Open this shell through Tauri to connect it to the Rust core.";

export function App() {
  const [status, setStatus] = useState<string>(unavailable);

  async function verifyCore(): Promise<void> {
    try {
      const result = await invoke<PingResult>("ping");
      setStatus(`XCoding core ${result.version} is ready.`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <main className="app-shell">
      <header>
        <div>
          <p className="eyebrow">XCoding</p>
          <h1>Local coding agent</h1>
        </div>
        <button type="button" onClick={verifyCore}>
          Verify core
        </button>
      </header>
      <section aria-label="Core status">
        <p className="label">Core connection</p>
        <p className="status">{status}</p>
      </section>
      <section className="placeholder" aria-label="Phase zero scope">
        <p className="label">Phase 0</p>
        <p>Protocol, session storage, and client-to-core connectivity are in place.</p>
      </section>
    </main>
  );
}
