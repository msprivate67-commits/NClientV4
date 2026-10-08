<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { platform } from "@tauri-apps/plugin-os";

import DetailRelatedSection from "@/components/DetailRelatedSection.vue";
import DetailTagsSection from "@/components/DetailTagsSection.vue";
import {
  ArrowLeft,
  RefreshCw,
  Loader,
  Languages,
  Download,
  Star,
  BookOpen,
  Check,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Heart,
  Share2,
} from "@lucide/vue";
import {
  androidShareText,
  cacheCommentTranslations,
  cachedCommentTranslation,
  commentCacheConfigKey,
  imageProxyUrl,
  matchingCacheEntry,
  openInBrowser,
  translateComment,
} from "@/api";
import type { Comment } from "@/types";
import { useGalleryStore } from "@/stores/gallery";
import { useFavoritesStore } from "@/stores/favorites";
import { useDownloadsStore } from "@/stores/downloads";
import { useSettingsStore } from "@/stores/settings";
import { useTitleTranslationsStore } from "@/stores/titleTranslations";
import { useOverlayStore } from "@/stores/overlay";
import { useDownloadedStore } from "@/stores/downloaded";
import { useTagsStore } from "@/stores/tags";
import { useScrollCache } from "@/composables/useScrollCache";
import { usePriorityPreloadQueue } from "@/composables/usePriorityPreloadQueue";
import {
  cachedImageObjectUrl,
  loadImageObjectUrl,
  pinImageObjectSource,
} from "@/composables/useImageObjectCache";

const props = defineProps<{ id: number | string; overlay?: boolean }>();
const emit = defineEmits<{ back: [] }>();
const router = useRouter();
const gallery = useGalleryStore();
const favorites = useFavoritesStore();
const downloads = useDownloadsStore();
const downloaded = useDownloadedStore();
const tagsStore = useTagsStore();

const downloadState = computed(() => {
  if (!g.value) return null;
  const entry = downloads.items.find(
    (d) => d.id === g.value!.id && (d.status === "downloading" || d.status === "pending"),
  );
  return entry?.status ?? null;
});
const settings = useSettingsStore();
const titleTranslations = useTitleTranslationsStore();
const { t: i18n } = useI18n();
const overlay = useOverlayStore();
const isAndroid = platform() === "android";

const id = computed(() => Number(props.id));
const error = ref<string | null>(null);
const commentsOpen = ref(false);
const tagsExpanded = ref(false);
const pagesExpanded = ref(true);
const relatedExpanded = ref(true);
const loading = ref(false);
const shareCopied = ref(false);
let shareCopiedTimer: ReturnType<typeof setTimeout> | null = null;
const viewRef = ref<HTMLElement | null>(null);

// Is this gallery already on disk in the local library? When true the download
// button is disabled — re-downloading would just duplicate the folder.
const isDownloaded = computed(() => (g.value ? downloaded.has(g.value.id) : false));
useScrollCache(viewRef);

const titleTranslation = computed(() => titleTranslations.entry(id.value));
const translating = computed(() => Boolean(
  titleTranslation.value?.queued || titleTranslation.value?.translating,
));
const translatedTitle = computed(() => titleTranslation.value?.translated ?? "");
const reasoningText = computed(() => titleTranslation.value?.reasoning ?? "");
const reasoningExpanded = ref(false);
const reasoningRef = ref<HTMLElement | null>(null);
const translateError = computed(() => titleTranslation.value?.error ?? "");

interface CommentTranslationState {
  translated: string;
  reasoning: string;
  reasoningExpanded: boolean;
  queued: boolean;
  translating: boolean;
  error: string;
}

const commentTranslations = ref(new Map<number, CommentTranslationState>());
const commentTranslationControllers = new Map<number, AbortController>();
const commentReasoningRefs = new Map<number, HTMLElement>();
const queuedCommentIds = new Set<number>();
let commentTranslationQueue: Comment[] = [];
let activeCommentTranslations = 0;
let commentTranslationRunId = 0;
let loadingAllCommentPages = false;
const COMMENT_TRANSLATION_CONCURRENCY = 4;

function commentTranslation(commentId: number): CommentTranslationState | undefined {
  return commentTranslations.value.get(commentId);
}

function updateCommentTranslation(
  commentId: number,
  patch: Partial<CommentTranslationState>,
) {
  const previous = commentTranslations.value.get(commentId) ?? {
    translated: "",
    reasoning: "",
    reasoningExpanded: false,
    queued: false,
    translating: false,
    error: "",
  };
  const next = new Map(commentTranslations.value);
  next.set(commentId, { ...previous, ...patch });
  commentTranslations.value = next;
}

function setCommentReasoningRef(commentId: number, element: unknown) {
  if (element instanceof HTMLElement) {
    commentReasoningRefs.set(commentId, element);
  } else {
    commentReasoningRefs.delete(commentId);
  }
}

async function scrollCommentReasoningToBottom(commentId: number) {
  await nextTick();
  const element = commentReasoningRefs.get(commentId);
  if (element) element.scrollTop = element.scrollHeight;
}

function resetCommentTranslations() {
  commentTranslationRunId += 1;
  commentTranslationControllers.forEach((controller) => controller.abort());
  commentTranslationControllers.clear();
  commentReasoningRefs.clear();
  queuedCommentIds.clear();
  commentTranslationQueue = [];
  activeCommentTranslations = 0;
  loadingAllCommentPages = false;
  commentTranslations.value = new Map();
}

function enqueueCommentTranslation(comment: Comment, force = false) {
  if (!comment.body.trim()) return;
  const current = commentTranslation(comment.id);
  if (current?.translating || current?.queued || queuedCommentIds.has(comment.id)) return;
  if (!force && (current?.translated || current?.error)) return;
  updateCommentTranslation(comment.id, {
    translated: force ? "" : current?.translated ?? "",
    reasoning: force ? "" : current?.reasoning ?? "",
    reasoningExpanded: false,
    queued: true,
    translating: false,
    error: "",
  });
  queuedCommentIds.add(comment.id);
  commentTranslationQueue.push(comment);
  pumpCommentTranslations();
}

