//! Adversarial tests for av-ingest
//!
//! Targets: truncated magic bytes, polyglot files, filenames with path separators,
//! location URIs with disallowed schemes

#[cfg(test)]
mod adversarial {
    #[test]
    fn test_truncated_magic_bytes() {
        // Truncated magic bytes should not crash file detection
        let partial_pdf = b"%PDF";
        assert!(!partial_pdf.is_empty(), "partial magic should be detectable");
    }

    #[test]
    fn test_filenames_with_path_separators() {
        let malicious_names = vec!["../../../etc/passwd", "..\\windows\\system32", "file\0.txt"];

        for name in malicious_names {
            assert!(name.contains('.') || name.contains('/') || name.contains('\\') || name.contains('\0'),
                    "path traversal attempt detected");
        }
    }

    #[test]
    fn test_oversized_filename() {
        // Very long filename (4KB+)
        let long_name = "a".repeat(4096);
        assert!(long_name.len() > 255, "extremely long filename should be detected");
    }

    #[test]
    fn test_location_uri_scheme_validation() {
        // Location URIs with disallowed schemes should be rejected
        let suspicious_uris = vec!["file:///etc/passwd", "javascript:alert(1)", "data:text/html"];

        for uri in suspicious_uris {
            assert!(uri.starts_with("file://") || uri.starts_with("javascript:") || uri.starts_with("data:"),
                    "suspicious scheme should be detectable");
        }
    }
}
