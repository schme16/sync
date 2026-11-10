// Browser-only mock of the backend, used for UI development (`npm run dev` in a normal browser).
import type { ActivityItem, Config, PairConfig, Snapshot } from "./api";
import { DEFAULT_RULES } from "./defaults";

const listeners = new Map<string, Set<(p: unknown) => void>>();
const emit = (ev: string, p: unknown) => listeners.get(ev)?.forEach((cb) => cb(p));

const pairs: PairConfig[] = [
  {
    id: "p1",
    name: "Projects",
    accountId: "a1",
    localPath: "C:\\Users\\me\\Projects",
    remoteFolderId: "f1",
    remotePathDisplay: "My Drive/Projects",
    driveId: null,
    mode: "twoWay",
    propagateDeletes: true,
    paused: false,
    useGlobalIgnore: true,
    ignore: "# Build output\ndist/\n*.tmp\n!keep.tmp\n",
    mounts: [{ localPath: "D:\\Notes", relPath: "Notes" }],
  },
  {
    id: "p2",
    name: "Photos backup",
    accountId: "a1",
    localPath: "D:\\Photos",
    remoteFolderId: "f2",
    remotePathDisplay: "My Drive/Backups/Photos",
    driveId: null,
    mode: "localToRemote",
    propagateDeletes: false,
    paused: false,
    useGlobalIgnore: true,
    ignore: "",
    mounts: [],
  },
  {
    id: "p3",
    name: "Shared docs",
    accountId: "a1",
    localPath: "C:\\Users\\me\\Shared",
    remoteFolderId: "f3",
    remotePathDisplay: "Team Drive/Docs",
    driveId: "d1",
    mode: "remoteToLocal",
    propagateDeletes: true,
    paused: true,
    useGlobalIgnore: true,
    ignore: "",
    mounts: [],
  },
];

const config: Config = {
  theme: "auto",
  launchAtStartup: true,
  startMinimized: false,
  maxConcurrentTransfers: 6,
  remotePollSecs: 20,
  fullRescanMins: 60,
  debounceMs: 1500,
  useTrash: true,
  deleteSafetyThreshold: 500,
  lowPriorityIo: true,
  globalIgnore: DEFAULT_RULES,
  googleClientId: "1234-example.apps.googleusercontent.com",
  googleClientSecret: "secret",
  checkForUpdates: true,
  skippedUpdateVersion: "",
  accounts: [{ id: "a1", email: "you@example.com", displayName: "Alex Example", photoLink: "" }],
  pairs,
};

const now = Date.now();
let snapshot: Snapshot = {
  pausedAll: false,
  transfers: [
    { id: 1, pairId: "p1", path: "web-app/assets/hero@2x.png", upload: true, total: 4_800_000, done: 2_100_000, startedMs: now - 4000 },
    { id: 2, pairId: "p1", path: "api/.git/objects/pack/pack-9f2e.pack", upload: false, total: 38_000_000, done: 9_400_000, startedMs: now - 9000 },
    { id: 3, pairId: "p2", path: "2026/Iceland/DSC_0142.NEF", upload: true, total: 26_000_000, done: 24_000_000, startedMs: now - 15000 },
  ],
  pairs: {
    p1: {
      state: "syncing",
      message: "Syncing — 42 remaining",
      lastSyncMs: now - 60_000,
      localItems: 18_342,
      remoteItems: 18_301,
      queued: 42,
      error: null,
      recentErrors: 0,
      pendingDeletes: null,
      warnings: ['Drive has 2 items named "Notes/todo.md"; only one is synced'],
      ruleErrors: [],
    },
    p2: {
      state: "needsConfirmation",
      message: "Large deletion waiting for confirmation",
      lastSyncMs: now - 3_600_000,
      localItems: 52_110,
      remoteItems: 52_960,
      queued: 0,
      error: null,
      recentErrors: 1,
      pendingDeletes: { local: 0, remote: 850, sample: ["2025/Japan", "2025/Wedding", "2024/Misc/IMG_0001.jpg"] },
      warnings: [],
      ruleErrors: [],
    },
    p3: {
      state: "paused",
      message: "Paused",
      lastSyncMs: now - 86_400_000,
      localItems: 912,
      remoteItems: 912,
      queued: 0,
      error: null,
      recentErrors: 0,
      pendingDeletes: null,
      warnings: [],
      ruleErrors: [],
    },
  },
};

const kinds: ActivityItem["kind"][] = ["upload", "download", "moveRemote", "deleteRemote", "mkdir", "conflict", "upload", "download", "error", "info"];
const paths = ["web-app/src/App.svelte", "api/.git/index", "notes/ideas.md", "web-app/package.json", "Notes/Groceries.md", "design/logo.fig", "README.md"];
const activity: ActivityItem[] = Array.from({ length: 40 }, (_, i) => {
  const kind = kinds[i % kinds.length];
  return {
    id: 1000 - i,
    ts: now - i * 47_000,
    pairId: i % 3 === 0 ? "p2" : "p1",
    kind,
    path: paths[i % paths.length],
    detail:
      kind === "error"
        ? "Drive API error 403: The user's Drive storage quota has been exceeded."
        : kind === "conflict"
          ? 'Changed on both sides; local copy kept as "ideas (conflict 2026-11-10 101500 DESKTOP).md"'
          : kind === "moveRemote"
            ? "Moved in Drive from notes/old-ideas.md"
            : kind === "info"
              ? 'Connected to "My Drive/Projects"'
              : kind === "deleteRemote"
                ? "Moved to Drive trash"
                : kind === "mkdir"
                  ? "Folder created in Drive"
                  : kind === "upload"
                    ? "Uploaded"
                    : "Downloaded",
  };
});

