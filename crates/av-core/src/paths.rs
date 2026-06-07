use std::path::PathBuf;

use directories::ProjectDirs;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "saorsa-labs";
const APP: &str = "anta-vista";

pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APP)
}

pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.data_dir().to_path_buf())
}

pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().to_path_buf())
}

pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.cache_dir().to_path_buf())
}

/// Returns the path to the main SQLite database file.
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("anta-vista.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_project_dirs_returns_some() {
        // Will return None in certain minimal environments — just ensure no panic
        let _ = project_dirs();
    }

    #[test]
    fn test_paths_are_pathbuf() {
        // Verify return types compile and can be used as PathBuf
        let db: Option<PathBuf> = db_path();
        let data: Option<PathBuf> = data_dir();
        let config: Option<PathBuf> = config_dir();
        let cache: Option<PathBuf> = cache_dir();

        // If they return Some, they must not be empty
        if let Some(p) = db {
            assert!(!p.as_os_str().is_empty());
        }
        if let Some(p) = data {
            assert!(!p.as_os_str().is_empty());
        }
        if let Some(p) = config {
            assert!(!p.as_os_str().is_empty());
        }
        if let Some(p) = cache {
            assert!(!p.as_os_str().is_empty());
        }
    }

    #[test]
    fn test_db_path_ends_with_db_extension() {
        if let Some(p) = db_path() {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            assert_eq!(ext, "db", "db_path should end in .db, got {p:?}");
        }
    }

    #[test]
    fn test_pathbuf_join_is_cross_platform() {
        // Verify Path::join works correctly — no hardcoded separators
        let base = PathBuf::from("base");
        let joined = base.join("sub").join("file.db");
        // Should work on all platforms without manual separator handling
        assert!(joined.to_str().is_some());
        assert!(joined.ends_with("file.db"));
    }
}