function pumpCommentTranslations() {
  const runId = commentTranslationRunId;
  while (
    runId === commentTranslationRunId
    && activeCommentTranslations < COMMENT_TRANSLATION_CONCURRENCY
    && commentTranslationQueue.length
  ) {
    const comment = commentTranslationQueue.shift();
    if (!comment) break;
    queuedCommentIds.delete(comment.id);
    activeCommentTranslations += 1;
    void runCommentTranslation(comment, runId).finally(() => {
      if (runId !== commentTranslationRunId) return;
      activeCommentTranslations -= 1;
      pumpCommentTranslations();
    });
  }
}

async function runCommentTranslation(comment: Comment, runId: number) {
  const controller = new AbortController();
  commentTranslationControllers.set(comment.id, controller);
  updateCommentTranslation(comment.id, {
    translated: "",
    reasoning: "",
    reasoningExpanded: true,
    queued: false,
    translating: true,
    error: "",
  });
  const s = settings.settings;
  // Cache first: this gallery's stored translation for this exact comment
  // (source body must still match) skips the AI request.
  const galleryId = comment.gallery_id || g.value?.id || 0;
  const cacheKey = commentCacheConfigKey({
    baseUrl: s.tl_base_url,
    model: s.tl_model,
    commentTargetLang: s.tl_comment_target_lang,
    thinking: s.tl_thinking,
    useProxy: s.tl_use_proxy,
  });
  const cachedEntry = await matchingCacheEntry(galleryId, cacheKey);
  const cached = cachedEntry
    ? cachedCommentTranslation(cachedEntry, comment.id, comment.body)
    : null;
  if (cached) {
    if (runId !== commentTranslationRunId || controller.signal.aborted) return;
    updateCommentTranslation(comment.id, {
      translated: cached,
      reasoningExpanded: false,
      translating: false,
    });
    commentTranslationControllers.delete(comment.id);
    return;
  }
  try {
    const translated = await translateComment(
      s.tl_base_url,
      s.tl_model,
      s.tl_api_key,
      title.value,
      comment.body,
      s.tl_comment_target_lang,
      s.tl_thinking,
      s.tl_use_proxy,
      {
        signal: controller.signal,
        onContent: (chunk) => {
          if (runId !== commentTranslationRunId || controller.signal.aborted) return;
          const state = commentTranslation(comment.id);
          updateCommentTranslation(comment.id, {
            translated: `${state?.translated ?? ""}${chunk}`,
          });
        },
        onReasoning: (chunk) => {
          if (runId !== commentTranslationRunId || controller.signal.aborted) return;
          const state = commentTranslation(comment.id);
          updateCommentTranslation(comment.id, {
            reasoning: `${state?.reasoning ?? ""}${chunk}`,
          });
          void scrollCommentReasoningToBottom(comment.id);
        },
      },
    );
    if (runId !== commentTranslationRunId || controller.signal.aborted) return;
    updateCommentTranslation(comment.id, {
      translated,
      reasoningExpanded: false,
      translating: false,
    });
    void cacheCommentTranslations(galleryId, cacheKey, [
      { id: comment.id, body: comment.body, translated },
    ]);
  } catch (error: unknown) {
    if (runId !== commentTranslationRunId || controller.signal.aborted) return;
    updateCommentTranslation(comment.id, {
      reasoningExpanded: false,
      translating: false,
      error: error instanceof Error ? error.message : String(error),
    });
  } finally {
    if (commentTranslationControllers.get(comment.id) === controller) {
      commentTranslationControllers.delete(comment.id);
    }
  }
}

async function maybeAutoTranslateComments() {
  const currentGallery = g.value;
  if (
    !commentsOpen.value
    || !currentGallery
    || !settings.settings.tl_auto_translate
    || !settings.settings.tl_api_key.trim()
  ) return;

  gallery.comments.forEach((comment) => enqueueCommentTranslation(comment));
  if (loadingAllCommentPages || gallery.commentsLoading) return;

  loadingAllCommentPages = true;
  const galleryId = currentGallery.id;
  try {
    while (
      g.value?.id === galleryId
      && commentsOpen.value
      && gallery.commentsPage < gallery.commentsNumPages
    ) {
      const previousPage = gallery.commentsPage;
      await gallery.loadMoreComments(galleryId);
      gallery.comments.forEach((comment) => enqueueCommentTranslation(comment));
      if (gallery.commentsPage <= previousPage) break;
    }
  } catch {
    // The comment list already displays pagination errors from the store.
  } finally {
    loadingAllCommentPages = false;
  }
}

function translateSingleComment(comment: Comment) {
  enqueueCommentTranslation(comment, true);
}

function toggleCommentReasoning(commentId: number) {
  const state = commentTranslation(commentId);
  if (!state?.reasoning) return;
  updateCommentTranslation(commentId, {
    reasoningExpanded: !state.reasoningExpanded,
  });
  if (!state.reasoningExpanded) void scrollCommentReasoningToBottom(commentId);
}

async function toggleTitleReasoning() {
  reasoningExpanded.value = !reasoningExpanded.value;
  if (!reasoningExpanded.value) return;
  await nextTick();
  if (reasoningRef.value) reasoningRef.value.scrollTop = reasoningRef.value.scrollHeight;
}

async function doTranslate() {
  if (!g.value) return;
  reasoningExpanded.value = true;
  titleTranslations.enqueue(g.value.id, title.value, { priority: true, force: true });
}

function maybeAutoTranslate() {
  if (
    !g.value
    || !settings.settings.tl_auto_translate
    || !settings.settings.tl_api_key.trim()
  ) return;
  titleTranslations.enqueue(g.value.id, title.value, { priority: true });
}

const g = computed(() => gallery.current);

