mod context;
mod diagnostics;
mod discovery;
pub mod launch;
mod library;
mod mcp;
pub mod mcp_library;
pub mod media;
// Public: the `aviary-media` binary drives this.
pub mod mcp_media;
pub mod mcp_protocol;
mod models;
pub mod providers;
mod runner;
pub mod store;
pub mod tokens;
mod watcher;
mod writer;

use discovery::DiscoveryResult;
use library::{EntryContent, LibrarySnapshot};
use store::Project;
use tauri::Manager;

/// Serves the last scan instantly, so a cold launch paints real content rather
/// than a spinner. `fresh: true` forces a walk of the filesystem.
///
/// Stale-while-revalidate is deliberate: the frontend renders the cached
/// snapshot immediately and calls again with `fresh` in the background. A cache
/// miss simply costs the scan that used to happen every time.
#[tauri::command]
async fn scan_library(
    fresh: Option<bool>,
    watcher: tauri::State<'_, watcher::LibraryWatcher>,
) -> Result<LibrarySnapshot, String> {
    let watcher = watcher.inner().clone();
    tauri::async_runtime::spawn_blocking(move || watcher.snapshot(fresh.unwrap_or(false)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn read_entry(path: String) -> Result<EntryContent, String> {
    tauri::async_runtime::spawn_blocking(move || library::read_entry(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn write_entry(
    path: String,
    content: String,
    expected_hash: String,
    force: bool,
) -> Result<writer::WriteOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        writer::write_entry(&path, &content, &expected_hash, force)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Memoised on (path, mtime, size) — re-tokenising an unchanged file is pure
/// waste, and the Context view asks for many files at once.
#[tauri::command]
async fn count_tokens(path: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || store::cached_tokens(&path))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn discover_runner_safety(
    runner: runner::Runner,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<runner::SafetyCapabilities, String> {
    let supervisor = supervisor.inner().clone();
    tauri::async_runtime::spawn_blocking(move || supervisor.safety_capabilities(runner))
        .await
        .map_err(|error| error.to_string())
}

/// Creates the durable session and first queued turn in one transaction before
/// the runner process starts.
#[tauri::command]
async fn create_chat_session(
    runner: runner::Runner,
    prompt: String,
    cwd: Option<String>,
    title: Option<String>,
    safety_option_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    channel: tauri::ipc::Channel<runner::EngineEvent>,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<runner::RunReceipt, String> {
    let supervisor = supervisor.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        supervisor.create_and_run(
            runner,
            prompt,
            cwd,
            title,
            runner::RunOptions {
                safety_option_id,
                model,
                effort,
            },
            channel,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Resolves a current bundle revision, then atomically stores its immutable
/// attachment snapshot with the session and first queued turn before spawning
/// the runner.
#[tauri::command]
async fn create_chat_session_with_bundle(
    bundle_id: String,
    expected_revision: i64,
    prompt: String,
    title: Option<String>,
    safety_option_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    channel: tauri::ipc::Channel<runner::EngineEvent>,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<runner::RunReceipt, String> {
    let supervisor = supervisor.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        supervisor.create_and_run_with_bundle(
            &bundle_id,
            expected_revision,
            prompt,
            title,
            runner::RunOptions {
                safety_option_id,
                model,
                effort,
            },
            channel,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_chat_turn(
    session_id: String,
    prompt: String,
    safety_option_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    channel: tauri::ipc::Channel<runner::EngineEvent>,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<runner::RunReceipt, String> {
    let supervisor = supervisor.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        supervisor.resume_and_run(
            &session_id,
            prompt,
            runner::RunOptions {
                safety_option_id,
                model,
                effort,
            },
            channel,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn resume_chat_session(
    session_id: String,
    prompt: String,
    safety_option_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    channel: tauri::ipc::Channel<runner::EngineEvent>,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<runner::RunReceipt, String> {
    run_chat_turn(
        session_id,
        prompt,
        safety_option_id,
        model,
        effort,
        channel,
        supervisor,
    )
    .await
}

#[tauri::command]
async fn list_chat_sessions(
    limit: Option<usize>,
) -> Result<Vec<store::sessions::SessionSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || runner::list_sessions(limit.unwrap_or(100)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_chat_session(
    session_id: String,
) -> Result<Option<store::sessions::SessionDetail>, String> {
    tauri::async_runtime::spawn_blocking(move || runner::load_session(&session_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn respond_permission(
    request_id: String,
    reply: runner::PermissionReply,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<(), String> {
    supervisor.respond_permission(&request_id, reply)
}

#[tauri::command]
fn interrupt_turn(
    turn_id: String,
    supervisor: tauri::State<'_, runner::Supervisor>,
) -> Result<(), String> {
    supervisor.interrupt(&turn_id)
}

#[tauri::command]
async fn list_models(runner: runner::Runner) -> Result<models::ModelCatalogue, String> {
    tauri::async_runtime::spawn_blocking(move || models::catalogue(runner))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn scan_mcp(fresh: Option<bool>, cwd: Option<String>) -> Result<mcp::McpSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(cwd) = cwd {
            let canonical = mcp::canonical_context_cwd(&cwd)?;
            return Ok(mcp::scan_for_context(&store::project_pairs(), &canonical));
        }
        if !fresh.unwrap_or(false) {
            if let Some(hit) = store::read_scan("mcp") {
                if let Ok(mut snapshot) = serde_json::from_str::<mcp::McpSnapshot>(&hit.payload) {
                    mcp::refresh_cached_health(&mut snapshot);
                    return Ok(snapshot);
                }
            }
        }
        let snapshot = mcp::scan(&store::project_pairs());
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = store::write_scan("mcp", &json, snapshot.scanned_ms);
        }
        Ok(snapshot)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn canonical_context_directory(cwd: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        mcp::canonical_context_cwd(&cwd).map(|path| path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Explicitly starts MCP control/server processes. Static inventory commands
/// never call this path, because a health check can contact configured network
/// endpoints.
#[tauri::command]
async fn check_mcp_health(
    runner: providers::Runner,
    cwd: String,
    effective_ids: Option<Vec<String>>,
) -> Result<mcp::McpHealthSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        mcp::check_health(
            &store::project_pairs(),
            runner,
            &cwd,
            effective_ids.as_deref(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn set_mcp_enabled(
    effective_id: String,
    cwd: Option<String>,
    enabled: bool,
    expected_revision: String,
) -> Result<writer::McpToggleOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let canonical = cwd.as_deref().map(mcp::canonical_context_cwd).transpose()?;
        let outcome = writer::set_mcp_enabled(
            &store::project_pairs(),
            canonical.as_deref(),
            &effective_id,
            enabled,
            &expected_revision,
        )?;
        invalidate_mcp_scan_after_toggle(&outcome, store::delete_scan);
        Ok(outcome)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn invalidate_mcp_scan_after_toggle(
    outcome: &writer::McpToggleOutcome,
    delete_scan: impl FnOnce(&str) -> Result<(), String>,
) {
    if matches!(outcome, writer::McpToggleOutcome::Written { .. }) {
        // The structured write has already succeeded, so a cache cleanup
        // failure must not turn its truthful outcome into an apparent failed
        // write. The next fresh scan still replaces this entry.
        let _ = delete_scan("mcp");
    }
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
    .map_err(|e| e.to_string())?
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
async fn add_project(
    name: String,
    path: String,
    watcher: tauri::State<'_, watcher::LibraryWatcher>,
) -> Result<Vec<Project>, String> {
    let watcher = watcher.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if !std::path::Path::new(&path).is_dir() {
            return Err(format!("{path} is not a directory"));
        }
        store::add_project(&name, &path)?;
        invalidate_project_scans();
        if let Err(error) = watcher.projects_changed() {
            // The durable project row has already committed. A disposable
            // cache failure must not tell the UI that the add itself failed.
            log::warn!("project added, but live library refresh failed: {error}");
        }
        Ok(store::projects())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn remove_project(
    path: String,
    watcher: tauri::State<'_, watcher::LibraryWatcher>,
) -> Result<Vec<Project>, String> {
    let watcher = watcher.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store::remove_project(&path)?;
        invalidate_project_scans();
        if let Err(error) = watcher.projects_changed() {
            log::warn!("project removed, but live library refresh failed: {error}");
        }
        Ok(store::projects())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Drops cached scans whose result depends on the project list.
fn invalidate_project_scans() {
    let _ = store::delete_scan("mcp");
    let _ = store::delete_scan("projects");
    let _ = store::delete_scan("library");
    let _ = store::delete_scan_prefix("library:scope:");
}

// --------------------------------------------------------------- bundles ---

fn resolve_bundle_with_catalog(
    bundle: store::bundles::Bundle,
    catalog: &store::bundles::LiveTargetCatalog,
) -> Result<store::bundles::ResolvedBundle, String> {
    store::bundles::resolve_bundle(bundle, catalog).map_err(|error| error.to_string())
}

#[derive(serde::Serialize)]
struct BundleChatPlan {
    runner: providers::Runner,
    cwd: String,
    model_id: Option<String>,
}

#[tauri::command]
async fn list_bundles() -> Result<Vec<store::bundles::ResolvedBundle>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let catalog = store::bundles::LiveTargetCatalog::scan();
        store::bundles::list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|bundle| resolve_bundle_with_catalog(bundle, &catalog))
            .collect()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn create_bundle(
    draft: store::bundles::BundleDraft,
) -> Result<store::bundles::ResolvedBundle, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let catalog = store::bundles::LiveTargetCatalog::scan();
        let bundle = store::bundles::create(draft, &catalog).map_err(|error| error.to_string())?;
        resolve_bundle_with_catalog(bundle, &catalog)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn update_bundle(
    id: String,
    expected_revision: i64,
    draft: store::bundles::BundleDraft,
) -> Result<store::bundles::ResolvedBundle, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let catalog = store::bundles::LiveTargetCatalog::scan();
        let bundle = store::bundles::update(&id, expected_revision, draft, &catalog)
            .map_err(|error| error.to_string())?;
        resolve_bundle_with_catalog(bundle, &catalog)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn delete_bundle(id: String, expected_revision: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        store::bundles::delete(&id, expected_revision).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Performs the same fresh resolution and fail-closed capability check used
/// by session creation, without mutating chat history.
#[tauri::command]
async fn prepare_bundle_chat(
    bundle_id: String,
    expected_revision: i64,
) -> Result<BundleChatPlan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let catalog = store::bundles::LiveTargetCatalog::scan();
        let prepared =
            store::bundles::resolve_for_attachment(&bundle_id, expected_revision, &catalog)
                .map_err(|error| error.to_string())?;
        store::bundles::validate_chat_support(&prepared).map_err(|error| error.to_string())?;
        Ok(BundleChatPlan {
            runner: prepared.runner,
            cwd: prepared.cwd,
            model_id: prepared.model_id,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_session_bundle(
    session_id: String,
) -> Result<Option<store::bundles::SessionBundleAttachment>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        store::bundles::get_session_attachment(&session_id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Re-resolves the exact saved revision, prepares a private one-use handoff,
/// and only then asks macOS to open it in Terminal.
#[tauri::command]
async fn launch_bundle_terminal(
    bundle_id: String,
    expected_revision: i64,
) -> Result<launch::PreparedLaunch, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let helper = launch::bundled_launch_helper().map_err(|error| error.to_string())?;
        let prepared = launch::prepare_bundle_terminal(&bundle_id, expected_revision, &helper)
            .map_err(|error| error.to_string())?;
        launch::open_terminal(&prepared).map_err(|error| error.to_string())?;
        Ok(prepared)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn media_mcp_registration(
    collection_id: Option<i64>,
) -> Result<mcp_protocol::McpRegistration, String> {
    tauri::async_runtime::spawn_blocking(move || mcp_protocol::media_registration(collection_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn library_mcp_registration() -> Result<mcp_protocol::McpRegistration, String> {
    tauri::async_runtime::spawn_blocking(mcp_protocol::library_registration)
        .await
        .map_err(|error| error.to_string())?
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
fn set_collection_membership(collection_id: i64, hash: String, member: bool) -> Result<(), String> {
    media::set_membership(collection_id, &hash, member)
}

/// Builds a bounded, user-copyable report from Aviary's own local logs.
/// Reading log tails is file I/O, so it stays off the UI thread.
#[tauri::command]
async fn collect_diagnostics(
    app: tauri::AppHandle,
    failure: Option<diagnostics::FrontendFailure>,
) -> Result<diagnostics::DiagnosticsBundle, String> {
    let version = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || diagnostics::collect(&version, failure))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logs_dir = diagnostics::prepare_logs_dir();
    let mut logger = tauri_plugin_log::Builder::new()
        .clear_targets()
        .max_file_size(1_000_000)
        // Four archives plus the active file: bounded to five files total.
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(4))
        .level(log::LevelFilter::Info)
        // Dependency chatter is not actionable diagnostics and can contain
        // data Aviary never chose to log.
        .filter(|metadata| diagnostics::is_local_log_target(metadata.target()));

    logger = match logs_dir {
        Some(path) => logger.target(tauri_plugin_log::Target::new(
            tauri_plugin_log::TargetKind::Folder {
                path,
                file_name: Some("aviary".to_string()),
            },
        )),
        None => logger.target(tauri_plugin_log::Target::new(
            tauri_plugin_log::TargetKind::Stderr,
        )),
    };

    #[cfg(debug_assertions)]
    {
        logger = logger.target(tauri_plugin_log::Target::new(
            tauri_plugin_log::TargetKind::Stdout,
        ));
    }

    let app = tauri::Builder::default()
        .plugin(logger.build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // The logger plugin is live before setup, so database startup
            // failures and every later panic reach the local log.
            diagnostics::install_panic_hook();
            diagnostics::log_startup(&app.package_info().version.to_string());

            // One-time lift of the old JSON settings into the database.
            store::migrate_settings_json();

            let supervisor = runner::Supervisor::new();
            let interrupted = supervisor
                .reconcile_startup()
                .map_err(std::io::Error::other)?;
            if interrupted != 0 {
                log::info!("reconciled {interrupted} unfinished chat turn(s)");
            }
            app.manage(supervisor);

            let live = watcher::LibraryWatcher::start(app.handle().clone())
                .map_err(std::io::Error::other)?;
            app.manage(live);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_library,
            resolve_context,
            discover_projects,
            scan_mcp,
            canonical_context_directory,
            check_mcp_health,
            set_mcp_enabled,
            discover_runner_safety,
            create_chat_session,
            create_chat_session_with_bundle,
            run_chat_turn,
            resume_chat_session,
            list_chat_sessions,
            load_chat_session,
            respond_permission,
            interrupt_turn,
            list_models,
            read_entry,
            count_tokens,
            write_entry,
            list_projects,
            add_project,
            remove_project,
            list_bundles,
            create_bundle,
            update_bundle,
            delete_bundle,
            prepare_bundle_chat,
            load_session_bundle,
            launch_bundle_terminal,
            media_mcp_registration,
            library_mcp_registration,
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
            set_collection_membership,
            collect_diagnostics
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<runner::Supervisor>().shutdown();
            app_handle.state::<watcher::LibraryWatcher>().shutdown();
        }
    });
}

#[cfg(test)]
mod p3_integration_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn generic_mcp_scan_is_invalidated_only_after_a_write() {
        let written = writer::McpToggleOutcome::Written {
            revision: "new-revision".into(),
            snapshot_id: "snapshot".into(),
        };
        let unchanged = writer::McpToggleOutcome::Unchanged {
            revision: "same-revision".into(),
        };

        let deleted = Cell::new(0);
        invalidate_mcp_scan_after_toggle(&written, |kind| {
            assert_eq!(kind, "mcp");
            deleted.set(deleted.get() + 1);
            Ok(())
        });
        for outcome in [
            unchanged,
            writer::McpToggleOutcome::Conflict,
            writer::McpToggleOutcome::NotFound,
        ] {
            invalidate_mcp_scan_after_toggle(&outcome, |_| {
                deleted.set(deleted.get() + 1);
                Ok(())
            });
        }

        assert_eq!(deleted.get(), 1);
    }
}
