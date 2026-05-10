import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

function findServerBinary(context: vscode.ExtensionContext): string {
  // Preference order:
  //   1. bin/gams-lsp-server  — bundled inside a packaged .vsix
  //   2. ../target/release/   — release dev build
  //   3. ../target/debug/     — debug dev build
  const candidates = [
    context.asAbsolutePath(path.join("bin", "gams-lsp-server")),
    context.asAbsolutePath(path.join("..", "target", "release", "gams-lsp-server")),
    context.asAbsolutePath(path.join("..", "target", "debug", "gams-lsp-server")),
  ];
  for (const bin of candidates) {
    if (fs.existsSync(bin)) {
      return bin;
    }
  }
  throw new Error(
    `gams-lsp-server binary not found. Run 'cargo build -p gams-lsp-server' first.\n` +
    `Searched:\n${candidates.map((c) => `  ${c}`).join("\n")}`
  );
}

export function activate(context: vscode.ExtensionContext): void {
  let serverBin: string;
  try {
    serverBin = findServerBinary(context);
  } catch (err) {
    void vscode.window.showErrorMessage(String(err));
    return;
  }

  const serverOptions: ServerOptions = {
    command: serverBin,
    args: [],
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "gams" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.gms"),
    },
  };

  client = new LanguageClient(
    "gams-lsp",
    "GAMS Language Server",
    serverOptions,
    clientOptions
  );

  client.start();
  context.subscriptions.push(client);
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