const title = computed(() => {
  if (!g.value) return "";
  const t = g.value.titles;
  const pref = settings.settings.title_type;
  if (pref === "pretty" && t.pretty) return t.pretty;
  if (pref === "english" && t.english) return t.english;
  if (pref === "japanese" && t.japanese) return t.japanese;
  return t.pretty || t.english || t.japanese || i18n("common.unnamed");
});

const coverSrc = computed(() => {
  const p = g.value?.cover?.path ?? g.value?.thumbnail?.path;
  return p ? imageProxyUrl(p) : "";
});

const tagsByType = computed(() => {
  const map = new Map<string, typeof g.value extends infer _ ? any : any>();
  if (!g.value) return map;
  for (const t of g.value.tags) {
    const list = map.get(t.type) ?? [];
    list.push(t);
    map.set(t.type, list);
  }
  return map;
});

const thumbColumns = computed(() => {
  const cols = settings.settings.page_thumbnail_columns;
  if (cols <= 0) return "auto";
  return cols;
});

async function load() {
  error.value = null;
  loading.value = true;
  try {
    await gallery.load(id.value);
    pinGalleryImages();
    setupThumbObserver();
    maybeAutoTranslate();
  } catch (e: any) {
    error.value = String(e?.message ?? e);
  } finally {
    loading.value = false;
  }
}

const loadedThumbs = ref(new Set<number>());
let thumbObserver: IntersectionObserver | null = null;
let releasePinnedImages: Array<() => void> = [];

function pinGalleryImages() {
  releasePinnedImages.forEach((release) => release());
  releasePinnedImages = [];
  const sources = new Set(
    (g.value?.pages ?? []).flatMap((page) => [page.thumbnail, page.path])
      .filter((source): source is string => !!source),
  );
  releasePinnedImages = [...sources].map(pinImageObjectSource);
}

const thumbnailPreloader = usePriorityPreloadQueue(async (index) => {
  const page = g.value?.pages[index];
  if (!page) return;
  await loadImageObjectUrl(page.thumbnail || page.path);
}, 4);

const fullImagePreloader = usePriorityPreloadQueue(async (index) => {
  const page = g.value?.pages[index];
  if (!page?.path) return;
  await loadImageObjectUrl(page.path);
}, 2);

function pageThumbnailSrc(index: number): string {
  const page = g.value?.pages[index];
  return cachedImageObjectUrl(page?.thumbnail || page?.path);
}

function setupThumbObserver() {
  thumbObserver?.disconnect();
  thumbObserver = null;
  loadedThumbs.value = new Set<number>();
  thumbnailPreloader.reset();
  fullImagePreloader.reset();
  void nextTick(() => {
    const root = viewRef.value;
    if (!root) return;
    thumbObserver = new IntersectionObserver(
      (entries) => {
        const next = new Set(loadedThumbs.value);
        let changed = false;
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const index = Number((entry.target as HTMLElement).dataset.pageIndex);
          if (!Number.isInteger(index) || next.has(index)) continue;
          next.add(index);
          thumbnailPreloader.enqueue([index], true);
          fullImagePreloader.enqueue([index], true);
          changed = true;
          thumbObserver?.unobserve(entry.target);
        }
        if (changed) loadedThumbs.value = next;
      },
      { root, rootMargin: "600px 0px", threshold: 0.01 },
    );
    const elements = [...root.querySelectorAll<HTMLElement>(".thumb-item")];
    elements.forEach((element) => {
      thumbObserver?.observe(element);
    });
    const rootRect = root.getBoundingClientRect();
    const nearby = elements
      .filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.bottom >= rootRect.top - 600 && rect.top <= rootRect.bottom + 600;
      })
      .map((element) => Number(element.dataset.pageIndex));
    thumbnailPreloader.enqueue(nearby, true);
    fullImagePreloader.enqueue(nearby, true);
    const allPages = Array.from(
      { length: g.value?.pages.length ?? 0 },
      (_, index) => index,
    );
    // Independent queues let thumbnails fill in quickly while full images are
    // downloaded at lower concurrency in the background. Both resolve to the
    // same shared object cache consumed by ReaderView.
    thumbnailPreloader.enqueue(allPages);
    fullImagePreloader.enqueue(allPages);
  });
}

async function toggleFavorite() {
  if (!g.value) return;
  await favorites.toggle({
    id: g.value.id,
    title: title.value,
    media_id: g.value.media_id,
    thumbnail: g.value.thumbnail.thumbnail || g.value.thumbnail.path || "",
  });
}

async function shareGallery() {
  if (!g.value) return;
  const url = `https://nhentai.net/g/${g.value.id}/`;
  if (isAndroid) {
    await androidShareText(url, i18n("gallery.share_chooser_title"));
    return;
  }

  await writeText(url);
  shareCopied.value = true;
  if (shareCopiedTimer !== null) clearTimeout(shareCopiedTimer);
  shareCopiedTimer = setTimeout(() => {
    shareCopied.value = false;
    shareCopiedTimer = null;
  }, 1800);
}

async function download() {
  if (!g.value) return;
  if (downloadState.value !== null) return;
  if (isDownloaded.value) return; // already on disk — nothing to download
  await downloads.enqueue({ gallery_id: g.value.id });
}

function read() {
  if (props.overlay) {
    overlay.openReader(id.value);
  } else {
    router.push({ name: "reader", params: { id: id.value } });
  }
}

function readPage(pageNum: number) {
  if (props.overlay) {
    overlay.openReader(id.value, pageNum);
  } else {
    router.push({ name: "reader", params: { id: id.value }, query: { page: pageNum } });
  }
}

function goBack() {
  // Non-overlay galleries are reached via router navigation; the app's global
  // top bar also exposes a back button, but we keep one here for when the
  // detail view is the active route.
  if (window.history.length > 1) {
    router.back();
  } else {
    router.push({ name: "home" });
  }
}