setInterval(() => {
  snapshot = {
    ...snapshot,
    transfers: snapshot.transfers.map((t) => ({ ...t, done: t.done >= t.total ? 0 : Math.min(t.total, t.done + t.total * 0.03) })),
  };
  emit("status", snapshot);
}, 500);

export function mockListen(event: string, cb: (p: unknown) => void) {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event)!.add(cb);
  return () => listeners.get(event)!.delete(cb);
}

const mockUpdate = {
  version: "0.2.0",
  notes: "## What's new\n- Update checks and one-click updates\n- Faster first scan on large folders",
  url: "https://github.com/schme16/sync/releases",
  publishedAt: new Date().toISOString(),
  assetName: "Sync_0.2.0_x64-setup.exe",
  assetSize: 5_170_000,
  manualReason: null,
};
const update = { current: "0.1.0", checking: false, lastCheckedMs: null as number | null, available: mockUpdate as typeof mockUpdate | null, progress: null as [number, number] | null, installing: false, error: null as string | null };
const pushUpdate = () => emit("update", structuredClone(update));

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function mockInvoke<T>(cmd: string, args: Record<string, any> = {}): Promise<T> {
  await sleep(60);
  switch (cmd) {
    case "get_config":
      return structuredClone(config) as T;
    case "save_settings":
      Object.assign(config, args.patch);
      return structuredClone(config) as T;
    case "get_snapshot":
      return structuredClone(snapshot) as T;
    case "get_activity":
      return structuredClone(activity) as T;
    case "app_info":
      return { version: "0.1.0", os: "windows", configDir: "C:\\Users\\me\\AppData\\Roaming\\schme16-sync", dataDir: "C:\\Users\\me\\AppData\\Local\\schme16-sync", logDir: "C:\\Users\\me\\AppData\\Local\\schme16-sync\\logs", hasBuiltinCredentials: false } as T;
    case "get_update_state":
      return structuredClone(update) as T;
    case "check_for_updates":
      update.checking = true;
      pushUpdate();
      await sleep(900);
      Object.assign(update, { checking: false, lastCheckedMs: Date.now(), available: mockUpdate });
      pushUpdate();
      return structuredClone(update) as T;
    case "skip_update":
      config.skippedUpdateVersion = args.version;
      update.available = null;
      pushUpdate();
      return undefined as T;
    case "install_update":
      for (let done = 0; done <= mockUpdate.assetSize; done += 650_000) {
        update.progress = [Math.min(done, mockUpdate.assetSize), mockUpdate.assetSize];
        pushUpdate();
        await sleep(250);
      }
      update.installing = true;
      pushUpdate();
      return undefined as T;
    case "drive_root":
      return { id: "root", name: "My Drive", driveId: null } as T;
    case "list_shared_drives":
      return [{ id: "d1", name: "Team Drive", driveId: "d1" }] as T;
    case "list_drive_folders":
      return ["Archive", "Backups", "Documents", "Photos", "Projects", "Receipts"].map((n, i) => ({ id: `${args.parentId}-${i}`, name: n, driveId: null })) as T;
    case "browse_pair":
      return [
        { name: "api", path: "api", isDir: true, local: true, remoteId: "x1", ignored: false, rule: null },
        { name: "design", path: "design", isDir: true, local: false, remoteId: "x2", ignored: true, rule: "/design/" },
        { name: "dist", path: "dist", isDir: true, local: true, remoteId: null, ignored: true, rule: "dist/" },
        { name: "Notes", path: "Notes", isDir: true, local: true, remoteId: "x3", ignored: false, rule: null },
        { name: "web-app", path: "web-app", isDir: true, local: true, remoteId: "x4", ignored: false, rule: null },
      ].map((e) => ({ ...e, path: args.path ? `${args.path}/${e.path}` : e.path })) as T;
    case "test_ignore": {
      const p: string = args.path || "";
      const ignored = /node_modules|\.tmp$|dist\//.test(p);
      return { result: { ignored, rule: ignored ? "node_modules/" : null, source: ignored ? "global" : null }, errors: [] } as T;
    }
    case "add_pair":
    case "update_pair": {
      const p = { ...args.pair, id: args.pair.id || `p${Date.now()}` };
      const i = config.pairs.findIndex((x) => x.id === p.id);
      if (i >= 0) config.pairs[i] = p;
      else config.pairs.push(p);
      return p as T;
    }
    case "remove_pair":
      config.pairs = config.pairs.filter((p) => p.id !== args.id);
      return undefined as T;
    case "add_account":
      await sleep(1200);
      throw "Add your Google API client ID and secret in Settings first (see the setup guide).";
    default:
      return undefined as T;
  }
}
