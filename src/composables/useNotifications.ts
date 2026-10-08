/**
 * Download notification coordinator.
 *
 * Only the item that is actually downloading owns the progress notification;
 * queued items stay silent. Android updates one fixed low-importance
 * notification, while Windows delegates to a native WinRT progress toast so
 * progress changes do not create a new popup.
 */
import {
  createChannel,
  Importance,
  isPermissionGranted,
  removeActive,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { platform } from "@tauri-apps/plugin-os";
import {
  windowsDownloadComplete,
  windowsDownloadProgress,
} from "@/api";
import type { DownloadProgress } from "@/types";
import { getLocale, LOCALE_MESSAGES, type AppLanguage } from "@/i18n";

const currentPlatform = platform();
const ACTIVE_DOWNLOAD_NOTIFICATION_ID = 0x4e434c54;
const ANDROID_PROGRESS_CHANNEL = "downloads-progress-v1";
const ANDROID_COMPLETE_CHANNEL = "downloads-complete-v1";
const ANDROID_NOTIFICATION_ICON = "ic_stat_nclientv4";

function tfn(key: string, params?: Record<string, unknown>): string {
  const locale = getLocale() as AppLanguage;
  const msgs = LOCALE_MESSAGES[locale] || LOCALE_MESSAGES.en;
  const parts = key.split(".");
  let val: unknown = msgs;
  for (const p of parts) val = (val as Record<string, unknown>)?.[p];
  if (typeof val !== "string") return key;
  if (!params) return val;
  return val.replace(/\{(\w+)\}/g, (_, k) => String(params[k] ?? `{${k}}`));
}

let permissionAsked = false;
let androidChannelsReady: Promise<void> | null = null;
let notificationsEnabled = true;

function ensureAndroidChannels(): Promise<void> {
  if (currentPlatform !== "android") return Promise.resolve();
  if (!androidChannelsReady) {
    androidChannelsReady = Promise.all([
      createChannel({
        id: ANDROID_PROGRESS_CHANNEL,
        name: tfn("notification.progress_channel"),
        description: tfn("notification.progress_channel_description"),
        importance: Importance.Low,
        vibration: false,
      }),
      createChannel({
        id: ANDROID_COMPLETE_CHANNEL,
        name: tfn("notification.complete_channel"),
        importance: Importance.Default,
        vibration: false,
      }),
    ]).then(() => undefined).catch(() => undefined);
  }
  return androidChannelsReady;
}

/** Ensure notification permission and Android channels are ready. */
export async function ensureNotificationPermission(): Promise<boolean> {
  try {
    let granted = await isPermissionGranted();
    if (!granted && !permissionAsked) {
      const permission = await requestPermission();
      granted = permission === "granted";
      permissionAsked = true;
    }
    if (granted) await ensureAndroidChannels();
    return granted;
  } catch {
    return false;
  }
}

interface NotificationOptions {
  ongoing?: boolean;
  autoCancel?: boolean;
  channelId?: string;
}

function notify(id: number, title: string, body: string, options: NotificationOptions = {}) {
  try {
    sendNotification({
      id,
      title,
      body,
      ongoing: options.ongoing,
      autoCancel: options.autoCancel,
      channelId: options.channelId,
      icon: ANDROID_NOTIFICATION_ICON,
      iconColor: "#5B8CFF",
    });
  } catch {
    // Notification plugin unavailable or blocked.
  }
}

function dismiss(id: number) {
  void removeActive([{ id }]).catch(() => undefined);
}

function percent(p: DownloadProgress): number {
  return p.total_pages > 0
    ? Math.min(100, Math.round((p.done_pages / p.total_pages) * 100))
    : 0;
}

function progressBody(p: DownloadProgress): string {
  return `${p.done_pages}/${p.total_pages} ${tfn("notification.pages")} (${percent(p)}%)`;
}

const finishedAnnounced = new Set<number>();
let activeDownloadId: number | null = null;
let lastPostedPct = -1;

/** Apply the user's notification preference and clear active plugin toasts. */
export async function configureDownloadNotifications(enabled: boolean) {
  notificationsEnabled = enabled;
  if (!enabled) {
    activeDownloadId = null;
    lastPostedPct = -1;
    if (currentPlatform !== "windows") {
      dismiss(ACTIVE_DOWNLOAD_NOTIFICATION_ID);
    }
    return;
  }
  await ensureNotificationPermission();
}

/** React to one backend download progress event. */
export async function handleDownloadNotification(p: DownloadProgress) {
  if (!notificationsEnabled) return;
  // Queue membership is visible in the Downloads screen; it should never
  // create an OS notification of its own.
  if (p.status === "pending") return;

  if (p.status === "finished") {
    if (finishedAnnounced.has(p.id)) return;
    finishedAnnounced.add(p.id);
    if (activeDownloadId === p.id) {
      activeDownloadId = null;
      lastPostedPct = -1;
    }

    const completion = tfn("notification.downloaded", { title: p.title });
    if (currentPlatform === "windows") {
      await windowsDownloadComplete(
        p.title,
        completion,
        tfn("notification.pages_saved", { n: p.total_pages }),
      ).catch(() => undefined);
      return;
    }

    dismiss(ACTIVE_DOWNLOAD_NOTIFICATION_ID);
    if (await ensureNotificationPermission()) {
      notify(
        p.id,
        tfn("notification.download_complete"),
        `${completion}\n${tfn("notification.pages_saved", { n: p.total_pages })}`,
        {
          autoCancel: true,
          channelId: currentPlatform === "android" ? ANDROID_COMPLETE_CHANNEL : undefined,
        },
      );
    }
    return;
  }

  if (p.status === "paused" || p.status === "canceled" || p.status === "failed") {
    if (activeDownloadId === p.id) {
      activeDownloadId = null;
      lastPostedPct = -1;
      if (currentPlatform !== "windows") dismiss(ACTIVE_DOWNLOAD_NOTIFICATION_ID);
    }
    return;
  }

  if (p.status !== "downloading") return;

  finishedAnnounced.delete(p.id);
  const initial = activeDownloadId !== p.id;
  if (initial) {
    activeDownloadId = p.id;
    lastPostedPct = -1;
  }

  const pct = percent(p);
  if (!initial && pct === lastPostedPct) return;
  lastPostedPct = pct;

  if (currentPlatform === "windows") {
    await windowsDownloadProgress(
      p.title,
      tfn("notification.progress_status"),
      progressBody(p),
      pct / 100,
      initial,
    ).catch(() => undefined);
    return;
  }

  if (!(await ensureNotificationPermission())) return;
  notify(
    ACTIVE_DOWNLOAD_NOTIFICATION_ID,
    tfn("notification.downloading", { title: p.title }),
    progressBody(p),
    {
      ongoing: true,
      channelId: currentPlatform === "android" ? ANDROID_PROGRESS_CHANNEL : undefined,
    },
  );
}
