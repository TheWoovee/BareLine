# Remaining paged spill fixture cleanup

The pressure spill regression refreshes viewports, which can start independent viewport-prefetch threads holding the paged actor. Draining the shared paged I/O queue alone does not wait for those threads. Fixture deletion could therefore race live Windows file handles and fail with OS error 5 after its behavioral assertions passed.

The fixture now retains the actor during teardown, drops the view to cancel background work, drains the shared worker, and uses bounded `Arc::try_unwrap` polling to obtain exclusive actor ownership. It destroys that actor synchronously before removing the temporary directory. The ownership wait fails explicitly after 15 seconds rather than masking a leaked owner or ignoring a cleanup error.

Source-only change; no tests, builds, or UI runs were executed in this lane. The focused pressure-spill regression remains queued for the shared validation gate.
