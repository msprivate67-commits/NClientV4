<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import GalleryCard from "@/components/GalleryCard.vue";
import EmptyState from "@/components/EmptyState.vue";
import { ArrowUp, ArrowDown, Languages, Loader } from "@lucide/vue";
import {
  cacheTitleTranslation,
  localScan,
  localList,
  localDelete,
  localSetTranslatedTitle,
  matchingCacheEntry,
  titleCacheConfigKey,
  translateTitle,
} from "@/api";
import { useDownloadedStore } from "@/stores/downloaded";
import { useOverlayStore } from "@/stores/overlay";
import { useSettingsStore } from "@/stores/settings";
import { useScrollCache } from "@/composables/useScrollCache";
import { stripLeadingId } from "@/utils/title";
import type { LocalGallery, SimpleGallery } from "@/types";

const items = ref<LocalGallery[]>([]);
const scanning = ref(false);
const viewRef = ref<HTMLElement | null>(null);
useScrollCache(viewRef);
const downloaded = useDownloadedStore();
const overlay = useOverlayStore();
const settings = useSettingsStore();
const { t } = useI18n();

// --- Title display preference (Task 6) ---------------------------------------
// Pure front-end preference: whether to show the translated title or the
// original. Defaults to "translated"; falls back to the original when a
// gallery has no translation. Persisted in localStorage (no backend change).
const SHOW_TRANSLATED_KEY = "nclientv4:localLibrary:showTranslated";
const showTranslated = ref(loadShowTranslated());

function loadShowTranslated(): boolean {
  try {
    const raw = localStorage.getItem(SHOW_TRANSLATED_KEY);
    if (raw === "0") return false;
  } catch { /* ignore */ }
  return true; // default: show translated when available
}

function setShowTranslated(value: boolean) {
  showTranslated.value = value;
  try {
    localStorage.setItem(SHOW_TRANSLATED_KEY, value ? "1" : "0");
  } catch { /* ignore */ }
}

// --- Batch translate-all (Task 5) --------------------------------------------
// Translates every title that has no saved translation yet, 4 at a time in the
// background. Already-translated items are skipped (non-empty translated_title).
// Individual failures are counted but never abort the batch.
const CONCURRENCY = 4;
const translating = ref(false);
const translateProgress = ref({ done: 0, total: 0, skipped: 0, failed: 0 });
// Shown after a batch finishes; cleared by the user or when a new run starts.
const translateDoneMsg = ref("");

function displayTitleFor(l: LocalGallery): string {
  const original = stripLeadingId(l.title || `#${l.id}`);
  if (showTranslated.value && l.translated_title) {
    return l.translated_title;
  }
  return original;
}

function toSimple(l: LocalGallery): SimpleGallery {
  return {
    id: l.id,
    media_id: l.media_id,
    // Keep the underlying SimpleGallery title as the original (stable identity);
    // the displayed title is overridden via the displayTitle prop below.
    title: stripLeadingId(l.title || `#${l.id}`),
    thumbnail: l.thumbnail_path ?? null,
    language: "all",
    tags: [],
    num_pages: l.num_pages,
  };
}

async function translateAll() {
  if (translating.value) return;
  // Targets: items with no saved translation. Computed once at start so newly
  // written translations during the run don't reshuffle the queue.
  const targets = items.value.filter((l) => !l.translated_title && l.id > 0);
  const total = targets.length;
  const skipped = items.value.filter((l) => l.translated_title).length;
  if (total === 0) {
    alert(t("localLibrary.translate_all_none"));
    return;
  }
  const ok = confirm(t("localLibrary.translate_all_confirm", { n: total }));
  if (!ok) return;

  translating.value = true;
  translateProgress.value = { done: 0, total, skipped, failed: 0 };
  translateDoneMsg.value = "";
  const s = settings.settings;
  const cacheKey = titleCacheConfigKey({
    baseUrl: s.tl_base_url,
    model: s.tl_model,
    targetLang: s.tl_target_lang,
    thinking: s.tl_thinking,
    useProxy: s.tl_use_proxy,
  });

  let cursor = 0;
  const worker = async () => {
    while (cursor < targets.length) {
      const idx = cursor++;
      const l = targets[idx];
      const original = stripLeadingId(l.title || `#${l.id}`);
      // Cache first: a stored translation for this gallery produced by the
      // same config skips the AI request entirely.
      const cachedEntry = await matchingCacheEntry(l.id, cacheKey);
      const cachedTitle =
        cachedEntry && cachedEntry.title_source === original
          ? cachedEntry.title_translated
          : "";
      if (cachedTitle.trim()) {
        try {
          await localSetTranslatedTitle(l.id, cachedTitle);
          const target = items.value.find((it) => it.id === l.id);
          if (target) {
            target.translated_title = cachedTitle;
          }
        } catch {
          translateProgress.value.failed++;
        } finally {
          translateProgress.value.done++;
        }
        continue;
      }
      try {
        const result = await translateTitle(
          s.tl_base_url, s.tl_model, s.tl_api_key,
          original, s.tl_target_lang, s.tl_thinking,
          s.tl_use_proxy,
        );
        await localSetTranslatedTitle(l.id, result);
        void cacheTitleTranslation(l.id, cacheKey, original, result);
        // Update the in-memory item so the card re-renders with the new title.
        const target = items.value.find((it) => it.id === l.id);
        if (target) {
          target.translated_title = result;
        }
      } catch {
        translateProgress.value.failed++;
      } finally {
        translateProgress.value.done++;
      }
    }
  };

  // Spawn CONCURRENCY workers over the shared cursor.
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, total) }, worker));
  translating.value = false;
  translateDoneMsg.value = t("localLibrary.translate_all_done", {
    done: translateProgress.value.done - translateProgress.value.failed,
    failed: translateProgress.value.failed,
    skipped: translateProgress.value.skipped,
  });
}

