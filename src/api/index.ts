/**
 * Public frontend API.
 *
 * Callers import from `@/api`; the implementation is grouped by domain so a
 * backend change does not turn this barrel into another monolithic module.
 */
export * from "./settings";
export * from "./gallery";
export * from "./notifications";
export * from "./library";
export * from "./downloads";
export * from "./system";
export * from "./translation";
export * from "./translationCache";

export type {
  AuthCredentials,
  AuthStatus,
  Comment,
  CommentsPage,
  DownloadEntry,
  DownloadProgress,
  DownloadRequest,
  DownloadRow,
  FavoriteRow,
  FavoriteStatus,
  FavoritesPage,
  Gallery,
  HistoryEntry,
  Language,
  LocalGallery,
  Page,
  SearchPage,
  SearchQuery,
  Settings,
  SimpleGallery,
  SortType,
  Tag,
  TagStatus,
  TagType,
  User,
} from "@/types";
