# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 1,001.80 | 142.90 | 1,131.99 | 143.45 | - | - |
| `candidate_budget_contiguous_10000` | 165.00 | 160.70 | 165.65 | 161.99 | - | - |
| `trie_flat_100` | 15.10 | 8.00 | 16.26 | 8.34 | - | - |
| `trie_flat_1000` | 169.10 | 62.20 | 173.83 | 62.74 | - | - |
| `trie_flat_10000` | 1,555.20 | 853.50 | 1,685.14 | 959.60 | - | - |
| `trie_deep_100` | 30.20 | 25.40 | 31.96 | 27.37 | - | - |
| `trie_deep_1000` | 335.00 | 239.00 | 340.93 | 252.09 | - | - |
| `trie_deep_10000` | 3,400.00 | 2,623.20 | 3,528.15 | 2,678.48 | - | - |
| `trie_presorted_10000` | 2,010.60 | 1,122.50 | 2,114.24 | 1,138.37 | - | - |
| `trie_render_flat_1000` | 14.00 | 21.20 | 14.33 | 21.26 | - | - |
| `trie_render_deep_1000` | 14.30 | 21.50 | 14.42 | 21.62 | - | - |
| `render_text_1000` | 5,400.50 | 5,892.20 | 5,554.31 | 6,018.84 | 359.54 MB/s | 331.79 MB/s |
| `render_xml_1000` | 17,694.00 | 18,345.20 | 18,049.01 | 18,920.04 | 109.54 MB/s | 104.49 MB/s |
| `xml_escape_100k` | 436.30 | 463.90 | 449.80 | 473.79 | 222.23 MB/s | 210.98 MB/s |
| `xml_stream_write_100k` | 223.10 | 239.60 | 350.82 | 239.75 | 284.93 MB/s | 416.93 MB/s |
| `xml_escape_clean_100k` | 164.00 | 169.40 | 170.28 | 170.27 | 586.96 MB/s | 586.97 MB/s |
| `header_escape_clean_100k` | 152.20 | 130.00 | 169.99 | 134.34 | 587.95 MB/s | 743.98 MB/s |
| `header_escape_dirty_100k` | 251.30 | 152.40 | 259.61 | 152.54 | 385.16 MB/s | 655.48 MB/s |
| `token_estimate_ascii_2mb` | 4,253.90 | 4,134.30 | 4,793.74 | 4,295.12 | 437.47 MB/s | 488.25 MB/s |
| `token_estimate_mixed_2mb` | 4,248.80 | 4,245.80 | 4,501.67 | 4,323.59 | 465.86 MB/s | 485.05 MB/s |
| `text_content_ascii_100k` | 11.30 | 109.00 | 29.12 | 112.20 | 3,432.72 MB/s | 890.84 MB/s |
| `text_content_mixed_100k` | 84.60 | 177.80 | 122.58 | 186.04 | 815.59 MB/s | 537.38 MB/s |
| `storage_push_sort_presorted_1000` | 29.20 | 28.80 | 29.90 | 30.06 | - | - |
| `storage_push_sort_reverse_1000` | 29.30 | 29.30 | 31.50 | 29.74 | - | - |
| `selector_decide_10000` | 2,726.80 | 1,899.90 | 2,994.36 | 1,991.01 | - | - |
| `local_collect_hierarchical_1000` | 139,675.50 | 136,220.30 | 142,099.03 | 145,104.20 | - | - |
