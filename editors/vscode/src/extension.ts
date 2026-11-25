// The H2 Rules and HaloScript extension: starts `h2 rules lsp` for `.rules`
// files and `h2 hs lsp` for `.hsc` files over stdio, and sends the Studio
// commands each server offers (`workspace/executeCommand`). The servers are
// crates/apps/h2-tools/src/rules_lsp.rs and hs_lsp.rs; the Studio link is
// crates/apps/h2-tools/src/studio_link.rs.

import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as vscode from 'vscode';
import {
  ExecuteCommandRequest,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from 'vscode-languageclient/node';
import { resolveServerPath } from './serverPath';

/** One language server the extension runs. */
interface Spec {
  /** Also the settings section: `<id>.trace.server`. */
  id: string;
  name: string;
  args: string[];
  language: string;
  /** Globs the server is told about through `didChangeWatchedFiles`. */
  watch: string[];
}

interface Server {
  spec: Spec;
  output: vscode.OutputChannel;
  traceOutput: vscode.OutputChannel;
  client?: LanguageClient;
  // The watchers of the running client; new ones for each start.
  watchers: vscode.FileSystemWatcher[];
}

const RULES: Spec = {
  id: 'h2rules',
  name: 'H2 Rules',
  args: ['rules', 'lsp'],
  language: 'rules',
  watch: ['**/*.rules'],
};

const HS: Spec = {
  id: 'h2hs',
  name: 'H2 HaloScript',
  args: ['hs', 'lsp'],
  language: 'haloscript',
  watch: ['**/h2hs.json', '**/*.hsc'],
};

let rules: Server;
let hs: Server;
let extension: vscode.ExtensionContext;
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

function appendAll(servers: Server[], line: string): void {
  for (const s of servers) {
    s.output.appendLine(line);
  }
}

/** Starts the given servers on one lookup of the h2 binary. */
async function start(servers: Server[]): Promise<void> {
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
    appendAll(servers, `${what} Looked in:`);
    for (const t of lookup.tried) {
      appendAll(servers, `  ${t}`);
    }
    void vscode.window
      .showErrorMessage(`H2: ${what} Set h2rules.serverPath to the h2 binary.`, 'Open Settings', 'Show Output')
      .then((choice) => {
        if (choice === 'Open Settings') {
          void vscode.commands.executeCommand('workbench.action.openSettings', 'h2rules.serverPath');
        } else if (choice === 'Show Output') {
          servers[0].output.show(true);
        }
      });
    return;
  }

  for (const server of servers) {
    const { spec, output } = server;
    output.appendLine(`Starting ${lookup.path} ${spec.args.join(' ')} (found by ${lookup.source}).`);
    server.watchers = spec.watch.map((glob) => vscode.workspace.createFileSystemWatcher(glob));
    const serverOptions: ServerOptions = {
      command: lookup.path,
      args: spec.args,
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        { scheme: 'file', language: spec.language },
        { scheme: 'untitled', language: spec.language },
      ],
      outputChannel: output,
      traceOutputChannel: server.traceOutput,
      synchronize: {
        fileEvents: server.watchers,
      },
    };
    // The id makes the client read `<id>.trace.server` itself.
    const next = new LanguageClient(spec.id, spec.name, serverOptions, clientOptions);
    server.client = next;
    try {
      await next.start();
    } catch (err) {
      output.appendLine(`The language server failed to start: ${message(err)}`);
      void vscode.window
        .showErrorMessage(`${spec.name}: the language server failed to start: ${message(err)}`, 'Show Output')
        .then((choice) => {
          if (choice === 'Show Output') {
            output.show(true);
          }
        });
    }
  }
}

async function stop(servers: Server[]): Promise<void> {
  for (const server of servers) {
    const old = server.client;
    const oldWatchers = server.watchers;
    server.client = undefined;
    server.watchers = [];
    if (old) {
      try {
        await old.stop();
      } catch (err) {
        server.output.appendLine(`Stopping the language server: ${message(err)}`);
      }
    }
    for (const w of oldWatchers) {
      w.dispose();
    }
  }
}

function enqueue(job: () => Promise<void>): Promise<void> {
  queue = queue.then(job, job);
  return queue;
}

function restart(servers: Server[]): Promise<void> {
  return enqueue(async () => {
    appendAll(servers, 'Restarting the language server.');
    await stop(servers);
    await start(servers);
  });
}

/** The active editor's document, if it is in `language`. */
function activeDocument(language: string, what: string): vscode.TextEditor | undefined {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== language) {
    void vscode.window.showErrorMessage(`${what}: open a .${language === 'rules' ? 'rules' : 'hsc'} file first.`);
    return undefined;
  }
  return editor;
}

/** Sends `workspace/executeCommand`; shows the server's error and returns undefined on failure. */
async function execute<T>(server: Server, command: string, args: unknown): Promise<T | undefined> {
  const client = server.client;
  if (!client) {
    void vscode.window.showErrorMessage(`${server.spec.name}: the language server is not running.`);
    return undefined;
  }
  try {
    return (await client.sendRequest(ExecuteCommandRequest.type, {
      command,
      arguments: [args],
    })) as T;
  } catch (err) {
    server.output.appendLine(`${command}: ${message(err)}`);
    void vscode.window.showErrorMessage(message(err));
    return undefined;
  }
}

interface ReloadResult {
  message: string;
}

interface ScenarioResult {
  message: string;
  ok: boolean;
  text: string;
}

