// `bun run dev`: the mock server and Vite together; Vite proxies /v1,
// /healthz and /.well-known to the mock.

const procs = [
  Bun.spawn(["bun", "run", "mock/server.ts"], { stdout: "inherit", stderr: "inherit", env: process.env }),
  Bun.spawn(["bunx", "--bun", "vite"], { stdout: "inherit", stderr: "inherit", env: process.env }),
];

const stop = () => { for (const p of procs) p.kill(); process.exit(0); };
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
await Promise.race(procs.map((p) => p.exited));
stop();
export {};
