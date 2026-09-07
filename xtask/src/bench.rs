// SPDX-License-Identifier: MPL-2.0
//! Headless source-path measurements; never substitutes for UI/comparative timings.
use bareline_document::{
    Budget, Edit, EditTransaction, TextOffset,
    paged::{PagedDocument, PagedSnapshot, TextWindow, WindowPoll},
};
use bareline_file_io::{
    cancellation::Cancellation,
    source::{FileSource, SourceOptions},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
#[path = "bench_product.rs"]
mod product;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const PAGE: usize = 64 * 1024;
const CACHE: usize = 4 * 1024 * 1024;
fn window(
    snapshot: &PagedSnapshot,
    file: &mut FileSource,
    start: usize,
    end: usize,
    budget: &Budget,
    pages: &mut u64,
) -> Result<TextWindow> {
    let mut request = snapshot
        .begin_read(TextOffset(start)..TextOffset(end), PAGE, budget)
        .map_err(|e| format!("read: {e:?}"))?;
    loop {
        match request.poll() {
            WindowPoll::Ready(value) => return Ok(value),
            WindowPoll::Pending(ticket) => {
                file.read_page(ticket).map_err(|e| format!("page: {e:?}"))?;
                *pages += 1;
            }
            _ => return Err("fixture window unavailable or invalid UTF-8".into()),
        }
    }
}
fn fixture(path: &Path, bytes: u64, long_line: bool) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut block = vec![b'x'; PAGE];
    if !long_line {
        for end in (127..PAGE).step_by(128) {
            block[end] = b'\n';
        }
    }
    let mut remaining = bytes;
    while remaining > 0 {
        let count = remaining.min(PAGE as u64) as usize;
        file.write_all(&block[..count])?;
        remaining -= count as u64;
    }
    file.sync_all()?;
    Ok(())
}
pub fn run(args: &[String]) -> Result<()> {
    let mut bytes = 1024 * 1024u64;
    let mut samples = 1usize;
    let mut long_line = false;
    let mut include_product = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bytes" => {
                i += 1;
                bytes = args.get(i).ok_or("missing --bytes")?.parse()?;
            }
            "--samples" => {
                i += 1;
                samples = args.get(i).ok_or("missing --samples")?.parse()?;
            }
            "--long-line" => long_line = true,
            "--product" => include_product = true,
            _ => {
                return Err(
                    "Usage: perf document [--bytes N] [--samples N] [--long-line] [--product]"
                        .into(),
                );
            }
        }
        i += 1;
    }
    if bytes == 0 || bytes > 5 * 1024 * 1024 * 1024 || !(1..=100).contains(&samples) {
        return Err("bytes must be 1..5GiB; samples 1..100".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let directory = root.join("tests/perf/results");
    fs::create_dir_all(&directory)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = directory.join(format!("fixture-{stamp}.bin"));
    fixture(&path, bytes, long_line)?;
    let measured = measure(&path, bytes, samples);
    fs::remove_file(&path)?;
    let records = measured?;
    let product = if include_product {
        Some(product::run(&directory, stamp)?)
    } else {
        None
    };
    let mut executable = fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut block = [0u8; PAGE];
    loop {
        let count = std::io::Read::read(&mut executable, &mut block)?;
        if count == 0 {
            break;
        }
        hash.update(&block[..count]);
    }
    let binary_sha256 = format!("{:x}", hash.finalize());
    let os_build = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let result = json!({"schema_version": 2, "product_workloads": product, "binary_sha256": binary_sha256, "os_build": os_build, "logical_cpus": std::thread::available_parallelism().ok().map(|n| n.get()), "kind": "headless_document_source", "fixture": {"generator": "ascii-x-v1", "bytes": bytes, "long_line": long_line}, "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "machine": std::env::var("COMPUTERNAME").ok(), "profile": if cfg!(debug_assertions) {"debug"} else {"release"}, "cache_state": "uncontrolled; fixture just generated", "renderer": null, "working_set_bytes": null, "private_bytes_scope": "whole xtask process point samples, not peak/editor idle", "samples": records});
    let output = directory.join(format!("document-{stamp}.json"));
    fs::write(&output, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", output.display());
    Ok(())
}
fn measure(path: &Path, bytes: u64, samples: usize) -> Result<Vec<serde_json::Value>> {
    let mut records = Vec::new();
    for sample in 0..samples {
        for resident in if sample % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            // Full resident fills above the current product threshold are intentionally omitted.
            if resident && bytes > 256 * 1024 * 1024 {
                continue;
            }
            let budget = Budget::new(if resident {
                bytes as usize + 2 * CACHE
            } else {
                2 * CACHE
            });
            let mut pages = 0;
            let started = Instant::now();
            let mut file = FileSource::open(
                path,
                Arc::new(bareline_platform_windows::WindowsFileSystem),
                SourceOptions {
                    resident_max_bytes: if resident { bytes } else { 0 },
                    page_size_bytes: PAGE,
                    page_cache_bytes: CACHE,
                },
                budget.clone(),
                Cancellation::default(),
            )
            .map_err(|e| format!("open: {e:?}"))?;
            let snapshot =
                PagedSnapshot::utf8(file.source(), 0).map_err(|e| format!("snapshot: {e:?}"))?;
            let mut doc = PagedDocument::new(snapshot.clone(), budget.clone(), Budget::new(CACHE));
            let view = window(
                &snapshot,
                &mut file,
                0,
                (bytes as usize).min(PAGE),
                &budget,
                &mut pages,
            )?;
            let viewport_us = started.elapsed().as_micros();
            let viewport_budget_bytes = budget.used();
            let viewport_pages = pages;
            let edit = Instant::now();
            doc.apply_materialized(
                EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(0)..TextOffset(0),
                        insert: "!".into(),
                    }],
                },
                &[view],
            )
            .map_err(|e| format!("edit: {e:?}"))?;
            let edit_us = edit.elapsed().as_micros();
            let undo = Instant::now();
            doc.undo().map_err(|e| format!("undo: {e:?}"))?;
            let undo_us = undo.elapsed().as_micros();
            let redo = Instant::now();
            doc.redo().map_err(|e| format!("redo: {e:?}"))?;
            let redo_us = redo.elapsed().as_micros();
            let search = Instant::now();
            let mut hits = 0u64;
            // Single-byte absent literal intentionally scans every byte without result growth.
            for start in (0..snapshot.len()).step_by(PAGE) {
                let value = window(
                    &snapshot,
                    &mut file,
                    start,
                    (start + PAGE).min(snapshot.len()),
                    &budget,
                    &mut pages,
                )?;
                hits += value.text().bytes().filter(|b| *b == b'~').count() as u64;
            }
            let search_us = search.elapsed().as_micros();
            records.push(json!({"sample": sample, "source": if resident {"resident"} else {"paged"}, "page_size_bytes": PAGE, "page_cache_bytes": CACHE, "open_editable_window_us": viewport_us, "viewport_budget_bytes": viewport_budget_bytes, "viewport_page_requests": viewport_pages, "edit_transaction_us": edit_us, "undo_us": undo_us, "redo_us": redo_us, "harness_absent_literal_scan_us": search_us, "scan_bytes": bytes, "matches": hits, "total_page_requests": pages, "final_budget_bytes": budget.used(), "process_private_bytes": bareline_platform_windows::private_bytes().ok(), "first_frame_us": null, "full_load_us": null}));
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_exact_length_and_existing_file_is_preserved() {
        let path = std::env::temp_dir().join(format!(
            "bareline-perf-fixture-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fixture(&path, PAGE as u64 + 7, false).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), PAGE + 7);
        assert_eq!(bytes[127], b'\n');
        assert!(fixture(&path, 1, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
    }
}
