mod discovery;
mod library;
mod providers;
mod tokens;
mod writer;

use discovery::DiscoveryResult;
use library::{EntryContent, LibrarySnapshot, Project};

#[tauri::command]
fn scan_library() -> LibrarySnapshot {
    library::scan()
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

#[tauri::command]
fn count_tokens(path: String) -> usize {
    tokens::count_file(&path)
}

#[tauri::command]
fn discover_projects() -> DiscoveryResult {
    let registered: Vec<String> = library::load_settings()
        .projects
        .into_iter()
        .map(|p| p.path)
        .collect();
    discovery::discover(&registered)
}

#[tauri::command]
fn list_projects() -> Vec<Project> {
    library::load_settings().projects
}

#[tauri::command]
fn add_project(name: String, path: String) -> Result<Vec<Project>, String> {
    let dir = std::path::Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("{path} is not a directory"));
    }

    let mut settings = library::load_settings();
    if settings.projects.iter().any(|p| p.path == path) {
        return Ok(settings.projects);
    }
    settings.projects.push(Project { name, path });
    library::save_settings(&settings)?;
    Ok(settings.projects)
}

#[tauri::command]
fn remove_project(path: String) -> Result<Vec<Project>, String> {
    let mut settings = library::load_settings();
    settings.projects.retain(|p| p.path != path);
    library::save_settings(&settings)?;
    Ok(settings.projects)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_library,
            discover_projects,
            read_entry,
            count_tokens,
            write_entry,
            list_projects,
            add_project,
            remove_project
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
