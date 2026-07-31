mod context;
mod discovery;
mod library;
mod mcp;
pub mod media;
// Public: the `aviary-media` binary drives this.
pub mod mcp_media;
mod models;
pub mod providers;
mod runner;
pub mod store;
pub mod tokens;
mod writer;

use discovery::DiscoveryResult;
use library::{EntryContent, LibrarySnapshot};
use store::Project;

/// Serves the last scan instantly, so a cold launch paints real content rather
/// than a spinner. `fresh: true` forces a walk of the filesystem.
///
/// Stale-while-revalidate is deliberate: the frontend renders the cached
/// snapshot immediately and calls again with `fresh` in the background. A cache
/// miss simply costs the scan that used to happen every time.
#[tauri::command]
async fn scan_library(fresh: Option<bool>) -> Result<LibrarySnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !fresh.unwrap_or(false) {
            if let Some(hit) = store::read_scan("library") {
                if let Ok(snapshot) = serde_json::from_str::<LibrarySnapshot>(&hit.payload) {
                    return snapshot;
                }
            }
        }
        let snapshot = library::scan();
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = store::write_scan("library", &json, snapshot.scanned_ms);
        }
        snapshot
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn read_entry(path: String) -> Result<EntryContent, String> {
    library::read_entry(&path)
}

#[tauri::command]
fn write_entry(
    path: String,
    content: String,
    expected_hash: String,
    force: bool,
) -> Result<writer::WriteOutcome, String> {
    writer::write_entry(&path, &content, &expected_hash, force)
}

/// Memoised on (path, mtime, size) — re-tokenising an unchanged file is pure
/// waste, and the Context view asks for many files at once.
#[tauri::command]
fn count_tokens(path: String) -> usize {
    store::cached_tokens(&path)
}

/// Runs one chat turn. Blocking work happens on a background thread so the
/// UI thread is never held by a subprocess.
#[tauri::command]
async fn run_turn(
    runner: runner::Runner,
    prompt: String,
    cwd: Option<String>,
    mode: runner::PermissionMode,
    model: Option<String>,
    effort: Option<String>,
    channel: tauri::ipc::Channel<runner::Event>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        runner::run_turn(runner, prompt, cwd, mode, model, effort, channel)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_models(runner: runner::Runner) -> models::ModelCatalogue {
    models::catalogue(runner)
}

#[tauri::command]
async fn scan_mcp(fresh: Option<bool>) -> Result<mcp::McpSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !fresh.unwrap_or(false) {
            if let Some(hit) = store::read_scan("mcp") {
                if let Ok(snapshot) = serde_json::from_str::<mcp::McpSnapshot>(&hit.payload) {
                    return snapshot;
                }
            }
        }
        let snapshot = mcp::scan(&store::project_pairs());
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = store::write_scan("mcp", &json, snapshot.scanned_ms);
        }
        snapshot
    })
    .await
    .map_err(|e| e.to_string())
}

/// Resolves the instruction stack a runner would load in `cwd`.
///
/// Blocking: walks ancestor directories and tokenises every instruction file,
/// so it runs off the UI thread like the other filesystem commands.
#[tauri::command]
async fn resolve_context(
    runner: providers::Runner,
    cwd: String,
) -> Result<context::Resolved, String> {
    tauri::async_runtime::spawn_blocking(move || {
        context::resolve(runner, &cwd, &store::project_pairs())
    })
    .await
    .map_err(|e| e.to_string())
}

/// Walking the filesystem for candidate projects is the slowest scan in the
/// app, so it is cached like the others.
#[tauri::command]
async fn discover_projects(fresh: Option<bool>) -> Result<DiscoveryResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !fresh.unwrap_or(false) {
            if let Some(hit) = store::read_scan("projects") {
                if let Ok(result) = serde_json::from_str::<DiscoveryResult>(&hit.payload) {
                    return result;
                }
            }
        }
        let registered: Vec<String> = store::projects().into_iter().map(|p| p.path).collect();
        let result = discovery::discover(&registered);
        if let Ok(json) = serde_json::to_string(&result) {
            let _ = store::write_scan("projects", &json, result.scanned_ms);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_projects() -> Vec<Project> {
    store::projects()
}

#[tauri::command]
fn add_project(name: String, path: String) -> Result<Vec<Project>, String> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    store::add_project(&name, &path)?;
    // Registering a project changes what the scanners see.
    invalidate_scans();
    Ok(store::projects())
}

#[tauri::command]
fn remove_project(path: String) -> Result<Vec<Project>, String> {
    store::remove_project(&path)?;
    invalidate_scans();
    Ok(store::projects())
}

/// Drops cached scans whose result depends on the project list.
fn invalidate_scans() {
    let conn = store::cache();
    let _ = conn.execute("DELETE FROM scan WHERE kind IN ('library','mcp','projects')", []);
}

// ------------------------------------------------------------ preferences ---

#[tauri::command]
fn get_preference(key: String) -> Option<String> {
    store::get_pref(&key)
}

#[tauri::command]
fn set_preference(key: String, value: String) -> Result<(), String> {
    store::set_pref(&key, &value)
}

#[tauri::command]
fn all_preferences() -> std::collections::BTreeMap<String, String> {
    store::all_prefs().into_iter().collect()
}

// ------------------------------------------------------------------ media ---

#[tauri::command]
async fn import_media(paths: Vec<String>) -> Result<Vec<media::MediaItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        for p in paths {
            match media::import(std::path::Path::new(&p)) {
                Ok(item) => out.push(item),
                // One bad file must not abort a multi-file drop.
                Err(e) => errors.push(e),
            }
        }
        if out.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_media(collection: Option<i64>) -> Vec<media::MediaItem> {
    media::list(collection)
}

#[tauri::command]
fn search_media(query: String, limit: Option<usize>) -> Vec<media::MediaItem> {
    media::search(&query, limit.unwrap_or(200))
}

#[tauri::command]
fn remove_media(hash: String) -> Result<(), String> {
    media::remove(&hash)
}

#[tauri::command]
fn set_media_tags(hash: String, tags: Vec<String>) -> Result<(), String> {
    media::set_tags(&hash, &tags)
}

#[tauri::command]
fn list_collections() -> Vec<media::Collection> {
    media::collections()
}

#[tauri::command]
fn create_collection(name: String) -> Result<i64, String> {
    media::create_collection(&name)
}

#[tauri::command]
fn set_collection_membership(
    collection_id: i64,
    hash: String,
    member: bool,
) -> Result<(), String> {
    media::set_membership(collection_id, &hash, member)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // One-time lift of the old JSON settings into the database.
    store::migrate_settings_json();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_library,
            resolve_context,
            discover_projects,
            scan_mcp,
            run_turn,
            list_models,
            read_entry,
            count_tokens,
            write_entry,
            list_projects,
            add_project,
            remove_project,
            get_preference,
            set_preference,
            all_preferences,
            import_media,
            list_media,
            search_media,
            remove_media,
            set_media_tags,
            list_collections,
            create_collection,
            set_collection_membership
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
