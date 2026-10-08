<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterView, useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import { onBackButtonPress } from "@tauri-apps/api/app";
import type { PluginListener } from "@tauri-apps/api/core";

import { ArrowLeft, Loader, Menu, Search as SearchIcon } from "@lucide/vue";
import AppSidebar from "@/components/AppSidebar.vue";
import GalleryView from "@/views/GalleryView.vue";
import ReaderView from "@/views/ReaderView.vue";
import LocalDetailView from "@/views/LocalDetailView.vue";
import LocalReaderView from "@/views/LocalReaderView.vue";
import { androidPrivacySet, cloudflareOpenChallenge, onDownloadProgress } from "@/api";
import { setLocale, detectPlatformLanguage, isValidLanguage } from "@/i18n";
import { useSettingsStore } from "@/stores/settings";
import { useDownloadsStore } from "@/stores/downloads";
import { useFavoritesStore } from "@/stores/favorites";
import { useTagsStore } from "@/stores/tags";
import { useOverlayStore } from "@/stores/overlay";
import { useReadProgressStore } from "@/stores/readProgress";
import { useDownloadedStore } from "@/stores/downloaded";
import { configureDownloadNotifications, handleDownloadNotification } from "@/composables/useNotifications";
import { useDraggablePosition } from "@/composables/useDraggablePosition";
import { useEdgeSwipe } from "@/composables/useEdgeSwipe";
import { useResponsiveSidebar } from "@/composables/useResponsiveSidebar";
import { useSwipeDismiss } from "@/composables/useSwipeDismiss";
import { applyTheme } from "@/composables/useTheme";
import { useClipboardGalleryReader } from "@/composables/useClipboardGalleryReader";

const settings = useSettingsStore();
const downloads = useDownloadsStore();
const favorites = useFavoritesStore();
const tags = useTagsStore();
const overlay = useOverlayStore();
const readProgress = useReadProgressStore();
const downloaded = useDownloadedStore();
const router = useRouter();
const route = useRoute();
const i18n = useI18n();
useClipboardGalleryReader(i18n);

const cloudflareBanner = ref(false);
const { isCompact, sidebarOpen, toggleSidebar } = useResponsiveSidebar();
const {
  panelRef: overlayPanelRef,
  panelStyle: overlayPanelStyle,
  onTouchStart: onOverlayTouchStart,
  onTouchMove: onOverlayTouchMove,
  onTouchEnd: onOverlayTouchEnd,
} = useSwipeDismiss(() => overlay.pop());
const {
  state: contentEdgeSwipe,
  onTouchStart: onContentTouchStart,
  onTouchMove: onContentTouchMove,
  onTouchEnd: onContentTouchEnd,
  onTouchCancel: onContentTouchCancel,
  onMouseDown: onContentMouseDown,
  onMouseMove: onContentMouseMove,
  onMouseUp: onContentMouseUp,
} = useEdgeSwipe(sidebarOpen, () => {
  sidebarOpen.value = true;
}, {
  enabled: isCompact,
  startWidth: 96,
  openThreshold: 0.4,
});

const canGoBack = computed(() => {
  if (overlay.hasAny()) return true;
  const detailRoutes = ["gallery", "reader", "reader-local"];
  return detailRoutes.includes(String(route.name));
});

watch(() => route.fullPath, () => {
  overlay.closeAll();
  // Auto-close the mobile drawer on navigation so the new view is fully
  // visible after picking a destination.
  if (isCompact.value) sidebarOpen.value = false;
});

watch(() => i18n.locale.value, (loc) => {
  document.documentElement.lang = loc;
}, { immediate: true });

watch(() => settings.settings.theme, (theme) => {
  applyTheme(theme);
}, { immediate: true });

