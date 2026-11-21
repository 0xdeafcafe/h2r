// Monaco's core editor entry: the editor with its contributions (find,
// folding, snippets, suggest, hover, go to, references, quick outline) and
// none of the bundled languages or their workers. Its API is the package's
// (editor.api.d.ts); it ships no types of its own. stats/rulesMonaco.ts.
declare module "monaco-editor/esm/vs/editor/edcore.main" {
  export * from "monaco-editor";
}
