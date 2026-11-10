import { api, on, setWindowTheme, type ActivityItem, type AppInfo, type Config, type Snapshot, type Theme, type UpdateState } from "./api";

export type View = "overview" | "folders" | "activity" | "rules" | "accounts" | "settings";

interface Toast {
  id: number;
  kind: "ok" | "error" | "info";
  text: string;
}

export const app = $state({
  ready: false,
  view: "overview" as View,
  config: null as Config | null,
  snapshot: { pairs: {}, transfers: [], pausedAll: false } as Snapshot,
  activity: [] as ActivityItem[],
  info: null as AppInfo | null,
  toasts: [] as Toast[],
  /** pair id being edited, "new", or null */
  editingPair: null as string | null,
  activityFilter: null as string | null,
  resolvedTheme: "light" as "light" | "dark",
  update: null as UpdateState | null,
  /** version whose banner was dismissed with "Later" this session */
  updateDismissed: null as string | null,
});

let toastId = 0;
export function toast(text: string, kind: Toast["kind"] = "info") {
  const id = ++toastId;
  app.toasts.push({ id, kind, text });
  setTimeout(() => (app.toasts = app.toasts.filter((t) => t.id !== id)), kind === "error" ? 7000 : 3500);
}

export function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

const media = window.matchMedia("(prefers-color-scheme: dark)");

export function applyTheme(theme: Theme) {
  const dark = theme === "dark" || (theme === "auto" && media.matches);
  app.resolvedTheme = dark ? "dark" : "light";
  document.documentElement.dataset.theme = app.resolvedTheme;
  setWindowTheme(theme);
}

media.addEventListener("change", () => app.config && applyTheme(app.config.theme));

export async function reloadConfig() {
  app.config = await api.getConfig();
}

export async function init() {
  const [config, snapshot, activity, info, update] = await Promise.all([
    api.getConfig(),
    api.getSnapshot(),
    api.getActivity(1500),
    api.appInfo(),
    api.getUpdateState(),
  ]);
  app.update = update;
  await on<UpdateState>("update", (u) => (app.update = u));
  app.config = config;
  app.snapshot = snapshot;
  app.activity = activity;
  app.info = info;
  applyTheme(config.theme);
  await on<Snapshot>("status", (s) => (app.snapshot = s));
  await on<ActivityItem[]>("activity", (items) => {
    const next = [...items].reverse().concat(app.activity);
    app.activity = next.length > 2000 ? next.slice(0, 2000) : next;
  });
  if (config.accounts.length === 0) app.view = "accounts";
  app.ready = true;
}
