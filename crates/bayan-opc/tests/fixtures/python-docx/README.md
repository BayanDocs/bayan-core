# Real `.docx` files from python-docx

Five small documents that bayan-opc's round-trip tests open and write back (work package CORE-005, acceptance criterion 1). They come from [python-docx](https://github.com/python-openxml/python-docx) 1.2.0, copied unchanged from its source archive `python_docx-1.2.0.tar.gz` on PyPI (published 2025-06-16, SHA-256 `7bc9d7b7d8a69c9c02ca09216118c86552704edc23bac179283f2e38f86220ce`).

License: MIT, Copyright (c) 2013 Steve Canny ([LICENSES/MIT.txt](../../../../../LICENSES/MIT.txt), recorded in `REUSE.toml`). They are python-docx's own test documents, made for its test suite, not personal documents; their metadata names the python-docx author and project.

| File | From the archive | Made by | What it exercises | SHA-256 |
|---|---|---|---|---|
| `doc-word-default-blank.docx` | `features/steps/test_files/` | Word 2011 for Mac (14.0) | Word's own ZIP layout: fixed timestamps, a stored (uncompressed) thumbnail | `560b0e299a592f6b7d1ec9abab98648f1d92bf8fa684ce544a0424fdde798903` |
| `par-hyperlinks.docx` | `features/steps/test_files/` | Word 2016 (16.0), then edited with a Unix ZIP tool | external hyperlink relationships; entries from two different writers, some with extra fields | `206539619eb347b1cd5b3fe88031cc18327accfe5d34a886f3d9120115f6bdad` |
| `doc-coreprops.docx` | `features/steps/test_files/` | Word, rewritten by python-docx | core properties; metadata XML in python-docx's style (single quotes, `\n`) | `358ba16289fc12f9c50a22d74ab610ef4dc74563c5ed7c889ac48f372b956dd0` |
| `hdr-header-footer.docx` | `features/steps/test_files/` | Word 2016 (16.0) | many parts: headers, footers, footnotes | `5a4cb270680d1365688073e4816be98c2ab1a7ba2921d86cb3efadca97a72d27` |
| `default.docx` | `src/docx/templates/` | Word, rewritten by python-docx | python-docx's default template: custom XML parts with their own relationships | `2094b5bddffe9cf973d61fe03388413804f034160718494a65db7e98da40d35d` |
