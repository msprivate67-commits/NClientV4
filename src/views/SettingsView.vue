<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { save as taSave, open as taOpen } from "@tauri-apps/plugin-dialog";
import { writeTextFile, readTextFile } from "@tauri-apps/plugin-fs";
import { platform } from "@tauri-apps/plugin-os";
import { useI18n } from "vue-i18n";
import { Check, ExternalLink, Monitor, Moon, Sun } from "@lucide/vue";
import { SUPPORTED_LANGUAGES, exportLocaleJson, applyImportedMessages, setLocale, getLocale, type AppLanguage } from "@/i18n";

import {
  authClear,
  authSetApiKey,
  cloudflareCheck,
  cloudflareIsSolved,
  cloudflareOpenChallenge,
  openApiKeyDocs,
  settingsClearCookies,
  settingsGetPaths,
  settingsPickDirectory,
  settingsListDownloadCandidates,
  testTranslationConnection,
  translationCacheClear,
  translationCacheCount,
} from "@/api";
import {
  defaultCommentTranslationTarget,
  defaultTitleTranslationTarget,
  isDefaultCommentTranslationTarget,
  isDefaultTitleTranslationTarget,
  useSettingsStore,
} from "@/stores/settings";
import { useFavoritesStore } from "@/stores/favorites";
import { useScrollCache } from "@/composables/useScrollCache";
import { applyTheme } from "@/composables/useTheme";
import type { Settings, ThemePreference } from "@/types";

const i18n = useI18n();

const settings = useSettingsStore();
const favorites = useFavoritesStore();
const draft = ref(JSON.parse(JSON.stringify(settings.settings)));
const saved = ref(false);
const appData = ref<string>("");
const viewRef = ref<HTMLElement | null>(null);
useScrollCache(viewRef);
const apiKeyInput = ref("");
const cfNeeded = ref(false);
const cfSolved = ref(false);
const isAndroid = platform() === "android";

const currentLang = ref<string>(getLocale());
const importMissing = ref<string[] | null>(null);
const importError = ref("");
const langSaved = ref(false);

const dirty = computed(() => JSON.stringify(draft.value) !== JSON.stringify(settings.settings));

watch(() => settings.loaded, (loaded) => {
  if (loaded) draft.value = JSON.parse(JSON.stringify(settings.settings));
}, { immediate: true });

// AI connection test state. Based on the live draft values, so the user can
// tweak base URL / model / key and re-test without saving first.
const tlTesting = ref(false);
const tlResult = ref<{ ok: boolean; message: string } | null>(null);

// AI translation cache state: how many galleries are stored, plus the clear
// action. The per-gallery cap itself is part of the draft settings above.
const cacheCount = ref(0);
const cacheClearing = ref(false);

async function refreshCacheCount() {
  try {
    cacheCount.value = await translationCacheCount();
  } catch (e) {
    console.warn(e);
  }
}

async function clearTranslationCache() {
  const n = cacheCount.value;
  if (n <= 0 || cacheClearing.value) return;
  if (!window.confirm(String(i18n.t("settings.ai_cache_clear_confirm", { n })))) return;
  cacheClearing.value = true;
  try {
    await translationCacheClear();
    cacheCount.value = 0;
  } finally {
    cacheClearing.value = false;
  }
}

async function testAiConnection() {
  tlTesting.value = true;
  tlResult.value = null;
  try {
    tlResult.value = await testTranslationConnection(
      draft.value.tl_base_url,
      draft.value.tl_model,
      draft.value.tl_api_key,
      draft.value.tl_use_proxy,
    );
  } finally {
    tlTesting.value = false;
  }
}

async function save() {
  await settings.save(draft.value);
  tlResult.value = null;
  draft.value = JSON.parse(JSON.stringify(settings.settings));
  saved.value = true;
  setTimeout(() => (saved.value = false), 1500);
}

async function changeTheme(theme: ThemePreference) {
  const previous = settings.settings.theme;
  draft.value.theme = theme;
  applyTheme(theme);
  try {
    await settings.save({ theme });
  } catch (error) {
    draft.value.theme = previous;
    applyTheme(previous);
    throw error;
  }
}

