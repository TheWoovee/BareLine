# QA-FIX-013 — resident scrollbar pointer ownership

The 24 MiB native fixture is below the default 256 MiB resident threshold. EditorSurface paints a resident scrollbar, while native scrolling Runtime registers only Paged editors. Register the exact resident geometry/metrics with that existing capture route and commit through EditorSurface::scroll, preserving bounded wrap semantics. Parent owns native validation; no tests/builds here.
