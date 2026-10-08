import { onBeforeUnmount, onMounted, watch } from "vue";
import { ask } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Composer } from "vue-i18n";
import { useRoute } from "vue-router";

import { androidShareTake } from "@/api";
import { useOverlayStore } from "@/stores/overlay";
import { useSettingsStore } from "@/stores/settings";

const NHENTAI_GALLERY_LINK = /(?:^|[^a-z0-9.-])(?:https?:\/\/)?(?:www\.)?nhentai\.net\/g\/(\d+)(?:[/?#][^\s]*)?/i;

export function extractNhentaiGalleryId(text: string): number | null {
  const match = NHENTAI_GALLERY_LINK.exec(text);
  if (!match) return null;
  const id = Number(match[1]);
  return Number.isSafeInteger(id) && id > 0 ? id : null;
}

export function useClipboardGalleryReader(i18n: Composer): void {
  const settings = useSettingsStore();
  const overlay = useOverlayStore();
  const route = useRoute();
  let unlisten: (() => void) | null = null;
  let checking = false;
  let processingShare = false;
  let suppressClipboardUntil = 0;
  let lastPromptedGalleryId: number | null = null;

  async function inspectClipboard(): Promise<void> {
    if (
      !settings.loaded
      || !settings.settings.clipboard_link_reader
      || checking
      || processingShare
    ) return;

    checking = true;
    try {
      const galleryId = extractNhentaiGalleryId(await readText());
      if (galleryId === null) {
        lastPromptedGalleryId = null;
        return;
      }
      const showingRoutedGallery = route.name === "gallery"
        && Number(route.params.id) === galleryId;
      if (
        galleryId === lastPromptedGalleryId
        || overlay.galleryId === galleryId
        || showingRoutedGallery
      ) return;

      // Record before opening the native dialog: closing it can emit another
      // focus event, which must not create a second prompt.
      lastPromptedGalleryId = galleryId;
      const shouldOpen = await ask(
        i18n.t("clipboard.gallery_prompt", { id: galleryId }),
        {
          title: i18n.t("clipboard.gallery_found"),
          kind: "info",
          okLabel: i18n.t("clipboard.view_details"),
          cancelLabel: i18n.t("common.cancel"),
        },
      );
      if (shouldOpen) overlay.openGallery(galleryId);
    } catch (error) {
      // Clipboard access can be unavailable on unsupported platforms or while
      // another process owns it. A later focus event will retry naturally.
      console.warn("clipboard link inspection failed", error);
    } finally {
      checking = false;
    }
  }

  async function inspectSharedText(): Promise<boolean> {
    if (!settings.loaded) return false;
    if (processingShare) return true;

    processingShare = true;
    let receivedSharedText = false;
    try {
      const text = await androidShareTake();
      if (!text) return false;
      receivedSharedText = true;
      const galleryId = extractNhentaiGalleryId(text);
      if (galleryId === null) return true;
      lastPromptedGalleryId = galleryId;

      const shouldOpen = await ask(
        i18n.t("clipboard.shared_gallery_prompt", { id: galleryId }),
        {
          title: i18n.t("clipboard.gallery_found"),
          kind: "info",
          okLabel: i18n.t("clipboard.view_details"),
          cancelLabel: i18n.t("common.cancel"),
        },
      );
      if (shouldOpen) overlay.openGallery(galleryId);
      return true;
    } catch (error) {
      // Desktop returns no shared text; Android retries only when a new share
      // Intent arrives, so a transient bridge error remains harmless.
      console.warn("shared link inspection failed", error);
      return false;
    } finally {
      processingShare = false;
      if (receivedSharedText) {
        // Closing the native confirmation dialog focuses the app again. Keep
        // that synthetic focus transition from falling through to clipboard
        // inspection after the share has already been handled.
        suppressClipboardUntil = Date.now() + 1500;
      }
    }
  }

  function onAndroidShare(): void {
    void inspectSharedText();
  }

  async function onWindowFocused(): Promise<void> {
    // Android stores ACTION_SEND text before focusing the activity. Always
    // consume that first; clipboard inspection is only the fallback when this
    // focus was unrelated to sharing.
    const receivedSharedText = await inspectSharedText();
    if (receivedSharedText || Date.now() < suppressClipboardUntil) return;
    await inspectClipboard();
  }

  onMounted(async () => {
    window.addEventListener("nclientv4:android-share", onAndroidShare);
    unlisten = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) void onWindowFocused();
    });
  });

  watch(
    () => settings.loaded,
    (loaded) => {
      if (loaded) void inspectSharedText();
    },
    { immediate: true },
  );

  onBeforeUnmount(() => {
    window.removeEventListener("nclientv4:android-share", onAndroidShare);
    unlisten?.();
    unlisten = null;
  });
}
