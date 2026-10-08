import { defineStore } from "pinia";
import { ref } from "vue";

import {
  cacheTitleTranslation,
  matchingCacheEntry,
  titleCacheConfigKey,
  translateTitle,
} from "@/api";
import { useSettingsStore } from "@/stores/settings";

export interface TitleTranslationState {
  sourceTitle: string;
  configKey: string;
  translated: string;
  reasoning: string;
  queued: boolean;
  translating: boolean;
  error: string;
}

interface TranslationJob {
  galleryId: number;
  sourceTitle: string;
  config: TranslationConfig;
  revision: number;
}

interface TranslationConfig {
  baseUrl: string;
  model: string;
  apiKey: string;
  targetLang: string;
  thinking: boolean;
  useProxy: boolean;
}

const TITLE_TRANSLATION_CONCURRENCY = 4;

export const useTitleTranslationsStore = defineStore("titleTranslations", () => {
  const settings = useSettingsStore();
  const entries = ref(new Map<number, TitleTranslationState>());
  const controllers = new Map<number, AbortController>();
  const revisions = new Map<number, number>();
  let pending: TranslationJob[] = [];
  let active = 0;

  function entry(galleryId: number): TitleTranslationState | undefined {
    return entries.value.get(galleryId);
  }

  function update(galleryId: number, patch: Partial<TitleTranslationState>) {
    const previous = entry(galleryId) ?? {
      sourceTitle: "",
      configKey: "",
      translated: "",
      reasoning: "",
      queued: false,
      translating: false,
      error: "",
    };
    const next = new Map(entries.value);
    next.set(galleryId, { ...previous, ...patch });
    entries.value = next;
  }

  function promote(galleryId: number) {
    const index = pending.findIndex((job) => job.galleryId === galleryId);
    if (index <= 0) return;
    const [job] = pending.splice(index, 1);
    pending.unshift(job);
  }

  function enqueue(
    galleryId: number,
    sourceTitle: string,
    options: { priority?: boolean; force?: boolean } = {},
  ) {
    const title = sourceTitle.trim();
    if (!Number.isInteger(galleryId) || galleryId <= 0 || !title) return;
    if (!settings.settings.tl_api_key.trim()) return;

    const config: TranslationConfig = {
      baseUrl: settings.settings.tl_base_url,
      model: settings.settings.tl_model,
      apiKey: settings.settings.tl_api_key,
      targetLang: settings.settings.tl_target_lang,
      thinking: settings.settings.tl_thinking,
      useProxy: settings.settings.tl_use_proxy,
    };
    const configKey = titleCacheConfigKey(config);
    const current = entry(galleryId);
    if (!options.force && current?.sourceTitle === title && current.configKey === configKey) {
      if (current.queued && options.priority) promote(galleryId);
      if (current.queued || current.translating || current.translated) return;
    }

    pending = pending.filter((job) => job.galleryId !== galleryId);
    controllers.get(galleryId)?.abort();
    const revision = (revisions.get(galleryId) ?? 0) + 1;
    revisions.set(galleryId, revision);
    update(galleryId, {
      sourceTitle: title,
      configKey,
      translated: "",
      reasoning: "",
      queued: true,
      translating: false,
      error: "",
    });
    const job = { galleryId, sourceTitle: title, config, revision };
    if (options.priority) pending.unshift(job);
    else pending.push(job);
    pump();
  }

  function pump() {
    while (active < TITLE_TRANSLATION_CONCURRENCY && pending.length) {
      const job = pending.shift();
      if (!job) break;
      active += 1;
      void run(job).finally(() => {
        active -= 1;
        pump();
      });
    }
  }

  async function run(job: TranslationJob) {
    if (revisions.get(job.galleryId) !== job.revision) return;
    const controller = new AbortController();
    controllers.set(job.galleryId, controller);
    update(job.galleryId, {
      queued: false,
      translating: true,
      translated: "",
      reasoning: "",
      error: "",
    });

    // Persistent cache first: a stored translation produced by the same
    // config for the same source title skips the AI request entirely.
    const cacheKey = titleCacheConfigKey(job.config);
    const cachedEntry = await matchingCacheEntry(job.galleryId, cacheKey);
    const cachedTitle =
      cachedEntry && cachedEntry.title_source === job.sourceTitle
        ? cachedEntry.title_translated
        : "";
    if (cachedTitle.trim()) {
      if (!isCurrent(job, controller)) return;
      update(job.galleryId, { translated: cachedTitle, translating: false });
      return;
    }

    try {
      const translated = await translateTitle(
        job.config.baseUrl,
        job.config.model,
        job.config.apiKey,
        job.sourceTitle,
        job.config.targetLang,
        job.config.thinking,
        job.config.useProxy,
        {
          signal: controller.signal,
          onContent: (chunk) => {
            if (!isCurrent(job, controller)) return;
            update(job.galleryId, {
              translated: `${entry(job.galleryId)?.translated ?? ""}${chunk}`,
            });
          },
          onReasoning: (chunk) => {
            if (!isCurrent(job, controller)) return;
            update(job.galleryId, {
              reasoning: `${entry(job.galleryId)?.reasoning ?? ""}${chunk}`,
            });
          },
        },
      );
      if (!isCurrent(job, controller)) return;
      update(job.galleryId, { translated, translating: false });
      void cacheTitleTranslation(job.galleryId, cacheKey, job.sourceTitle, translated);
    } catch (error: unknown) {
      if (!isCurrent(job, controller)) return;
      update(job.galleryId, {
        translating: false,
        error: error instanceof Error ? error.message : String(error),
      });
    } finally {
      if (controllers.get(job.galleryId) === controller) {
        controllers.delete(job.galleryId);
      }
    }
  }

  function isCurrent(job: TranslationJob, controller: AbortController) {
    return !controller.signal.aborted && revisions.get(job.galleryId) === job.revision;
  }

  return { entries, entry, enqueue };
});