onMounted(async () => {
  try {
    await settings.load();

    // Initialize app language: use saved preference, or auto-detect from platform.
    const savedLang = settings.settings.app_language;
    if (savedLang && isValidLanguage(savedLang)) {
      i18n.locale.value = savedLang;
      setLocale(savedLang);
    } else {
      const detected = await detectPlatformLanguage();
      i18n.locale.value = detected;
      setLocale(detected);
      await settings.save({ app_language: detected });
    }

    // Keep untouched translation defaults aligned with the UI language.
    // Translation requests report their own errors; startup must not probe the
    // configured provider or gate automatic translation on a stale status.
    await settings.syncTranslationTargetsForLanguage(i18n.locale.value);

    await Promise.allSettled([
      downloads.init(),
      favorites.load(),
      tags.load().then(() => tags.loadBlacklist()),
      readProgress.load(),
      downloaded.load(),
    ]);
    // Ask for notification permission only when the user has notifications
    // enabled. The preference also gates every later progress event.
    configureDownloadNotifications(settings.settings.notifications_enabled).catch(() => {});
    // FLAG_SECURE is applied by the native Android window. This is intentionally
    // not a web overlay: Android owns Recents previews and capture protection.
    androidPrivacySet(settings.settings.privacy_screen).catch(() => {});
    // Soft CF check on launch (best effort).
    const needed = await settings.checkCloudflare();
    cloudflareBanner.value = needed;
  } catch (e) {
    console.error("init failed", e);
  }

  // Watch download progress so we can (a) keep the "downloaded" cover badge in
  // sync the moment a gallery finishes on disk, and (b) post Android
  // notifications for progress + completion. Unsubscribed on teardown below.
  progressUnlisten = await onDownloadProgress(async (p) => {
    if (p.status === "finished") {
      // The backend indexes a finished gallery into the local library, so the
      // id is now on disk — optimistically flip the badge, then re-sync.
      downloaded.add(p.id);
      await downloaded.refresh();
    }
    // Notifications are guarded by the persisted user preference internally.
    handleDownloadNotification(p).catch(() => {});
  });
});

const floatingTopbar = computed(() => {
  const inFlowRoutes = ["gallery", "reader", "reader-local"];
  return !inFlowRoutes.includes(String(route.name));
});

watch(() => settings.settings.notifications_enabled, (enabled) => {
  if (settings.loaded) {
    configureDownloadNotifications(enabled).catch(() => {});
  }
});

watch(() => settings.settings.privacy_screen, (enabled) => {
  if (settings.loaded) {
    androidPrivacySet(enabled).catch(() => {});
  }
});

async function solveCloudflare() {
  await cloudflareOpenChallenge();
}


// Programmatic back triggered by the on-screen ← / Close buttons. On Android
// this runs inside the SPA only; the hardware back button is handled by
// `onBackButtonPress` below, which is the single authority that can prevent
// the app from being closed.
function goBack() {
  if (overlay.hasAny()) {
    overlay.pop();
    return;
  }
  router.back();
}

function shouldRouteBack() {
  const routeBackRoutes = ["gallery", "reader", "reader-local"];
  return routeBackRoutes.includes(String(route.name));
}

// --- Hardware back button (Android) -------------------------------------
// Tauri routes the Android hardware back button to this JS callback (added in
// @tauri-apps/api 2.9 / tauri 2.9). Registering it *prevents* the WebView from
// running its default goBack/exit, so the app can never be kicked to the home
// screen by the back button — it is fully hijacked here. Desktop is a no-op.
//
// Priority: overlay → SPA route back → swallow (stay in app). router.back() is
// a safe no-op when there is no route history to pop, so on the root screen the
// press is simply swallowed and the app stays open.
let backButtonUnlisten: PluginListener | null = null;

// Unsubscribe handle for the download:progress listener set up in onMounted.
// Used to refresh the "downloaded" cover badge on completion and to post
// download notifications. Typed loosely (UnlistenFn) to avoid importing the
// type just for teardown.
let progressUnlisten: (() => void) | null = null;

function handleBackButton() {
  // An open overlay always closes first (reader/gallery slide-over panel).
  if (overlay.hasAny()) {
    overlay.pop();
    return;
  }

  if (shouldRouteBack()) {
    router.back();
  }
}

