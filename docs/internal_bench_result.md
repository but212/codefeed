# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 1,001.80 | 156.40 | 1,131.99 | 158.70 | - | - |
| `candidate_budget_contiguous_10000` | 165.00 | 185.00 | 165.65 | 191.83 | - | - |
| `trie_flat_100` | 15.10 | 7.50 | 16.26 | 7.73 | - | - |
| `trie_flat_1000` | 169.10 | 58.10 | 173.83 | 58.74 | - | - |
| `trie_flat_10000` | 1,555.20 | 793.80 | 1,685.14 | 836.31 | - | - |
| `trie_deep_100` | 30.20 | 25.20 | 31.96 | 25.56 | - | - |
| `trie_deep_1000` | 335.00 | 238.90 | 340.93 | 239.73 | - | - |
| `trie_deep_10000` | 3,400.00 | 2,570.40 | 3,528.15 | 2,631.42 | - | - |
| `trie_presorted_10000` | 2,010.60 | 1,081.80 | 2,114.24 | 1,095.18 | - | - |
| `trie_render_flat_1000` | 14.00 | 21.00 | 14.33 | 21.10 | - | - |
| `trie_render_deep_1000` | 14.30 | 21.30 | 14.42 | 22.91 | - | - |
| `render_text_1000` | 5,400.50 | 5,185.10 | 5,554.31 | 5,281.62 | 359.54 MB/s | 378.10 MB/s |
| `render_xml_1000` | 17,694.00 | 17,253.70 | 18,049.01 | 17,633.97 | 109.54 MB/s | 112.11 MB/s |
| `xml_escape_100k` | 436.30 | 374.70 | 449.80 | 392.56 | 222.23 MB/s | 254.63 MB/s |
| `xml_stream_write_100k` | 223.10 | 183.80 | 350.82 | 186.76 | 284.93 MB/s | 535.22 MB/s |
| `xml_escape_clean_100k` | 164.00 | 97.20 | 170.28 | 97.87 | 586.96 MB/s | 1,021.21 MB/s |
| `header_escape_clean_100k` | 152.20 | 118.00 | 169.99 | 121.07 | 587.95 MB/s | 825.54 MB/s |
| `header_escape_dirty_100k` | 251.30 | 238.50 | 259.61 | 246.94 | 385.16 MB/s | 404.91 MB/s |
| `token_estimate_ascii_2mb` | 4,253.90 | 4,139.50 | 4,793.74 | 4,245.55 | 437.47 MB/s | 493.95 MB/s |
| `token_estimate_mixed_2mb` | 4,248.80 | 4,129.50 | 4,501.67 | 4,216.88 | 465.86 MB/s | 497.32 MB/s |
| `text_content_ascii_100k` | 11.30 | 8.70 | 29.12 | 8.78 | 3,432.72 MB/s | 11,386.88 MB/s |
| `text_content_mixed_100k` | 84.60 | 82.40 | 122.58 | 85.52 | 815.59 MB/s | 1,169.08 MB/s |
| `storage_push_sort_presorted_1000` | 29.20 | 28.00 | 29.90 | 30.69 | - | - |
| `storage_push_sort_reverse_1000` | 29.30 | 28.70 | 31.50 | 32.09 | - | - |
| `selector_decide_10000` | 2,726.80 | 2,008.70 | 2,994.36 | 2,113.46 | - | - |
| `local_walk_hierarchical_1000` | 14,054.80 | 13,668.50 | 15,752.81 | 14,549.55 | - | - |
| `local_collect_hierarchical_1000` | 139,675.50 | 113,991.30 | 142,099.03 | 116,241.04 | - | - |