interface Position {
  line: number;
  character: number;
}

interface UsesResult {
  message: string;
  kind: string;
  name: string;
  uses: { name: string; item: string; count: number; range: { start: Position; end: Position } | null }[];
}

async function reloadInGame(): Promise<void> {
  const editor = activeDocument('rules', 'H2: Reload Script in Game');
  if (!editor) {
    return;
  }
  const doc = editor.document;
  if (doc.isDirty && !(await doc.save())) {
    return;
  }
  const result = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: 'Reloading the script in the game' },
    () => execute<ReloadResult>(rules, 'h2.studio.reload', { uri: doc.uri.toString() }),
  );
  if (result) {
    void vscode.window.showInformationMessage(result.message);
  }
}

/** The scenario file to run: the last one for this script, or a fresh pick. */
async function pickScenario(doc: vscode.TextDocument): Promise<string | undefined> {
  const key = `h2rules.scenario:${doc.uri.toString()}`;
  const last = extension.workspaceState.get<string>(key);
  if (last && fs.existsSync(last)) {
    const run = `Run ${path.basename(last)}`;
    const choose = 'Choose a file...';
    const picked = await vscode.window.showQuickPick([run, choose], {
      placeHolder: last,
    });
    if (picked === undefined) {
      return undefined;
    }
    if (picked === run) {
      return last;
    }
  }
  const files = await vscode.window.showOpenDialog({
    canSelectMany: false,
    openLabel: 'Run Scenario',
    filters: { JSON: ['json'] },
    defaultUri: vscode.Uri.joinPath(doc.uri, '..'),
  });
  const file = files?.[0]?.fsPath;
  if (file) {
    await extension.workspaceState.update(key, file);
  }
  return file;
}

async function runScenario(): Promise<void> {
  const editor = activeDocument('rules', 'H2: Run Scenario Test');
  if (!editor) {
    return;
  }
  const doc = editor.document;
  const scenario = await pickScenario(doc);
  if (!scenario) {
    return;
  }
  const result = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: `Running ${path.basename(scenario)}` },
    () =>
      execute<ScenarioResult>(rules, 'h2.studio.runScenario', {
        uri: doc.uri.toString(),
        source: doc.getText(),
        scenario,
      }),
  );
  if (!result) {
    return;
  }
  rules.output.appendLine(`--- Scenario ${scenario} ---`);
  rules.output.appendLine(result.text);
  rules.output.show(true);
  if (result.ok) {
    void vscode.window.showInformationMessage(result.message);
  } else {
    void vscode.window.showWarningMessage(result.message);
  }
}

function positionParams(editor: vscode.TextEditor): { uri: string; position: Position } {
  const at = editor.selection.active;
  return { uri: editor.document.uri.toString(), position: { line: at.line, character: at.character } };
}

async function showUses(): Promise<void> {
  const editor = activeDocument('haloscript', 'H2: Show Uses in Map');
  if (!editor) {
    return;
  }
  const result = await execute<UsesResult>(hs, 'h2.studio.uses', positionParams(editor));
  if (!result) {
    return;
  }
  if (result.uses.length === 0) {
    void vscode.window.showInformationMessage(result.message);
    return;
  }
  const items = result.uses.map((use) => ({
    label: `${use.name}  x${use.count}`,
    description: use.item,
    range: use.range,
  }));
  const picked = await vscode.window.showQuickPick(items, { placeHolder: result.message });
  if (!picked?.range) {
    return;
  }
  const range = new vscode.Range(
    picked.range.start.line,
    picked.range.start.character,
    picked.range.end.line,
    picked.range.end.character,
  );
  editor.selection = new vscode.Selection(range.start, range.end);
  editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
}

async function showInStudio(): Promise<void> {
  const editor = activeDocument('haloscript', 'H2: Show in Studio');
  if (!editor) {
    return;
  }
  const result = await execute<{ message: string }>(hs, 'h2.studio.show', positionParams(editor));
  if (result) {
    void vscode.window.showInformationMessage(result.message);
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  extension = context;
  rules = {
    spec: RULES,
    output: vscode.window.createOutputChannel(RULES.name),
    traceOutput: vscode.window.createOutputChannel(`${RULES.name} Trace`),
    watchers: [],
  };
  hs = {
    spec: HS,
    output: vscode.window.createOutputChannel(HS.name),
    traceOutput: vscode.window.createOutputChannel(`${HS.name} Trace`),
    watchers: [],
  };
  context.subscriptions.push(
    rules.output,
    rules.traceOutput,
    hs.output,
    hs.traceOutput,
    vscode.commands.registerCommand('h2rules.restartServer', () => restart([rules])),
    vscode.commands.registerCommand('h2rules.showOutput', () => rules.output.show(true)),
    vscode.commands.registerCommand('h2rules.reloadInGame', () => reloadInGame()),
    vscode.commands.registerCommand('h2rules.runScenario', () => runScenario()),
    vscode.commands.registerCommand('h2hs.restartServer', () => restart([hs])),
    vscode.commands.registerCommand('h2hs.showUses', () => showUses()),
    vscode.commands.registerCommand('h2hs.showInStudio', () => showInStudio()),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('h2rules.serverPath')) {
        void restart([rules, hs]);
      }
    }),
  );
  await enqueue(() => start([rules, hs]));
}

export function deactivate(): Promise<void> {
  return enqueue(() => (rules && hs ? stop([rules, hs]) : Promise.resolve()));
}
