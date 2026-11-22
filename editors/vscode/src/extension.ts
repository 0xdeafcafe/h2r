// The H2 Rules extension: starts `h2 rules lsp` over stdio for `.rules`
// files. The server is crates/apps/cli/src/rules_lsp.rs.

import * as fs from 'fs';
import * as os from 'os';
import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';
import { resolveServerPath } from './serverPath';

const CLIENT_ID = 'h2rules';
const CLIENT_NAME = 'H2 Rules';

let client: LanguageClient | undefined;
let output: vscode.OutputChannel;
let traceOutput: vscode.OutputChannel;
// The watcher of the running client; a new one for each start.
let watcher: vscode.FileSystemWatcher | undefined;
// Starts and stops run one at a time, in order.
let queue: Promise<void> = Promise.resolve();

function isExecutable(file: string): boolean {
  try {
    if (!fs.statSync(file).isFile()) {
      return false;
    }
    if (process.platform !== 'win32') {
      fs.accessSync(file, fs.constants.X_OK);
    }
    return true;
  } catch {
    return false;
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function start(): Promise<void> {
  const setting = vscode.workspace.getConfiguration('h2rules').get<string>('serverPath', '');
  const lookup = resolveServerPath(
    setting,
    process.env.PATH ?? process.env.Path,
    os.homedir(),
    isExecutable,
    process.platform,
    process.env.PATHEXT,
  );
  if (!lookup.found) {
    const what = lookup.setting
      ? `h2rules.serverPath is set to "${lookup.setting}", which is not an executable file.`
      : 'Could not find the h2 binary on PATH or in ~/.cargo/bin.';
    output.appendLine(`${what} Looked in:`);
    for (const t of lookup.tried) {
      output.appendLine(`  ${t}`);
    }
    void vscode.window
      .showErrorMessage(`H2 Rules: ${what} Set h2rules.serverPath to the h2 binary.`, 'Open Settings', 'Show Output')
      .then((choice) => {
        if (choice === 'Open Settings') {
          void vscode.commands.executeCommand('workbench.action.openSettings', 'h2rules.serverPath');
        } else if (choice === 'Show Output') {
          output.show(true);
        }
      });
    return;
  }

  output.appendLine(`Starting ${lookup.path} rules lsp (found by ${lookup.source}).`);
  const rulesWatcher = vscode.workspace.createFileSystemWatcher('**/*.rules');
  watcher = rulesWatcher;
  const serverOptions: ServerOptions = {
    command: lookup.path,
    args: ['rules', 'lsp'],
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'rules' },
      { scheme: 'untitled', language: 'rules' },
    ],
    outputChannel: output,
    traceOutputChannel: traceOutput,
    synchronize: {
      fileEvents: rulesWatcher,
    },
  };
  // The id makes the client read `h2rules.trace.server` itself.
  const next = new LanguageClient(CLIENT_ID, CLIENT_NAME, serverOptions, clientOptions);
  client = next;
  try {
    await next.start();
  } catch (err) {
    output.appendLine(`The language server failed to start: ${message(err)}`);
    void vscode.window
      .showErrorMessage(`H2 Rules: the language server failed to start: ${message(err)}`, 'Show Output')
      .then((choice) => {
        if (choice === 'Show Output') {
          output.show(true);
        }
      });
  }
}

async function stop(): Promise<void> {
  const old = client;
  const oldWatcher = watcher;
  client = undefined;
  watcher = undefined;
  if (!old) {
    oldWatcher?.dispose();
    return;
  }
  try {
    await old.stop();
  } catch (err) {
    output.appendLine(`Stopping the language server: ${message(err)}`);
  }
  oldWatcher?.dispose();
}

function enqueue(job: () => Promise<void>): Promise<void> {
  queue = queue.then(job, job);
  return queue;
}

function restart(): Promise<void> {
  return enqueue(async () => {
    output.appendLine('Restarting the language server.');
    await stop();
    await start();
  });
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel('H2 Rules');
  traceOutput = vscode.window.createOutputChannel('H2 Rules Trace');
  context.subscriptions.push(
    output,
    traceOutput,
    vscode.commands.registerCommand('h2rules.restartServer', () => restart()),
    vscode.commands.registerCommand('h2rules.showOutput', () => output.show(true)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('h2rules.serverPath')) {
        void restart();
      }
    }),
  );
  await enqueue(start);
}

export function deactivate(): Promise<void> {
  return enqueue(stop);
}
