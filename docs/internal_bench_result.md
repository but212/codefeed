# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 202.80 | 154.10 | 207.58 | 154.99 | - | - |
| `candidate_budget_contiguous_10000` | 192.40 | 164.30 | 198.50 | 167.76 | - | - |
| `trie_flat_100` | 7.60 | 7.50 | 7.95 | 7.75 | - | - |
| `trie_flat_1000` | 59.20 | 57.40 | 62.54 | 61.53 | - | - |
| `trie_flat_10000` | 819.10 | 812.20 | 843.35 | 862.78 | - | - |
| `trie_deep_100` | 25.30 | 24.40 | 25.72 | 25.00 | - | - |
| `trie_deep_1000` | 238.50 | 230.30 | 241.98 | 275.32 | - | - |
| `trie_deep_10000` | 2,634.90 | 2,535.00 | 2,665.05 | 2,832.67 | - | - |
| `trie_presorted_10000` | 1,068.40 | 1,038.70 | 1,088.42 | 1,073.42 | - | - |
| `trie_render_flat_1000` | 13.30 | 12.50 | 13.36 | 12.84 | - | - |
| `trie_render_deep_1000` | 13.60 | 12.80 | 13.66 | 13.16 | - | - |
| `render_text_1000` | 4,203.40 | 4,375.50 | 4,374.27 | 4,557.85 | 456.53 MB/s | 438.14 MB/s |
| `render_xml_1000` | 15,285.80 | 13,815.10 | 16,098.83 | 13,982.75 | 122.80 MB/s | 141.39 MB/s |
| `xml_escape_100k` | 355.90 | 305.40 | 367.22 | 316.54 | 272.20 MB/s | 315.78 MB/s |
| `xml_stream_write_100k` | 190.10 | 56.70 | 191.83 | 56.78 | 521.08 MB/s | 1,760.38 MB/s |
| `xml_escape_clean_100k` | 188.10 | 112.20 | 193.13 | 113.46 | 517.51 MB/s | 880.88 MB/s |
| `header_escape_clean_100k` | 227.70 | 113.20 | 241.57 | 117.79 | 413.74 MB/s | 848.50 MB/s |
| `header_escape_dirty_100k` | 243.90 | 138.20 | 248.07 | 141.92 | 403.07 MB/s | 704.54 MB/s |
| `token_estimate_ascii_2mb` | 3,121.90 | 2,996.30 | 3,275.66 | 3,093.94 | 640.21 MB/s | 677.81 MB/s |
| `token_estimate_mixed_2mb` | 3,057.00 | 2,996.50 | 3,221.69 | 3,070.32 | 650.95 MB/s | 683.04 MB/s |
| `text_content_ascii_100k` | 8.90 | 8.70 | 9.00 | 8.82 | 11,110.94 MB/s | 11,337.79 MB/s |
| `text_content_mixed_100k` | 84.60 | 83.00 | 84.65 | 87.15 | 1,180.98 MB/s | 1,147.16 MB/s |
| `storage_push_sort_presorted_1000` | 28.30 | 25.90 | 30.68 | 28.24 | - | - |
| `storage_push_sort_reverse_1000` | 28.60 | 26.90 | 28.95 | 27.30 | - | - |
| `selector_decide_10000` | 1,780.70 | 1,675.50 | 1,916.82 | 1,798.59 | - | - |
| `local_walk_hierarchical_1000` | 13,663.70 | 13,321.60 | 14,481.71 | 14,182.12 | - | - |
| `local_collect_hierarchical_1000` | 118,893.70 | 118,423.20 | 120,124.82 | 120,061.48 | - | - |