function goToSettings() {
  if (props.overlay) {
    overlay.closeAll();
  }
  router.push({ name: "settings" });
}

onMounted(load);
watch(id, () => {
  resetCommentTranslations();
  reasoningExpanded.value = false;
  commentsOpen.value = false;
  pagesExpanded.value = true;
  relatedExpanded.value = true;
  void load();
});
watch(
  [
    () => settings.settings.tl_base_url,
    () => settings.settings.tl_model,
    () => settings.settings.tl_api_key,
    () => settings.settings.tl_target_lang,
    () => settings.settings.tl_thinking,
    () => settings.settings.tl_use_proxy,
    () => settings.settings.tl_auto_translate,
  ],
  () => {
    maybeAutoTranslate();
    void maybeAutoTranslateComments();
  },
);
watch(
  [
    commentsOpen,
    () => gallery.comments.map((comment) => comment.id).join(","),
  ],
  () => void maybeAutoTranslateComments(),
);
watch(reasoningText, async () => {
  if (!reasoningExpanded.value) return;
  await nextTick();
  if (reasoningRef.value) reasoningRef.value.scrollTop = reasoningRef.value.scrollHeight;
});
watch(translating, (active, wasActive) => {
  if (!active && wasActive && reasoningText.value) reasoningExpanded.value = false;
});
onUnmounted(() => {
  thumbObserver?.disconnect();
  resetCommentTranslations();
  releasePinnedImages.forEach((release) => release());
  releasePinnedImages = [];
  if (shareCopiedTimer !== null) clearTimeout(shareCopiedTimer);
});

async function toggleComments() {
  commentsOpen.value = !commentsOpen.value;
  if (commentsOpen.value && g.value && gallery.commentsPage === 0) {
    try {
      await gallery.loadComments(g.value.id);
    } catch {
      // The store exposes the request error beside the comment list.
    }
  }
  if (commentsOpen.value) void maybeAutoTranslateComments();
}

async function onTagClick(t: any) {
  if (props.overlay) {
    overlay.closeAll();
  }
  const name = encodeURIComponent(t.name);
  const type = encodeURIComponent(t.type);
  router.push({ name: "search", query: { tags: `${t.id}:accepted:${name}:${type}` } });
}

async function toggleTagBlacklist(tag: import("@/types").Tag) {
  if (!settings.settings.auth.api_key.trim()) {
    error.value = i18n("tags.blacklist_requires_api_key");
    return;
  }
  try {
    if (tagsStore.blacklistedIds.has(tag.id)) {
      await tagsStore.removeBlacklist(tag.id);
    } else {
      tagsStore.merge([tag]);
      await tagsStore.addBlacklist(tag.id);
    }
  } catch (cause) {
    error.value = String(cause);
  }
}
</script>

