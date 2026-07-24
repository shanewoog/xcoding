import readline from "node:readline";

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

function write(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

for await (const line of rl) {
  const trimmed = line.trim();
  if (!trimmed) {
    continue;
  }
  let message;
  try {
    message = JSON.parse(trimmed);
  } catch {
    continue;
  }

  if (message.method === "initialize") {
    write({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion: "2024-11-05",
        capabilities: { tools: {} },
        serverInfo: { name: "demo", version: "0.0.1" },
      },
    });
    continue;
  }

  if (message.method === "notifications/initialized") {
    continue;
  }

  if (message.method === "tools/list") {
    write({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        tools: [
          {
            name: "echo",
            description: "Echo text back for MCP e2e",
            inputSchema: {
              type: "object",
              properties: {
                text: { type: "string", description: "Text to echo" },
              },
              required: ["text"],
            },
          },
        ],
      },
    });
    continue;
  }

  if (message.method === "tools/call") {
    const text = message.params?.arguments?.text ?? "";
    write({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        content: [{ type: "text", text: `echo:${text}` }],
        isError: false,
      },
    });
    continue;
  }

  if (message.id != null) {
    write({
      jsonrpc: "2.0",
      id: message.id,
      error: { code: -32601, message: `Method not found: ${message.method}` },
    });
  }
}
