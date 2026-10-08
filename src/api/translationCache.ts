import { invoke } from "@tauri-apps/api/core";

import type { TagTranslationInput } from "@/api";

/**
 * Persistent AI translation cache, one record per gallery keyed by the
 * gallery ID. The frontend consults it before every AI request so an
 * already-translated gallery never spends a second request.
 */
export interface TranslationCacheEntry {
  gallery_id: number;
  /** Fingerprint of the translation settings this entry was produced with. */
  config_key: string;
  title_source: string;
  title_translated: string;
  /** tag ID -> { name, translated } */
  tags: Record<string, { name: string; translated: string }>;
  /** comment ID -> { body, translated } */
  comments: Record<string, { body: string; translated: string }>;
  updated_at: string;
}

export const translationCacheGet = (galleryId: number): Promise<TranslationCacheEntry | null> =>
  invoke("translation_cache_get", { galleryId });

export const translationCacheClear = (): Promise<number> => invoke("translation_cache_clear");

export const translationCacheCount = (): Promise<number> => invoke("translation_cache_count");

/** Config fields that invalidate cached title / tag translations. */
export function titleCacheConfigKey(config: {
  baseUrl: string;
  model: string;
  targetLang: string;
  thinking: boolean;
  useProxy: boolean;
}): string {
  return JSON.stringify({
    baseUrl: config.baseUrl,
    model: config.model,
    targetLang: config.targetLang,
    thinking: config.thinking,
    useProxy: config.useProxy,
  });
}

/** Same fingerprint for comments, which use their own target language. */
export function commentCacheConfigKey(config: {
  baseUrl: string;
  model: string;
  commentTargetLang: string;
  thinking: boolean;
  useProxy: boolean;
}): string {
  return JSON.stringify({
    baseUrl: config.baseUrl,
    model: config.model,
    targetLang: config.commentTargetLang,
    thinking: config.thinking,
    useProxy: config.useProxy,
  });
}

function emptyEntry(galleryId: number, configKey: string): TranslationCacheEntry {
  return {
    gallery_id: galleryId,
    config_key: configKey,
    title_source: "",
    title_translated: "",
    tags: {},
    comments: {},
    updated_at: "",
  };
}

/**
 * Load a gallery's entry, but only one produced by the same translation
 * config — anything else is treated as a cache miss.
 */
export async function matchingCacheEntry(
  galleryId: number,
  configKey: string,
): Promise<TranslationCacheEntry | null> {
  if (!Number.isInteger(galleryId) || galleryId <= 0) return null;
  try {
    const entry = await translationCacheGet(galleryId);
    return entry && entry.config_key === configKey ? entry : null;
  } catch {
    // Cache failures must never block an actual translation.
    return null;
  }
}

async function writeEntry(entry: TranslationCacheEntry): Promise<void> {
  try {
    await invoke("translation_cache_set", { entry });
  } catch {
    // A failed cache write only costs a future AI request, never correctness.
  }
}

/** Merge a translated title into the gallery's cache entry. */
export async function cacheTitleTranslation(
  galleryId: number,
  configKey: string,
  sourceTitle: string,
  translated: string,
): Promise<void> {
  if (!Number.isInteger(galleryId) || galleryId <= 0 || !translated.trim()) return;
  const entry = await matchingCacheEntry(galleryId, configKey);
  const base = entry ?? emptyEntry(galleryId, configKey);
  base.title_source = sourceTitle;
  base.title_translated = translated;
  await writeEntry(base);
}

export interface TagCacheItems {
  id: number;
  name: string;
}

/** Merge tag translations into the gallery's cache entry. */
export async function cacheTagTranslations(
  galleryId: number,
  configKey: string,
  tags: TagCacheItems[],
  translations: Map<number, string>,
): Promise<void> {
  if (!Number.isInteger(galleryId) || galleryId <= 0 || !translations.size) return;
  const entry = await matchingCacheEntry(galleryId, configKey);
  const base = entry ?? emptyEntry(galleryId, configKey);
  for (const tag of tags) {
    const translated = translations.get(tag.id);
    if (translated?.trim()) base.tags[String(tag.id)] = { name: tag.name, translated };
  }
  await writeEntry(base);
}

export interface CommentCacheItems {
  id: number;
  body: string;
}

/** Merge comment translations into the gallery's cache entry. */
export async function cacheCommentTranslations(
  galleryId: number,
  configKey: string,
  comments: (CommentCacheItems & { translated: string })[],
): Promise<void> {
  if (!Number.isInteger(galleryId) || galleryId <= 0 || !comments.length) return;
  const entry = await matchingCacheEntry(galleryId, configKey);
  const base = entry ?? emptyEntry(galleryId, configKey);
  for (const comment of comments) {
    if (comment.translated.trim()) {
      base.comments[String(comment.id)] = {
        body: comment.body,
        translated: comment.translated,
      };
    }
  }
  await writeEntry(base);
}

/** Look up cached tag translations, split into hits and misses. */
export function splitCachedTags(
  entry: TranslationCacheEntry | null,
  tags: TagTranslationInput[],
): { hits: Map<number, string>; misses: TagTranslationInput[] } {
  const hits = new Map<number, string>();
  const misses: TagTranslationInput[] = [];
  for (const tag of tags) {
    const cached = entry?.tags[String(tag.id)];
    if (cached && cached.name === tag.name && cached.translated.trim()) {
      hits.set(tag.id, cached.translated);
    } else {
      misses.push(tag);
    }
  }
  return { hits, misses };
}

/** Look up one cached comment translation (source body must still match). */
export function cachedCommentTranslation(
  entry: TranslationCacheEntry | null,
  commentId: number,
  body: string,
): string | null {
  const cached = entry?.comments[String(commentId)];
  return cached && cached.body === body && cached.translated.trim()
    ? cached.translated
    : null;
}