const translateBtnLabel = computed(() => {
  if (translating.value) {
    return t("localLibrary.translating_progress", {
      done: translateProgress.value.done,
      total: translateProgress.value.total,
    });
  }
  return t("localLibrary.translate_all");
});

type SortField = "name" | "date";
const sortField = ref<SortField>("date");
const sortAsc = ref(false);
const selectMode = ref(false);
const selectedIds = ref(new Set<number>());

const sorted = computed(() => {
  const arr = [...items.value];
  arr.sort((a, b) => {
    let cmp: number;
    if (sortField.value === "name") {
      cmp = (a.title || "").localeCompare(b.title || "");
    } else {
      cmp = (a.scanned_at || "").localeCompare(b.scanned_at || "");
    }
    return sortAsc.value ? cmp : -cmp;
  });
  return arr;
});

const allSelected = computed(() =>
  sorted.value.length > 0 && sorted.value.every((l) => selectedIds.value.has(l.id)),
);

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) {
    selectedIds.value = new Set();
  }
}

function toggleSelectAll() {
  if (allSelected.value) {
    selectedIds.value = new Set();
  } else {
    selectedIds.value = new Set(sorted.value.map((l) => l.id));
  }
}

async function deleteSelected() {
  const idsToDelete = [...selectedIds.value];
  if (!idsToDelete.length) return;
  // Find the corresponding folder for each selected ID.
  const folders = idsToDelete
    .map((id) => items.value.find((l) => l.id === id)?.folder)
    .filter(Boolean) as string[];
  selectedIds.value = new Set();
  // Delete one by one — the backend removes the folder from disk.
  for (const folder of folders) {
    try {
      await localDelete(folder);
    } catch { /* ignore individual failures */ }
  }
  // Remove deleted items from the local list and re-sync the downloaded set.
  const deletedSet = new Set(folders);
  items.value = items.value.filter((l) => !deletedSet.has(l.folder));
  await downloaded.refresh();
}

function onSelect(id: number) {
  selectedIds.value = new Set(selectedIds.value).add(id);
}

function onDeselect(id: number) {
  const next = new Set(selectedIds.value);
  next.delete(id);
  selectedIds.value = next;
}

async function scan() {
  scanning.value = true;
  try {
    // Scanning refreshes the on-disk metadata in the database. Read the saved
    // rows afterwards so persisted translated titles are included.
    await localScan();
    items.value = await localList();
    selectedIds.value = new Set();
    await downloaded.refresh();
  } finally {
    scanning.value = false;
  }
}

// The detail view is shown over this still-mounted page. Once it closes, read
// the saved rows again so an individually translated title appears immediately
// without requiring a manual rescan.
watch(() => overlay.localDetailFolder, async (current, previous) => {
  if (previous !== null && current === null) {
    try {
      items.value = await localList();
    } catch (e) {
      console.warn("local library refresh failed", e);
    }
  }
});

onMounted(scan);
</script>

