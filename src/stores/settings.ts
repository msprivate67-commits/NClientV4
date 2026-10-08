import { defineStore } from "pinia";
import { ref, computed } from "vue";

import {
  authStatus,
  cloudflareCheck,
  cloudflareIsSolved,
  settingsGet,
  settingsSet,
  type AuthStatus,
} from "@/api";
import type { Settings } from "@/types";

export const DEFAULT_TITLE_TRANSLATION_TARGETS: Record<string, string> = {
  zh: "简体中文，尽量用古典章回体小说标题风格",
  en: "English, in the style of Shakespearean verse",
  ru: "Русский язык, в стиле стихов Пушкина",
  ja: "日本語、伝統的な俳句の文体",
};

export const DEFAULT_COMMENT_TRANSLATION_TARGETS: Record<string, string> = {
  zh: "简体中文，古典文言文风格，或诗句对联风格",
  en: "English, in a poetic style, such as verse or rhyming couplets",
  ru: "Русский язык, в классическом литературном или поэтическом стиле, включая рифмованные двустишия",
  ja: "日本語、古典的な文語調、または詩歌や対句の文体",
};

export function defaultTitleTranslationTarget(language: string): string {
  return DEFAULT_TITLE_TRANSLATION_TARGETS[language] ?? DEFAULT_TITLE_TRANSLATION_TARGETS.zh;
}

export function defaultCommentTranslationTarget(language: string): string {
  return DEFAULT_COMMENT_TRANSLATION_TARGETS[language] ?? DEFAULT_COMMENT_TRANSLATION_TARGETS.zh;
}

export function isDefaultTitleTranslationTarget(value: string): boolean {
  const normalized = value.trim();
  return !normalized || Object.values(DEFAULT_TITLE_TRANSLATION_TARGETS).includes(normalized);
}

export function isDefaultCommentTranslationTarget(value: string): boolean {
  const normalized = value.trim();
  return !normalized || Object.values(DEFAULT_COMMENT_TRANSLATION_TARGETS).includes(normalized);
}

const DEFAULT_SETTINGS: Settings = {
  mirror: "nhentai.net",
  user_agent: "",
  request_timeout_secs: 30,
  auth: {
    api_key: "",
    valid: false,
  },
  proxy_type: "none",
  proxy_host: "",
  proxy_port: 1080,
  proxy_username: "",
  proxy_password: "",
  sort_type: "recent_all_time",
  only_language: "all",
  title_type: "auto",
  exact_tag_match: false,
  remove_avoided_galleries: true,
  show_titles: true,
  page_thumbnail_columns: 0,
  use_rtl: false,
  default_zoom_pct: 100,
  reader_fit_mode: "height",
  reader_direction: "vertical",
  button_change_page: true,
  usage_wifi: "full",
  usage_mobile: "thumbnail",
  keep_history: true,
  max_history: 100,
  favorite_limit: 100,
  download_dir: "",
  parallel_downloads: 1,
  parallel_pages: 8,
  notifications_enabled: true,
  privacy_screen: false,
  clipboard_link_reader: true,
  lock_screen: false,
  pin: "",
  tl_base_url: "https://api.deepseek.com",
  tl_model: "deepseek-v4-flash",
  tl_api_key: "",
  tl_target_lang: "简体中文，尽量用古典章回体小说标题风格",
  tl_comment_target_lang: "简体中文，古典文言文风格，或诗句对联风格",
  tl_thinking: false,
  tl_auto_translate: true,
  tl_auto_translate_gallery_titles: false,
  tl_use_proxy: false,
  tl_cache_limit: 1000,
  app_language: "",
  theme: "system",
};

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<Settings>({ ...DEFAULT_SETTINGS });
  const loaded = ref(false);
  const auth = ref<AuthStatus>({
    has_credentials: false,
    api_key_valid: false,
    cloudflare_solved: false,
  });
  const cloudflareNeeded = ref(false);

  const mirror = computed(() => settings.value.mirror);
  const baseUrl = computed(() => `https://${settings.value.mirror}/`);

  async function load() {
    if (loaded.value) return settings.value;
    settings.value = await settingsGet();
    if (!settings.value.download_dir) {
      // Defensive default (backend normally fills this).
      settings.value.download_dir = "";
    }
    loaded.value = true;
    await refreshAuth();
    return settings.value;
  }

  async function save(patch: Partial<Settings>) {
    const next = { ...settings.value, ...patch };
    settings.value = await settingsSet(next);
    return settings.value;
  }

  async function syncTranslationTargetsForLanguage(language: string) {
    const patch: Partial<Settings> = {};
    const titleTarget = defaultTitleTranslationTarget(language);
    const commentTarget = defaultCommentTranslationTarget(language);
    if (
      isDefaultTitleTranslationTarget(settings.value.tl_target_lang)
      && settings.value.tl_target_lang !== titleTarget
    ) {
      patch.tl_target_lang = titleTarget;
    }
    if (
      isDefaultCommentTranslationTarget(settings.value.tl_comment_target_lang)
      && settings.value.tl_comment_target_lang !== commentTarget
    ) {
      patch.tl_comment_target_lang = commentTarget;
    }
    if (!Object.keys(patch).length) return settings.value;
    settings.value = await settingsSet({ ...settings.value, ...patch });
    return settings.value;
  }

  async function refreshAuth() {
    auth.value = await authStatus();
    return auth.value;
  }

  async function checkCloudflare() {
    try {
      cloudflareNeeded.value = await cloudflareCheck();
    } catch (e) {
      console.warn("cloudflare check failed", e);
      cloudflareNeeded.value = false;
    }
    return cloudflareNeeded.value;
  }

  async function isCloudflareSolved() {
    return cloudflareIsSolved();
  }

  return {
    settings,
    loaded,
    auth,
    cloudflareNeeded,
    mirror,
    baseUrl,
    load,
    save,
    refreshAuth,
    syncTranslationTargetsForLanguage,
    checkCloudflare,
    isCloudflareSolved,
  };
});
