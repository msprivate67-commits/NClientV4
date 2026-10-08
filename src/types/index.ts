// Frontend type definitions — mirrors the Rust `models.rs`.

export type Language = "all" | "english" | "japanese" | "chinese";
export type SortType =
  | "recent_all_time"
  | "popular_all_time"
  | "popular_week"
  | "popular_day"
  | "popular_month";
export type TitleType = "pretty" | "english" | "japanese" | "auto";
export type DataUsageType = "none" | "thumbnail" | "full";
export type ThemePreference = "dark" | "system" | "light";
export type LocalSortType =
  | "title_asc"
  | "title_desc"
  | "date_asc"
  | "date_desc"
  | "pages_asc"
  | "pages_desc";

export type TagType =
  | "unknown"
  | "parody"
  | "character"
  | "tag"
  | "artist"
  | "group"
  | "language"
  | "category";

export type TagStatus = "default" | "accepted" | "avoided";

export interface Tag {
  id: number;
  name: string;
  type: TagType;
  count: number;
  status?: TagStatus;
  blacklisted?: boolean;
}

export interface Page {
  index: number;
  path: string | null;
  thumbnail: string | null;
  width: number;
  height: number;
}

export interface Titles {
  english: string;
  pretty: string;
  japanese: string;
}

export interface Gallery {
  id: number;
  media_id: number;
  upload_date: string | null;
  num_favorites: number;
  num_pages: number;
  titles: Titles;
  tags: Tag[];
  cover: Page;
  thumbnail: Page;
  pages: Page[];
  is_favorited: boolean;
  related: SimpleGallery[];
}

export interface SimpleGallery {
  id: number;
  media_id: number;
  title: string;
  thumbnail: string | null;
  language: Language;
  tags: Tag[];
  num_pages: number;
}

export interface SearchPage {
  galleries: SimpleGallery[];
  page: number;
  num_pages: number;
  per_page: number;
}

export interface SearchQuery {
  query?: string;
  tags?: Tag[];
  accepted_tag_ids?: number[];
  avoided_tag_ids?: number[];
  page: number;
  sort?: SortType;
  only_language?: Language;
  from_page?: number | null;
  to_page?: number | null;
}

export interface FavoriteGroup {
  id: number;
  name: string;
  count: number;
}

export interface FavoritesPage {
  galleries: SimpleGallery[];
  page: number;
  num_pages: number;
  blacklisted?: boolean;
}

export interface FavoriteStatus {
  favorited: boolean;
  num_favorites: number | null;
}

export interface CommentUser {
  id: number;
  username: string;
  slug: string;
  avatar_url: string | null;
  is_superuser: boolean;
  is_staff: boolean;
}

export interface Comment {
  id: number;
  gallery_id: number;
  poster: CommentUser;
  body: string;
  create_date: string | null;
  post_date: string | null;
  vote: number | null;
}

export interface CommentsPage {
  comments: Comment[];
  gallery_id: number;
  page: number;
  num_pages: number;
  per_page: number;
  total: number | null;
}

export interface User {
  id: number;
  username: string;
  slug: string;
  avatar_url: string | null;
  is_superuser: boolean;
  is_staff: boolean;
}

export interface LocalGallery {
  id: number;
  title: string;
  thumbnail_path: string | null;
  folder: string;
  num_pages: number;
  page_files: string[];
  media_id: number;
  scanned_at: string;
  translated_title: string;
}

export interface HistoryEntry {
  gallery_id: number;
  title: string;
  media_id: number;
  thumbnail: string;
  visited_at: string;
}

export type ProxyType = "none" | "http" | "socks5";

export interface AuthCredentials {
  api_key: string;
  valid: boolean;
}

export interface Settings {
  mirror: string;
  user_agent: string;
  request_timeout_secs: number;
  auth: AuthCredentials;

  proxy_type: ProxyType;
  proxy_host: string;
  proxy_port: number;
  proxy_username: string;
  proxy_password: string;

  sort_type: SortType;
  only_language: Language;
  title_type: TitleType;
  exact_tag_match: boolean;
  remove_avoided_galleries: boolean;
  show_titles: boolean;

  page_thumbnail_columns: number;
  use_rtl: boolean;
  default_zoom_pct: number;
  reader_fit_mode: string;
  reader_direction: string;
  button_change_page: boolean;

  usage_wifi: DataUsageType;
  usage_mobile: DataUsageType;

  keep_history: boolean;
  max_history: number;
  favorite_limit: number;

  download_dir: string;
  parallel_downloads: number;
  parallel_pages: number;
  notifications_enabled: boolean;
  privacy_screen: boolean;
  clipboard_link_reader: boolean;

  lock_screen: boolean;
  pin: string;

  tl_base_url: string;
  tl_model: string;
  tl_api_key: string;
  tl_target_lang: string;
  tl_comment_target_lang: string;
  tl_thinking: boolean;
  tl_auto_translate: boolean;
  tl_auto_translate_gallery_titles: boolean;
  tl_use_proxy: boolean;
  tl_cache_limit: number;

  app_language: string;
  theme: ThemePreference;
}

export interface FavoriteRow {
  id: number;
  title: string;
  media_id: number;
  thumbnail: string;
  added_at: string;
}

export interface DownloadRow {
  id: number;
  title: string;
  media_id: number;
  thumbnail: string;
  folder: string;
  total_pages: number;
  done_pages: number;
  status: string;
  updated_at: string;
}

export type DownloadStatus =
  | "pending"
  | "downloading"
  | "paused"
  | "finished"
  | "canceled"
  | "failed";

export interface DownloadEntry {
  id: number;
  title: string;
  folder: string;
  thumbnail: string | null;
  status: DownloadStatus;
  done_pages: number;
  total_pages: number;
  bytes_per_second: number | null;
  total_bytes: number | null;
}

export interface DownloadProgress {
  id: number;
  title: string;
  folder: string;
  status: DownloadStatus;
  done_pages: number;
  total_pages: number;
  current_page: number | null;
  error: string | null;
  bytes_per_second: number | null;
  total_bytes: number | null;
}

export interface DownloadRequest {
  gallery_id: number;
  from_page?: number | null;
  to_page?: number | null;
}

export interface AuthStatus {
  has_credentials: boolean;
  api_key_valid: boolean;
  cloudflare_solved: boolean;
}
