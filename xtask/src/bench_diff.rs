// SPDX-License-Identifier: MPL-2.0
//! Headless full-input diff throughput; generated pages never form a giant allocation.
use bareline_diff::{
    CancelToken, CoarseReason, CompareCompleteness, CompareOptions, DiffKind,
    paged::{PagedCompareJob, PagedComparePoll, Side},
};
use bareline_document::{
    Budget, TextOffset,
    paged::PagedSnapshot,
    source::{Generation, MemorySource, SourceKind},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Instant;

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if !args.is_empty() {
        return Err("Usage: cargo xtask perf diff".into());
    }
    let mut samples = Vec::new();
    for length in [10 * 1024 * 1024usize, 1024 * 1024 * 1024] {
        let pages = Budget::new(256 * 1024);
        let (left, lp) = MemorySource::new(
            length as u64,
            Generation(1),
            SourceKind::Paged,
            64 * 1024,
            64 * 1024,
            pages.clone(),
        )
        .map_err(|e| format!("{e:?}"))?;
        let (right, rp) = MemorySource::new(
            length as u64,
            Generation(2),
            SourceKind::Paged,
            64 * 1024,
            64 * 1024,
            pages.clone(),
        )
        .map_err(|e| format!("{e:?}"))?;
        let mut options = CompareOptions::default();
        options.limits.max_memory_bytes = 4 * 1024 * 1024;
        let mut job = PagedCompareJob::new(
            PagedSnapshot::utf8(left, 0).map_err(|e| format!("{e:?}"))?,
            PagedSnapshot::utf8(right, 0).map_err(|e| format!("{e:?}"))?,
            options,
            CancelToken::default(),
        );
        let (a, b) = (vec![b'a'; 64 * 1024], vec![b'b'; 64 * 1024]);
        let (mut consumed, mut blocks, mut page_peak, mut max_poll_us) =
            (0usize, 0usize, 0usize, 0u128);
        let start = Instant::now();
        let terminal = loop {
            let poll_started = Instant::now();
            let result = job.poll();
            max_poll_us = max_poll_us.max(poll_started.elapsed().as_micros());
            match result {
                PagedComparePoll::Pending { side, ticket } => {
                    let (publisher, bytes, generation) = match side {
                        Side::Left => (&lp, &a, Generation(1)),
                        Side::Right => (&rp, &b, Generation(2)),
                    };
                    let count = (length - ticket.page as usize * bytes.len()).min(bytes.len());
                    publisher
                        .publish(ticket, &bytes[..count], generation)
                        .map_err(|e| format!("{e:?}"))?;
                    consumed += count;
                    page_peak = page_peak.max(pages.used());
                }
                PagedComparePoll::Progress => {}
                PagedComparePoll::CoarseBlock(hunk) => {
                    if hunk.kind != DiffKind::Changed
                        || hunk.left != (TextOffset(0)..TextOffset(length))
                        || hunk.right != hunk.left
                    {
                        return Err("incorrect divergent block".into());
                    }
                    blocks += 1;
                }
                PagedComparePoll::Finished(state) => break state,
                _ => return Err("unexpected local output in capped full traversal".into()),
            }
        };
        let elapsed = start.elapsed();
        if consumed != length * 2
            || blocks != 1
            || terminal != CompareCompleteness::Coarse(CoarseReason::Bytes)
        {
            return Err("incomplete full-input verification".into());
        }
        samples.push(json!({"bytes_per_side": length, "validated_input_bytes": consumed,
            "elapsed_us": elapsed.as_micros(), "throughput_mib_s": consumed as f64 / 1048576.0 / elapsed.as_secs_f64(),
            "max_observed_poll_us": max_poll_us, "hunks": blocks, "terminal": format!("{terminal:?}"),
            "source_page_peak_bytes": page_peak, "job_memory_cap_bytes": 4194304,
            "fixture_buffers_bytes": 131072, "process_private_bytes_after": bareline_platform_windows::private_bytes().ok(),
            "process_lifetime_peak_commit_bytes": bareline_platform_windows::peak_private_bytes().ok()}));
    }
    let mut binary = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop { let count = binary.read(&mut bytes)?; if count == 0 { break; } hash.update(&bytes[..count]); }
    let output = json!({"schema_version": 2, "scenario": "bounded_diff_full_input", "os": std::env::consts::OS,
        "binary_sha256": format!("{:x}", hash.finalize()), "machine": std::env::var("COMPUTERNAME").ok(),
        "logical_cpus": std::thread::available_parallelism().ok().map(|n| n.get()), "cache_state": "generated memory pages",
        "arch": std::env::consts::ARCH, "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "fixture": "generated ASCII divergent pages; all bytes consumed; no disk throughput claim",
        "memory_scope": "source allocations measured; job cap conservative; OS lifetime process peak includes harness and prior workloads",
        "samples": samples});
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let target = root.join("target/perf");
    std::fs::create_dir_all(&target)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let path = target.join(format!("diff-{stamp}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&output)?)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    println!("Evidence: {}", path.display());
    Ok(())
}