// NOTE: the overlay (reader/gallery slide-over) is not a router route, so we do
// NOT push placeholder history entries for it. The hardware back button closes
// it via `onBackButtonPress` above, and the on-screen buttons / drag gestures
// close it directly via `overlay.pop()`. Keeping the history stack clean avoids
// leftover entries that would make `router.back()` feel stuck (needing extra
// presses to actually navigate).

onMounted(() => {
  window.addEventListener("nclientv4:android-back", handleBackButton);

  // Hijack the Android hardware back button globally. Registering this callback
  // prevents the WebView from running its default goBack/exit, so the app can
  // never be kicked to the home screen by the back button — it is fully
  // hijacked here. The promise rejects on desktop (no such event), which we
  // ignore — desktop uses the on-screen ← button and Esc keys instead.
  onBackButtonPress(() => handleBackButton())
    .then((listener) => {
      backButtonUnlisten = listener;
    })
    .catch(() => {
      // Not Android (or plugin unavailable) — nothing to do.
    });
});

onBeforeUnmount(() => {
  window.removeEventListener("nclientv4:android-back", handleBackButton);
  backButtonUnlisten?.unregister();
  backButtonUnlisten = null;
});

onUnmounted(() => {
  progressUnlisten?.();
  progressUnlisten = null;
});

