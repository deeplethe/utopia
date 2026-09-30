# OCR page fixture

`ark-ocr-pages.pdf` is a synthetic three-page PDF created for OCR acceptance tests.
Pages 1 and 3 contain rasterized Chinese and English text, dates, amounts, a small
table, an instruction printed as source text, and an unlabelled line chart. Page 2
is blank. There is no text layer. All content is authored for this test and carries
no third-party or private data.

The fixture tests real Poppler rendering, original page numbering after a blank
page, and restarting a failed pass from page 1. Mock replies test those contracts;
they do not measure a model's recognition accuracy.

With `ARK_API_KEY` set, run the manual Agent Plan acceptance test:

```sh
cargo test -p utopia-server live_agent_plan_reads_scans -- --ignored --nocapture
```

This sends the probe and this fixture to the real API. Optional
`ARK_OCR_TEST_PDF` adds a public scan, and `ARK_OCR_TEST_OUTPUT` saves its text and
provenance without credentials. `ARK_OCR_TEST_BASE_URL` and
`ARK_OCR_TEST_MODEL` override the Agent Plan defaults.
