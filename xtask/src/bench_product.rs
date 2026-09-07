// SPDX-License-Identifier: MPL-2.0
//! Small real product workloads, deliberately independent of huge-file fixture size.
use bareline_document::{Budget, Document};
use bareline_file_io::{
    cancellation::Cancellation,
    recovery::{self, RecoveryEdit, RecoveryMetadata, RecoveryWriter},
};
use bareline_search::{SearchJob, SearchMode, SearchQuery};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Instant};

pub fn run(directory: &Path, stamp: u128) -> super::Result<Value> {
    let text = "INFO event=1234 stable record\n".repeat(2048);
    let document = Document::from_utf8(&text, Budget::new(1024 * 1024), Budget::new(1024 * 1024))
        .map_err(|e| format!("fixture document: {e:?}"))?;
    let snapshot = document.snapshot();
    let mut query = SearchQuery::literal("event=[0-9]+");
    query.mode = SearchMode::Regex;
    let start = Instant::now();
    let regex = bareline_search::scan(&snapshot, &query, &SearchJob::default(), |_| {});
    let regex_us = start.elapsed().as_micros();
    let start = Instant::now();
    let opened = bareline_search::sources::scan_open_documents(
        [snapshot.clone(), snapshot.clone()],
        &SearchQuery::literal("event="),
        &SearchJob::default(),
        |_| {},
    );
    let open_documents_us = start.elapsed().as_micros();
    let right = Document::from_utf8(
        &text.replacen("1234", "5678", 1),
        Budget::new(1024 * 1024),
        Budget::new(1024 * 1024),
    )
    .map_err(|e| format!("diff fixture: {e:?}"))?;
    let start = Instant::now();
    let mut first_batch_us = None;
    let mut batch_count = 0;
    let diff = bareline_diff::compare_batches(
        &snapshot,
        &right.snapshot(),
        &Default::default(),
        &Default::default(),
        16,
        |_| {
            first_batch_us.get_or_insert_with(|| start.elapsed().as_micros());
            batch_count += 1;
            true
        },
    );
    let diff_total_us = start.elapsed().as_micros();
    let recovery_path = directory.join(format!("recovery-{stamp}"));
    let platform = bareline_platform_windows::WindowsFileSystem;
    let cancel = Cancellation::default();
    let start = Instant::now();
    let mut writer = RecoveryWriter::create(
        &recovery_path,
        RecoveryMetadata {
            source_generation: "synthetic-owned-v1".into(),
            codec_catalog_version: "utf8-fixture-v1".into(),
            original_len: text.len() as u64,
        },
        &platform,
    )?;
    writer.seal_baseline(&mut text.as_bytes(), || Ok(true), &cancel, &platform)?;
    let baseline_us = start.elapsed().as_micros();
    let start = Instant::now();
    writer.append(
        1,
        &[RecoveryEdit {
            offset: 0,
            removed: Vec::new(),
            inserted: b"!".to_vec(),
        }],
    )?;
    writer.checkpoint(&platform)?;
    let durable_transaction_us = start.elapsed().as_micros();
    drop(writer);
    let recovered = directory.join(format!("recovered-{stamp}.txt"));
    let start = Instant::now();
    let inspection = recovery::recover_to(&recovery_path, &recovered, &cancel)?;
    let replay_us = start.elapsed().as_micros();
    let recovered_bytes = fs::metadata(&recovered)?.len();
    // Preserve the small owned journal with the raw measurement for reproducibility.
    Ok(
        json!({"fixture_bytes": text.len(), "regex": {"elapsed_us": regex_us, "matches": regex.count(), "completeness": format!("{:?}", regex.completeness()), "retained_bytes": regex.retained_bytes()}, "open_documents": {"document_count": 2, "snapshot_storage": "shared clone", "elapsed_us": open_documents_us, "matches": opened.count(), "completeness": format!("{:?}", opened.completeness()), "retained_bytes": opened.retained_bytes()}, "diff": {"first_batch_us": first_batch_us, "total_us": diff_total_us, "batches": batch_count, "completeness": format!("{:?}", diff.completeness), "accounted_peak_bytes": diff.stats.peak_accounted_bytes}, "recovery": {"baseline_us": baseline_us, "durable_transaction_us": durable_transaction_us, "replay_us": replay_us, "recovered_bytes": recovered_bytes, "status": format!("{:?}", inspection.status), "validated_records": inspection.validated_records, "journal_directory": recovery_path}}),
    )
}
