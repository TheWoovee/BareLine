# QA-FIX-010 — native multi-file Open

Authority: PR-004 file lifecycle. Editor Open used a single-selection IFileDialog and GetResult, so multiple file selections could not reach the workspace. Add an editor-specific IFileOpenDialog with ALLOWMULTISELECT and enumerate GetResults. All paths are gathered before scheduling opens; cancellation remains silent, errors remain visible. Import/settings dialogs retain single-file semantics.

Verification: static only; native picker and builds/tests deferred by user.
