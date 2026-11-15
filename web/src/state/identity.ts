// Who "you" are in this browser: accounts made here (IndexedDB), and which
// one is signed in, per server.

import { createRoot, createSignal, createMemo } from "solid-js";
import { deleteAccount, loadAccounts, saveAccount, type LocalAccount } from "./account";
import { live } from "./live";

const ACTIVE = "h2live.active";

export const identity = createRoot(() => {
  const [accounts, setAccounts] = createSignal<LocalAccount[]>([]);
  const [activeMap, setActiveMap] = createSignal<Record<string, string>>(read());

  function read(): Record<string, string> {
    try { return JSON.parse(localStorage.getItem(ACTIVE) ?? "{}"); } catch { return {}; }
  }
  function write(m: Record<string, string>) {
    setActiveMap(m);
    try { localStorage.setItem(ACTIVE, JSON.stringify(m)); } catch { /* private mode */ }
  }

  void loadAccounts().then(setAccounts, () => {});

  const here = createMemo(() => accounts().filter((a) => a.server === live.server().base));
  const me = createMemo(() => {
    const x = activeMap()[live.server().base];
    return here().find((a) => a.xuid === x) ?? null;
  });

  return {
    accounts: here,
    me,
    async add(a: LocalAccount) {
      await saveAccount(a).catch(() => {});
      setAccounts([...accounts().filter((x) => !(x.server === a.server && x.xuid === a.xuid)), a]);
      this.signIn(a);
    },
    signIn(a: LocalAccount) {
      write({ ...activeMap(), [a.server]: a.xuid });
    },
    signOut() {
      const m = { ...activeMap() };
      delete m[live.server().base];
      write(m);
    },
    async forget(a: LocalAccount) {
      await deleteAccount(a).catch(() => {});
      setAccounts(accounts().filter((x) => !(x.server === a.server && x.xuid === a.xuid)));
      if (me()?.xuid === a.xuid) this.signOut();
    },
  };
});

export function download(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
