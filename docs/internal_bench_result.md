# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 161.40 | 202.80 | 188.04 | 207.58 | - | - |
| `candidate_budget_contiguous_10000` | 179.60 | 192.40 | 181.67 | 198.50 | - | - |
| `trie_flat_100` | 7.60 | 7.60 | 7.90 | 7.95 | - | - |
| `trie_flat_1000` | 58.60 | 59.20 | 59.58 | 62.54 | - | - |
| `trie_flat_10000` | 793.50 | 819.10 | 800.91 | 843.35 | - | - |
| `trie_deep_100` | 24.50 | 25.30 | 24.96 | 25.72 | - | - |
| `trie_deep_1000` | 230.40 | 238.50 | 234.39 | 241.98 | - | - |
| `trie_deep_10000` | 2,556.60 | 2,634.90 | 2,944.51 | 2,665.05 | - | - |
| `trie_presorted_10000` | 1,070.60 | 1,068.40 | 1,088.95 | 1,088.42 | - | - |
| `trie_render_flat_1000` | 15.90 | 13.30 | 18.12 | 13.36 | - | - |
| `trie_render_deep_1000` | 16.10 | 13.60 | 16.30 | 13.66 | - | - |
| `render_text_1000` | 4,364.70 | 4,203.40 | 4,588.34 | 4,374.27 | 435.23 MB/s | 456.53 MB/s |
| `render_xml_1000` | 15,823.50 | 15,285.80 | 16,436.65 | 16,098.83 | 120.28 MB/s | 122.80 MB/s |
| `xml_escape_100k` | 283.50 | 355.90 | 285.24 | 367.22 | 350.43 MB/s | 272.20 MB/s |
| `xml_stream_write_100k` | 192.70 | 190.10 | 194.61 | 191.83 | 513.64 MB/s | 521.08 MB/s |
| `xml_escape_clean_100k` | 186.70 | 188.10 | 189.60 | 193.13 | 527.15 MB/s | 517.51 MB/s |
| `header_escape_clean_100k` | 202.70 | 227.70 | 210.46 | 241.57 | 474.89 MB/s | 413.74 MB/s |
| `header_escape_dirty_100k` | 170.10 | 243.90 | 171.57 | 248.07 | 582.81 MB/s | 403.07 MB/s |
| `token_estimate_ascii_2mb` | 3,044.60 | 3,121.90 | 3,159.72 | 3,275.66 | 663.70 MB/s | 640.21 MB/s |
| `token_estimate_mixed_2mb` | 3,161.40 | 3,057.00 | 3,523.12 | 3,221.69 | 644.42 MB/s | 650.95 MB/s |
| `text_content_ascii_100k` | 9.00 | 8.90 | 9.07 | 9.00 | 11,017.86 MB/s | 11,110.94 MB/s |
| `text_content_mixed_100k` | 85.70 | 84.60 | 104.37 | 84.65 | 957.93 MB/s | 1,180.98 MB/s |
| `storage_push_sort_presorted_1000` | 28.10 | 28.30 | 32.39 | 30.68 | - | - |
| `storage_push_sort_reverse_1000` | 28.90 | 28.60 | 31.80 | 28.95 | - | - |
| `selector_decide_10000` | 1,979.90 | 1,780.70 | 2,256.14 | 1,916.82 | - | - |
| `local_walk_hierarchical_1000` | 14,070.60 | 13,663.70 | 15,241.69 | 14,481.71 | - | - |
| `local_collect_hierarchical_1000` | 119,563.10 | 118,893.70 | 122,209.60 | 120,124.82 | - | - |