const globalSpeedLabel = computed(() => {
  const bps = downloads.totalSpeed;
  if (bps <= 0) return "";
  if (bps >= 1024 * 1024) return `↓ ${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
  if (bps >= 1024) return `↓ ${(bps / (1024)).toFixed(0)} KB/s`;
  return `↓ ${bps.toFixed(0)} B/s`;
});

const {
  drag: speedDrag,
  style: speedFloatStyle,
  onPointerDown: onSpeedDown,
  onPointerMove: onSpeedMove,
  onPointerUp: onSpeedUp,
} = useDraggablePosition({
  storageKey: "nclientv4:speedFloat:pos",
  rightMargin: 80,
  bottomMargin: 32,
  minimumMaxX: 40,
  minimumMaxY: 20,
});

// Local search-box model. Kept deliberately separate from the route so typing
// does NOT trigger a search on every keystroke — the user must press Enter or
// click the submit button (doSearch). We only seed it from the URL when the
// route changes externally (e.g. opening a tag link from another page).
const searchQuery = ref(String(route.query.q ?? ""));

watch(
  () => route.query.q,
  (q) => {
    searchQuery.value = String(q ?? "");
  },
);

function doSearch() {
  const q = searchQuery.value.trim();
  if (route.name === "search") {
    // Empty query is allowed on the search page (clears the text filter) so the
    // user can drop a text search but keep tag / language filters.
    router.replace({ query: { ...route.query, q: q || undefined } });
  } else {
    if (!q) return;
    router.push({ name: "search", query: { q } });
  }
}
</script>

<template>
  <div class="app" :class="{ 'sidebar-collapsed': !sidebarOpen, compact: isCompact }">
    <!-- Desktop: inline collapsible sidebar (collapses to an icon rail).
         Mobile: slide-over drawer + tap-to-dismiss backdrop, rendered via v-if
         so it occupies no layout space at all when closed. -->
    <AppSidebar v-if="!isCompact" :open="sidebarOpen" @toggle="toggleSidebar" />
    <template v-else>
      <Transition name="fade">
        <div
          v-if="sidebarOpen || contentEdgeSwipe.dragging"
          class="drawer-backdrop"
          :class="{ 'is-dragging': contentEdgeSwipe.dragging }"
          :style="contentEdgeSwipe.dragging ? { opacity: contentEdgeSwipe.progress * 0.5 } : undefined"
          @click="toggleSidebar"
        />
      </Transition>
      <AppSidebar
        mobile
        :open="sidebarOpen"
        :drag-progress="contentEdgeSwipe.progress"
        :dragging="contentEdgeSwipe.dragging"
        @toggle="toggleSidebar"
        @navigate="sidebarOpen = false"
      />
    </template>

    <main
      class="content"
      :class="{ 'edge-swiping': contentEdgeSwipe.tracking, 'has-floating-topbar': floatingTopbar }"
      @touchstart="onContentTouchStart"
      @touchmove="onContentTouchMove"
      @touchend="onContentTouchEnd"
      @touchcancel="onContentTouchCancel"
      @mousedown="onContentMouseDown"
      @mousemove="onContentMouseMove"
      @mouseup="onContentMouseUp"
    >
      <header class="topbar glass-surface">
        <button v-if="canGoBack" class="icon-btn back-btn" @click="goBack" :title="$t('common.back')">
          <ArrowLeft :size="18" />
        </button>
        <button
          class="icon-btn"
          :title="isCompact ? $t('common.menu') : $t('common.toggle_sidebar')"
          @click="toggleSidebar"
        >
          <Menu :size="18" />
        </button>
        <div class="search glass-surface">
          <SearchIcon :size="16" />
          <input
            v-model="searchQuery"
            type="text"
            class="search-input"
            :placeholder="$t('common.search_placeholder')"
            @keydown.enter="doSearch"
          />
          <button
            type="button"
            class="search-btn"
            :disabled="!searchQuery.trim() && route.name !== 'search'"
            :title="$t('common.search')"
            @click="doSearch"
          >{{ $t('common.search') }}</button>
        </div>
      </header>

      <div v-if="cloudflareBanner" class="banner cf">
        <span>{{ $t('cloudflare.verification_required', { mirror: settings.mirror }) }}</span>
        <button @click="solveCloudflare">{{ $t('cloudflare.solve_now') }}</button>
      </div>

      <RouterView v-slot="{ Component }">
        <Suspense>
          <KeepAlive>
            <component :is="Component" />
          </KeepAlive>
          <template #fallback>
            <div class="route-loading" role="status">
              <Loader :size="24" class="spin" />
              <span>{{ $t('common.loading') }}</span>
            </div>
          </template>
        </Suspense>
      </RouterView>
    </main>

    <Transition name="slide-in">
      <div v-if="overlay.galleryId !== null" :key="'gallery-' + overlay.galleryId" class="overlay-wrapper">
        <div
          ref="overlayPanelRef"
          class="overlay-panel"
          :style="overlayPanelStyle"
          @touchstart="onOverlayTouchStart"
          @touchmove="onOverlayTouchMove"
          @touchend="onOverlayTouchEnd"
        >
          <GalleryView :id="overlay.galleryId" overlay @back="overlay.pop()" />
        </div>
      </div>
    </Transition>
    <Transition name="slide-in">
      <div v-if="overlay.localDetailFolder !== null" :key="'local-detail-' + overlay.localDetailFolder" class="overlay-wrapper">
        <div
          ref="overlayPanelRef"
          class="overlay-panel"
          :style="overlayPanelStyle"
          @touchstart="onOverlayTouchStart"
          @touchmove="onOverlayTouchMove"
          @touchend="onOverlayTouchEnd"
        >
          <LocalDetailView :folder="overlay.localDetailFolder" overlay @back="overlay.pop()" />
        </div>
      </div>
    </Transition>
    <Transition name="slide-in">
      <div v-if="overlay.readerId !== null" :key="'reader-' + overlay.readerId" class="overlay-wrapper overlay-wrapper--full">
        <div
          ref="overlayPanelRef"
          class="overlay-panel overlay-panel--full"
          :style="overlayPanelStyle"
        >
          <ReaderView :id="overlay.readerId" overlay @back="overlay.pop()" />
        </div>
      </div>
    </Transition>
    <Transition name="slide-in">
      <div v-if="overlay.localReaderFolder !== null" :key="'local-' + overlay.localReaderFolder" class="overlay-wrapper overlay-wrapper--full">
        <div
          ref="overlayPanelRef"
          class="overlay-panel overlay-panel--full"
          :style="overlayPanelStyle"
        >
          <LocalReaderView :folder="overlay.localReaderFolder" overlay @back="overlay.pop()" />
        </div>
      </div>
    </Transition>

    <div
      v-if="globalSpeedLabel"
      class="global-speed"
      :class="{ dragging: speedDrag.active }"
      :style="speedFloatStyle()"
      @pointerdown="onSpeedDown"
      @pointermove="onSpeedMove"
      @pointerup="onSpeedUp"
      @pointercancel="onSpeedUp"
    >{{ globalSpeedLabel }}</div>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  height: 100vh;
  background: var(--bg);
  color: var(--text);
}
.content {
  position: relative;
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow: hidden;
}
.content.has-floating-topbar .topbar {
  position: absolute;
  inset: 0 0 auto;
  z-index: 100;
}
.content.has-floating-topbar > :deep(.view) {
  padding-top: calc(var(--app-header-height) + 14px) !important;
}
.content.has-floating-topbar > .banner {
  margin-top: calc(var(--app-header-height) + 10px);
}
/* While an edge-drag-to-open is in progress, suppress text selection so the
   mouse drag doesn't start highlighting content. */
.content.edge-swiping {
  user-select: none;
  -webkit-user-select: none;
}
.topbar {
  display: flex;
  align-items: center;
  gap: 10px;
  height: var(--app-header-height);
  padding: 8px 14px;
  border-bottom: 1px solid var(--border);
  background: var(--surface);
  flex-shrink: 0;
}
.icon-btn {
  background: transparent;
  border: none;
  color: var(--text);
  font-size: 1.1rem;
  cursor: pointer;
  padding: 4px 8px;
}
.back-btn {
  font-size: 1.3rem;
  font-weight: 700;
  margin-right: -4px;
}
.search {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 12px;
  color: var(--text-dim);
  max-width: 540px;
}
.search-input {
  flex: 1;
  background: transparent;
  border: none;
  color: var(--text);
  font-size: 0.85rem;
  outline: none;
  min-width: 0;
}
.search-input::placeholder {
  color: var(--text-dim);
  opacity: 0.7;
}
.search-btn {
  background: var(--accent);
  border: none;
  color: #fff;
  border-radius: 6px;
  padding: 4px 14px;
  font-size: 0.8rem;
  font-weight: 600;
  cursor: pointer;
  white-space: nowrap;
  flex-shrink: 0;
}
.search-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.banner {
  margin: 10px 14px 0;
  padding: 10px 14px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  font-size: 0.85rem;
}
.banner.cf {
  background: rgba(255, 170, 50, 0.12);
  border: 1px solid rgba(255, 170, 50, 0.5);
  color: #ffce80;
}
.banner button {
  background: var(--accent);
  border: none;
  color: #fff;
  padding: 6px 14px;
  border-radius: 6px;
  cursor: pointer;
}

.route-loading {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-dim);
  background: var(--bg);
}
.spin {
  animation: spin 1s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}

.overlay-wrapper {
  position: fixed;
  inset: 0;
  z-index: 1000;
}
.overlay-wrapper--full {
  z-index: 1001;
}
.overlay-panel {
  width: 100%;
  height: 100%;
  background: var(--bg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  touch-action: pan-y;
}
.overlay-panel--full {
  background: #000;
}

.slide-in-enter-active,
.slide-in-leave-active {
  transition: transform 0.28s ease;
}
.slide-in-enter-from {
  transform: translateX(100%);
}
.slide-in-leave-to {
  transform: translateX(100%);
}

.global-speed {
  position: fixed;
  bottom: 8px;
  left: 8px;
  z-index: 1500;
  background: rgba(0, 0, 0, 0.75);
  color: var(--accent);
  padding: 4px 10px;
  border-radius: 6px;
  font-size: 0.78rem;
  font-variant-numeric: tabular-nums;
  /* Draggable: must receive pointer events. A hint cursor tells the user it
     can be grabbed. */
  cursor: grab;
  touch-action: none;
  user-select: none;
  -webkit-user-select: none;
}
.global-speed.dragging {
  cursor: grabbing;
}

/* Dimmed backdrop behind the mobile drawer; tap to close. Sits above content
   but below the drawer itself (z-index 1200 in AppSidebar). */
.drawer-backdrop {
  position: fixed;
  inset: 0;
  z-index: 1100;
  background: rgba(0, 0, 0, 0.5);
  transition: opacity 0.22s ease;
}
.drawer-backdrop.is-dragging {
  transition: none;
}
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.22s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
