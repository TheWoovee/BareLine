# QA-FIX-029 — owned Run confirmation

Run used an ownerless MessageBoxW, unlike document confirmations. Route the existing confirmation through WindowsPlatform and pass its live HWND within the backend. Default No and exact preview/permission semantics remain unchanged. This corrects ownership/activation; it does not establish the cause of the separate Close automation observation (008). No tests/builds here; parent validates native presentation.
