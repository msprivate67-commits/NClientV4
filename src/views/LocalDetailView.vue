<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import {
  apiGetComments,
  cacheCommentTranslations,
  cacheTitleTranslation,
  cachedCommentTranslation,
  cachedTitleFor,
  commentCacheConfigKey,
  imageProxyUrl,
  localGet,
  localGetMeta,
  localSetTranslatedTitle,
  matchingCacheEntry,
  openInBrowser,
  titleCacheConfigKey,
  translateComment,
  translateTitle,
} from "@/api";
import {
  BookOpen,
  ExternalLink,
  Loader,
  RefreshCw,
  Languages,
  ArrowLeft,
  ChevronDown,
  ChevronUp,
} from "@lucide/vue";
import DetailRelatedSection from "@/components/DetailRelatedSection.vue";
import DetailTagsSection from "@/components/DetailTagsSection.vue";
import { useOverlayStore } from "@/stores/overlay";
import { useSettingsStore } from "@/stores/settings";
import { useTagsStore } from "@/stores/tags";
import { stripLeadingId } from "@/utils/title";
import type { Comment, Gallery, LocalGallery, Tag } from "@/types";

const props = defineProps<{ folder: string; overlay?: boolean }>();
const emit = defineEmits<{ back: [] }>();
const router = useRouter();
const { t: i18n } = useI18n();
const overlayStore = useOverlayStore();
const settingsStore = useSettingsStore();
const tagsStore = useTagsStore();

const local = ref<LocalGallery | null>(null);
const loading = ref(true);
const translating = ref(false);
const translated = ref("");
const reasoningText = ref("");
const reasoningExpanded = ref(false);
const reasoningRef = ref<HTMLElement | null>(null);
const translateError = ref("");
const tagActionError = ref("");
let translationController: AbortController | null = null;
// Offline metadata (tags + related) read from the folder's `.nomedia` file.
// May be `null` for imported folders without a cached gallery JSON; the tags and
// related sections simply stay hidden in that case.
const meta = ref<Gallery | null>(null);
const tagsExpanded = ref(false);
const commentsOpen = ref(false);
const comments = ref<Comment[]>([]);
const commentsPage = ref(0);
const commentsNumPages = ref(1);
const commentsTotal = ref<number | null>(null);
const commentsLoading = ref(false);
const commentsError = ref<string | null>(null);

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

const title = computed(() => stripLeadingId(local.value?.title || `#${props.folder}`));
const translatedTitle = computed(() => local.value?.translated_title || "");
const onlineGalleryId = computed(() => local.value?.id ?? Number(props.folder));
const coverSrc = computed(() => {
  const t = local.value?.thumbnail_path;
  return t ? imageProxyUrl(t) : "";
});

// Tags grouped by type, mirroring GalleryView's tagsByType.
const tagsByType = computed(() => {
  const map = new Map<string, Tag[]>();
  if (!meta.value) return map;
  for (const t of meta.value.tags) {
    const list = map.get(t.type) ?? [];
    list.push(t);
    map.set(t.type, list);
  }
  return map;
});

async function load() {
  loading.value = true;
  try {
    local.value = await localGet(Number(props.folder));
  } catch {
    local.value = null;
  } finally {
    loading.value = false;
  }
  // Fetch the cached gallery metadata in parallel with rendering the header —
  // a missing/corrupt .nomedia returns null and the extra sections stay hidden.
  try {
    meta.value = await localGetMeta(Number(props.folder));
  } catch {
    meta.value = null;
  }
}

