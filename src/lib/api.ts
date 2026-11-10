// Typed bridge to the Rust backend. When opened in a plain browser (no Tauri), a small in-memory
// mock is used instead so the interface can be previewed and designed without the app running.

import type { UnlistenFn } from "@tauri-apps/api/event";

export type Theme = "auto" | "light" | "dark";
export type SyncMode = "twoWay" | "localToRemote" | "remoteToLocal";

export interface Account {
  id: string;
  email: string;
  displayName: string;
  photoLink: string;
}

export interface Mount {
  localPath: string;
  relPath: string;
}

export interface PairConfig {
  id: string;
  name: string;
  accountId: string;
  localPath: string;
  remoteFolderId: string;
  remotePathDisplay: string;
  driveId: string | null;
  mode: SyncMode;
  propagateDeletes: boolean;
  paused: boolean;
  useGlobalIgnore: boolean;
  ignore: string;
  mounts: Mount[];
}

export interface Config {
  theme: Theme;
  launchAtStartup: boolean;
  startMinimized: boolean;
  maxConcurrentTransfers: number;
  remotePollSecs: number;
  fullRescanMins: number;
  debounceMs: number;
  useTrash: boolean;
  deleteSafetyThreshold: number;
  lowPriorityIo: boolean;
  globalIgnore: string;
  googleClientId: string;
  googleClientSecret: string;
  accounts: Account[];
  pairs: PairConfig[];
}

export type PairState = "starting" | "scanning" | "syncing" | "idle" | "paused" | "offline" | "error" | "needsConfirmation";

export interface RuleError {
  line: number;
  message: string;
}

export interface PairStatus {
  state: PairState;
  message: string;
  lastSyncMs: number | null;
  localItems: number;
  remoteItems: number;
  queued: number;
  error: string | null;
  recentErrors: number;
  pendingDeletes: { local: number; remote: number; sample: string[] } | null;
  warnings: string[];
  ruleErrors: RuleError[];
}

export interface Transfer {
  id: number;
  pairId: string;
  path: string;
  upload: boolean;
  total: number;
  done: number;
  startedMs: number;
}

export interface Snapshot {
  pairs: Record<string, PairStatus>;
  transfers: Transfer[];
  pausedAll: boolean;
}

export type ActivityKind =
  | "upload"
  | "download"
  | "deleteLocal"
  | "deleteRemote"
  | "moveLocal"
  | "moveRemote"
  | "mkdir"
  | "conflict"
  | "error"
  | "warning"
  | "info";

export interface ActivityItem {
  id: number;
  ts: number;
  pairId: string;
  kind: ActivityKind;
  path: string;
  detail: string;
}

export interface FolderItem {
  id: string;
  name: string;
  driveId: string | null;
}

export interface BrowseEntry {
  name: string;
  path: string;
  isDir: boolean;
  local: boolean;
  remoteId: string | null;
  ignored: boolean;
  rule: string | null;
}

export interface IgnoreTest {
  result: { ignored: boolean; rule: string | null; source: string | null };
  errors: RuleError[];
}

export interface AppInfo {
  version: string;
  os: string;
  configDir: string;
  dataDir: string;
  logDir: string;
  hasBuiltinCredentials: boolean;
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) return (await import("./mock")).mockInvoke<T>(cmd, args);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri) return (await import("./mock")).mockListen(event, cb as (p: unknown) => void);
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(event, (e) => cb(e.payload));
}

export const api = {
  getConfig: () => call<Config>("get_config"),
  saveSettings: (patch: Partial<Config>) => call<Config>("save_settings", { patch }),
  getSnapshot: () => call<Snapshot>("get_snapshot"),
  getActivity: (limit = 1000) => call<ActivityItem[]>("get_activity", { limit }),
  clearActivity: () => call<void>("clear_activity"),
  addAccount: () => call<Account>("add_account"),
  cancelAddAccount: () => call<void>("cancel_add_account"),
  removeAccount: (id: string) => call<void>("remove_account", { id }),
  driveRoot: (accountId: string) => call<FolderItem>("drive_root", { accountId }),
  listDriveFolders: (accountId: string, parentId: string) => call<FolderItem[]>("list_drive_folders", { accountId, parentId }),
  listSharedDrives: (accountId: string) => call<FolderItem[]>("list_shared_drives", { accountId }),
  createDriveFolder: (accountId: string, parentId: string, name: string) => call<FolderItem>("create_drive_folder", { accountId, parentId, name }),
  addPair: (pair: PairConfig) => call<PairConfig>("add_pair", { pair }),
  updatePair: (pair: PairConfig) => call<PairConfig>("update_pair", { pair }),
  removePair: (id: string) => call<void>("remove_pair", { id }),
  setPairPaused: (id: string, paused: boolean) => call<void>("set_pair_paused", { id, paused }),
  setPausedAll: (paused: boolean) => call<void>("set_paused_all", { paused }),
  syncNow: (id?: string) => call<void>("sync_now", { id: id ?? null }),
  confirmDeletes: (id: string, apply: boolean) => call<void>("confirm_deletes", { id, apply }),
  testIgnore: (rules: string, useGlobal: boolean, path: string, isDir: boolean) => call<IgnoreTest>("test_ignore", { rules, useGlobal, path, isDir }),
  browsePair: (pairId: string, path: string, folderId: string | null) => call<BrowseEntry[]>("browse_pair", { pairId, path, folderId }),
  setFolderExcluded: (pairId: string, path: string, excluded: boolean) => call<PairConfig>("set_folder_excluded", { pairId, path, excluded }),
  openLocal: (pairId: string, path?: string) => call<void>("open_local", { pairId, path: path ?? null }),
  openInDrive: (pairId: string) => call<void>("open_in_drive", { pairId }),
  appInfo: () => call<AppInfo>("app_info"),
  openLogDir: () => call<void>("open_log_dir"),
};

export async function pickDirectory(title: string): Promise<string | null> {
  if (!isTauri) return window.prompt(title, "C:\\Users\\me\\Projects") || null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false, title });
  return typeof r === "string" ? r : null;
}

export async function openUrl(url: string) {
  if (!isTauri) {
    window.open(url, "_blank");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

export async function setWindowTheme(theme: Theme) {
  if (!isTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTheme(theme === "auto" ? null : theme).catch(() => {});
}