<template>
  <div ref="viewRef" class="view">
    <div class="view-header">
      <div class="view-title">{{ $t('localLibrary.title') }}</div>
      <div class="toolbar">
        <button
          class="btn"
          :disabled="translating"
          :title="$t('localLibrary.translate_all_hint')"
          @click="translateAll"
        >
          <Loader v-if="translating" :size="14" class="spin" />
          <Languages v-else :size="14" />
          {{ translateBtnLabel }}
        </button>
        <button class="btn" :disabled="scanning" @click="scan">
          {{ scanning ? $t('localLibrary.scanning') : $t('localLibrary.rescan') }}
        </button>
      </div>
    </div>

    <div class="sort-row">
      <div class="sort-group title-group">
        <button
          class="btn small"
          :class="{ active: showTranslated }"
          :title="$t('localLibrary.show_translated_hint')"
          @click="setShowTranslated(true)"
        >{{ $t('localLibrary.show_translated') }}</button>
        <button
          class="btn small"
          :class="{ active: !showTranslated }"
          @click="setShowTranslated(false)"
        >{{ $t('localLibrary.show_original') }}</button>
      </div>
      <div class="sort-group">
        <span class="sort-label">{{ $t('localLibrary.sort_label') }}</span>
        <button
          class="btn small"
          :class="{ active: sortField === 'name' }"
          @click="sortField = 'name'"
        >{{ $t('localLibrary.sort_name') }}</button>
        <button
          class="btn small"
          :class="{ active: sortField === 'date' }"
          @click="sortField = 'date'"
        >{{ $t('localLibrary.sort_date') }}</button>
        <button class="btn small icon" @click="sortAsc = !sortAsc" :title="sortAsc ? $t('localLibrary.ascending') : $t('localLibrary.descending')">
          <ArrowUp v-if="sortAsc" :size="14" />
          <ArrowDown v-else :size="14" />
        </button>
      </div>
      <div class="select-group">
        <button
          class="btn small"
          :class="{ active: selectMode }"
          @click="toggleSelectMode"
        >{{ selectMode ? $t('localLibrary.cancel') : $t('localLibrary.select') }}</button>
        <template v-if="selectMode">
          <button class="btn small" @click="toggleSelectAll">
            {{ allSelected ? $t('localLibrary.deselect_all') : $t('localLibrary.select_all') }}
          </button>
          <button
            class="btn small danger"
            :disabled="selectedIds.size === 0"
            @click="deleteSelected"
          >
            {{ $t('localLibrary.delete_selected', { n: selectedIds.size }) }}
          </button>
        </template>
      </div>
    </div>

    <div v-if="translateDoneMsg" class="batch-done">
      {{ translateDoneMsg }}
      <button class="link-btn" @click="translateDoneMsg = ''">×</button>
    </div>

    <div v-if="sorted.length" class="grid">
      <GalleryCard
        v-for="l in sorted"
        :key="l.folder"
        :gallery="toSimple(l)"
        local
        :thumbnail-override="l.thumbnail_path"
        :display-title="displayTitleFor(l)"
        :selectable="selectMode"
        :selected="selectedIds.has(l.id)"
        @select="onSelect"
        @deselect="onDeselect"
      />
    </div>
    <EmptyState
      v-else
      :title="$t('localLibrary.no_local')"
      :hint="$t('localLibrary.no_local_hint')"
    />
  </div>
</template>

<style scoped>
.sort-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 14px;
}
.sort-group,
.select-group {
  display: flex;
  align-items: center;
  gap: 6px;
}
/* Translated/original title toggle: sits at the left of the sort row. A small
   gap separates it from the sort buttons. */
.title-group {
  padding-right: 8px;
  border-right: 1px solid var(--border);
  margin-right: 4px;
}
.batch-done {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 10px;
  padding: 8px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  font-size: 0.82rem;
  color: var(--text);
}
.link-btn {
  background: none;
  border: none;
  color: var(--text-dim);
  cursor: pointer;
  padding: 0 4px;
  font-size: 1.1rem;
  line-height: 1;
}
.link-btn:hover {
  color: var(--text);
}
.spin {
  animation: spin 1s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}
.sort-label {
  font-size: 0.82rem;
  color: var(--text-dim);
  margin-right: 2px;
}
.btn.small {
  padding: 3px 10px;
  font-size: 0.78rem;
  border-radius: 5px;
}
.btn.small.icon {
  padding: 3px 8px;
  font-size: 0.85rem;
  line-height: 1;
}
.btn.small.active {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}
.btn.small.danger {
  background: rgba(220, 60, 60, 0.15);
  border-color: rgba(220, 60, 60, 0.5);
  color: #f08080;
}
.btn.small.danger:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 12px;
}
@media (max-width: 560px) {
  .grid {
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }
}
</style>
