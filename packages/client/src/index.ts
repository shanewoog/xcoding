import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { dirname } from "node:path";
import { mkdir } from "node:fs/promises";
import {
  JSON_RPC_VERSION,
  isJsonRpcFailure,
  type JsonRpcRequest,
  type JsonRpcResponse,
} from "@xcoding/protocol";

export interface StdioRpcClientOptions {
  serverPath: string;
  databasePath: string;
}

export class StdioRpcClient {
  private readonly process: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<
    number,
    {
      resolve: (value: unknown) => void;
      reject: (reason: Error) => void;
    }
  >();
  private buffer = "";
  private nextId = 1;
  private closed = false;

  private constructor(process: ChildProcessWithoutNullStreams) {
    this.process = process;
    process.stdout.setEncoding("utf8");
    process.stdout.on("data", (chunk: string) => this.handleOutput(chunk));
    process.stderr.setEncoding("utf8");
    process.stderr.on("data", (chunk: string) => {
      // stderr is reserved for server diagnostics. Requests surface a useful error on exit.
      this.lastDiagnostic = chunk.trim();
    });
    process.once("error", (error) => this.rejectPending(error));
    process.once("exit", () => this.rejectPending(new Error(this.exitMessage())));
  }

  private lastDiagnostic = "";

  static async start(options: StdioRpcClientOptions): Promise<StdioRpcClient> {
    await mkdir(dirname(options.databasePath), { recursive: true });
    const process = spawn(options.serverPath, ["--db", options.databasePath], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });

    return new StdioRpcClient(process);
  }

  async request<TResult>(method: string, params: unknown): Promise<TResult> {
    if (this.closed) {
      throw new Error("XCoding core connection is closed");
    }

    const id = this.nextId++;
    const request: JsonRpcRequest = {
      jsonrpc: JSON_RPC_VERSION,
      id,
      method,
      params,
    };

    const response = new Promise<TResult>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
    });

    this.process.stdin.write(`${JSON.stringify(request)}\n`);
    return response;
  }

  async close(): Promise<void> {
    if (this.closed) {
      return;
    }

    this.closed = true;
    this.process.stdin.end();
    await new Promise<void>((resolve) => this.process.once("exit", () => resolve()));
  }

  private handleOutput(chunk: string): void {
    this.buffer += chunk;
    let newlineIndex = this.buffer.indexOf("\n");

    while (newlineIndex >= 0) {
      const line = this.buffer.slice(0, newlineIndex).trim();
      this.buffer = this.buffer.slice(newlineIndex + 1);
      newlineIndex = this.buffer.indexOf("\n");

      if (!line) {
        continue;
      }

      let response: JsonRpcResponse;
      try {
        response = JSON.parse(line) as JsonRpcResponse;
      } catch {
        this.rejectPending(new Error(`XCoding core returned invalid JSON: ${line}`));
        continue;
      }

      if (response.id === null) {
        if (isJsonRpcFailure(response)) {
          this.rejectPending(new Error(response.error.message));
        } else {
          this.rejectPending(new Error("XCoding core returned an invalid response ID"));
        }
        continue;
      }

      const pending = this.pending.get(response.id);
      if (!pending) {
        continue;
      }
      this.pending.delete(response.id);

      if (isJsonRpcFailure(response)) {
        pending.reject(new Error(`XCoding RPC ${response.error.code}: ${response.error.message}`));
      } else {
        pending.resolve(response.result);
      }
    }
  }

  private exitMessage(): string {
    return this.lastDiagnostic
      ? `XCoding core exited: ${this.lastDiagnostic}`
      : "XCoding core exited before responding";
  }

  private rejectPending(error: Error): void {
    for (const { reject } of this.pending.values()) {
      reject(error);
    }
    this.pending.clear();
  }
}
