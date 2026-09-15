# PR-T01/T02 final integration corrections

- Base: `bc5baab471751681f88486a1a6e97a08c031cf02`
- Scope: retain the exact T01 cache producer registry during resident promotion, and route T02 migration filename identity through the shared lossless path codec.

## Confirmed causes

- Resident promotion publishes staging directories under `Bareline-owned-spill`, then nested disk transcoders try to publish `bareline-transcode-<pid>-<serial>` as `CacheKind::Transcode` under that same root. This occurs through both `prepare_resident` and `SpillOwnedResident`'s original-baseline/open paths. T01 permits transcode producers only under `Bareline-transcode`, so the rejection is correct and production must select the authorized producer root.
- Profile migration duplicated platform-specific `OsStr` byte encoding inside the neutral file-I/O crate. `bareline_platform::SerializedPath` already owns the lossless native path identity contract.

## Corrective contract

- A registered producer root may resolve a sibling only after the supplied root passes the existing exact registry check. The destination name comes from `owned_cache::ROOTS`, must authorize the requested kind, and must be unique. Ambiguous kinds fail closed. Already-authorized roots are unchanged; unregistered custom roots receive no registered authority and retain their previous path.
- `DiskTranscoder::new` applies the resolver at the transcode producer boundary, and `continuation` delegates through it. Resident and baseline staging remain under `Bareline-owned-spill`; all raw/text/map transcode output and its cleanup owner remain under `Bareline-transcode`.
- Migration directory-entry hashing obtains native identity bytes through `SerializedPath`, preserving Windows UTF-16 code units and Unix bytes without a second platform-specific codec.

## Verification ownership

- Worker: portability guard, focused formatting, and diff validation only.
- Root: `owned_cache::tests::producer_root_resolution_requires_exact_registered_authority`, `owned_store::registered_producer_tests::resident_and_original_baseline_route_nested_transcodes_to_exact_registered_root`, the two failed promotion tests (`large_selected_copy_promotes_exact_destination_and_waits_for_durable_actor` and `promotion_rebinds_linked_views_without_replacing_tabs_or_history`), and final Cargo/native gates after source review.
