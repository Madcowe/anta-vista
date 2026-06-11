use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    Warn,
}

impl TestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TestStatus::Pass => "PASS",
            TestStatus::Fail => "FAIL",
            TestStatus::Skip => "SKIP",
            TestStatus::Warn => "WARN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub test_id: String,
    pub category: String,
    pub name: String,
    pub transport: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub details: String,
    pub debug: serde_json::Value,
}

/// Print a single test result as JSON-line to stdout
pub fn print_json_line(result: &TestResult) {
    if let Ok(serialized) = serde_json::to_string(result) {
        println!("{}", serialized);
    }
}

/// Print the final Markdown summary report to stdout
pub fn print_markdown_summary(node_id: &str, peer_id: Option<&str>, results: &[TestResult]) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    
    println!("\n# Anta-Vista Test Suite Summary");
    println!("**Date**: {}", now);
    println!("**Node ID**: {}", node_id);
    println!("**Peer ID**: {}", peer_id.unwrap_or("None (Autodetected/Not Specified)"));
    println!();
    
    println!("| Test ID | Category | Name | Transport | Status | Duration | Details |");
    println!("|---|---|---|---|---|---|---|");
    
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut warned = 0;
    
    for r in results {
        match r.status {
            TestStatus::Pass => passed += 1,
            TestStatus::Fail => failed += 1,
            TestStatus::Skip => skipped += 1,
            TestStatus::Warn => warned += 1,
        }
        
        let status_emoji = match r.status {
            TestStatus::Pass => "✅ PASS",
            TestStatus::Fail => "❌ FAIL",
            TestStatus::Skip => "⚠️ SKIP",
            TestStatus::Warn => "⚠️ WARN",
        };
        
        println!(
            "| {} | {} | {} | {} | {} | {}ms | {} |",
            r.test_id, r.category, r.name, r.transport, status_emoji, r.duration_ms, r.details
        );
    }
    
    println!();
    println!("**Execution Result**: {} Passed, {} Failed, {} Skipped, {} Warned.", passed, failed, skipped, warned);
    println!();
}
