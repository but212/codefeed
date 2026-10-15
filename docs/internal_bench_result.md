# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 1,001.80 | 140.90 | 1,131.99 | 142.35 | - | - |
| `candidate_budget_contiguous_10000` | 165.00 | 166.90 | 165.65 | 184.50 | - | - |
| `trie_flat_100` | 15.10 | 7.30 | 16.26 | 7.57 | - | - |
| `trie_flat_1000` | 169.10 | 57.30 | 173.83 | 58.09 | - | - |
| `trie_flat_10000` | 1,555.20 | 812.20 | 1,685.14 | 852.53 | - | - |
| `trie_deep_100` | 30.20 | 25.30 | 31.96 | 27.05 | - | - |
| `trie_deep_1000` | 335.00 | 238.60 | 340.93 | 246.20 | - | - |
| `trie_deep_10000` | 3,400.00 | 2,672.90 | 3,528.15 | 2,721.65 | - | - |
| `trie_presorted_10000` | 2,010.60 | 1,020.90 | 2,114.24 | 1,034.81 | - | - |
| `trie_render_flat_1000` | 14.00 | 21.00 | 14.33 | 21.02 | - | - |
| `trie_render_deep_1000` | 14.30 | 21.20 | 14.42 | 21.32 | - | - |
| `render_text_1000` | 5,400.50 | 6,446.80 | 5,554.31 | 6,873.36 | 359.54 MB/s | 290.54 MB/s |
| `render_xml_1000` | 17,694.00 | 19,078.20 | 18,049.01 | 19,668.08 | 109.54 MB/s | 100.52 MB/s |
| `xml_escape_100k` | 436.30 | 381.80 | 449.80 | 399.74 | 222.23 MB/s | 250.06 MB/s |
| `xml_stream_write_100k` | 223.10 | 188.50 | 350.82 | 191.79 | 284.93 MB/s | 521.18 MB/s |
| `xml_escape_clean_100k` | 164.00 | 115.80 | 170.28 | 128.72 | 586.96 MB/s | 776.44 MB/s |
| `header_escape_clean_100k` | 152.20 | 121.00 | 169.99 | 121.55 | 587.95 MB/s | 822.28 MB/s |
| `header_escape_dirty_100k` | 251.30 | 300.10 | 259.61 | 304.17 | 385.16 MB/s | 328.73 MB/s |
| `token_estimate_ascii_2mb` | 4,253.90 | 4,237.60 | 4,793.74 | 4,310.68 | 437.47 MB/s | 486.49 MB/s |
| `token_estimate_mixed_2mb` | 4,248.80 | 4,237.90 | 4,501.67 | 4,346.88 | 465.86 MB/s | 482.45 MB/s |
| `text_content_ascii_100k` | 11.30 | 8.90 | 29.12 | 9.01 | 3,432.72 MB/s | 11,088.75 MB/s |
| `text_content_mixed_100k` | 84.60 | 82.30 | 122.58 | 83.55 | 815.59 MB/s | 1,196.65 MB/s |
| `storage_push_sort_presorted_1000` | 29.20 | 27.90 | 29.90 | 32.12 | - | - |
| `storage_push_sort_reverse_1000` | 29.30 | 28.40 | 31.50 | 29.12 | - | - |
| `selector_decide_10000` | 2,726.80 | 1,810.90 | 2,994.36 | 1,927.20 | - | - |
| `local_walk_hierarchical_1000` | 14,054.80 | - | 15,752.81 | - | - | - |
| `local_collect_hierarchical_1000` | 139,675.50 | 137,506.60 | 142,099.03 | 140,991.09 | - | - |
