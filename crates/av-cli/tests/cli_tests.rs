use av_core::types::{
    EmbeddingProfile, EmbeddingRecord, FeedbackEvent, FeedbackKind, ResourceDescriptor,
    ResourceKind,
};
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

// Let's test the CLI commands using standard test database setup
#[test]
fn test_cli_status_command() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test_anta_vista.db");
    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };

    // Open connection to initialize schema
    let conn = av_store::open(&db_path).unwrap();

    // Insert a dummy resource
    let rd = ResourceDescriptor {
        id: "test-resource-id-1234567890abcdef".to_string(),
        kind: ResourceKind::Text,
        location: "file:///test.txt".to_string(),
        location_scheme: Some("file".to_string()),
        location_canonical: Some("file:///test.txt".to_string()),
        mime_type: "text/plain".to_string(),
        filename: Some("test.txt".to_string()),
        metadata_json: serde_json::json!({}),
        description_text: "Test description".to_string(),
        created_at: 123456,
    };
    av_store::repo::resources::insert(&conn, &rd).unwrap();

    // Call status run with non-interactive mode
    let cli = av_cli::Cli {
        non_interactive: true,
        config: None,
        timeout: 1000,
        stream: false,
        verbose: 0,
        command: av_cli::Commands::Status,
    };

    let startup_state = av_cli::startup::StartupState {
        config: av_core::config::AvConfig::default(),
        config_path: None,
        x0x_config: None,
        antd_running: false,
        minilm_loaded: true,
        listener_running: false,
    };

    // We capture stdout to inspect JSON output
    // Note: since print_output prints to stdout/stderr, we can verify it doesn't panic
    let res = av_cli::cmd::status::run(cli, startup_state);
    assert!(res.is_ok());

    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::remove_var("ANTA_VISTA_DB_PATH") };
}

#[test]
fn test_cli_purge_command() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test_anta_vista.db");
    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };

    let conn = av_store::open(&db_path).unwrap();

    let rd = ResourceDescriptor {
        id: "test-res".to_string(),
        kind: ResourceKind::Text,
        location: "file:///test.txt".to_string(),
        location_scheme: Some("file".to_string()),
        location_canonical: Some("file:///test.txt".to_string()),
        mime_type: "text/plain".to_string(),
        filename: Some("test.txt".to_string()),
        metadata_json: serde_json::json!({}),
        description_text: "Test description".to_string(),
        created_at: 123456,
    };
    av_store::repo::resources::insert(&conn, &rd).unwrap();

    let cli = av_cli::Cli {
        non_interactive: true,
        config: None,
        timeout: 1000,
        stream: false,
        verbose: 0,
        command: av_cli::Commands::Purge {
            resource: Some("test-res".to_string()),
            name: None,
            all: false,
            cache: false,
            no_confirm: true,
        },
    };

    let startup_state = av_cli::startup::StartupState {
        config: av_core::config::AvConfig::default(),
        config_path: None,
        x0x_config: None,
        antd_running: false,
        minilm_loaded: true,
        listener_running: false,
    };

    let res = av_cli::cmd::purge::run(
        cli,
        startup_state,
        Some("test-res".to_string()),
        None,
        false,
        false,
        true,
    );
    assert!(res.is_ok());

    // Verify it is deleted
    let conn2 = av_store::open(&db_path).unwrap();
    let fetched = av_store::repo::resources::get(&conn2, "test-res").unwrap();
    assert!(fetched.is_none());

    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::remove_var("ANTA_VISTA_DB_PATH") };
}

#[test]
fn test_cli_purge_resource_removes_related_rows() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test_anta_vista.db");
    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };

    let conn = av_store::open(&db_path).unwrap();
    let rd = ResourceDescriptor {
        id: "test-res-related".to_string(),
        kind: ResourceKind::Text,
        location: "file:///related.txt".to_string(),
        location_scheme: Some("file".to_string()),
        location_canonical: Some("file:///related.txt".to_string()),
        mime_type: "text/plain".to_string(),
        filename: Some("related.txt".to_string()),
        metadata_json: serde_json::json!({}),
        description_text: "Related cleanup test".to_string(),
        created_at: 123456,
    };
    av_store::repo::resources::insert(&conn, &rd).unwrap();

    let profile = EmbeddingProfile {
        model_id: "test-model".to_string(),
        model_version: "1".to_string(),
        dim: 3,
        normalized: true,
        preproc_version: "test".to_string(),
    };
    av_store::repo::embeddings::insert_profile(&conn, "test-profile", &profile).unwrap();
    av_store::repo::embeddings::insert(
        &conn,
        &EmbeddingRecord {
            resource_id: "test-res-related".to_string(),
            profile_id: "test-profile".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            l2_norm: 1.0,
            created_at: 123457,
        },
    )
    .unwrap();
    av_store::repo::feedback::insert(
        &conn,
        &FeedbackEvent {
            schema_version: 1,
            feedback_id: "feedback-related".to_string(),
            query_text: "related".to_string(),
            resource_id: "test-res-related".to_string(),
            by_agent_id: "agent-a".to_string(),
            kind: FeedbackKind::Useful,
            timestamp: 123458,
            signature: vec![],
        },
    )
    .unwrap();

    let cli = av_cli::Cli {
        non_interactive: true,
        config: None,
        timeout: 1000,
        stream: false,
        verbose: 0,
        command: av_cli::Commands::Purge {
            resource: Some("test-res-related".to_string()),
            name: None,
            all: false,
            cache: false,
            no_confirm: true,
        },
    };
    let startup_state = av_cli::startup::StartupState {
        config: av_core::config::AvConfig::default(),
        config_path: None,
        x0x_config: None,
        antd_running: false,
        minilm_loaded: true,
        listener_running: false,
    };

    av_cli::cmd::purge::run(
        cli,
        startup_state,
        Some("test-res-related".to_string()),
        None,
        false,
        false,
        true,
    )
    .unwrap();

    let conn2 = av_store::open(&db_path).unwrap();
    assert!(av_store::repo::resources::get(&conn2, "test-res-related")
        .unwrap()
        .is_none());
    assert!(
        av_store::repo::embeddings::get(&conn2, "test-res-related", "test-profile")
            .unwrap()
            .is_none()
    );
    assert!(
        av_store::repo::feedback::list_by_resource(&conn2, "test-res-related")
            .unwrap()
            .is_empty()
    );

    // SAFETY: tests are single-threaded (Mutex-guarded), so env var access is safe
    unsafe { std::env::remove_var("ANTA_VISTA_DB_PATH") };
}
