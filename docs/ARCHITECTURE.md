# NClientV4 Backend Architecture

The Rust backend is organized around the nhentai API v2, local persistence,
download management, and Tauri command handlers.

| Module | Responsibility |
| --- | --- |
| `src-tauri/src/http.rs` | reqwest client, proxy settings, cookies, auth header, Cloudflare detection |
| `src-tauri/src/api.rs` | nhentai API v2 browse, search, detail, favorites, comments, and tags |
| `src-tauri/src/models.rs` | shared serializable API and frontend/backend data models |
| `src-tauri/src/cloudflare.rs` | challenge detection and `cf_clearance` capture via webview |
| `src-tauri/src/downloader.rs` | gallery download queue, progress, pause, cancel, and resume |
| `src-tauri/src/db.rs` | SQLite favorites, history, tags, local library, and download rows |
| `src-tauri/src/export.rs` | PDF and ZIP export |
| `src-tauri/src/config.rs` | persistent settings, API key auth, paths, and request preferences |
| `src-tauri/src/commands.rs` | Tauri bridge exposed to the Vue frontend |
| `src-tauri/src/error.rs` | unified application error type |

## Key Behaviors

### User-Agent & Auth Header

Every request to `/api/v2/...` carries:

- `User-Agent: NClientV4/<version> (unofficial nhentai desktop client)`
- `Authorization: Key <api_key>` when an API key is set

The 401/403 invalidation flag is represented by `AuthCredentials.valid` and
reported through `auth_status`.

### Cookie Persistence

`reqwest_cookie_store` stores cookies in `<app_data>/cookies.json`, reloaded on
startup. `cf_clearance` and session cookies survive restarts.

### Search Query Construction

`api.rs::search` builds v2 search URLs:

- `query=<urlencode>` joined with `+` separators
- each tag appended as `+tag:"name"` or `-tag:"name"` when avoided
- `&page=N`
- `&sort=<popular|popular-week|popular-today|popular-month>` when not recent

### Random Endpoint

`galleries/random` returns just `{"id": N}`; the backend follows up with a full
`galleries/<id>` request.

### Favorites

Without an API key, favorites are stored in the local SQLite database. When an
API key is configured, local favorites are pushed to `/galleries/{id}/favorite`
and the unified favorites page reads from `/favorites`.

### Download Folder Layout

`<download_dir>/<title>/.<id>` is the marker file that lets the local library
scanner identify the gallery and resume downloads without re-fetching metadata.
Pages are saved as `001.<ext>`, `002.<ext>`, and so on. A `.nomedia` JSON file
stores full metadata.

### JPEG Integrity Check

`config.rs::is_jpeg_corrupted` checks the trailing `FF D9` bytes; the downloader
skips re-downloading pages whose existing file passes the check.