<template>
  <div ref="viewRef" class="view gallery-view" :class="{ 'overlay-mode': overlay }">
    <div v-if="overlay" class="overlay-bar glass-surface">
      <button class="btn" @click="emit('back')"><ArrowLeft :size="16" /></button>
      <span class="overlay-title">{{ title }}</span>
    </div>

    <!-- Sticky title bar: stays pinned to the top while the page scrolls,
         so the back button + title are always reachable on long pages. -->
    <div v-else class="sticky-title-bar glass-surface">
      <button class="btn ghost back" @click="goBack" :title="$t('gallery.back')"><ArrowLeft :size="16" /></button>
      <span class="sticky-title">{{ title }}</span>
    </div>

    <div v-if="error" class="error">{{ error }}</div>

    <div v-if="g" class="header">
      <div class="cover">
        <img v-if="coverSrc" :src="coverSrc" :alt="title" />
      </div>
      <div class="info">
        <div class="title-stack">
          <h1 class="title">{{ title }}</h1>
          <div v-if="reasoningText" class="reasoning-block">
            <button class="reasoning-toggle" @click="toggleTitleReasoning">
              <ChevronUp v-if="reasoningExpanded" :size="13" />
              <ChevronDown v-else :size="13" />
              {{ $t('gallery.reasoning') }}
            </button>
            <div v-show="reasoningExpanded" ref="reasoningRef" class="reasoning-text">{{ reasoningText }}</div>
          </div>
        </div>
        <div v-if="translatedTitle" class="translated-title">{{ translatedTitle }}</div>
        <div v-if="translateError" class="tl-error">{{ translateError }}</div>
        <div v-if="translateError" class="tl-error-hint">
          {{ $t('gallery.translate_error_hint') }}
          <button class="link-btn" @click="goToSettings">{{ $t('gallery.go_to_ai_settings') }}</button>
        </div>
        <div class="meta">
          <span>#{{ g.id }}</span>
          <span>·</span>
          <span>{{ g.num_pages }} {{ $t('gallery.pages') }}</span>
          <span>·</span>
          <span><Heart :size="12" /> {{ g.num_favorites }}</span>
          <span v-if="g.upload_date">·</span>
          <span v-if="g.upload_date">{{ new Date(g.upload_date).toLocaleDateString() }}</span>
        </div>
        <div class="primary-actions">
          <button
            class="btn primary read-btn"
            @click="read"
            :title="$t('gallery.read')"
          >
            <BookOpen :size="18" /> {{ $t('gallery.read') }}
          </button>
          <div class="tool-btns">
            <button class="btn" :disabled="loading" @click="load" :title="$t('gallery.reload_gallery')">
              <RefreshCw v-if="!loading" :size="14" />{{ loading ? $t('common.refreshing') : ' ' + $t('common.refresh') }}
            </button>
            <button
              class="btn"
              :disabled="translating"
              @click="doTranslate"
              :title="translatedTitle ? $t('gallery.retranslate') : $t('gallery.translate_title')"
            >
              <span v-if="translating"><Loader :size="14" class="spin" /> {{ $t('gallery.translating') }}</span>
              <span v-else-if="translatedTitle"><RefreshCw :size="14" /> {{ $t('gallery.retranslate') }}</span>
              <span v-else><Languages :size="14" /> {{ $t('gallery.translate') }}</span>
            </button>
          </div>
        </div>
        <div class="actions">
          <button class="btn" @click="openInBrowser(String(g.id))">{{ $t('gallery.webpage') }}</button>
          <button
            class="btn"
            :disabled="downloadState !== null || isDownloaded"
            :class="{ downloading: downloadState === 'downloading', queued: downloadState === 'pending', done: isDownloaded && downloadState === null }"
            :title="isDownloaded && downloadState === null ? $t('gallery.already_downloaded') : undefined"
            @click="download"
          >
            <span v-if="downloadState === 'downloading'"><Loader :size="14" class="spin" /> {{ $t('gallery.downloading') }}</span>
            <span v-else-if="downloadState === 'pending'"><Loader :size="14" class="spin" /> {{ $t('gallery.queued') }}</span>
            <span v-else-if="isDownloaded"><Check :size="14" /> {{ $t('gallery.downloaded') }}</span>
            <span v-else><Download :size="14" /> {{ $t('gallery.download_btn') }}</span>
          </button>
          <button
            class="btn"
            :class="{ primary: g.is_favorited || favorites.ids.has(g.id) }"
            @click="toggleFavorite"
          >
            {{ '' }}<Star :size="14" :fill="g.is_favorited || favorites.ids.has(g.id) ? 'currentColor' : 'none'" /> {{ $t('gallery.favorite') }}
          </button>
          <button class="btn" type="button" @click="shareGallery">
            <Check v-if="shareCopied" :size="14" />
            <Share2 v-else :size="14" />
            {{ shareCopied ? $t('gallery.link_copied') : $t('gallery.share') }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="g" class="body">
      <DetailTagsSection
        v-model:expanded="tagsExpanded"
        :groups="tagsByType"
        :blacklisted-ids="tagsStore.blacklistedIds"
        :gallery-id="g.id"
        @select="onTagClick"
        @toggle-blacklist="toggleTagBlacklist"
      />

      <section v-if="g.pages.length" class="page-thumbs detail-card">
        <div class="section-toggle-bar">
          <div class="section-title">{{ $t('gallery.section_pages') }}</div>
          <button
            class="btn small"
            type="button"
            :aria-expanded="pagesExpanded"
            @click="pagesExpanded = !pagesExpanded"
          >
            <ChevronUp v-if="pagesExpanded" :size="14" />
            <ChevronDown v-else :size="14" />
            {{ pagesExpanded ? $t('gallery.collapse') : $t('gallery.expand') }}
          </button>
        </div>
        <div
          v-show="pagesExpanded"
          class="thumb-grid"
          :style="thumbColumns === 'auto' ? undefined : { gridTemplateColumns: `repeat(${thumbColumns}, 1fr)` }"
        >
          <div
            v-for="(page, i) in g.pages"
            :key="i"
            class="thumb-item"
            :data-page-index="i"
            @click="readPage(i + 1)"
          >
            <img
              v-if="loadedThumbs.has(i) && (page.thumbnail || page.path)"
              :src="pageThumbnailSrc(i)"
              :alt="$t('common.page_n', { n: i + 1 })"
              loading="eager"
              decoding="async"
              class="thumb-img"
              @load="($event.target as HTMLImageElement).classList.add('loaded')"
            />
            <span class="thumb-label">{{ i + 1 }}</span>
          </div>
        </div>
      </section>

      <DetailRelatedSection
        v-if="g.related.length"
        v-model:expanded="relatedExpanded"
        :galleries="g.related"
      />

      <section class="comments detail-card">
        <div class="section-toggle-bar comments-toolbar">
          <div class="section-title">
            {{ $t('gallery.section_comments') }}
            <span v-if="gallery.commentsTotal !== null">({{ gallery.commentsTotal }})</span>
          </div>
          <button class="btn small" type="button" :aria-expanded="commentsOpen" @click="toggleComments">
            <ChevronUp v-if="commentsOpen" :size="14" />
            <ChevronDown v-else :size="14" />
            {{ commentsOpen ? $t('gallery.collapse') : $t('gallery.expand') }}
          </button>
          <button
            v-if="commentsOpen"
            class="btn"
            @click="openInBrowser(`g/${g.id}/#comment-post-container`)"
          >
            <ExternalLink :size="14" /> {{ $t('gallery.add_comment') }}
          </button>
        </div>
        <p v-if="commentsOpen" class="hint website-actions-hint">
          {{ $t('gallery.website_actions_hint') }}
        </p>
        <div v-if="commentsOpen" class="comment-list">
          <div v-if="gallery.commentsLoading && gallery.commentsPage === 0" class="empty">
            {{ $t('common.loading') }}
          </div>
          <div v-for="c in gallery.comments" :key="c.id" class="comment">
            <div class="comment-avatar" aria-hidden="true">
              <img
                v-if="c.poster.avatar_url"
                :src="imageProxyUrl(c.poster.avatar_url)"
                alt=""
                loading="lazy"
                decoding="async"
              />
              <span v-else>{{ c.poster.username.charAt(0).toUpperCase() }}</span>
            </div>
            <div class="comment-content">
              <div class="who">
                <strong>{{ c.poster.username }}</strong>
                <span v-if="c.post_date">{{ new Date(c.post_date).toLocaleString() }}</span>
                <button
                  class="comment-translate-button"
                  type="button"
                  :disabled="commentTranslation(c.id)?.translating || commentTranslation(c.id)?.queued"
                  @click="translateSingleComment(c)"
                >
                  <Loader
                    v-if="commentTranslation(c.id)?.translating || commentTranslation(c.id)?.queued"
                    :size="12"
                    class="spin"
                  />
                  <Languages v-else :size="12" />
                  {{
                    commentTranslation(c.id)?.translating || commentTranslation(c.id)?.queued
                      ? $t('gallery.translating')
                      : commentTranslation(c.id)?.translated
                        ? $t('gallery.retranslate_comment')
                        : $t('gallery.translate_comment')
                  }}
                </button>
              </div>
              <div class="comment-body">{{ c.body }}</div>
              <div v-if="commentTranslation(c.id)?.reasoning" class="comment-reasoning">
                <button
                  class="reasoning-toggle"
                  type="button"
                  @click="toggleCommentReasoning(c.id)"
                >
                  <ChevronUp v-if="commentTranslation(c.id)?.reasoningExpanded" :size="12" />
                  <ChevronDown v-else :size="12" />
                  {{ $t('gallery.reasoning') }}
                </button>
                <div
                  v-show="commentTranslation(c.id)?.reasoningExpanded"
                  :ref="(element) => setCommentReasoningRef(c.id, element)"
                  class="reasoning-text comment-reasoning-text"
                >{{ commentTranslation(c.id)?.reasoning }}</div>
              </div>
              <div
                v-if="commentTranslation(c.id)?.translated"
                class="translated-comment"
              >{{ commentTranslation(c.id)?.translated }}</div>
              <div v-if="commentTranslation(c.id)?.error" class="inline-error comment-translate-error">
                {{ commentTranslation(c.id)?.error }}
              </div>
            </div>
          </div>
          <button
            v-if="gallery.commentsPage < gallery.commentsNumPages"
            class="btn load-more-comments"
            :disabled="gallery.commentsLoading"
            @click="g && gallery.loadMoreComments(g.id)"
          >
            <Loader v-if="gallery.commentsLoading" :size="14" class="spin" />
            {{ $t('gallery.load_more_comments') }}
          </button>
          <div v-if="gallery.commentsError" class="inline-error">{{ gallery.commentsError }}</div>
          <div v-else-if="!gallery.commentsLoading && !gallery.comments.length" class="empty">
            {{ $t('gallery.no_comments') }}
          </div>
        </div>
      </section>
    </div>

    <div v-else-if="loading" class="loading-skeleton">
      <div class="header">
        <div class="cover skeleton-pulse"></div>
        <div class="info">
          <div class="loading-status" role="status">
            <Loader :size="20" class="spin" />
            <span>{{ $t('common.loading') }}</span>
          </div>
          <div class="skeleton-line w-70"></div>
          <div class="skeleton-line w-30"></div>
          <div class="primary-actions loading-actions" aria-hidden="true">
            <button class="btn primary read-btn" disabled>
              <BookOpen :size="18" /> {{ $t('gallery.read') }}
            </button>
            <div class="tool-btns">
              <button class="btn" disabled><RefreshCw :size="14" /> {{ $t('common.refresh') }}</button>
              <button class="btn" disabled><Download :size="14" /> {{ $t('common.download') }}</button>
              <button class="btn" disabled><Star :size="14" /> {{ $t('gallery.favorite') }}</button>
            </div>
          </div>
        </div>
      </div>
      <div class="skeleton-section">
        <div class="skeleton-line w-15"></div>
        <div class="chips">
          <span class="skeleton-chip"></span>
          <span class="skeleton-chip w-60"></span>
          <span class="skeleton-chip w-50"></span>
          <span class="skeleton-chip w-40"></span>
          <span class="skeleton-chip w-30"></span>
        </div>
      </div>
      <div class="skeleton-section">
        <div class="skeleton-line w-20"></div>
        <div class="chips">
          <span class="skeleton-chip"></span>
          <span class="skeleton-chip w-45"></span>
          <span class="skeleton-chip w-55"></span>
        </div>
      </div>
      <div class="skeleton-section">
        <div class="skeleton-line w-25"></div>
        <div class="chips">
          <span class="skeleton-chip w-70"></span>
          <span class="skeleton-chip w-35"></span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.gallery-view {
  /* Keep the scroll container as wide as the window. Individual sections own
     their readable widths, while the page grid can use all available space. */
  --detail-gutter: clamp(14px, 2vw, 36px);
  width: 100%;
  max-width: none;
  /* min-width:0 is essential: without it a long unbreakable title (or any
     intrinsic-content child) would make this flex/block item grow to its
     content's min-content width, so the whole page would visibly track the
     title length. With it the page width follows the window, not the title. */
  min-width: 0;
  height: 100%;
  margin: 0;
  /* Keep content inset on the sides/bottom, but let the title bar occupy the
     real top edge. This avoids relying on a negative margin to cancel top
     padding, which can leave a gap with sticky positioning in some WebViews. */
  padding: 0 var(--detail-gutter) var(--detail-gutter);
  overflow-y: auto;
}
.error {
  padding: 12px 14px;
  background: rgba(255, 80, 80, 0.1);
  border: 1px solid rgba(255, 80, 80, 0.4);
  border-radius: 8px;
  color: #ff9e9e;
  margin-bottom: 14px;
  font-size: 0.85rem;
}
/* Pinned back + title row. Stays visible while the detail page scrolls so
   the user can always navigate away, even deep in a long comments/thread. */
.sticky-title-bar {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  gap: 10px;
  height: var(--app-header-height);
  padding: 0 var(--detail-gutter);
  margin: 0 calc(var(--detail-gutter) * -1) 18px;
  background: var(--bg);
  border-bottom: 1px solid var(--border);
}
.sticky-title-bar .back {
  font-size: 1.2rem;
  font-weight: 700;
  padding: 2px 10px;
  flex-shrink: 0;
}
.sticky-title {
  /* flex:1 + min-width:0 lets the title ellipsize instead of pushing the back
     button off-screen when the title is very long. */
  flex: 1;
  min-width: 0;
  font-size: 0.95rem;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.btn.ghost {
  background: transparent;
  border-color: transparent;
}
.header {
  display: flex;
  gap: clamp(20px, 2.2vw, 36px);
  width: min(100%, 1600px);
  margin: 0 auto 24px;
  padding: clamp(18px, 1.8vw, 28px);
  background: linear-gradient(135deg, var(--surface), var(--surface-2));
  border: 1px solid var(--border);
  border-radius: 14px;
}
.cover {
  width: clamp(220px, 15vw, 290px);
  flex-shrink: 0;
  aspect-ratio: 3 / 4;
  background: var(--surface);
  border-radius: 10px;
  overflow: hidden;
}
.cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-width: 1080px;
  padding: 4px 0;
}
.title {
  margin: 0;
  /* Explicitly constrain to the info column so the heading's own intrinsic
     (max-content) width can never push the column/page wider than the window.
     This is what keeps the detail page width driven by the window, not by the
     title length. */
  width: 100%;
  max-width: 100%;
  font-size: 1.35rem;
  line-height: 1.35;
  /* Long titles (no spaces, e.g. long romanized/japanese strings) must wrap
     rather than inflate the info column and overflow the window. */
  overflow-wrap: anywhere;
  word-break: break-word;
}
.title-stack {
  width: fit-content;
  max-width: 100%;
  min-width: 0;
}
.title-stack .title {
  width: auto;
}
.primary-actions {
  display: flex;
  align-items: stretch;
  gap: 10px;
  width: min(100%, 900px);
}
.read-btn {
  flex: 1 1 420px;
  font-size: 1rem;
  font-weight: 700;
  padding: 12px 24px;
  white-space: nowrap;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
}
.tool-btns {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
.tool-btns .btn {
  padding: 10px 14px;
  font-size: 0.82rem;
  white-space: nowrap;
}
.translated-title {
  /* Constrain + wrap so a long translated line can't inflate the column. */
  width: 100%;
  max-width: 100%;
  color: var(--accent);
  font-size: 1.05rem;
  font-weight: 500;
  font-style: normal;
  overflow-wrap: anywhere;
  word-break: break-word;
}
.reasoning-block {
  width: 100%;
  max-width: 100%;
  min-width: 0;
}
.reasoning-toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--text-dim);
  font-size: 0.75rem;
  cursor: pointer;
}
.reasoning-text {
  margin-top: 4px;
  height: calc(3 * 1.45em);
  padding: 6px 8px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.04);
  color: var(--text-dim);
  font-size: 0.75rem;
  font-style: italic;
  line-height: 1.45;
  white-space: pre-wrap;
  overflow-y: auto;
  overflow-wrap: anywhere;
}
.tl-error {
  color: #f08080;
  font-size: 0.82rem;
  padding: 6px 10px;
  background: rgba(220, 60, 60, 0.1);
  border-radius: 6px;
  overflow-wrap: anywhere;
}
.tl-error-hint {
  font-size: 0.78rem;
  color: var(--text-dim);
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.link-btn {
  background: none;
  border: none;
  color: var(--accent);
  cursor: pointer;
  text-decoration: underline;
  padding: 0;
  font-size: inherit;
}
.meta {
  color: var(--text-dim);
  font-size: 0.85rem;
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.btn.downloading {
  opacity: 0.7;
  cursor: default;
  color: var(--accent);
}
.btn.queued {
  opacity: 0.7;
  cursor: default;
  color: #ffce80;
}
.btn.done {
  opacity: 0.7;
  cursor: default;
  color: #6ec16e;
}
.body {
  margin-top: 8px;
  width: 100%;
  display: grid;
  gap: 18px;
}
.detail-card {
  min-width: 0;
  padding: clamp(16px, 1.6vw, 24px);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
}
.detail-card > .section-title {
  margin-top: 0;
}
.section-toggle-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.section-toggle-bar .section-title {
  margin: 0;
  margin-right: auto;
}
.section-toggle-bar + .thumb-grid,
.related-content {
  margin-top: 12px;
}
.tag-toggle-bar {
  display: flex;
  align-items: center;
}
.tags-content {
  margin-top: 12px;
}
.tag-group {
  margin-bottom: 14px;
}
.tag-group:last-child {
  margin-bottom: 0;
}
.related {
  margin-top: 0;
}
.page-thumbs {
  margin-top: 0;
}
.thumb-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 8px;
}
.thumb-item {
  aspect-ratio: 3 / 4;
  border-radius: 6px;
  overflow: hidden;
  background: var(--surface);
  cursor: pointer;
  position: relative;
  transition: transform 0.15s, box-shadow 0.15s;
}
.thumb-item:hover {
  transform: scale(1.05);
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  z-index: 1;
}
.thumb-item:active {
  transform: scale(0.97);
}
.thumb-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  opacity: 0;
  transition: opacity 0.3s ease;
}
.thumb-img.loaded {
  opacity: 1;
}
.thumb-label {
  position: absolute;
  bottom: 4px;
  right: 4px;
  background: rgba(0, 0, 0, 0.7);
  color: #fff;
  font-size: 0.7rem;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
  pointer-events: none;
}
.comments {
  margin-top: 0;
}
.comments-toolbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}
.comments-toolbar .btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.website-actions-hint {
  margin: 8px 0 0;
}
.load-more-comments {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 10px;
}
.inline-error {
  margin-top: 10px;
  color: #ff9e9e;
  font-size: 0.82rem;
  overflow-wrap: anywhere;
}
.comment {
  display: flex;
  gap: 10px;
  padding: 10px 0;
  border-top: 1px solid var(--border);
}
.comment-avatar {
  display: grid;
  place-items: center;
  flex: 0 0 38px;
  width: 38px;
  height: 38px;
  overflow: hidden;
  border-radius: 50%;
  color: var(--text-dim);
  background: var(--surface-2);
  font-size: 0.8rem;
  font-weight: 700;
}
.comment-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.comment-content {
  min-width: 0;
  flex: 1;
}
.comment .who {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: baseline;
  font-size: 0.78rem;
  color: var(--text-dim);
}
.comment-translate-button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--accent);
  font: inherit;
  cursor: pointer;
}
.comment-translate-button:disabled {
  color: var(--text-dim);
  cursor: default;
}
.comment-body {
  margin-top: 4px;
  font-size: 0.88rem;
  white-space: pre-wrap;
}
.comment-reasoning {
  margin-top: 8px;
}
.comment-reasoning-text {
  height: auto;
  max-height: calc(6 * 1.45em + 12px);
}
.translated-comment {
  margin-top: 8px;
  padding: 8px 10px;
  border-left: 2px solid var(--accent);
  border-radius: 0 6px 6px 0;
  background: var(--accent-soft);
  color: var(--text);
  font-size: 0.88rem;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.comment-translate-error {
  margin-top: 6px;
}
.empty {
  color: var(--text-dim);
  padding: 14px 0;
}

.loading-skeleton {
  animation: fadein 0.2s ease;
}
.loading-skeleton .header {
  display: flex;
  gap: 18px;
  margin-bottom: 18px;
}
.loading-skeleton .cover {
  width: 220px;
  flex-shrink: 0;
  aspect-ratio: 3 / 4;
}
.loading-skeleton .info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding-top: 6px;
}
.loading-status {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 32px;
  color: var(--text-dim);
  font-weight: 600;
}
.loading-actions {
  opacity: 0.72;
}
.skeleton-pulse {
  background: var(--surface-2);
  border-radius: 8px;
  animation: pulse 1.5s ease-in-out infinite;
}
.skeleton-line {
  height: 14px;
  background: var(--surface-2);
  border-radius: 6px;
  animation: pulse 1.5s ease-in-out infinite;
}
.skeleton-line.w-10 { width: 10%; }
.skeleton-line.w-15 { width: 15%; }
.skeleton-line.w-20 { width: 20%; }
.skeleton-line.w-25 { width: 25%; }
.skeleton-line.w-30 { width: 30%; }
.skeleton-line.w-40 { width: 40%; }
.skeleton-line.w-45 { width: 45%; }
.skeleton-line.w-50 { width: 50%; }
.skeleton-line.w-55 { width: 55%; }
.skeleton-line.w-60 { width: 60%; }
.skeleton-line.w-70 { width: 70%; }
.skeleton-line.w-100 { width: 100%; }
.skeleton-chip {
  display: inline-block;
  height: 26px;
  width: 80px;
  background: var(--surface-2);
  border-radius: 6px;
  animation: pulse 1.5s ease-in-out infinite;
}
.skeleton-chip.w-30 { width: 60px; }
.skeleton-chip.w-35 { width: 70px; }
.skeleton-chip.w-40 { width: 80px; }
.skeleton-chip.w-45 { width: 90px; }
.skeleton-chip.w-50 { width: 100px; }
.skeleton-chip.w-55 { width: 110px; }
.skeleton-chip.w-60 { width: 130px; }
.skeleton-chip.w-70 { width: 150px; }
.skeleton-section {
  margin-bottom: 18px;
  padding-bottom: 14px;
  border-bottom: 1px solid var(--border);
}
.skeleton-section .chips {
  margin-top: 10px;
}
@keyframes pulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 0.8; }
}
@keyframes fadein {
  from { opacity: 0; }
  to { opacity: 1; }
}

