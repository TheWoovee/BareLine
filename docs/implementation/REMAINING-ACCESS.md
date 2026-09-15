# Remaining accessibility follow-up

Scope: retest 005/011 focus/value qualification and 015 giant-line selection, under the existing PR024 accessibility and PR003 selection contracts.

015: power selection normalization previously read each entire logical line to snap a caret to a grapheme boundary. A 20 MiB line exceeded its 16 MiB power budget even for a collapsed caret. Endpoint snapping now needs no read; interior snapping uses a Unicode GraphemeCursor with bounded 4 KiB reads and a cumulative context budget. Huge ambiguous clusters may still report BudgetExceeded rather than guessing a boundary. Existing occurrence-selection fixes are preserved.

Added a focused regression for EOF, interior ASCII, combining-mark boundaries and offsets inside a multibyte scalar on a 20 MiB line with a 16 KiB context budget. Added a production native Shell snapshot regression checking Find query focus/value followed by replacement focus and changed value.

005/011: current production projection contains Find/Replace values and focuses the selected field. No demonstrated provider failure was found that explains the delayed Sky observation; no speculative provider workaround was added. Physical UIA observation remains required, including Outline and other custom panels. The new regression and affected compile/tests are deliberately not run in this source-only cohort; integration owner owns the gate. No native UI, build or commit performed.