function resetComments() {
  commentsOpen.value = false;
  comments.value = [];
  commentsPage.value = 0;
  commentsNumPages.value = 1;
  commentsTotal.value = null;
  commentsLoading.value = false;
  commentsError.value = null;
  resetCommentTranslations();
}

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
  const s = settingsStore.settings;
  // Cache first: this gallery's stored translation for this exact comment
  // (source body must still match) skips the AI request.
  const galleryId = comment.gallery_id || local.value?.id || 0;
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
  const galleryId = onlineGalleryId.value;
  if (
    !commentsOpen.value
    || !galleryId
    || !settingsStore.settings.tl_auto_translate
    || !settingsStore.settings.tl_api_key.trim()
  ) return;

  comments.value.forEach((comment) => enqueueCommentTranslation(comment));
  if (loadingAllCommentPages || commentsLoading.value) return;

  loadingAllCommentPages = true;
  try {
    while (
      onlineGalleryId.value === galleryId
      && commentsOpen.value
      && commentsPage.value < commentsNumPages.value
    ) {
      const previousPage = commentsPage.value;
      await loadMoreComments();
      comments.value.forEach((comment) => enqueueCommentTranslation(comment));
      if (commentsPage.value <= previousPage) break;
    }
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

async function loadComments(page = 1) {
  const galleryId = onlineGalleryId.value;
  if (!galleryId || commentsLoading.value) return;
  commentsLoading.value = true;
  commentsError.value = null;
  try {
    const result = await apiGetComments(galleryId, page);
    comments.value = page === 1
      ? result.comments
      : [...comments.value, ...result.comments.filter(
          (incoming) => !comments.value.some((existing) => existing.id === incoming.id),
        )];
    commentsPage.value = result.page;
    commentsNumPages.value = result.num_pages;
    commentsTotal.value = result.total;
    void maybeAutoTranslateComments();
  } catch (error: unknown) {
    commentsError.value = error instanceof Error ? error.message : String(error);
  } finally {
    commentsLoading.value = false;
  }
}

async function loadMoreComments() {
  if (commentsPage.value >= commentsNumPages.value) return;
  await loadComments(commentsPage.value + 1);
}

async function toggleComments() {
  commentsOpen.value = !commentsOpen.value;
  if (commentsOpen.value && commentsPage.value === 0) {
    await loadComments();
  }
  if (commentsOpen.value) void maybeAutoTranslateComments();
}

async function doTranslate() {
  if (!local.value) return;
  translationController?.abort();
  const controller = new AbortController();
  translationController = controller;
  translating.value = true;
  translated.value = "";
  reasoningText.value = "";
  reasoningExpanded.value = true;
  translateError.value = "";
  const s = settingsStore.settings;

  // Cache first: a stored translation for this gallery produced by the same
  // config skips the AI request entirely.
  const galleryId = local.value.id;
  const cacheKey = titleCacheConfigKey({
    baseUrl: s.tl_base_url,
    model: s.tl_model,
    targetLang: s.tl_target_lang,
    thinking: s.tl_thinking,
    useProxy: s.tl_use_proxy,
  });
  const cachedEntry = await matchingCacheEntry(galleryId, cacheKey);
  const cachedTitle = cachedTitleFor(cachedEntry, title.value);
  if (cachedTitle.trim()) {
    translated.value = cachedTitle;
    reasoningExpanded.value = false;
    translating.value = false;
    translationController = null;
    return;
  }

  try {
    const result = await translateTitle(
      s.tl_base_url, s.tl_model, s.tl_api_key,
      title.value, s.tl_target_lang, s.tl_thinking,
      s.tl_use_proxy,
      {
        signal: controller.signal,
        onContent: (chunk) => {
          if (translationController === controller) translated.value += chunk;
        },
        onReasoning: (chunk) => {
          if (translationController === controller) reasoningText.value += chunk;
        },
      },
    );
    if (translationController !== controller) return;
    translated.value = result;
    await localSetTranslatedTitle(local.value.id, result);
    void cacheTitleTranslation(galleryId, cacheKey, title.value, result);
    if (local.value) {
      local.value = { ...local.value, translated_title: result };
    }
  } catch (e: unknown) {
    if (controller.signal.aborted || translationController !== controller) return;
    translateError.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (translationController === controller) {
      translating.value = false;
      translationController = null;
      if (reasoningText.value) reasoningExpanded.value = false;
    }
  }
}

function read() {
  overlayStore.openLocalReader(String(local.value?.id ?? props.folder));
}

function onTagClick(t: Tag) {
  // Same behavior as GalleryView: close any overlay panel, then search.
  if (props.overlay) {
    overlayStore.closeAll();
  }
  const name = encodeURIComponent(t.name);
  const type = encodeURIComponent(t.type);
  router.push({ name: "search", query: { tags: `${t.id}:accepted:${name}:${type}` } });
}

async function toggleTagBlacklist(tag: Tag) {
  if (!settingsStore.settings.auth.api_key.trim()) {
    tagActionError.value = String(i18n("tags.blacklist_requires_api_key"));
    return;
  }
  try {
    tagsStore.merge([tag]);
    if (tagsStore.blacklistedIds.has(tag.id)) {
      await tagsStore.removeBlacklist(tag.id);
    } else {
      await tagsStore.addBlacklist(tag.id);
    }
  } catch (cause) {
    tagActionError.value = String(cause);
  }
}

function goToSettings() {
  // From an overlay panel, close it first so the settings route renders full
  // screen (same pattern as onTagClick).
  if (props.overlay) {
    overlayStore.closeAll();
  }
  router.push({ name: "settings" });
}

onMounted(load);
watch(() => props.folder, () => {
  translationController?.abort();
  translationController = null;
  translating.value = false;
  translated.value = "";
  reasoningText.value = "";
  translateError.value = "";
  resetComments();
  void load();
});
watch(reasoningText, async () => {
  if (!reasoningExpanded.value) return;
  await nextTick();
  if (reasoningRef.value) reasoningRef.value.scrollTop = reasoningRef.value.scrollHeight;
});
watch(
  [
    commentsOpen,
    () => comments.value.map((comment) => comment.id).join(","),
    () => settingsStore.settings.tl_api_key,
    () => settingsStore.settings.tl_auto_translate,
  ],
  () => void maybeAutoTranslateComments(),
);
onUnmounted(() => {
  translationController?.abort();
  resetCommentTranslations();
});
</script>

<template>
  <div class="view" :class="{ 'overlay-mode': overlay }">
    <div v-if="overlay" class="overlay-bar glass-surface">
      <button class="btn" @click="emit('back')"><ArrowLeft :size="16" /></button>
      <span class="overlay-title">{{ title }}</span>
    </div>

    <div v-if="loading" class="loading">{{ $t('localDetail.loading') }}</div>
    <template v-else-if="local">
      <div class="header">
        <div class="cover" v-if="coverSrc">
          <img :src="coverSrc" :alt="title" />
        </div>
        <div class="info">
          <div class="title-stack">
            <h1 class="title">{{ title }}</h1>
            <div v-if="reasoningText" class="reasoning-block">
              <button class="reasoning-toggle" @click="reasoningExpanded = !reasoningExpanded">
                <ChevronUp v-if="reasoningExpanded" :size="13" />
                <ChevronDown v-else :size="13" />
                {{ $t('localDetail.reasoning') }}
              </button>
              <div v-show="reasoningExpanded" ref="reasoningRef" class="reasoning-text">{{ reasoningText }}</div>
            </div>
          </div>
          <div v-if="translatedTitle || translated" class="translated-title">
            {{ translated || translatedTitle }}
          </div>
          <div v-if="translateError" class="tl-error">{{ translateError }}</div>
          <div v-if="translateError" class="tl-error-hint">
            {{ $t('localDetail.translate_error_hint') }}
            <button class="link-btn" @click="goToSettings">{{ $t('localDetail.go_to_ai_settings') }}</button>
          </div>
          <div class="meta">
            <span>#{{ onlineGalleryId }}</span>
            <span>路</span>
            <span>{{ local.num_pages }} {{ $t('localDetail.pages') }}</span>
          </div>
          <div class="primary-actions">
            <button class="btn primary read-btn" @click="read"><BookOpen :size="18" /> {{ $t('localDetail.read') }}</button>
            <div class="tool-btns">
              <button
                class="btn"
                :disabled="translating"
                @click="doTranslate"
              >
                <span v-if="translating"><Loader :size="14" class="spin" /> {{ $t('localDetail.translating') }}</span>
                <span v-else-if="translatedTitle || translated"><RefreshCw :size="14" /> {{ $t('localDetail.retranslate') }}</span>
                <span v-else><Languages :size="14" /> {{ $t('localDetail.translate') }}</span>
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- Tags + related: sourced from the cached gallery JSON (.nomedia).
           Mirrors the online GalleryView layout. Only rendered when metadata
           is present. -->
      <div class="body">
        <div v-if="tagActionError" class="error">{{ tagActionError }}</div>
        <DetailTagsSection
          v-if="tagsByType.size"
          v-model:expanded="tagsExpanded"
          :groups="tagsByType"
          :blacklisted-ids="tagsStore.blacklistedIds"
          :gallery-id="local.id"
          @select="onTagClick"
          @toggle-blacklist="toggleTagBlacklist"
        />

        <DetailRelatedSection
          v-if="meta && meta.related.length"
          :galleries="meta.related"
          :collapsible="false"
        />

        <section v-if="onlineGalleryId" class="comments detail-card">
          <div class="section-toggle-bar comments-toolbar">
            <div class="section-title">
              {{ $t('gallery.section_comments') }}
              <span v-if="commentsTotal !== null">({{ commentsTotal }})</span>
            </div>
            <button
              class="btn small"
              type="button"
              :aria-expanded="commentsOpen"
              @click="toggleComments"
            >
              <ChevronUp v-if="commentsOpen" :size="14" />
              <ChevronDown v-else :size="14" />
              {{ commentsOpen ? $t('gallery.collapse') : $t('gallery.expand') }}
            </button>
            <button
              v-if="commentsOpen"
              class="btn"
              @click="openInBrowser(`g/${onlineGalleryId}/#comment-post-container`)"
            >
              <ExternalLink :size="14" /> {{ $t('gallery.add_comment') }}
            </button>
          </div>
          <p v-if="commentsOpen" class="hint website-actions-hint">
            {{ $t('gallery.website_actions_hint') }}
          </p>
          <div v-if="commentsOpen" class="comment-list">
            <div v-if="commentsLoading && commentsPage === 0" class="empty">
              {{ $t('common.loading') }}
            </div>
            <div v-for="c in comments" :key="c.id" class="comment">
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
              v-if="commentsPage < commentsNumPages"
              class="btn load-more-comments"
              :disabled="commentsLoading"
              @click="loadMoreComments"
            >
              <Loader v-if="commentsLoading" :size="14" class="spin" />
              {{ $t('gallery.load_more_comments') }}
            </button>
            <div v-if="commentsError" class="inline-error">{{ commentsError }}</div>
            <div v-else-if="!commentsLoading && !comments.length" class="empty">
              {{ $t('gallery.no_comments') }}
            </div>
          </div>
        </section>
      </div>
    </template>
    <div v-else class="error">{{ $t('localDetail.load_error') }}</div>
  </div>
</template>

<style scoped>
.view {
  --detail-gutter: clamp(14px, 2vw, 36px);
  width: 100%;
  max-width: none;
  /* min-width:0 keeps the page width driven by the window rather than by the
     title's intrinsic (max-content) width — see GalleryView for the same rule. */
  min-width: 0;
  margin: 0;
  padding: 0 var(--detail-gutter) var(--detail-gutter);
  overflow-y: auto;
  height: 100%;
}
.overlay-mode {
  height: 100%;
  overflow-y: auto;
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
  /* Constrain to the info column so a long title can't push the page wider
     than the window — page width follows the window, not the title length. */
  width: 100%;
  max-width: 100%;
  font-size: 1.35rem;
  line-height: 1.35;
  /* Long titles must wrap rather than inflate the info column and overflow. */
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
.translated-title {
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
.loading, .error {
  color: var(--text-dim);
  padding: 20px;
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
.section-title {
  font-size: 0.82rem;
  color: var(--text-dim);
  margin: 18px 0 6px;
  text-transform: capitalize;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
/* Related galleries: thumbnail grid (same component as the online detail page).
   GalleryCard handles click → online detail. */
.related {
  margin-top: 0;
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
  overflow-wrap: anywhere;
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
@media (max-width: 768px) {
  .view {
    --detail-gutter: 14px;
    max-width: 100%;
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
  .primary-actions > .btn:not(.read-btn) {
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
  .tool-btns .btn {
    min-height: 40px;
  }
}
</style>
