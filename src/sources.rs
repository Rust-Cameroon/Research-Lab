use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCES_JSON: &str = "data/sources/sources.json";
const SOURCES_FILES_DIR: &str = "data/sources/files";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String, // "url" | "file"
    pub title: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>, // relative path under data/
    pub added_at: u64,
}

pub fn add_source_file_path(rel_path_under_data: &str, title: Option<String>) -> Result<Source> {
    // Do not write file; just record it. Expect a path like "data/sources/files/xxx".
    let mut list = read_sources();
    let ts = now_secs();
    let src = Source {
        id: format!("file_{}", ts),
        kind: "file".to_string(),
        title,
        url: None,
        path: Some(rel_path_under_data.to_string()),
        added_at: ts,
    };
    list.push(src.clone());
    write_sources(&list)?;
    Ok(src)
}

pub fn remove_source(id: &str) -> Result<bool> {
    let mut list = read_sources();
    let before = list.len();
    list.retain(|s| s.id != id);
    let removed = list.len() != before;
    if removed { write_sources(&list)?; }
    Ok(removed)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sources_path() -> PathBuf { PathBuf::from(SOURCES_JSON) }
fn files_dir() -> PathBuf { PathBuf::from(SOURCES_FILES_DIR) }

fn read_sources() -> Vec<Source> {
    let p = sources_path();
    if !p.exists() {
        return vec![];
    }
    let Ok(s) = fs::read_to_string(&p) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Vec<Source>>(&s) else { return vec![] };
    v
}

fn write_sources(list: &Vec<Source>) -> Result<()> {
    if let Some(parent) = sources_path().parent() { fs::create_dir_all(parent)?; }
    let s = serde_json::to_string_pretty(list)?;
    fs::write(sources_path(), s)?;
    Ok(())
}

pub fn list_sources() -> Vec<Source> { read_sources() }

pub fn list_sources_json() -> serde_json::Value {
    let list = read_sources();
    json!({ "sources": list })
}

pub fn add_source_url(url: &str, title: Option<String>) -> Result<Source> {
    let mut list = read_sources();
    let ts = now_secs();
    let id = format!("url_{}", ts);
    let src = Source {
        id,
        kind: "url".to_string(),
        title,
        url: Some(url.to_string()),
        path: None,
        added_at: ts,
    };
    list.push(src.clone());
    write_sources(&list)?;
    Ok(src)
}

pub fn save_uploaded_file(original_filename: &str, data: &[u8]) -> Result<Source> {
    // Ensure directories
    crate::utils::ensure_dir(&files_dir())?;
    // Sanitize filename minimally
    let safe_name = original_filename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();
    let ts = now_secs();
    let rel_path = format!("{}/{}_{}", SOURCES_FILES_DIR, ts, safe_name);
    let abs_path = Path::new(&rel_path);
    if let Some(parent) = abs_path.parent() { fs::create_dir_all(parent)?; }
    fs::write(abs_path, data)?;

    let mut list = read_sources();
    let src = Source {
        id: format!("file_{}", ts),
        kind: "file".to_string(),
        title: Some(safe_name.clone()),
        url: None,
        path: Some(rel_path.replace("data/", "data/")),
        added_at: ts,
    };
    list.push(src.clone());
    write_sources(&list)?;
    Ok(src)
}
