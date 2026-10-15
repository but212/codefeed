# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 1,001.80 | 164.90 | 1,131.99 | 165.31 | - | - |
| `candidate_budget_contiguous_10000` | 165.00 | 181.90 | 165.65 | 185.00 | - | - |
| `trie_flat_100` | 15.10 | 7.80 | 16.26 | 8.10 | - | - |
| `trie_flat_1000` | 169.10 | 63.50 | 173.83 | 77.48 | - | - |
| `trie_flat_10000` | 1,555.20 | 857.60 | 1,685.14 | 890.79 | - | - |
| `trie_deep_100` | 30.20 | 25.50 | 31.96 | 25.97 | - | - |
| `trie_deep_1000` | 335.00 | 243.00 | 340.93 | 254.39 | - | - |
| `trie_deep_10000` | 3,400.00 | 2,687.10 | 3,528.15 | 2,733.35 | - | - |
| `trie_presorted_10000` | 2,010.60 | 1,160.70 | 2,114.24 | 1,177.83 | - | - |
| `trie_render_flat_1000` | 14.00 | 16.10 | 14.33 | 16.13 | - | - |
| `trie_render_deep_1000` | 14.30 | 16.40 | 14.42 | 16.46 | - | - |
| `render_text_1000` | 5,400.50 | 4,270.60 | 5,554.31 | 4,411.00 | 359.54 MB/s | 452.73 MB/s |
| `render_xml_1000` | 17,694.00 | 16,429.80 | 18,049.01 | 16,762.41 | 109.54 MB/s | 117.94 MB/s |
| `xml_escape_100k` | 436.30 | 390.20 | 449.80 | 397.51 | 222.23 MB/s | 251.46 MB/s |
| `xml_stream_write_100k` | 223.10 | 193.20 | 350.82 | 194.03 | 284.93 MB/s | 515.17 MB/s |
| `xml_escape_clean_100k` | 164.00 | 98.10 | 170.28 | 98.43 | 586.96 MB/s | 1,015.38 MB/s |
| `header_escape_clean_100k` | 152.20 | 112.90 | 169.99 | 116.54 | 587.95 MB/s | 857.61 MB/s |
| `header_escape_dirty_100k` | 251.30 | 215.60 | 259.61 | 217.82 | 385.16 MB/s | 459.05 MB/s |
| `token_estimate_ascii_2mb` | 4,253.90 | 3,054.70 | 4,793.74 | 3,208.66 | 437.47 MB/s | 653.57 MB/s |
| `token_estimate_mixed_2mb` | 4,248.80 | 3,122.60 | 4,501.67 | 3,254.34 | 465.86 MB/s | 644.42 MB/s |
| `text_content_ascii_100k` | 11.30 | 9.00 | 29.12 | 9.06 | 3,432.72 MB/s | 11,034.89 MB/s |
| `text_content_mixed_100k` | 84.60 | 87.00 | 122.58 | 91.02 | 815.59 MB/s | 1,098.34 MB/s |
| `storage_push_sort_presorted_1000` | 29.20 | 28.40 | 29.90 | 29.11 | - | - |
| `storage_push_sort_reverse_1000` | 29.30 | 29.20 | 31.50 | 29.57 | - | - |
| `selector_decide_10000` | 2,726.80 | 1,892.50 | 2,994.36 | 1,963.09 | - | - |
| `local_walk_hierarchical_1000` | 14,054.80 | 13,331.00 | 15,752.81 | 14,395.20 | - | - |
| `local_collect_hierarchical_1000` | 139,675.50 | 113,754.80 | 142,099.03 | 115,961.33 | - | - |
