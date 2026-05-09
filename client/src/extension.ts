import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

export function activate(context: vscode.ExtensionContext) {
  const serverBin = context.asAbsolutePath(
    path.join("..", "target", "debug", "gams-lsp-server")
  );

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