.overlay-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  height: var(--app-header-height);
  padding: 0 var(--detail-gutter);
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
  margin: 0 calc(var(--detail-gutter) * -1) 18px;
  /* Keep the back + title visible while the overlay detail scrolls. */
  position: sticky;
  top: 0;
  z-index: 5;
}
.overlay-bar .btn {
  background: transparent;
  border: none;
  color: var(--accent);
  padding: 4px 10px;
  cursor: pointer;
  font-size: 0.95rem;
  font-weight: 600;
}
.overlay-bar .btn:hover {
  background: var(--accent-soft);
  border-radius: 6px;
}
.overlay-title {
  /* min-width:0 + flex:1 so the title ellipsizes within the overlay bar rather
     than stretching the bar beyond the panel width. */
  flex: 1;
  min-width: 0;
  font-size: 0.95rem;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ---------------------------------------------------------------------------
   Responsive
   --------------------------------------------------------------------------- */
@media (max-width: 768px) {
  .gallery-view {
    --detail-gutter: 14px;
  }
  .header {
    flex-direction: column;
    align-items: center;
    gap: 16px;
    padding: 16px;
    border-radius: 10px;
  }
  .cover {
    width: 200px;
    max-width: 100%;
    aspect-ratio: 3 / 4;
  }
  .info {
    width: 100%;
    align-items: stretch;
    text-align: center;
  }
  .title {
    font-size: 1.15rem;
  }
  .title-stack {
    width: 100%;
    margin-inline: auto;
    text-align: center;
  }
  .title-stack .reasoning-text {
    text-align: left;
  }
  .meta {
    justify-content: center;
  }
  .primary-actions {
    flex-direction: column;
  }
  .read-btn {
    flex: 0 0 auto;
    width: 100%;
    font-size: 1.05rem;
    padding: 14px 20px;
  }
  .tool-btns {
    justify-content: center;
  }
  .actions {
    gap: 6px;
  }
  .actions .btn {
    flex: 1 1 auto;
    text-align: center;
  }
  .body {
    gap: 12px;
  }
  .detail-card {
    padding: 14px;
    border-radius: 10px;
  }
}

@media (min-width: 1440px) {
  .header {
    justify-content: center;
  }
  .title {
    font-size: 1.55rem;
    line-height: 1.4;
  }
  .info {
    flex: 0 1 1000px;
    width: min(65vw, 1000px);
    gap: 14px;
  }
  .primary-actions {
    width: 100%;
    flex-direction: column;
    align-items: flex-start;
  }
  .read-btn {
    flex: 0 0 auto;
    width: min(100%, 360px);
    min-height: 48px;
    font-size: 1.05rem;
  }
  .actions .btn,
  .tool-btns .btn {
    min-height: 40px;
  }
  .thumb-grid {
    gap: 12px;
  }
}
</style>