async function pickDownloadDir() {
  try {
    const selected = await taOpen({ directory: true, multiple: false });
    if (typeof selected === "string") {
      draft.value.download_dir = selected;
      return;
    }
    if (selected === null) return;
  } catch {
    // Dialog might fail on some platforms (e.g. Android) — try backend fallback
  }
  // No native dialog (typical on Android): offer the backend-provided
  // app-scoped candidate directories so no broad storage permission is needed.
  try {
    const candidates = await settingsListDownloadCandidates();
    if (candidates.length === 0) {
      const picked = await settingsPickDirectory();
      if (picked) draft.value.download_dir = picked;
      return;
    }
    if (candidates.length === 1) {
      draft.value.download_dir = candidates[0][1];
      return;
    }
    const menu = candidates
      .map(([label, path], i) => `${i + 1}. ${label}\n   ${path}`)
      .join("\n\n");
    const choice = window.prompt(
      `Choose a download directory (enter the number):\n\n${menu}`,
      "2",
    );
    const idx = Number.parseInt((choice ?? "").trim(), 10) - 1;
    if (Number.isInteger(idx) && idx >= 0 && idx < candidates.length) {
      draft.value.download_dir = candidates[idx][1];
    }
  } catch {
    // Both failed; user can type manually in the editable field
  }
}

async function saveApiKey() {
  if (!apiKeyInput.value.trim()) return;
  const updated = await authSetApiKey(apiKeyInput.value.trim());
  settings.settings = updated;
  apiKeyInput.value = "";
  await settings.refreshAuth();
  draft.value = JSON.parse(JSON.stringify(settings.settings));
  await favorites.syncAfterLogin();
}

async function clearAuth() {
  settings.settings = await authClear();
  await settings.refreshAuth();
  draft.value.auth = {
    api_key: "",
    valid: false,
  };
  await favorites.load();
}

async function clearCookies() {
  await settingsClearCookies();
}

async function checkCf() {
  cfNeeded.value = await cloudflareCheck();
  cfSolved.value = await cloudflareIsSolved();
}

async function solveCf() {
  await cloudflareOpenChallenge();
}

async function changeLang(code: AppLanguage) {
  const useTitleDefault = isDefaultTitleTranslationTarget(draft.value.tl_target_lang);
  const useCommentDefault = isDefaultCommentTranslationTarget(
    draft.value.tl_comment_target_lang,
  );
  currentLang.value = code;
  setLocale(code);
  i18n.locale.value = code;
  const patch: Partial<Settings> = { app_language: code };
  if (useTitleDefault) patch.tl_target_lang = defaultTitleTranslationTarget(code);
  if (useCommentDefault) {
    patch.tl_comment_target_lang = defaultCommentTranslationTarget(code);
  }
  await settings.save(patch);
  draft.value = JSON.parse(JSON.stringify(settings.settings));
  langSaved.value = true;
  setTimeout(() => langSaved.value = false, 1500);
}

async function exportLang() {
  try {
    const json = exportLocaleJson(currentLang.value);
    const path = await taSave({
      defaultPath: `nclientv4-${currentLang.value}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (path) {
      await writeTextFile(path, json);
    }
  } catch (e) {
    console.error("export failed", e);
  }
}

async function importLang() {
  importError.value = "";
  importMissing.value = null;
  try {
    const selected = await taOpen({
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!selected) return;
    const path = selected as string;
    const content = await readTextFile(path);
    const { missingKeys } = applyImportedMessages(currentLang.value, content);
    importMissing.value = missingKeys;
    i18n.locale.value = currentLang.value;
    langSaved.value = true;
    setTimeout(() => langSaved.value = false, 1500);
  } catch (e: any) {
    importError.value = String(e?.message ?? e);
  }
}

onMounted(async () => {
  draft.value = JSON.parse(JSON.stringify(settings.settings));
  try {
    const paths = await settingsGetPaths();
    appData.value = paths.app_data;
  } catch (e) {
    console.warn(e);
  }
  checkCf();
  refreshCacheCount();
});
</script>

<template>
  <div ref="viewRef" class="view settings">
    <div class="view-header">
      <div class="view-title">{{ $t('settings.title') }}</div>
    </div>

    <div class="settings-grid">
    <section class="settings-card settings-card--wide">
      <div class="section-title">{{ $t('settings.section_language') }}</div>
      <div class="row">
        <label style="min-width: 120px;">{{ $t('settings.app_language') }}</label>
        <select v-model="currentLang" @change="changeLang(currentLang as AppLanguage)">
          <option v-for="lang in SUPPORTED_LANGUAGES" :key="lang.code" :value="lang.code">
            {{ lang.nativeName }}
          </option>
        </select>
        <span v-if="langSaved" class="ok" style="font-size:0.82rem;">{{ $t('common.saved') }}</span>
      </div>
      <p class="hint">{{ $t('settings.app_language_hint') }}</p>
      <div class="row" style="margin-top: 8px;">
        <button class="btn" @click="exportLang">{{ $t('settings.export_lang') }}</button>
        <button class="btn" @click="importLang">{{ $t('settings.import_lang') }}</button>
      </div>
      <p class="hint" style="margin-top: 4px;">{{ $t('settings.export_lang_hint') }}</p>
      <p class="hint">{{ $t('settings.import_lang_hint') }}</p>
      <div v-if="importError" class="tl-error" style="margin-top: 6px;">{{ importError }}</div>
      <div v-if="importMissing !== null && importMissing.length > 0" class="error" style="margin-top: 6px; font-size:0.8rem;">
        Missing keys ({{ importMissing.length }}) — falling back to English:
        <div style="max-height:80px; overflow-y:auto; margin-top:4px;">
          <span v-for="k in importMissing" :key="k" style="display:inline-block; margin:2px 4px; background:var(--surface-2); padding:1px 6px; border-radius:4px; font-size:0.7rem;">{{ k }}</span>
        </div>
      </div>
      <div v-else-if="importMissing !== null && importMissing.length === 0" class="ok" style="margin-top: 6px; font-size:0.82rem;">
        <Check :size="14" /> All keys present
      </div>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_site') }}</div>
      <div class="fields">
        <div class="field">
          <label>{{ $t('settings.mirror_host') }}</label>
          <input v-model="draft.mirror" type="text" placeholder="nhentai.net" />
        </div>
        <div class="field">
          <label>{{ $t('settings.user_agent') }}</label>
          <input v-model="draft.user_agent" type="text" placeholder="NClientV4/4.0.1 ..." />
        </div>
        <div class="field">
          <label>{{ $t('settings.request_timeout') }}</label>
          <input v-model.number="draft.request_timeout_secs" type="number" min="5" max="300" />
        </div>
      </div>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_proxy') }}</div>
      <div class="fields">
        <div class="field">
          <label>{{ $t('settings.proxy_type') }}</label>
          <select v-model="draft.proxy_type">
            <option value="none">{{ $t('settings.proxy_none') }}</option>
            <option value="http">{{ $t('settings.proxy_http') }}</option>
            <option value="socks5">{{ $t('settings.proxy_socks5') }}</option>
          </select>
        </div>
        <div class="field">
          <label>{{ $t('settings.host') }}</label>
          <input v-model="draft.proxy_host" type="text" placeholder="127.0.0.1" :disabled="draft.proxy_type === 'none'" />
        </div>
        <div class="field">
          <label>{{ $t('settings.port') }}</label>
          <input v-model.number="draft.proxy_port" type="number" min="1" max="65535" :disabled="draft.proxy_type === 'none'" />
        </div>
      </div>
      <div class="fields" style="margin-top: 8px;">
        <div class="field">
          <label>{{ $t('settings.username_optional') }}</label>
          <input v-model="draft.proxy_username" type="text" placeholder="optional" :disabled="draft.proxy_type === 'none'" />
        </div>
        <div class="field">
          <label>{{ $t('settings.password_optional') }}</label>
          <input v-model="draft.proxy_password" type="password" placeholder="optional" :disabled="draft.proxy_type === 'none'" />
        </div>
      </div>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_cloudflare') }}</div>
      <div class="row">
        <span>{{ $t('settings.cf_status') }}</span>
        <strong v-if="cfSolved" class="ok">{{ $t('settings.cf_solved') }}</strong>
        <strong v-else-if="cfNeeded" class="warn">{{ $t('settings.cf_challenge_needed') }}</strong>
        <strong v-else>{{ $t('settings.cf_unknown') }}</strong>
        <button class="btn" @click="checkCf">{{ $t('settings.check') }}</button>
        <button class="btn" :disabled="cfSolved" @click="solveCf">{{ $t('settings.solve') }}</button>
        <button class="btn" @click="clearCookies">{{ $t('settings.clear_cookies') }}</button>
      </div>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_api') }}</div>
      <div class="row">
        <span>{{ $t('settings.has_key') }}</span>
        <strong>{{ settings.auth.has_credentials ? $t('common.yes') : $t('common.no') }}</strong>
        <strong v-if="settings.auth.has_credentials" :class="{ ok: settings.auth.api_key_valid, warn: !settings.auth.api_key_valid }">
          {{ settings.auth.api_key_valid ? $t('settings.key_valid') : $t('settings.key_invalid') }}
        </strong>
      </div>
      <div class="row">
        <input v-model="apiKeyInput" type="password" :placeholder="$t('settings.paste_api_key')" />
        <button class="btn primary" @click="saveApiKey">{{ $t('settings.save_key') }}</button>
        <button class="btn" @click="openApiKeyDocs">
          <ExternalLink :size="14" /> {{ $t('settings.get_api_key') }}
        </button>
        <button class="btn danger" :disabled="!settings.auth.has_credentials" @click="clearAuth">{{ $t('settings.clear') }}</button>
      </div>
      <p class="hint">{{ $t('settings.api_key_steps') }}</p>
      <p class="hint" v-html="$t('settings.api_hint', { code: '<code>Authorization: Key &lt;key&gt;</code>' })"></p>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_browsing') }}</div>
      <div class="checkboxes">
        <label>
          <input v-model="draft.clipboard_link_reader" type="checkbox" />
          {{ $t('settings.clipboard_link_reader') }}
        </label>
      </div>
      <p class="hint">{{ $t('settings.clipboard_link_reader_hint') }}</p>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_display') }}</div>
      <div class="theme-setting">
        <span class="theme-label">{{ $t('settings.theme') }}</span>
        <div class="theme-toggle" role="radiogroup" :aria-label="$t('settings.theme')">
          <button type="button" role="radio" :aria-checked="draft.theme === 'dark'" :class="{ active: draft.theme === 'dark' }" @click="changeTheme('dark')">
            <Moon :size="16" />
            <span>{{ $t('settings.theme_dark') }}</span>
          </button>
          <button type="button" role="radio" :aria-checked="draft.theme === 'system'" :class="{ active: draft.theme === 'system' }" @click="changeTheme('system')">
            <Monitor :size="16" />
            <span>{{ $t('settings.theme_system') }}</span>
          </button>
          <button type="button" role="radio" :aria-checked="draft.theme === 'light'" :class="{ active: draft.theme === 'light' }" @click="changeTheme('light')">
            <Sun :size="16" />
            <span>{{ $t('settings.theme_light') }}</span>
          </button>
        </div>
      </div>
      <div class="fields">
        <div class="field">
          <label>{{ $t('settings.page_thumb_columns') }}</label>
          <input v-model.number="draft.page_thumbnail_columns" type="number" min="0" max="10" />
        </div>
        <div class="field">
          <label>{{ $t('settings.default_zoom') }}</label>
          <input v-model.number="draft.default_zoom_pct" type="number" min="20" max="300" />
        </div>
      </div>
      <div class="checkboxes">
        <label><input type="checkbox" v-model="draft.button_change_page" /> {{ $t('settings.page_change_buttons') }}</label>
        <label v-if="isAndroid">
          <input v-model="draft.privacy_screen" type="checkbox" />
          {{ $t('settings.privacy_screen') }}
        </label>
      </div>
      <p v-if="isAndroid" class="hint">{{ $t('settings.privacy_screen_hint') }}</p>
    </section>

    <section class="settings-card">
      <div class="section-title">{{ $t('settings.section_downloads') }}</div>
      <div class="field path-field">
        <label>{{ $t('settings.download_dir') }}</label>
        <div class="row">
          <input v-model="draft.download_dir" type="text" :placeholder="$t('settings.download_dir_placeholder')" />
          <button class="btn" @click="pickDownloadDir">{{ $t('settings.browse') }}</button>
        </div>
      </div>
      <div class="checkboxes">
        <label>
          <input v-model="draft.notifications_enabled" type="checkbox" />
          {{ $t('settings.download_notifications') }}
        </label>
      </div>
      <p class="hint">{{ $t('settings.download_notifications_hint') }}</p>
    </section>

    <section class="settings-card settings-card--wide">
      <div class="section-title">{{ $t('settings.section_ai') }}</div>
      <div class="fields">
        <div class="field">
          <label>{{ $t('settings.ai_base_url') }}</label>
          <input v-model="draft.tl_base_url" type="text" placeholder="https://api.deepseek.com" />
        </div>
        <div class="field">
          <label>{{ $t('settings.ai_model') }}</label>
          <input v-model="draft.tl_model" type="text" placeholder="deepseek-v4-flash" />
        </div>
        <div class="field">
          <label>{{ $t('settings.ai_api_key') }}</label>
          <input v-model="draft.tl_api_key" type="password" placeholder="sk-…" />
        </div>
        <div class="field">
          <label>{{ $t('settings.ai_title_target_lang') }}</label>
          <input
            v-model="draft.tl_target_lang"
            type="text"
            :placeholder="defaultTitleTranslationTarget(currentLang)"
          />
        </div>
        <div class="field">
          <label>{{ $t('settings.ai_comment_target_lang') }}</label>
          <input
            v-model="draft.tl_comment_target_lang"
            type="text"
            :placeholder="defaultCommentTranslationTarget(currentLang)"
          />
        </div>
      </div>
      <div class="checkboxes">
        <label><input type="checkbox" v-model="draft.tl_thinking" /> {{ $t('settings.ai_thinking') }}</label>
        <label><input type="checkbox" v-model="draft.tl_auto_translate" /> {{ $t('settings.ai_auto_translate') }}</label>
        <label><input type="checkbox" v-model="draft.tl_auto_translate_gallery_titles" /> {{ $t('settings.ai_auto_translate_gallery_titles') }}</label>
        <label><input type="checkbox" v-model="draft.tl_use_proxy" /> {{ $t('settings.ai_use_proxy') }}</label>
      </div>
      <div class="fields" style="margin-top: 10px;">
        <div class="field">
          <label>{{ $t('settings.ai_cache_limit') }}</label>
          <input v-model.number="draft.tl_cache_limit" type="number" min="0" max="1000000" step="100" />
        </div>
      </div>
      <p class="hint">{{ $t('settings.ai_cache_limit_hint') }}</p>
      <div class="row" style="margin-top: 10px;">
        <span>{{ $t('settings.ai_cache_count', { n: cacheCount }) }}</span>
        <button
          class="btn danger"
          :disabled="cacheCount === 0 || cacheClearing"
          @click="clearTranslationCache"
        >
          {{ $t('settings.ai_cache_clear') }}
        </button>
      </div>
      <p class="hint">{{ $t('settings.ai_cache_hint') }}</p>
      <div class="row" style="margin-top: 10px;">
        <button class="btn" :disabled="tlTesting" @click="testAiConnection">
          {{ tlTesting ? $t('settings.ai_testing') : $t('settings.ai_test_connection') }}
        </button>
        <strong v-if="tlResult" :class="{ ok: tlResult.ok, warn: !tlResult.ok }">
          {{ tlResult.ok ? $t('settings.ai_connection_ok') : $t('settings.ai_connection_fail') }}
        </strong>
        <span v-if="tlResult && !tlResult.ok && tlResult.message" class="tl-error" style="margin: 0;">
          {{ tlResult.message }}
        </span>
      </div>
    </section>

    <section v-if="appData" class="settings-card settings-card--wide">
      <div class="section-title">{{ $t('settings.section_data') }}</div>
      <p class="hint">{{ $t('settings.app_data_dir') }} <code>{{ appData }}</code></p>
    </section>
    </div>

    <div class="save-bar">
      <button class="btn primary" :disabled="!dirty" @click="save">
        {{ saved ? $t('common.saved') : $t('common.save') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.settings {
  --settings-gutter: clamp(14px, 2vw, 36px);
  width: 100%;
  max-width: none;
  height: 100%;
  margin: 0;
  overflow-y: auto;
  padding: 20px var(--settings-gutter) 104px;
}

.view-header {
  width: min(100%, 1440px);
  margin: 0 auto 16px;
  padding: 0 4px;
}
.view-title {
  font-size: 1.4rem;
  font-weight: 700;
}
.settings-grid {
  width: min(100%, 1440px);
  margin: 0 auto;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 18px;
  align-items: start;
}
.settings-card {
  min-width: 0;
  margin: 0;
  padding: clamp(18px, 1.7vw, 26px);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
}
.settings-card--wide {
  grid-column: 1 / -1;
}
.settings-card > .section-title {
  margin-top: 0;
  font-size: 0.82rem;
  font-weight: 700;
  color: var(--text-dim);
}
.fields {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 10px;
}
.theme-setting {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 18px;
}
.theme-label {
  font-size: 0.85rem;
  font-weight: 600;
}
.theme-toggle {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 3px;
  width: min(100%, 360px);
  padding: 4px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 12px;
}
.theme-toggle button {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  min-width: 0;
  padding: 8px 10px;
  background: transparent;
  border: 0;
  border-radius: 8px;
  color: var(--text-dim);
  cursor: pointer;
  font: inherit;
  font-size: 0.8rem;
  transition: background 0.18s ease, box-shadow 0.18s ease, color 0.18s ease;
}
.theme-toggle button:hover {
  color: var(--text);
}
.theme-toggle button.active {
  background: var(--surface);
  color: var(--accent);
  box-shadow: 0 2px 8px var(--glass-shadow);
}
.theme-toggle button:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}
.row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  flex-wrap: wrap;
}
.row input[type="text"],
.row input[type="password"],
.row input:read-only {
  flex: 1;
  /* min-width:0 so a long value (e.g. a download dir path) can shrink/wrap
     inside the flex row instead of forcing the row wider than the viewport. */
  min-width: 0;
  width: 100%;
  background: var(--surface);
  border: 1px solid var(--border);
  color: var(--text);
  border-radius: 6px;
  padding: 6px 10px;
}
.checkboxes {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 10px;
}
.checkboxes label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 0.85rem;
  color: var(--text);
}
.hint {
  font-size: 0.78rem;
  color: var(--text-dim);
  margin: 6px 0 0;
}
.hint code {
  background: var(--surface-2);
  padding: 1px 5px;
  border-radius: 4px;
}
.ok {
  color: #6ec16e;
}
.warn {
  color: #ffce80;
}
.tl-error {
  color: #f08080;
  font-size: 0.78rem;
  padding: 4px 8px;
  background: rgba(220, 60, 60, 0.1);
  border-radius: 6px;
  overflow-wrap: anywhere;
}
.save-bar {
  position: fixed;
  right: clamp(22px, 3vw, 48px);
  bottom: clamp(18px, 2.5vw, 36px);
  z-index: 20;
  display: flex;
  justify-content: flex-end;
  padding: 8px;
  background: color-mix(in srgb, var(--surface) 92%, transparent);
  border: 1px solid var(--border);
  border-radius: 12px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.28);
  backdrop-filter: blur(12px);
}
.save-bar .btn {
  font-size: 1rem;
  font-weight: 700;
  min-width: 132px;
  padding: 11px 28px;
  box-shadow: 0 4px 14px var(--accent-soft);
}
.save-bar .btn:disabled {
  box-shadow: none;
}

@media (max-width: 960px) {
  .settings-grid {
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
  .settings-card--wide {
    grid-column: auto;
  }
}

@media (max-width: 768px) {
  .settings {
    --settings-gutter: 14px;
    padding-top: 14px;
    padding-bottom: 92px;
  }
  .settings-card {
    padding: 16px;
    border-radius: 10px;
  }
  .fields {
    grid-template-columns: minmax(0, 1fr);
  }
  .theme-setting {
    align-items: stretch;
    flex-direction: column;
    gap: 8px;
  }
  .theme-toggle {
    width: 100%;
  }
  .save-bar {
    right: 14px;
    bottom: 14px;
  }
  .save-bar .btn {
    min-width: 116px;
    padding: 10px 22px;
  }
}
</style>
